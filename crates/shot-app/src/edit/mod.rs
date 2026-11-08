//! The Edit screen: three workspaces over the same panes (Text, Picture, Timeline), a preview
//! drawn through the render graph, and export.
//!
//! Every edit is a `shot-core` function on the cut, written to `cut.kdl` by [`VideoFolder`]. The
//! screen keeps only view state: the workspace, the chapter on show, the selection, the playhead.

mod overlays;
mod parts;
mod picture;
mod place;
mod product;
mod proposals;
mod text;
mod timeline;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, Pixels, ScrollHandle, Window,
    actions, div, prelude::*,
};
use shot_core::{Cut, EditError, Id, Micros, TakeAt, TimeRange, WordState};
use shot_mcp::bridge::Claude;
use shot_media::timing::ClockFollower;
use shot_render::export::{self, ExportOptions};

use crate::agent::AgentRun;
use crate::engine::{Engine, Plan, PreviewFrame};
use crate::folder::VideoFolder;
use crate::model::{
    self, BlockDoc, EditMap, FPS, LAYOUT_KEYS, Selection, View, apply_layout, document,
};
use crate::peaks::Peaks;
use crate::score::Cues;
use crate::sound::Sound;
use crate::theme::*;

const CONTEXT: &str = "EditScreen";

actions!(
    edit,
    [
        TogglePlay,
        DeleteSelection,
        RestoreSelection,
        ClearSelection,
        Layout1,
        Layout2,
        Layout3,
        Layout4,
        ShowText,
        ShowPicture,
        ShowTimeline,
        PreviousWord,
        NextWord,
        ExtendPrevious,
        ExtendNext,
        AcceptProposal,
        ShowProposal,
        PickSound,
    ]
);

pub fn bind_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, context),
        KeyBinding::new("backspace", DeleteSelection, context),
        KeyBinding::new("r", RestoreSelection, context),
        KeyBinding::new("escape", ClearSelection, context),
        KeyBinding::new("1", Layout1, context),
        KeyBinding::new("2", Layout2, context),
        KeyBinding::new("3", Layout3, context),
        KeyBinding::new("4", Layout4, context),
        KeyBinding::new("alt-1", ShowText, context),
        KeyBinding::new("alt-2", ShowPicture, context),
        KeyBinding::new("alt-3", ShowTimeline, context),
        KeyBinding::new("left", PreviousWord, context),
        KeyBinding::new("right", NextWord, context),
        KeyBinding::new("shift-left", ExtendPrevious, context),
        KeyBinding::new("shift-right", ExtendNext, context),
        KeyBinding::new("tab", AcceptProposal, context),
        KeyBinding::new("g", ShowProposal, context),
        KeyBinding::new("s", PickSound, context),
    ]);
}

/// A cut run by its block and media start.
type RunKey = (usize, usize, Micros);

fn run_key(at: TakeAt, from: Micros) -> RunKey {
    (at.chapter, at.take, from)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    Text,
    Picture,
    Timeline,
    /// The Product demo screen, where product tours open.
    Product,
}

/// A frame picked at a render is on screen about one refresh later, so it is picked for the
/// sound heard then. One refresh at 60 Hz.
const VIDEO_AHEAD: Duration = Duration::from_micros(16_667);

/// The wall clock that paces playback when there is no sound, started once the first frame
/// after play is queued.
#[derive(Clone, Copy)]
struct Clock {
    started: Instant,
    from: u64,
}

pub enum ExportStatus {
    Running { rendered: usize, to_render: usize },
    Done { path: String, seconds: f64 },
    Failed(String),
}

