//! The Record screen: device pickers, tracks with live levels, the transcript as stable words
//! arrive from the mic, elapsed time, start and stop.
//!
//! A recorder thread owns the capture session from start to stop, so the session never moves
//! between threads. Starting loads the speech model first (downloading it on first use), then
//! starts capture with a mic tap feeding the transcriber. Stopping finishes the take, flushes the
//! transcript to the cache, writes the take's elements and appends the take to `cut.kdl`. The
//! Vision and Claude passes of the elements run after the rough cut, in `shot analyze`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding,
    ObjectFit, ScrollHandle, Window, actions, div, prelude::*, px, relative, surface,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::progress::Progress;
use objc2_core_video::{CVPixelBufferGetHeight, CVPixelBufferGetWidth};
use shot_analyze::elements::{self, Depth};
use shot_analyze::{SpeechDetector, Transcriber, cache, models};
use shot_capture::events::Hotkey;
use shot_capture::{Devices, Preview, PreviewFrame, Session, SessionConfig, Source};
use shot_core::{Id, Micros, Transcript};

use crate::folder::VideoFolder;
use crate::prompter::{Live, PrompterPane};
use crate::theme::*;
use crate::ui::{self, alert, hint, icon, label, mono};

const CONTEXT: &str = "RecordScreen";

actions!(record, [ToggleRecording]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("space", ToggleRecording, Some(CONTEXT))]);
}

/// A finished take, emitted once it is in `cut.kdl`.
pub struct Recorded;

/// What the recorder thread is doing.
#[derive(Clone)]
enum Phase {
    Idle,
    Downloading { done: u64, total: u64 },
    Loading,
    Recording { take: Id, started: Instant },
    Finishing,
    Finished { take: Id, duration: Micros },
    Failed(String),
}

/// Sources picked for the next take.
#[derive(Clone)]
struct Choice {
    display: Option<u32>,
    camera: Option<String>,
    mic: Option<String>,
    system_audio: bool,
}

enum Command {
    Start(PathBuf, Choice, Arc<Live>, Vec<Hotkey>),
    Stop,
}

/// State the recorder thread shares with the screen.
struct Shared {
    phase: Mutex<Phase>,
    words: Mutex<Vec<String>>,
    /// Mic peak of the last buffer in dBFS, as f32 bits.
    level: AtomicU32,
    /// The running session's live frames, while there is one.
    preview: Mutex<Option<Preview>>,
}

