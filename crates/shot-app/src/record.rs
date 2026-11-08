//! The Record screen: device pickers, tracks with live levels and their privacy access, the
//! transcript as stable words arrive from the mic, elapsed time, start and stop.
//!
//! While the screen shows, the selected inputs run on standby: the stage previews them (styled
//! by a [`Rehearsal`] or raw) and the mic drives the meter, so everything can be checked before
//! a take. Leaving the screen or closing the window stops them, and the camera light with them.
//!
//! A recorder thread owns the standby and the capture session from start to stop, so neither
//! moves between threads. The standby keeps the stage live while the models load and hands
//! over once the take's own preview shows, so recording starts without a gap. Starting loads the speech model first (downloading it on first use), then
//! starts capture with a mic tap feeding the transcriber, and the live take ([`crate::live`]),
//! which streams the take's elements and paints the styled monitor. A switch shows the raw feed
//! instead. Stopping finishes the take, flushes the transcript to the cache, writes the take's
//! elements and appends the take to `cut.kdl`. The Vision and Claude passes of the elements run
//! after the rough cut, in `shot analyze`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, KeyBinding,
    ObjectFit, ScrollHandle, Subscription, Window, actions, div, prelude::*, px, relative, surface,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::progress::Progress;
use gpui_component::switch::Switch;
use gpui_component::{Disableable, Sizable};
use objc2_core_video::{CVPixelBufferGetHeight, CVPixelBufferGetWidth};
use shot_analyze::elements::{self, Depth};
use shot_analyze::{SpeechDetector, Transcriber, cache, models};
use shot_capture::events::Hotkey;
use shot_capture::{
    Access, CaptureError, Devices, Permission, Preview, PreviewFrame, Session, SessionConfig,
    Source, Standby,
};
use shot_core::{Id, Micros, Transcript};

use crate::folder::VideoFolder;
use crate::live::{Feeds, Heard, LiveTake, Rehearsal, Setup, View};
use crate::prompter::{Live, PrompterPane, TogglePrompter};
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
    Failed(Failure),
}

/// Why a take did not start or finish, with the privacy permission to grant when that is why.
#[derive(Clone)]
struct Failure {
    message: String,
    permission: Option<Permission>,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            permission: None,
        }
    }
}

impl From<CaptureError> for Failure {
    fn from(error: CaptureError) -> Self {
        match error {
            CaptureError::Permission(permission) => {
                let then = match permission {
                    Permission::ScreenRecording | Permission::InputMonitoring => {
                        "then quit and reopen Shot Studio"
                    }
                    Permission::Camera => "then start again, or turn the camera off",
                    Permission::Microphone => "then start again, or turn the mic off",
                };
                Self {
                    message: format!(
                        "Recording needs {permission} access. Allow Shot Studio, or the terminal \
                         you started it from, in Privacy & Security, {then}."
                    ),
                    permission: Some(permission),
                }
            }
            error => error.to_string().into(),
        }
    }
}

/// The Privacy & Security pane of System Settings for `permission`.
fn settings_url(permission: Permission) -> &'static str {
    match permission {
        Permission::ScreenRecording => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        Permission::Camera => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
        }
        Permission::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        Permission::InputMonitoring => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
    }
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
    /// Runs the chosen inputs on standby for the next take, replacing any standby.
    Watch(Box<Setup>, Id, Choice),
    /// Stops the standby.
    Rest,
    Start(Box<Setup>, Choice, Arc<Live>, Vec<Hotkey>),
    Stop,
}

/// The inputs on standby and the style painted over them. The painter stops first.
struct Inputs {
    _rehearsal: Rehearsal,
    _capture: Standby,
}

/// State the recorder thread shares with the screen.
struct Shared {
    phase: Mutex<Phase>,
    words: Mutex<Vec<String>>,
    /// Mic peak of the last buffer in dBFS, as f32 bits.
    level: Arc<AtomicU32>,
    /// The live frames of the standby or the running session, while there is one.
    preview: Mutex<Option<Preview>>,
    /// The styled monitor of the standby or the running take, while there is one.
    monitor: Mutex<Option<Arc<View>>>,
    /// Why an input on standby did not start, other than a missing permission.
    standby_error: Mutex<Option<String>>,
    /// Whether the monitor shows the style or the raw feed.
    styled: Arc<AtomicBool>,
}