pub struct EditScreen {
    folder: Entity<VideoFolder>,
    workspace: Workspace,
    view: View,
    map: EditMap,
    doc: Vec<BlockDoc>,
    /// The chapter the Text workspace shows.
    chapter: usize,
    selection: Option<Selection>,
    dragging: bool,
    /// Cut runs opened to show their removed words, by block and media start.
    expanded: HashSet<RunKey>,
    engine: Engine,
    /// The edit's audio, and the playback clock while it plays.
    sound: Sound,
    shown: Option<PreviewFrame>,
    playhead: u64,
    playing: bool,
    clock: Option<Clock>,
    /// Picks frames from the sound's clock, see [`ClockFollower`].
    follower: ClockFollower,
    /// A seek's frame is still on its way.
    seeking: bool,
    peaks: HashMap<Id, Option<Arc<Peaks>>>,
    export: Option<Arc<Mutex<ExportStatus>>>,
    error: Option<String>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// Where the overview bar and the lanes were last painted, for mouse mapping.
    overview_bounds: Rc<Cell<Bounds<Pixels>>>,
    product: product::ProductState,
    /// What the overlay rules last added, for the Overlays pane.
    overlay_note: Option<String>,
    /// The sound and pop pickers and the callout tool.
    place: place::PlaceState,
    /// Where the Picture preview and the Sound lane were last painted, for mouse mapping.
    preview_bounds: Rc<Cell<Bounds<Pixels>>>,
    sound_lane_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The channel repo and slug when the folder is a video of one, for agent proposals.
    channel: Option<(PathBuf, String)>,
    proposals: Vec<proposals::CutProposal>,
    cards: Vec<proposals::Card>,
    /// The proposal Tab and Esc act on.
    proposal: Option<String>,
    /// Scroll the transcript to the selected proposal on the next render.
    scroll_to_card: bool,
    claude: Option<Claude>,
    /// Claude writing another take.
    run: Option<AgentRun>,
}

impl EditScreen {
    pub fn new(folder: Entity<VideoFolder>, cx: &mut Context<Self>) -> Self {
        cx.observe(&folder, |this, _, cx| {
            this.rebuild(cx);
            cx.notify();
        })
        .detach();
        let product_tour =
            folder.read(cx).project.video.format == shot_core::FormatKind::ProductTour;
        let channel = crate::studio::channel(&folder.read(cx).dir);
        let mut this = Self {
            folder,
            workspace: if product_tour {
                Workspace::Product
            } else {
                Workspace::Text
            },
            view: View::Edited,
            map: EditMap {
                spans: Vec::new(),
                chapters: Vec::new(),
                frames: 0,
            },
            doc: Vec::new(),
            chapter: 0,
            selection: None,
            dragging: false,
            expanded: HashSet::new(),
            engine: Engine::spawn(),
            sound: Sound::spawn(),
            shown: None,
            playhead: 0,
            playing: false,
            clock: None,
            follower: ClockFollower::default(),
            seeking: false,
            peaks: HashMap::new(),
            export: None,
            error: None,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            overview_bounds: Rc::default(),
            product: product::ProductState::default(),
            overlay_note: None,
            place: place::PlaceState::default(),
            preview_bounds: Rc::default(),
            sound_lane_bounds: Rc::default(),
            channel,
            proposals: Vec::new(),
            cards: Vec::new(),
            proposal: None,
            scroll_to_card: false,
            claude: Claude::find(),
            run: None,
        };
        this.rebuild(cx);
        this
    }