impl Shared {
    fn set(&self, phase: Phase) {
        *self.phase.lock().expect("recorder phase") = phase;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Picker {
    Display,
    Camera,
    Mic,
}

pub struct RecordScreen {
    folder: Entity<VideoFolder>,
    devices: Devices,
    choice: Choice,
    open: Option<Picker>,
    shared: Arc<Shared>,
    commands: Sender<Command>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    error: Option<String>,
    prompter: Entity<PrompterPane>,
}

impl EventEmitter<Recorded> for RecordScreen {}

impl RecordScreen {
    pub fn new(folder: Entity<VideoFolder>, cx: &mut Context<Self>) -> Self {
        let devices = shot_capture::list_devices();
        let choice = Choice {
            display: devices.displays.iter().find(|d| d.main).map(|d| d.id),
            camera: devices
                .cameras
                .iter()
                .find(|d| d.default)
                .map(|d| d.unique_id.clone()),
            mic: devices
                .mics
                .iter()
                .find(|d| d.default)
                .map(|d| d.unique_id.clone()),
            system_audio: true,
        };
        let shared = Arc::new(Shared {
            phase: Mutex::new(Phase::Idle),
            words: Mutex::new(Vec::new()),
            level: AtomicU32::new(f32::NEG_INFINITY.to_bits()),
            preview: Mutex::new(None),
        });
        let (commands, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("shot-recorder".into())
            .spawn({
                let shared = shared.clone();
                move || recorder(rx, &shared)
            })
            .expect("spawning the recorder thread");
        let prompter = cx.new(|cx| PrompterPane::new(folder.clone(), cx));
        Self {
            folder,
            devices,
            choice,
            open: None,
            shared,
            commands,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            error: None,
            prompter,
        }
    }

    fn phase(&self) -> Phase {
        self.shared.phase.lock().expect("recorder phase").clone()
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.phase(), Phase::Recording { .. })
    }

    fn toggle(&mut self, _: &ToggleRecording, _: &mut Window, cx: &mut Context<Self>) {
        match self.phase() {
            Phase::Idle | Phase::Failed(_) | Phase::Finished { .. } => {
                self.shared.words.lock().expect("words").clear();
                self.error = None;
                let dir = self.folder.read(cx).dir.clone();
                self.shared.set(Phase::Loading);
                let (live, hotkeys) = self.prompter.update(cx, |prompter, cx| {
                    prompter.reset(cx);
                    prompter.arm();
                    (prompter.live(), prompter.log_filter())
                });
                let _ = self
                    .commands
                    .send(Command::Start(dir, self.choice.clone(), live, hotkeys));
                self.open = None;
                self.poll(cx);
            }
            Phase::Recording { .. } => {
                let _ = self.commands.send(Command::Stop);
            }
            Phase::Downloading { .. } | Phase::Loading | Phase::Finishing => {}
        }
        cx.notify();
    }

    /// Redraws while the recorder works, and hands a finished take to the folder.
    fn poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                let Ok(phase) = this.update(cx, |this, cx| {
                    cx.notify();
                    let phase = this.phase();
                    if matches!(
                        phase,
                        Phase::Finishing | Phase::Finished { .. } | Phase::Idle | Phase::Failed(_)
                    ) {
                        this.prompter.update(cx, |prompter, _| prompter.disarm());
                    }
                    phase
                }) else {
                    break;
                };
                match phase {
                    Phase::Finished { take, duration } => {
                        this.update(cx, |this, cx| {
                            let result = this.folder.update(cx, |folder, cx| {
                                let result = folder
                                    .append_take(take.clone(), duration)
                                    .and_then(|()| crate::cut::run::rough_cut(folder, &take));
                                cx.notify();
                                result
                            });
                            match result {
                                Ok(()) => {
                                    this.analyze_later(&take, cx);
                                    this.shared.set(Phase::Idle);
                                    cx.emit(Recorded);
                                }
                                Err(error) => this.error = Some(error),
                            }
                            cx.notify();
                        })
                        .ok();
                        break;
                    }
                    Phase::Idle | Phase::Failed(_) => break,
                    _ => {}
                }
            }
        })
        .detach();
    }

    fn next_take(&self, cx: &App) -> Id {
        next_take_id(&self.folder.read(cx).dir)
    }
}

fn next_take_id(dir: &std::path::Path) -> Id {
    let names: Vec<String> = std::fs::read_dir(dir.join("media"))
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    shot_capture::take::next_take_id(names.iter().map(String::as_str))
}

/// Bytes free on the volume holding `dir`.
fn free_space(dir: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `stats` is a writable statvfs.
    let status = unsafe { libc::statvfs(path.as_ptr(), &mut stats) };
    (status == 0).then(|| stats.f_bavail as u64 * stats.f_frsize)
}

fn recorder(commands: Receiver<Command>, shared: &Shared) {
    while let Ok(command) = commands.recv() {
        let Command::Start(dir, choice, live, hotkeys) = command else {
            continue;
        };
        match record(&dir, choice, &live, hotkeys, &commands, shared) {
            Ok((take, duration)) => shared.set(Phase::Finished { take, duration }),
            Err(error) => shared.set(Phase::Failed(error)),
        }
    }
}