impl Shared {
    fn set(&self, phase: Phase) {
        *self.phase.lock().expect("recorder phase") = phase;
    }

    /// Shows `preview` and `monitor` on the stage, or nothing.
    fn show(&self, live: Option<(Preview, Arc<View>)>) {
        let (preview, monitor) = live.unzip();
        *self.preview.lock().expect("preview") = preview;
        *self.monitor.lock().expect("monitor") = monitor;
        self.level
            .store(f32::NEG_INFINITY.to_bits(), Ordering::Relaxed);
    }
}

/// The privacy access of each source, read again while the screen shows.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Permissions {
    screen: Access,
    camera: Access,
    mic: Access,
    events: Access,
}

impl Permissions {
    fn read() -> Self {
        Self {
            screen: shot_capture::access(Permission::ScreenRecording),
            camera: shot_capture::access(Permission::Camera),
            mic: shot_capture::access(Permission::Microphone),
            events: shot_capture::access(Permission::InputMonitoring),
        }
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
    error: Option<Failure>,
    prompter: Entity<PrompterPane>,
    /// Display names by id, like "Studio Display".
    display_names: Vec<(u32, String)>,
    /// The screen is showing: set on its first render, cleared when focus leaves it.
    on_screen: bool,
    /// The redraw loop of the standby runs.
    ticking: bool,
    permissions: Permissions,
    _focus_out: Option<Subscription>,
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
            level: Arc::new(AtomicU32::new(f32::NEG_INFINITY.to_bits())),
            preview: Mutex::new(None),
            monitor: Mutex::new(None),
            standby_error: Mutex::new(None),
            styled: Arc::new(AtomicBool::new(true)),
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
            display_names: crate::prompter::display_names(),
            on_screen: false,
            ticking: false,
            permissions: Permissions::read(),
            _focus_out: None,
        }
    }

    fn setup(&self, cx: &App) -> Setup {
        let folder = self.folder.read(cx);
        Setup {
            dir: folder.dir.clone(),
            project: folder.project.clone(),
            migrated: folder.is_migrated(),
        }
    }

    /// Runs the chosen inputs on standby, and redraws the stage while the screen shows.
    fn watch(&mut self, cx: &mut Context<Self>) {
        self.permissions = Permissions::read();
        self.restart(cx);
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            let mut tick = 0u32;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                tick += 1;
                let Ok(on_screen) = this.update(cx, |this, cx| {
                    // While a take runs, its own poll redraws.
                    if this.on_screen && matches!(this.phase(), Phase::Idle | Phase::Failed(_)) {
                        // Access changes in System Settings; a granted source starts.
                        if tick.is_multiple_of(30) {
                            let now = Permissions::read();
                            if now != this.permissions {
                                this.permissions = now;
                                this.restart(cx);
                            }
                        }
                        cx.notify();
                    }
                    this.on_screen
                }) else {
                    break;
                };
                if !on_screen {
                    this.update(cx, |this, _| this.ticking = false).ok();
                    break;
                }
            }
        })
        .detach();
    }

    /// Starts the standby with the current choice.
    fn restart(&mut self, cx: &mut Context<Self>) {
        let setup = self.setup(cx);
        let take = self.next_take(cx);
        let _ = self
            .commands
            .send(Command::Watch(Box::new(setup), take, self.choice.clone()));
    }

    fn phase(&self) -> Phase {
        self.shared.phase.lock().expect("recorder phase").clone()
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.phase(), Phase::Recording { .. })
    }

    /// Starts the prompter over from the folder's script, after the script changed.
    pub fn reload_script(&mut self, cx: &mut Context<Self>) {
        self.prompter.update(cx, |prompter, cx| prompter.reset(cx));
    }

    fn toggle(&mut self, _: &ToggleRecording, _: &mut Window, cx: &mut Context<Self>) {
        match self.phase() {
            Phase::Idle | Phase::Failed(_) | Phase::Finished { .. } => {
                self.shared.words.lock().expect("words").clear();
                self.error = None;
                let setup = self.setup(cx);
                self.shared.set(Phase::Loading);
                let (live, hotkeys) = self.prompter.update(cx, |prompter, cx| {
                    prompter.reset(cx);
                    prompter.arm();
                    (prompter.live(), prompter.log_filter())
                });
                let _ = self.commands.send(Command::Start(
                    Box::new(setup),
                    self.choice.clone(),
                    live,
                    hotkeys,
                ));
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
                                Err(error) => {
                                    this.error = Some(error.into());
                                    this.shared.set(Phase::Idle);
                                    this.restart(cx);
                                }
                            }
                            cx.notify();
                        })
                        .ok();
                        break;
                    }
                    Phase::Failed(_) => {
                        // A take that failed before capture keeps the standby; one that failed later starts it again.
                        this.update(cx, |this, cx| {
                            if this.on_screen
                                && this.shared.preview.lock().expect("preview").is_none()
                            {
                                this.restart(cx);
                            }
                        })
                        .ok();
                        break;
                    }
                    Phase::Idle => break,
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
    let mut standby: Option<Inputs> = None;
    while let Ok(command) = commands.recv() {
        match command {
            Command::Watch(setup, take, choice) => {
                // The old inputs stop before the new ones start on the same devices.
                drop(standby.take());
                shared.show(None);
                standby = Some(watch(*setup, take, &choice, shared));
            }
            Command::Rest => {
                standby = None;
                shared.show(None);
                shared.standby_error.lock().expect("standby error").take();
            }
            Command::Start(setup, choice, live, hotkeys) => {
                let result = record(
                    *setup,
                    choice,
                    &live,
                    hotkeys,
                    &commands,
                    shared,
                    &mut standby,
                );
                match result {
                    Ok((take, duration)) => shared.set(Phase::Finished { take, duration }),
                    Err(error) => shared.set(Phase::Failed(error)),
                }
            }
            Command::Stop => {}
        }
    }
}