    /// Re-derives everything from the files, keeping the playhead on the same media.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let keep = self.map.source_at(self.playhead);
        let folder = self.folder.read(cx);
        let project = &folder.shown;
        self.map = EditMap::of(project, self.view);
        self.doc = document(
            &project.cut,
            &folder.transcripts,
            &self.map,
            self.view,
            project.format.default_layout,
        );
        self.chapter = self
            .chapter
            .min(project.cut.chapters.len().saturating_sub(1));
        let frame = keep
            .and_then(|(at, time)| self.map.output_frame(at, time))
            .unwrap_or(self.playhead)
            .min(self.map.frames.saturating_sub(1));
        self.playhead = frame;
        let media = folder.dir.join("media");
        let cues = Cues::of(&folder.dir, project.cut.music.as_ref(), folder.lock());
        self.sound.set_plan(&self.map, media.clone(), cues, frame);
        self.engine.set_plan(
            Plan {
                map: self.map.clone(),
                media,
                effects: project.format.effects(),
                theme: folder.theme.clone(),
            },
            frame,
        );
        self.seeking = true;
        self.clock = None;
        if let Some(selection) = self.selection {
            let valid = self
                .block(selection.at)
                .is_some_and(|b| *selection.words().end() < b.words.len());
            if !valid {
                self.selection = None;
            }
        }
        let takes: Vec<Id> = project
            .cut
            .chapters
            .iter()
            .flat_map(|c| c.takes.iter().map(|t| t.take.clone()))
            .collect();
        for take in takes {
            self.load_peaks(take, cx);
        }
        self.reload_proposals(cx);
    }

    fn load_peaks(&mut self, take: Id, cx: &mut Context<Self>) {
        if self.peaks.contains_key(&take) {
            return;
        }
        self.peaks.insert(take.clone(), None);
        let folder = self.folder.read(cx);
        let path = folder.media(&take, "mic.mp4");
        let length = model::blocks(&folder.project.cut)
            .filter(|(_, edit)| edit.take == take)
            .filter_map(|(_, edit)| edit.ranges.last().map(|r| r.range().to()))
            .max()
            .unwrap_or_default();
        if !path.exists() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let peaks = cx
                .background_spawn(async move { Peaks::read(&path, length) })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(peaks) = peaks {
                    this.peaks.insert(take, Some(Arc::new(peaks)));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Shows the last chapter, after a recording appended it.
    pub fn show_last_chapter(&mut self, cx: &mut Context<Self>) {
        self.chapter = self.map.chapters.len().saturating_sub(1);
        if let Some(chapter) = self.map.chapters.get(self.chapter) {
            self.seek(chapter.frames.start);
        }
        cx.notify();
    }

    fn block(&self, at: TakeAt) -> Option<&BlockDoc> {
        self.doc.iter().find(|b| b.at == at)
    }

    // Playback.

    fn seek(&mut self, frame: u64) {
        let frame = frame.min(self.map.frames.saturating_sub(1));
        self.playhead = frame;
        self.engine.seek(frame);
        self.sound.seek(frame);
        self.seeking = true;
        self.clock = None;
    }

    fn set_playing(&mut self, playing: bool) {
        if playing && self.playhead + 1 >= self.map.frames {
            self.seek(0);
        }
        self.playing = playing;
        self.clock = None;
        self.engine.set_playing(playing);
        if !playing {
            self.sound.set_playing(false);
            self.seek(self.playhead);
        }
    }

    /// Picks the frame for this render.
    fn advance(&mut self) {
        if !self.playing {
            if self.seeking
                && let Some(frame) = self.engine.take_next()
            {
                self.playhead = frame.frame;
                self.shown = Some(frame);
                self.seeking = false;
            }
            return;
        }
        let clock = match self.clock {
            Some(clock) => clock,
            None => {
                let Some(front) = self.engine.front() else {
                    return;
                };
                // Sound starts with the first frame, so neither runs ahead of the other.
                self.sound.set_playing(true);
                self.follower = ClockFollower::default();
                let clock = Clock {
                    started: Instant::now(),
                    from: front,
                };
                self.clock = Some(clock);
                self.seeking = false;
                clock
            }
        };
        let due = if self.sound.is_clock() {
            match self.sound.heard(VIDEO_AHEAD) {
                Some(frame) => self.follower.frame(frame),
                // The device has not played yet: hold the frame on screen.
                None => return,
            }
        } else {
            clock.from + (clock.started.elapsed().as_secs_f64() * f64::from(FPS)) as u64
        };
        if let Some(frame) = self.engine.take_due(due) {
            self.playhead = frame.frame;
            self.shown = Some(frame);
        }
        if due + 1 >= self.map.frames {
            self.playing = false;
            self.engine.set_playing(false);
            self.sound.set_playing(false);
            self.clock = None;
        }
        if let Some(chapter) = self.map.chapter_at(self.playhead) {
            self.chapter = chapter;
        }
    }

    fn toggle_play(&mut self, _: &TogglePlay, _: &mut Window, cx: &mut Context<Self>) {
        self.set_playing(!self.playing);
        cx.notify();
    }

    // Selection.

    fn select_word(&mut self, at: TakeAt, index: usize, extend: bool, cx: &mut Context<Self>) {
        self.proposal = None;
        self.selection = match self.selection {
            Some(selection) if extend && selection.at == at => Some(Selection {
                head: index,
                ..selection
            }),
            _ => Some(Selection::word(at, index)),
        };
        if let Some(frame) = self
            .block(at)
            .and_then(|b| b.words.get(index))
            .and_then(|w| w.output)
        {
            self.seek(frame);
        }
        cx.notify();
    }

    fn move_selection(&mut self, step: isize, extend: bool, cx: &mut Context<Self>) {
        let Some(selection) = self.selection else {
            return;
        };
        let Some(len) = self.block(selection.at).map(|b| b.words.len()) else {
            return;
        };
        let head = selection
            .head
            .saturating_add_signed(step)
            .min(len.saturating_sub(1));
        if extend {
            self.selection = Some(Selection { head, ..selection });
            cx.notify();
        } else {
            self.select_word(selection.at, head, false, cx);
        }
    }

    /// The selected block, its words' media range, and whether it holds kept and removed words.
    fn selected(&self) -> Option<(TakeAt, TimeRange, bool, bool)> {
        let selection = self.selection?;
        let block = self.block(selection.at)?;
        let range = selection.range(&block.words)?;
        let words = &block.words[selection.words()];
        let kept = words.iter().any(|w| w.state == WordState::Kept);
        let removed = words.iter().any(|w| w.state != WordState::Kept);
        Some((selection.at, range, kept, removed))
    }

    // Edits.

    fn edit(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&Cut) -> Result<Cut, EditError>,
    ) {
        let result = self.folder.update(cx, |folder, cx| {
            let cut = change(&folder.project.cut).map_err(|e| e.to_string())?;
            folder.write_cut(cut)?;
            cx.notify();
            Ok::<_, String>(())
        });
        self.error = result.err();
        cx.notify();
    }

    fn delete_selection(&mut self, _: &DeleteSelection, _: &mut Window, cx: &mut Context<Self>) {
        let Some((at, range, true, _)) = self.selected() else {
            return;
        };
        let Some(take) = self.block(at).map(|b| b.take.clone()) else {
            return;
        };
        let Some(transcript) = self.folder.read(cx).transcripts.get(&take).cloned() else {
            return;
        };
        self.selection = None;
        self.edit(cx, |cut| cut.delete_words(at, &transcript, range));
    }

    fn restore_selection(&mut self, _: &RestoreSelection, _: &mut Window, cx: &mut Context<Self>) {
        let Some((at, range, _, true)) = self.selected() else {
            return;
        };
        self.selection = None;
        let result = self.folder.update(cx, |folder, cx| {
            let result = folder.restore(at, range);
            cx.notify();
            result
        });
        self.error = result.err();
        cx.notify();
    }

    fn set_layout(&mut self, key: usize, cx: &mut Context<Self>) {
        let layout = LAYOUT_KEYS[key];
        let target = match self.selected() {
            Some((at, range, ..)) => Some((at, range)),
            None => self.map.stretch_at(self.playhead),
        };
        let Some((at, range)) = target else {
            return;
        };
        let default = self.folder.read(cx).project.format.default_layout;
        self.edit(cx, |cut| apply_layout(cut, at, range, layout, default));
    }

    fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        if self.view != view {
            self.view = view;
            self.selection = None;
            self.rebuild(cx);
            cx.notify();
        }
    }

    fn set_workspace(&mut self, workspace: Workspace, cx: &mut Context<Self>) {
        self.workspace = workspace;
        cx.notify();
    }

    // Export.

    /// Exports the cut named `cut`, the main cut or a derived one, to `.shot/export/<cut>.mp4`.
    pub fn start_export(&mut self, cut: &str, cx: &mut Context<Self>) {
        let running = self.export.as_ref().is_some_and(|s| {
            matches!(
                *s.lock().expect("export status"),
                ExportStatus::Running { .. }
            )
        });
        if running {
            return;
        }
        let folder = self.folder.read(cx);
        let dir = folder.dir.clone();
        let project = folder.project.clone();
        let status = Arc::new(Mutex::new(ExportStatus::Running {
            rendered: 0,
            to_render: 0,
        }));
        self.export = Some(status.clone());
        let relative = format!(".shot/export/{cut}.mp4");
        let out = dir.join(&relative);
        let cut = cut.to_owned();
        std::thread::Builder::new()
            .name("shot-export".into())
            .spawn({
                let status = status.clone();
                move || {
                    let progress = |p: export::Progress| {
                        *status.lock().expect("export status") = ExportStatus::Running {
                            rendered: p.rendered,
                            to_render: p.to_render,
                        };
                    };
                    let result = export::export(
                        &dir,
                        &project,
                        &cut,
                        &out,
                        ExportOptions::default(),
                        &progress,
                    );
                    *status.lock().expect("export status") = match result {
                        Ok(report) => ExportStatus::Done {
                            path: relative,
                            seconds: (report.render_time + report.join_time).as_secs_f64(),
                        },
                        Err(error) => ExportStatus::Failed(error.to_string()),
                    };
                }
            })
            .expect("spawning the export thread");
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let done = !matches!(
                    *status.lock().expect("export status"),
                    ExportStatus::Running { .. }
                );
                if this.update(cx, |_, cx| cx.notify()).is_err() || done {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }
}