/// Records one take, from loading the model to the cached transcript.
fn record(
    dir: &std::path::Path,
    choice: Choice,
    live: &Arc<Live>,
    hotkeys: Vec<Hotkey>,
    commands: &Receiver<Command>,
    shared: &Shared,
) -> Result<(Id, Micros), String> {
    // The transcriber needs the take id before capture creates the folder; a mismatch is fixed
    // after stop.
    let expected = next_take_id(dir);
    let transcriber = match &choice.mic {
        Some(_) => {
            let model_dir = models::models_dir();
            let fetch = |spec: models::ModelSpec| {
                spec.ensure(&model_dir, |p| {
                    shared.set(Phase::Downloading {
                        done: p.done,
                        total: p.total,
                    })
                })
                .map_err(|e| e.to_string())
            };
            let whisper = fetch(models::whisper_model())?;
            let vad = fetch(models::vad_model())?;
            let follow = fetch(shot_prompter::follow_model())?;
            shared.set(Phase::Loading);
            let sink = WordSink::default();
            let log = elements::Log::create(dir, &expected).map_err(|e| e.to_string())?;
            let on_word = {
                let sink = sink.clone();
                move |word: &shot_core::Word| {
                    sink.push(word.text.clone());
                    // Only a crash keeps the log; a failed line is written again at stop.
                    let _ = log.word(word);
                }
            };
            let transcriber = Transcriber::new(&whisper, &vad, expected.clone(), on_word)
                .map_err(|e| e.to_string())?;
            let listener = shot_prompter::Listener::new(&follow, {
                let live = live.clone();
                move |words| live.lock().hear(words)
            })?;
            Some((Arc::new(transcriber), sink, listener))
        }
        None => None,
    };
    // Speech for the auto-cut rules, detected while recording like the transcript.
    let speech = match &transcriber {
        Some(_) => Some(Arc::new(
            SpeechDetector::new(&models::vad_model().path(&models::models_dir()))
                .map_err(|e| e.to_string())?,
        )),
        None => None,
    };

    let mut config = SessionConfig::new(dir);
    config.display = choice.display;
    config.camera = choice.camera.map_or(Source::Off, Source::Named);
    config.mic = choice.mic.map_or(Source::Off, Source::Named);
    config.system_audio = choice.system_audio;
    config.hotkeys = hotkeys;
    let level = Arc::new(AtomicU32::new(f32::NEG_INFINITY.to_bits()));
    let transcriber = transcriber.map(|(transcriber, sink, listener)| {
        let tap = transcriber.clone();
        let speech = speech.clone();
        let level = level.clone();
        // The tap owns the listener, so voice follow stops with the mic.
        config.mic_tap = Some(Box::new(move |t, samples, rate| {
            let peak = samples.iter().fold(0f32, |p, s| p.max(s.abs()));
            level.store((20.0 * peak.max(1e-6).log10()).to_bits(), Ordering::Relaxed);
            tap.feed(t, samples, rate);
            listener.feed(samples, rate);
            if let Some(speech) = &speech {
                speech.feed(t, samples, rate);
            }
        }));
        (transcriber, sink)
    });
    let session = Session::start(config).map_err(|e| e.to_string())?;
    live.set_marks(Some(session.marks()));
    *shared.preview.lock().expect("preview") = Some(session.preview());
    shared.set(Phase::Recording {
        take: session.id().clone(),
        started: Instant::now(),
    });
    let mut last = Instant::now();
    loop {
        match commands.recv_timeout(Duration::from_millis(16)) {
            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Command::Start(..)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some((_, sink)) = &transcriber {
            sink.drain_into(&shared.words);
        }
        live.lock().tick(last.elapsed());
        last = Instant::now();
        shared
            .level
            .store(level.load(Ordering::Relaxed), Ordering::Relaxed);
    }
    shared.set(Phase::Finishing);
    live.set_marks(None);
    shared.preview.lock().expect("preview").take();
    let take = session.stop().map_err(|e| e.to_string())?;
    let mut heard = Transcript::new(take.id.clone());
    if let Some((transcriber, sink)) = transcriber {
        let transcriber = Arc::try_unwrap(transcriber)
            .map_err(|_| "the mic tap still holds the transcriber".to_owned())?;
        let transcript = transcriber.finish().map_err(|e| e.to_string())?;
        sink.drain_into(&shared.words);
        let transcript = if transcript.take() == &take.id {
            transcript
        } else {
            let mut moved = Transcript::new(take.id.clone());
            for word in transcript.words() {
                let _ = moved.push(word.clone());
            }
            moved
        };
        cache::write_cached(dir, &transcript).map_err(|e| e.to_string())?;
        heard = transcript;
    }
    let speech = match speech {
        Some(speech) => {
            let speech = Arc::try_unwrap(speech)
                .map_err(|_| "the mic tap still holds the speech detector".to_owned())?
                .finish()
                .map_err(|e| e.to_string())?;
            cache::write_speech(dir, &take.id, &speech).map_err(|e| e.to_string())?;
            speech
        }
        None => shot_analyze::Speech::default(),
    };
    let found = elements::analyze(dir, &heard, &speech, Depth::Quick).map_err(|e| e.to_string())?;
    elements::write(dir, &found).map_err(|e| e.to_string())?;
    if take.id != expected {
        let _ = std::fs::remove_file(elements::path(dir, &expected));
    }
    Ok((take.id, take.duration))
}

impl RecordScreen {
    /// Adds what Vision and Claude find to the take's elements in a `shot analyze` of its own,
    /// so it never holds up the rough cut, then rebuilds a migrated video's lock with them.
    /// Without the `shot` command or Claude Code that part is skipped.
    fn analyze_later(&self, take: &Id, cx: &mut Context<Self>) {
        let Ok(shot) = crate::agent::shot_executable() else {
            return;
        };
        let mut command = std::process::Command::new(shot);
        command
            .args(["analyze", "--video"])
            .arg(&self.folder.read(cx).dir)
            .args(["--take", take.as_str()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if shot_mcp::bridge::Claude::find().is_some() {
            command.arg("--claude");
        }
        let folder = self.folder.clone();
        cx.spawn(async move |_, cx| {
            let analyzed = cx
                .background_spawn(async move { command.status() })
                .await
                .is_ok_and(|s| s.success());
            if analyzed {
                folder.update(cx, |folder, cx| {
                    // A failed rebuild shows on the next edit; nothing waits on this one.
                    let _ = folder.rebuild_lock();
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

/// Stable words handed from the transcriber's thread to the recorder.
#[derive(Clone, Default)]
struct WordSink(Arc<Mutex<Vec<String>>>);

impl WordSink {
    fn push(&self, word: String) {
        self.0.lock().expect("word sink").push(word);
    }

    fn drain_into(&self, words: &Mutex<Vec<String>>) {
        let new: Vec<String> = std::mem::take(&mut *self.0.lock().expect("word sink"));
        if !new.is_empty() {
            words.lock().expect("words").extend(new);
        }
    }
}

impl Focusable for RecordScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RecordScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let phase = self.phase();
        if matches!(phase, Phase::Recording { .. }) {
            window.request_animation_frame();
        }
        if let Phase::Failed(error) = &phase {
            self.error.get_or_insert_with(|| error.clone());
        }
        let busy = !matches!(
            phase,
            Phase::Idle | Phase::Failed(_) | Phase::Finished { .. }
        );
        div()
            .id("record")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::toggle))
            .flex_1()
            .min_h_0()
            .flex()
            .gap(px(8.))
            .px(px(8.))
            .pb(px(8.))
            .child(self.tracks(busy, cx))
            .child(self.stage(&phase, cx))
            .map(|row| {
                // Without beats the prompter has nothing to show, so the live transcript stays.
                if self
                    .folder
                    .read(cx)
                    .project
                    .script
                    .chapters
                    .iter()
                    .any(|c| !c.beats.is_empty())
                {
                    row.child(self.prompter.clone())
                } else {
                    row.child(self.transcript(&phase))
                }
            })
    }
}

impl RecordScreen {
    fn tracks(&self, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let display = self
            .devices
            .displays
            .iter()
            .find(|d| Some(d.id) == self.choice.display);
        let name_of = |list: &[shot_capture::Device], id: &Option<String>| {
            id.as_ref()
                .and_then(|id| list.iter().find(|d| &d.unique_id == id))
                .map_or("Off".to_owned(), |d| d.name.clone())
        };
        let level = f32::from_bits(self.shared.level.load(Ordering::Relaxed));
        let recording = self.is_recording();
        let dir = self.folder.read(cx).dir.clone();
        let take = match self.phase() {
            Phase::Recording { take, .. } => take,
            _ => self.next_take(cx),
        };
        let free = free_space(&dir).map_or(String::new(), |bytes| {
            format!(", {} GB free", bytes / 1_000_000_000)
        });
        ui::pane()
            .id("tracks")
            .w(px(256.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(12.))
            .overflow_y_scroll()
            .child(label("Tracks").px(px(4.)).pt(px(2.)).pb(px(6.)))
            .child(self.source(
                Picker::Display,
                "display",
                "Screen",
                display.map_or("None".into(), |d| format!("{}x{}", d.width, d.height)),
                display.map_or("No display".into(), |d| {
                    if d.main {
                        "Main display".into()
                    } else {
                        format!("Display {}", d.id)
                    }
                }),
                busy,
                cx,
            ))
            .child(self.source(
                Picker::Camera,
                "camera",
                "Camera",
                String::new(),
                name_of(&self.devices.cameras, &self.choice.camera),
                busy,
                cx,
            ))
            .child(
                self.source(
                    Picker::Mic,
                    "mic",
                    "Mic",
                    if recording && level.is_finite() {
                        format!("{level:.0} dB")
                    } else {
                        String::new()
                    },
                    name_of(&self.devices.mics, &self.choice.mic),
                    busy,
                    cx,
                )
                .child(meter(if recording { level } else { f32::NEG_INFINITY })),
            )
            .child(
                card()
                    .id("system-audio")
                    .cursor_pointer()
                    .when(!busy, |card| {
                        card.on_click(cx.listener(|this, _, _, cx| {
                            this.choice.system_audio = !this.choice.system_audio;
                            cx.notify();
                        }))
                    })
                    .child(card_head("system-audio", "System audio", String::new()))
                    .child(card_value(if self.choice.system_audio {
                        "On"
                    } else {
                        "Off"
                    })),
            )
            .child(
                card()
                    .child(card_head("cursor", "Cursor and keys", "events".into()))
                    .child(card_value("Zooms and ripples stay editable")),
            )
            .child(
                div()
                    .mt_auto()
                    .px(px(4.))
                    .pt(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        mono(format!("media/{take}/"))
                            .text_size(TEXT_SM)
                            .text_color(KBD_TEXT),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child(format!("Crash safe{free}")),
                    ),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn source(
        &self,
        picker: Picker,
        icon_name: &'static str,
        name: &'static str,
        meta: String,
        value: String,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let open = self.open == Some(picker);
        let options: Vec<(Option<String>, String)> = match picker {
            Picker::Display => self
                .devices
                .displays
                .iter()
                .map(|d| {
                    (
                        Some(d.id.to_string()),
                        format!(
                            "{} {}x{}",
                            if d.main { "Main display" } else { "Display" },
                            d.width,
                            d.height
                        ),
                    )
                })
                .collect(),
            Picker::Camera | Picker::Mic => {
                let list = if picker == Picker::Camera {
                    &self.devices.cameras
                } else {
                    &self.devices.mics
                };
                std::iter::once((None, "Off".to_owned()))
                    .chain(
                        list.iter()
                            .map(|d| (Some(d.unique_id.clone()), d.name.clone())),
                    )
                    .collect()
            }
        };
        let selected = match picker {
            Picker::Display => self.choice.display.map(|d| d.to_string()),
            Picker::Camera => self.choice.camera.clone(),
            Picker::Mic => self.choice.mic.clone(),
        };
        card()
            .id(name)
            .when(!busy, |card| {
                card.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open = if this.open == Some(picker) {
                            None
                        } else {
                            Some(picker)
                        };
                        cx.notify();
                    }))
            })
            .child(card_head(icon_name, name, meta))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(card_value(value))
                    .child(div().flex_1())
                    .when(!busy, |row| row.child(icon("chevron-down", ICON_SM, MUTED))),
            )
            .when(open, |card| {
                card.child(div().flex().flex_col().gap(px(1.)).pt(px(4.)).children(
                    options.into_iter().enumerate().map(|(index, (id, text))| {
                        let on = id == selected;
                        div()
                            .id((name, index))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(32.))
                            .px(px(8.))
                            .rounded(RADIUS_SM)
                            .text_size(px(14.))
                            .cursor_pointer()
                            .hover(|s| s.bg(FLOAT))
                            .when(on, |row| row.bg(FLOAT).text_color(STRONG))
                            .child(div().flex_1().truncate().child(text))
                            .when(on, |row| row.child(icon("check", ICON_SM, STRONG)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                match picker {
                                    Picker::Display => {
                                        this.choice.display =
                                            id.as_ref().and_then(|d| d.parse().ok())
                                    }
                                    Picker::Camera => this.choice.camera = id.clone(),
                                    Picker::Mic => this.choice.mic = id.clone(),
                                }
                                this.open = None;
                                cx.stop_propagation();
                                cx.notify();
                            }))
                    }),
                ))
            })
    }

    fn stage(&self, phase: &Phase, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, detail): (String, String) = match phase {
            Phase::Idle | Phase::Failed(_) | Phase::Finished { .. } => (
                format!("Take {}", self.next_take(cx)),
                "Press Space or the record button to start.".into(),
            ),
            Phase::Downloading { .. } => (
                "Downloading the speech model".into(),
                "Once, on first use. Recording starts when it is ready.".into(),
            ),
            Phase::Loading => (
                "Starting".into(),
                "Loading the speech model and the capture devices.".into(),
            ),
            Phase::Recording { take, .. } => (
                format!("Recording take {take}"),
                "Space stops and opens the take in Edit.".into(),
            ),
            Phase::Finishing => (
                "Finishing the take".into(),
                "Writing the last fragments and the transcript.".into(),
            ),
        };
        let elapsed = match phase {
            Phase::Recording { started, .. } => started.elapsed(),
            _ => Duration::ZERO,
        };
        let recording = matches!(phase, Phase::Recording { .. });
        let live = self.shared.preview.lock().expect("preview").clone();
        let screen = live.as_ref().and_then(Preview::screen);
        let camera = live.as_ref().and_then(Preview::camera);
        ui::pane()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_color(MUTED)
                            .child(self.folder.read(cx).project.video.title.clone()),
                    ),
            )
            .children(self.error.clone().map(alert))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .rounded(px(12.))
                    .bg(gpui::rgb(0x050607))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(14.))
                    .when_some(screen.clone(), |stage, screen| {
                        stage.child(live_screen(screen, camera))
                    })
                    .when(recording && screen.is_none(), |stage| {
                        stage.child(
                            div()
                                .size(px(14.))
                                .rounded_full()
                                .bg(RECORD)
                                .border_4()
                                .border_color(gpui::Hsla::from(RECORD).opacity(0.18)),
                        )
                    })
                    .when_some(
                        match phase {
                            Phase::Downloading { done, total } => Some((*done, *total)),
                            _ => None,
                        },
                        |stage, (done, total)| {
                            stage.child(
                                div()
                                    .w(px(320.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.))
                                    .child(
                                        Progress::new("model")
                                            .value(done as f32 / total.max(1) as f32 * 100.0),
                                    )
                                    .child(
                                        mono(format!(
                                            "{} of {} MB",
                                            done / 1_000_000,
                                            total / 1_000_000
                                        ))
                                        .text_size(TEXT_SM)
                                        .text_color(MUTED),
                                    ),
                            )
                        },
                    )
                    .when(screen.is_none(), |stage| {
                        stage.child(div().text_color(MUTED).child(detail))
                    }),
            )
            .children(
                self.prompter
                    .update(cx, |prompter, _| prompter.toast())
                    .map(|toast| div().flex().justify_center().child(toast)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(20.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(
                                Button::new("record-button")
                                    .danger()
                                    .accessibility_label(if recording {
                                        "Stop recording"
                                    } else {
                                        "Start recording"
                                    })
                                    .w(px(46.))
                                    .h(px(46.))
                                    .rounded(px(12.))
                                    .child(
                                        div()
                                            .size(px(14.))
                                            .bg(ON_ACCENT)
                                            .when(recording, |d| d.rounded(px(3.)))
                                            .when(!recording, |d| d.rounded_full()),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle(&ToggleRecording, window, cx)
                                    })),
                            )
                            .child(
                                mono(format!(
                                    "{:02}:{:02}",
                                    elapsed.as_secs() / 60,
                                    elapsed.as_secs() % 60
                                ))
                                .text_size(px(26.))
                                .font_weight(FontWeight::MEDIUM),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .justify_center()
                            .when(recording, |row| row.child(self.prompter.read(cx).legend())),
                    )
                    .child(hint("Space", if recording { "Stop" } else { "Start" })),
            )
    }

    fn transcript(&self, phase: &Phase) -> impl IntoElement {
        let words = self.shared.words.lock().expect("words").join(" ");
        let empty = match (&self.choice.mic, phase) {
            (None, _) => Some("Turn on a mic to see the transcript while you talk."),
            (Some(_), Phase::Recording { .. }) if words.is_empty() => {
                Some("Words appear here once they are stable, a second or two after you say them.")
            }
            (Some(_), Phase::Idle | Phase::Failed(_) | Phase::Finished { .. })
                if words.is_empty() =>
            {
                Some("The transcript appears here while you talk.")
            }
            _ => None,
        };
        self.scroll.scroll_to_bottom();
        ui::pane()
            .w(px(412.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(14.))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Transcript"))
            .child(
                div()
                    .id("live-transcript")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .text_size(px(18.))
                    .line_height(px(30.))
                    .text_color(BODY)
                    .when(!words.is_empty(), |list| list.child(words))
                    .children(
                        empty.map(|text| div().text_size(TEXT_BASE).text_color(MUTED).child(text)),
                    ),
            )
    }
}