/// Starts `choice` on standby and shows it on the stage.
fn watch(setup: Setup, take: Id, choice: &Choice, shared: &Shared) -> Inputs {
    let level = shared.level.clone();
    let capture = Standby::start(
        choice.display,
        &choice.camera.clone().map_or(Source::Off, Source::Named),
        &choice.mic.clone().map_or(Source::Off, Source::Named),
        move |peak| level.store(peak.to_bits(), Ordering::Relaxed),
    );
    // Missing permissions show in Tracks; anything else above the stage.
    *shared.standby_error.lock().expect("standby error") = capture
        .errors()
        .iter()
        .find(|e| !matches!(e, CaptureError::Permission(_)))
        .map(ToString::to_string);
    let preview = capture.preview();
    let rehearsal = Rehearsal::start(
        setup,
        take,
        preview.clone(),
        screen_size(choice.display),
        shared.styled.clone(),
    );
    shared.show(Some((preview, rehearsal.view())));
    Inputs {
        _rehearsal: rehearsal,
        _capture: capture,
    }
}

/// The pixel size of `display`, or of the main display.
fn screen_size(display: Option<u32>) -> (u32, u32) {
    shot_capture::list_devices()
        .displays
        .iter()
        .find(|d| display.map_or(d.main, |id| d.id == id))
        .map_or((16, 9), |d| (d.width as u32, d.height as u32))
}

/// Whether the take's own preview shows what the standby showed, so the standby can stop.
fn caught_up(preview: &Preview, view: &View, camera: bool, styled: bool) -> bool {
    preview.screen().is_some()
        && (!camera || preview.camera().is_some())
        && (!styled || view.frame().is_some() || view.error().is_some())
}