impl Focusable for EditScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for EditScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(error) = self.engine.error() {
            self.error.get_or_insert(error);
        }
        if let Some(error) = self.sound.error() {
            self.error.get_or_insert(format!("no sound: {error}"));
        }
        self.advance();
        if self.playing || self.seeking {
            window.request_animation_frame();
        }
        let body = match self.workspace {
            Workspace::Text => text::render(self, window, cx).into_any_element(),
            Workspace::Picture => picture::render(self, window, cx).into_any_element(),
            Workspace::Timeline => timeline::render(self, window, cx).into_any_element(),
            Workspace::Product => product::render(self, window, cx).into_any_element(),
        };
        div()
            .id("edit")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .on_action(cx.listener(Self::toggle_play))
            .on_action(cx.listener(Self::delete_selection))
            .on_action(cx.listener(Self::restore_selection))
            .on_action(cx.listener(|this, _: &ClearSelection, _, cx| {
                if !this.close_picker() && !this.reject_selected(cx) {
                    this.selection = None;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &PickSound, _, cx| {
                this.toggle_picker(place::Picker::Sound, cx)
            }))
            .on_action(cx.listener(|this, _: &AcceptProposal, _, cx| this.accept_selected(cx)))
            .on_action(cx.listener(|this, _: &ShowProposal, _, cx| this.show_next_proposal(cx)))
            .on_action(cx.listener(|this, _: &Layout1, _, cx| this.set_layout(0, cx)))
            .on_action(cx.listener(|this, _: &Layout2, _, cx| this.set_layout(1, cx)))
            .on_action(cx.listener(|this, _: &Layout3, _, cx| this.set_layout(2, cx)))
            .on_action(cx.listener(|this, _: &Layout4, _, cx| this.set_layout(3, cx)))
            .on_action(
                cx.listener(|this, _: &ShowText, _, cx| this.set_workspace(Workspace::Text, cx)),
            )
            .on_action(cx.listener(|this, _: &ShowPicture, _, cx| {
                this.set_workspace(Workspace::Picture, cx)
            }))
            .on_action(cx.listener(|this, _: &ShowTimeline, _, cx| {
                this.set_workspace(Workspace::Timeline, cx)
            }))
            .on_action(
                cx.listener(|this, _: &PreviousWord, _, cx| this.move_selection(-1, false, cx)),
            )
            .on_action(cx.listener(|this, _: &NextWord, _, cx| this.move_selection(1, false, cx)))
            .on_action(
                cx.listener(|this, _: &ExtendPrevious, _, cx| this.move_selection(-1, true, cx)),
            )
            .on_action(cx.listener(|this, _: &ExtendNext, _, cx| this.move_selection(1, true, cx)))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = false),
            )
            .child(body)
            .text_color(TEXT)
    }
}