/// The screen, 80% of the stage wide, with the camera as a rounded square in its corner.
fn live_screen(screen: PreviewFrame, camera: Option<PreviewFrame>) -> Div {
    let aspect = CVPixelBufferGetWidth(&screen.buffer) as f32
        / CVPixelBufferGetHeight(&screen.buffer) as f32;
    div()
        .relative()
        .w(relative(0.8))
        .max_h(relative(0.92))
        .aspect_ratio(aspect)
        .rounded(px(8.))
        .overflow_hidden()
        .shadow_lg()
        .child(
            surface(screen.buffer)
                .size_full()
                .object_fit(ObjectFit::Contain),
        )
        .children(camera.map(|camera| {
            div()
                .absolute()
                .right(relative(0.03))
                .bottom(relative(0.05))
                .w(relative(0.26))
                .aspect_ratio(1.)
                .rounded(px(16.))
                .overflow_hidden()
                .shadow_lg()
                .child(
                    surface(camera.buffer)
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
        }))
}

fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .p(px(12.))
        .rounded(RADIUS_MD)
        .bg(CARD)
}

fn card_head(icon_name: &'static str, name: &'static str, meta: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(icon(icon_name, ICON, TEXT))
        .child(div().font_weight(FontWeight::MEDIUM).child(name))
        .child(div().flex_1())
        .child(mono(meta).text_size(TEXT_SM).text_color(MUTED))
}

fn card_value(text: impl Into<gpui::SharedString>) -> Div {
    div()
        .text_size(TEXT_SM)
        .text_color(MUTED)
        .truncate()
        .child(text.into())
}

/// A level meter from -60 to 0 dBFS, lime up to -6 dB and yellow above.
fn meter(level: f32) -> Div {
    let fill = if level.is_finite() {
        ((level + 60.0) / 60.0).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let safe = fill.min(0.9);
    let hot = (fill - 0.9).max(0.0);
    div()
        .h(px(5.))
        .rounded(px(3.))
        .bg(WELL)
        .overflow_hidden()
        .flex()
        .child(div().h_full().w(gpui::relative(safe)).bg(ACCENT))
        .child(
            div()
                .h_full()
                .w(gpui::relative(hot))
                .bg(gpui::rgb(0xe8d44d)),
        )
}