/// Records one take, from loading the model to the cached transcript. The standby keeps the
/// stage live until the take's preview catches up, and stops then.
fn record(
    setup: Setup,
    choice: Choice,
    live: &Arc<Live>,
    hotkeys: Vec<Hotkey>,
    commands: &Receiver<Command>,
    shared: &Shared,
    standby: &mut Option<Inputs>,
) -> Result<(Id, Micros), Failure> {
    let dir = setup.dir.clone();
    let dir = dir.as_path();
    // The transcriber needs the take id before capture creates the folder; a mismatch is fixed
    // after stop.
    let expected = next_take_id(dir);
    let mut live_take = LiveTake::start(setup, expected.clone());
    let (heard, heard_rx) = mpsc::channel();
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
            let on_word = {
                let (sink, heard) = (sink.clone(), heard.clone());
                move |word: &shot_core::Word| {
                    sink.push(word.text.clone());
                    let _ = heard.send(Heard::Word(word.clone()));
                }
            };
            let on_tentative = move |words: &[shot_core::Word]| {
                let _ = heard.send(Heard::Tentative(words.to_vec()));
            };
            let transcriber = Transcriber::with_tentative(
                &whisper,
                &vad,
                expected.clone(),
                on_word,
                on_tentative,
            )
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

    let camera_on = choice.camera.is_some();
    let mut config = SessionConfig::new(dir);
    config.display = choice.display;
    config.camera = choice.camera.map_or(Source::Off, Source::Named);
    config.mic = choice.mic.map_or(Source::Off, Source::Named);
    config.system_audio = choice.system_audio;
    config.hotkeys = hotkeys;
    let level = shared.level.clone();
    let transcriber = transcriber.map(|(transcriber, sink, listener)| {
        let tap = transcriber.clone();
        let speech = speech.clone();
        let level = level.clone();
        // The tap owns the listener, so voice follow stops with the mic.
        config.mic_tap = Some(Box::new(move |t, samples, rate| {
            level.store(
                shot_capture::peak_dbfs(samples).to_bits(),
                Ordering::Relaxed,
            );
            tap.feed(t, samples, rate);
            listener.feed(samples, rate);
            if let Some(speech) = &speech {
                speech.feed(t, samples, rate);
            }
        }));
        (transcriber, sink)
    });
    let screen_size = screen_size(choice.display);
    let session = Session::start(config)?;
    let origin = Instant::now() - Duration::from_micros(session.elapsed().0);
    live.set_marks(Some(session.marks()));
    let preview = session.preview();
    live_take.record(Feeds {
        take: session.id().clone(),
        origin,
        preview: preview.clone(),
        screen_size,
        heard: heard_rx,
        styled: shared.styled.clone(),
        prompter: live.clone(),
    });
    let view = live_take.view();
    let switched = Instant::now();
    if standby.is_none() {
        shared.show(Some((preview.clone(), view.clone())));
    }
    shared.set(Phase::Recording {
        take: session.id().clone(),
        started: Instant::now(),
    });
    live.lock().start();
    let mut last = Instant::now();
    loop {
        match commands.recv_timeout(Duration::from_millis(16)) {
            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            // Leaving the screen waits for the take: the title bar stays put while recording.
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Some((_, sink)) = &transcriber {
            sink.drain_into(&shared.words);
        }
        live.lock().tick(last.elapsed());
        last = Instant::now();
        let styled = shared.styled.load(Ordering::Relaxed);
        if standby.is_some()
            && (caught_up(&preview, &view, camera_on, styled)
                || switched.elapsed() > Duration::from_secs(2))
        {
            *shared.preview.lock().expect("preview") = Some(preview.clone());
            *shared.monitor.lock().expect("monitor") = Some(view.clone());
            *standby = None;
        }
    }
    *standby = None;
    shared.set(Phase::Finishing);
    live.set_marks(None);
    shared.preview.lock().expect("preview").take();
    shared.monitor.lock().expect("monitor").take();
    live_take.stop();
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
        let busy = busy_stage(&phase);
        let has_beats = self
            .folder
            .read(cx)
            .project
            .script
            .chapters
            .iter()
            .any(|c| !c.beats.is_empty());
        if !self.on_screen {
            self.show(has_beats, window, cx);
        }
        div()
            .id("record")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::toggle))
            .on_action(cx.listener(|this, _: &TogglePrompter, _, cx| {
                this.prompter.update(cx, |prompter, cx| prompter.toggle(cx))
            }))
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
                if has_beats {
                    row.child(self.prompter.clone())
                } else {
                    row.child(self.transcript(&phase))
                }
            })
    }
}

