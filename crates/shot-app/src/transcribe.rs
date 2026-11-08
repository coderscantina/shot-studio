//! Transcribing takes from the app: the job `shot transcribe` runs (the pinned speech models,
//! the take's mic track through the streaming transcriber, the transcript into the cache), on a
//! background thread with progress. A screen holds one [`Transcribe`] per folder and renders it
//! where a transcript is missing; it shows nothing while every take of the cut has one:
//!
//! ```ignore
//! let transcribe = cx.new(|cx| Transcribe::new(folder.clone(), cx));
//! div().child(transcribe.clone())
//! ```
//!
//! When it is done the folder holds the new transcripts and [`Transcribed`] names their takes.
//! Stop ends it after the current decode and writes nothing for the take it was on.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    Context, Entity, EventEmitter, FontWeight, Hsla, Window, div, prelude::*, px, relative,
};
use shot_analyze::{Transcriber, cache, decode_mono, models};
use shot_core::{Id, Micros};

use crate::folder::VideoFolder;
use crate::model;
use crate::theme::*;
use crate::ui::{ActionLook, alert, icon, proposal_action};

const POLL: Duration = Duration::from_millis(200);

/// The takes that have a transcript now.
pub struct Transcribed(pub Vec<Id>);

/// What the background job is doing, shared with the screen.
#[derive(Default)]
struct Progress {
    /// The take it works on, as the n-th of how many.
    take: (usize, usize),
    /// Bytes of a speech model downloaded so far, and of all of it.
    download: Option<(u64, u64)>,
    /// Media time of the last word heard, and the take's length.
    heard: Arc<AtomicU64>,
    length: Micros,
    /// Set by Stop. The job then ends with the takes it finished.
    cancel: Arc<AtomicBool>,
    /// Stops the transcriber of the current take.
    stopper: Option<Arc<AtomicBool>>,
    /// Set once when the job ends.
    result: Option<Result<Vec<Id>, String>>,
}

enum State {
    Idle,
    Running(Arc<Mutex<Progress>>),
    Failed(String),
}

pub struct Transcribe {
    folder: Entity<VideoFolder>,
    state: State,
}

impl EventEmitter<Transcribed> for Transcribe {}

impl Transcribe {
    pub fn new(folder: Entity<VideoFolder>, cx: &mut Context<Self>) -> Self {
        cx.observe(&folder, |_, _, cx| cx.notify()).detach();
        Self {
            folder,
            state: State::Idle,
        }
    }

