//! Import clip: a file from outside, a phone clip first of all, made the next take ([0039]).
//! The Record screen's button picks one, and a video dropped on the Record screen or the Takes
//! view starts one too. The clip transcodes on a thread of its own while a row shows how far it
//! is, with Cancel. Then the take is appended and analysed as after a recording.
//!
//! [0039]: ../../../../docs/decisions/0039-ingested-clips-are-takes.md

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Context, Div, Entity, EventEmitter, ExternalPaths, PathPromptOptions, div, prelude::*, px,
};
use gpui_component::Sizable;
use gpui_component::button::Button;
use gpui_component::progress::Progress as Bar;
use shot_capture::{IngestError, Progress};

use crate::folder::VideoFolder;
use crate::record::Recorded;
use crate::theme::*;
use crate::ui::{alert, mono};

pub struct Import {
    folder: Entity<VideoFolder>,
    running: Option<Running>,
    error: Option<String>,
}

struct Running {
    name: String,
    progress: Arc<Progress>,
}

/// Emitted once an imported take is in `cut.kdl`, as after a recording.
impl EventEmitter<Recorded> for Import {}

impl Import {
    pub fn new(folder: Entity<VideoFolder>) -> Self {
        Self {
            folder,
            running: None,
            error: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Asks for a clip with the file picker and imports it.
    pub fn pick(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import as a take".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            if let Some(path) = paths.into_iter().next() {
                this.update(cx, |this, cx| this.start(path, cx)).ok();
            }
        })
        .detach();
    }

    /// Imports the first of the dropped files.
    pub fn drop_paths(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        if let Some(path) = paths.paths().first() {
            self.start(path.clone(), cx);
        }
    }

    /// Ingests `file` into the next take, then appends and analyses it. One import at a time.
    pub fn start(&mut self, file: PathBuf, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let dir = self.folder.read(cx).dir.clone();
        let progress = Arc::new(Progress::default());
        self.error = None;
        self.running = Some(Running {
            name: file.file_name().map_or_else(
                || file.display().to_string(),
                |n| n.to_string_lossy().into(),
            ),
            progress: progress.clone(),
        });
        let ingest = cx.background_spawn({
            let progress = progress.clone();
            async move { shot_capture::ingest(&file, &dir, &progress) }
        });
        cx.spawn(async move |this, cx| {
            let result = ingest.await;
            this.update(cx, |this, cx| {
                this.running = None;
                match result {
                    Ok(ingested) => this.finish(ingested, cx),
                    Err(IngestError::Cancelled) => {}
                    Err(error) => this.error = Some(format!("The clip was not imported: {error}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        // Redraws the row while the ingest runs.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let running = this.update(cx, |this, cx| {
                    cx.notify();
                    this.is_running()
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn finish(&mut self, ingested: shot_capture::Ingested, cx: &mut Context<Self>) {
        let take = ingested.take;
        let appended = self.folder.update(cx, |folder, cx| {
            let result = folder.append_take(take.clone(), ingested.duration);
            cx.notify();
            result
        });
        match appended {
            Ok(()) => {
                crate::record::analyze_later(&self.folder, &take, cx);
                cx.emit(Recorded);
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// The row of the running import with its Cancel, or why the last one failed.
    pub fn row(&self) -> Option<Div> {
        if let Some(running) = &self.running {
            let progress = running.progress.clone();
            let (done, total) = (progress.done(), progress.total());
            let share = done.as_secs_f32() / total.as_secs_f32().max(f32::EPSILON);
            return Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .p(px(12.))
                    .rounded(RADIUS_MD)
                    .bg(CARD)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(TEXT)
                                    .child(format!("Importing {}", running.name)),
                            )
                            .child(
                                Button::new("cancel-import")
                                    .small()
                                    .label("Cancel")
                                    .on_click(move |_, _, _| progress.cancel()),
                            ),
                    )
                    .child(Bar::new("import-progress").value(share * 100.0))
                    .child(
                        mono(format!(
                            "{:.0} of {:.0} s",
                            done.as_secs_f32(),
                            total.as_secs_f32()
                        ))
                        .text_size(TEXT_SM)
                        .text_color(MUTED),
                    ),
            );
        }
        self.error.clone().map(alert)
    }
}