impl RecordScreen {
    /// The screen shows: the prompter output opens with it, and closes when focus leaves the
    /// screen, which is what the title bar does when it switches screens. Focus also "leaves"
    /// when another app or the output itself becomes active, which is no reason to close.
    fn show(&mut self, has_beats: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.on_screen = true;
        if self._focus_out.is_none() {
            self._focus_out = Some(cx.on_focus_out(&self.focus, window, |this, _, window, cx| {
                if window.is_window_active() {
                    this.on_screen = false;
                    let _ = this.commands.send(Command::Rest);
                    this.prompter.update(cx, |prompter, cx| prompter.leave(cx));
                }
            }));
        }
        self.watch(cx);
        if has_beats {
            let display = window
                .display(cx)
                .and_then(|display| u32::try_from(u64::from(display.id())).ok());
            self.prompter
                .update(cx, |prompter, cx| prompter.enter(display, cx));
        }
    }

    /// A display by its name, like "Studio Display", marked when it holds the menu bar.
    fn screen_name(&self, display: &shot_capture::Display) -> String {
        let name = self
            .display_names
            .iter()
            .find(|(id, _)| *id == display.id)
            .map_or("Display", |(_, name)| name.as_str());
        if display.main {
            format!("{name} (main)")
        } else {
            name.to_owned()
        }
    }