    /// The takes of the cut without a transcript, in cut order, with their lengths.
    fn missing(&self, cx: &Context<Self>) -> Vec<(Id, Micros)> {
        let folder = self.folder.read(cx);
        let mut missing: Vec<(Id, Micros)> = Vec::new();
        for (_, block) in model::blocks(&folder.project.cut) {
            if folder.transcripts.contains_key(&block.take) {
                continue;
            }
            let end = block.ranges.last().map_or(Micros(0), |r| r.range().to());
            match missing.iter_mut().find(|(take, _)| *take == block.take) {
                Some((_, length)) => *length = (*length).max(end),
                None => missing.push((block.take.clone(), end)),
            }
        }
        missing
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let missing = self.missing(cx);
        if missing.is_empty() || matches!(self.state, State::Running(_)) {
            return;
        }
        let progress = Arc::new(Mutex::new(Progress::default()));
        self.state = State::Running(progress.clone());
        let dir = self.folder.read(cx).dir.clone();
        {
            let progress = progress.clone();
            cx.background_spawn(async move {
                let result = transcribe_all(&dir, &missing, &progress);
                progress.lock().expect("not poisoned").result = Some(result);
            })
            .detach();
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let result = progress.lock().expect("not poisoned").result.take();
                let finished = result.is_some();
                let updated = this.update(cx, |this, cx| {
                    if let Some(result) = result {
                        this.finish(result, cx);
                    }
                    cx.notify();
                });
                if finished || updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Ends the job after the current decode, keeping only the takes it finished.
    fn stop(&mut self, cx: &mut Context<Self>) {
        if let State::Running(progress) = &self.state {
            let progress = progress.lock().expect("not poisoned");
            progress.cancel.store(true, Ordering::Relaxed);
            if let Some(stopper) = &progress.stopper {
                stopper.store(true, Ordering::Relaxed);
            }
            cx.notify();
        }
    }

    /// Loads the new transcripts into the folder, or shows why the job failed.
    fn finish(&mut self, result: Result<Vec<Id>, String>, cx: &mut Context<Self>) {
        let loaded = result.and_then(|takes| {
            self.folder.update(cx, |folder, cx| {
                cx.notify();
                folder.load_transcripts().map(|()| takes)
            })
        });
        match loaded {
            Ok(takes) => {
                self.state = State::Idle;
                if !takes.is_empty() {
                    cx.emit(Transcribed(takes));
                }
            }
            Err(error) => self.state = State::Failed(error),
        }
    }
}

/// Transcribes each take in turn and writes its transcript to the cache, until Stop.
fn transcribe_all(
    dir: &Path,
    takes: &[(Id, Micros)],
    progress: &Mutex<Progress>,
) -> Result<Vec<Id>, String> {
    let set = |f: &dyn Fn(&mut Progress)| f(&mut progress.lock().expect("not poisoned"));
    let cancel = progress.lock().expect("not poisoned").cancel.clone();
    let cancelled = || cancel.load(Ordering::Relaxed);
    let model_dir = models::models_dir();
    let fetch = |spec: models::ModelSpec| -> Result<PathBuf, String> {
        let path = spec
            .ensure(&model_dir, |p| {
                set(&|state| state.download = Some((p.done, p.total)));
            })
            .map_err(|e| format!("The speech model could not be downloaded: {e}"))?;
        set(&|state| state.download = None);
        Ok(path)
    };
    let whisper = fetch(models::whisper_model())?;
    let vad = fetch(models::vad_model())?;
    let mut done = Vec::new();
    for (n, (take, length)) in takes.iter().enumerate() {
        if cancelled() {
            break;
        }
        let heard = Arc::new(AtomicU64::new(0));
        set(&|state| {
            state.take = (n, takes.len());
            state.heard = heard.clone();
            state.length = *length;
        });
        let mic = dir.join("media").join(take.as_str()).join("mic.mp4");
        if !mic.exists() {
            return Err(format!(
                "Take {take} has no microphone recording, so there is nothing to transcribe."
            ));
        }
        let fail = |e: shot_analyze::Error| format!("Take {take} could not be transcribed: {e}");
        let transcriber = Transcriber::new(&whisper, &vad, take.clone(), move |word| {
            heard.store(word.range.to().0, Ordering::Relaxed);
        })
        .map_err(fail)?;
        let stopper = transcriber.stopper();
        set(&|state| {
            // Stop may have come while the models loaded.
            stopper.store(state.cancel.load(Ordering::Relaxed), Ordering::Relaxed);
            state.stopper = Some(stopper.clone());
        });
        decode_mono(&mic, |t, samples, rate| transcriber.feed(t, samples, rate)).map_err(fail)?;
        let transcript = transcriber.finish().map_err(fail)?;
        if cancelled() {
            break;
        }
        cache::write_cached(dir, &transcript).map_err(fail)?;
        done.push(take.clone());
    }
    Ok(done)
}

/// A 36 px button, lime when `accent`.
fn button(id: &'static str, text: &'static str, accent: bool) -> gpui::Stateful<gpui::Div> {
    let color: Hsla = if accent {
        ON_ACCENT.into()
    } else {
        TEXT.into()
    };
    div()
        .id(id)
        .flex_none()
        .h(px(36.))
        .px(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .rounded(RADIUS_MD)
        .cursor_pointer()
        .text_color(color)
        .font_weight(FontWeight::MEDIUM)
        .when(accent, |this| this.bg(ACCENT).hover(|s| s.opacity(0.9)))
        .when(!accent, |this| this.bg(RAISED).hover(|s| s.bg(FLOAT)))
        .child(icon("text", ICON, color))
        .child(text)
}

impl Render for Transcribe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let missing = self.missing(cx);
        let card = div()
            .p(px(12.))
            .rounded(RADIUS_MD)
            .bg(WELL)
            .flex()
            .flex_col()
            .gap(px(10.))
            .text_size(TEXT_SM)
            .line_height(px(19.));
        let takes = missing
            .iter()
            .map(|(take, _)| take.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        match &self.state {
            State::Idle if missing.is_empty() => div(),
            State::Idle => card
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(TEXT_BASE)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT)
                                .child(if missing.len() == 1 {
                                    format!("Take {takes} has no transcript")
                                } else {
                                    format!("Takes {takes} have no transcript")
                                }),
                        )
                        .child(div().text_color(MUTED).child(
                            "The rules and the text editor work from the words. Transcribing runs on this Mac.",
                        )),
                )
                .child(
                    div().flex().child(
                        button("transcribe", "Transcribe", true)
                            .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                    ),
                ),
            State::Running(progress) => {
                let progress = progress.lock().expect("not poisoned");
                let (text, fraction) = match progress.download {
                    Some((done, total)) => (
                        "Downloading the speech model".to_owned(),
                        done as f32 / total.max(1) as f32,
                    ),
                    None => {
                        let heard = progress.heard.load(Ordering::Relaxed);
                        let (n, of) = progress.take;
                        let take = missing.first().map(|(t, _)| t.to_string());
                        let which = match (of > 1, take) {
                            (true, _) => format!("Transcribing take {} of {of}", n + 1),
                            (false, Some(take)) => format!("Transcribing take {take}"),
                            (false, None) => "Transcribing".to_owned(),
                        };
                        (which, heard as f32 / progress.length.0.max(1) as f32)
                    }
                };
                let fraction = fraction.clamp(0.0, 1.0);
                let stopping = progress.cancel.load(Ordering::Relaxed);
                card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_color(BODY)
                        .child(div().flex_1().min_w_0().child(text))
                        .child(
                            div()
                                .text_color(MUTED)
                                .child(format!("{}%", (fraction * 100.0).round() as u32)),
                        )
                        .child(if stopping {
                            div().text_color(MUTED).child("Stopping").into_any_element()
                        } else {
                            proposal_action("transcribe-stop", "Stop", None, None, ActionLook::Plain)
                                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
                                .into_any_element()
                        }),
                )
                .child(
                    div().h(px(4.)).rounded_full().bg(FLOAT).child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(ACCENT)
                            .w(relative(fraction)),
                    ),
                )
            }
            State::Failed(error) => card.child(alert(error.clone())).child(
                div().flex().child(
                    button("transcribe-again", "Try again", false)
                        .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                ),
            ),
        }
    }
}