    /// The error of the last take, with a way to the setting that fixes a missing permission.
    fn failure(&self) -> Option<Div> {
        let failure = self.error.as_ref()?;
        Some(
            alert(failure.message.clone()).children(failure.permission.map(|permission| {
                let url = settings_url(permission);
                Button::new("open-privacy")
                    .small()
                    .label("Open Privacy & Security")
                    .on_click(move |_, _, cx| cx.open_url(url))
            })),
        )
    }
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
        let camera_black = self.choice.camera.is_some()
            && self
                .shared
                .monitor
                .lock()
                .expect("monitor")
                .as_ref()
                .is_some_and(|view| view.camera_black());
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
            .child(
                self.source(
                    Picker::Display,
                    "display",
                    "Screen",
                    display.map_or(String::new(), |d| format!("{}x{}", d.width, d.height)),
                    display.map_or("No display".into(), |d| self.screen_name(d)),
                    busy,
                    cx,
                )
                .child(access(Permission::ScreenRecording, self.permissions.screen)),
            )
            .child(
                self.source(
                    Picker::Camera,
                    "camera",
                    "Camera",
                    String::new(),
                    name_of(&self.devices.cameras, &self.choice.camera),
                    busy,
                    cx,
                )
                .when(self.choice.camera.is_some(), |card| {
                    card.child(access(Permission::Camera, self.permissions.camera))
                })
                .when(camera_black, |card| {
                    card.child(
                        div()
                            .text_size(TEXT_SM)
                            .text_color(WARN)
                            .child("Black frames. Check the lens cover. The take still records."),
                    )
                }),
            )
            .child(
                self.source(
                    Picker::Mic,
                    "mic",
                    "Mic",
                    if level.is_finite() {
                        format!("{level:.0} dB")
                    } else {
                        String::new()
                    },
                    name_of(&self.devices.mics, &self.choice.mic),
                    busy,
                    cx,
                )
                .when(self.choice.mic.is_some(), |card| {
                    card.child(meter(level))
                        .child(access(Permission::Microphone, self.permissions.mic))
                }),
            )
            .child(
                card().child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(icon("system-audio", ICON, TEXT))
                        .child(div().font_weight(FontWeight::MEDIUM).child("System audio"))
                        .child(div().flex_1())
                        .child(
                            Switch::new("system-audio")
                                .small()
                                .checked(self.choice.system_audio)
                                .disabled(busy)
                                .accessibility_label("Record system audio")
                                .on_click(cx.listener(|this, on: &bool, _, cx| {
                                    this.choice.system_audio = *on;
                                    cx.notify();
                                })),
                        ),
                ),
            )
            .child(
                card()
                    .child(card_head("cursor", "Cursor and keys", "events".into()))
                    .child(card_value("Zooms and ripples stay editable"))
                    .child(access(Permission::InputMonitoring, self.permissions.events)),
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
                        format!("{}, {}x{}", self.screen_name(d), d.width, d.height),
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
                                this.restart(cx);
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
        let styled = self.shared.styled.load(Ordering::Relaxed);
        let monitor = self.shared.monitor.lock().expect("monitor").clone();
        let monitor_error = monitor.as_ref().and_then(|m| m.error()).filter(|_| styled);
        let monitored = monitor.and_then(|m| m.frame()).filter(|_| styled);
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
                    )
                    .child(
                        ui::segmented(
                            "monitor",
                            WELL,
                            [("Styled", true), ("Raw", false)]
                                .into_iter()
                                .map(|(name, on)| {
                                    let shared = self.shared.clone();
                                    ui::Segment::new(name, styled == on, move |_, window, _| {
                                        shared.styled.store(on, Ordering::Relaxed);
                                        window.refresh();
                                    })
                                })
                                .collect(),
                        )
                        .ml_auto()
                        .w(px(180.)),
                    ),
            )
            .children(self.failure())
            .children(
                self.shared
                    .standby_error
                    .lock()
                    .expect("standby error")
                    .clone()
                    .filter(|_| !busy_stage(phase))
                    .map(|e| {
                        alert(format!("The preview could not start: {e}")).child(
                            Button::new("retry-preview")
                                .small()
                                .label("Try again")
                                .on_click(cx.listener(|this, _, _, cx| this.restart(cx))),
                        )
                    }),
            )
            .children(monitor_error.map(|e| alert(format!("The monitor shows the raw feed: {e}"))))
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
                    .map(|stage| match (monitored, screen.clone()) {
                        (Some(frame), _) => stage.child(styled_screen(frame)),
                        (None, Some(screen)) => stage.child(live_screen(screen, camera)),
                        (None, None) => stage,
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

/// The styled monitor, 80% of the stage wide: the layout places the camera.
fn styled_screen(frame: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>) -> Div {
    let aspect = CVPixelBufferGetWidth(&frame) as f32 / CVPixelBufferGetHeight(&frame) as f32;
    div()
        .w(relative(0.8))
        .max_h(relative(0.92))
        .aspect_ratio(aspect)
        .rounded(px(8.))
        .overflow_hidden()
        .shadow_lg()
        .child(surface(frame).size_full().object_fit(ObjectFit::Contain))
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

fn busy_stage(phase: &Phase) -> bool {
    !matches!(
        phase,
        Phase::Idle | Phase::Failed(_) | Phase::Finished { .. }
    )
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

/// A track's privacy access: allowed, waiting for the macOS prompt, or how to allow it.
fn access(permission: Permission, access: Access) -> Div {
    let row = div().flex().items_center().gap(px(6.)).text_size(TEXT_SM);
    match access {
        // Access alone, not a working device: a failed preview shows above the stage.
        Access::Allowed => row
            .text_color(MUTED)
            .child(icon("check", ICON_SM, ACCENT))
            .child("Access allowed"),
        Access::NotAsked => row.text_color(MUTED).child("Answer the macOS prompt."),
        Access::Denied => {
            // Screen and event access only apply after a restart.
            let fix = match permission {
                Permission::ScreenRecording | Permission::InputMonitoring => {
                    "No access. Allow it and reopen."
                }
                Permission::Camera | Permission::Microphone => "No access. Allow it in Settings.",
            };
            let url = settings_url(permission);
            div()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(8.))
                .child(div().text_size(TEXT_SM).text_color(WARN).child(fix))
                .child(
                    Button::new(("open-privacy", permission as usize))
                        .small()
                        .label("Open Privacy & Security")
                        .on_click(move |_, _, cx| cx.open_url(url)),
                )
        }
    }
}

/// A level meter from -80 to 0 dBFS, lime up to -6 dB and yellow above. The low floor keeps
/// a quiet mic, such as an interface at low gain, visibly moving.
fn meter(level: f32) -> Div {
    const FLOOR: f32 = -80.0;
    const HOT: f32 = (FLOOR + 6.0) / FLOOR;
    let fill = if level.is_finite() {
        ((level - FLOOR) / -FLOOR).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let safe = fill.min(HOT);
    let hot = (fill - HOT).max(0.0);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_permission_names_its_settings_pane_and_how_to_retry() {
        let failure = Failure::from(CaptureError::Permission(Permission::ScreenRecording));
        assert_eq!(failure.permission, Some(Permission::ScreenRecording));
        assert!(
            failure
                .message
                .ends_with("then quit and reopen Shot Studio.")
        );
        assert_eq!(Failure::from(CaptureError::NoDisplay(7)).permission, None);
    }
}
