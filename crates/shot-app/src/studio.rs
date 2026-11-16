//! The app window for one video folder: the title bar with the Record, Cut and Edit screens, and
//! the Board and Script screens when the folder is a video of a channel repo. A video outside a
//! channel keeps a quiet Board entry that says why there is no board and offers to move the
//! video into a channel or open one.

use std::path::{Path, PathBuf};

use gpui::{
    AnyWindowHandle, App, Bounds, Context, Entity, Focusable, FontWeight, Global,
    PathPromptOptions, SharedString, Subscription, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowOptions, actions, div, point, prelude::*, px, size,
};
use gpui_component::Root;
use gpui_component::tooltip::Tooltip;
use shot_core::{Cut, Lock, Status};

use crate::board::{self, BoardScreen, ShowCurrent, channel};
use crate::cut::{CutScreen, GoTo};
use crate::edit::EditScreen;
use crate::folder::VideoFolder;
use crate::record::{RecordScreen, Recorded, ReviewTakes};
use crate::script::{ReadyToRecord, ScriptScreen};
use crate::ship::ShipScreen;
use crate::theme::*;
use crate::ui::{ActionLook, alert, icon, mono, pane, proposal_action};

/// A screen of the studio window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Board,
    Script,
    Record,
    Cut,
    Edit,
    Ship,
}

impl Screen {
    /// The screen of the next step of a video in `status`.
    pub fn for_status(status: Status) -> Self {
        match status {
            Status::Ideas | Status::Scripting => Self::Script,
            Status::Ready => Self::Record,
            Status::Recorded => Self::Cut,
            Status::Editing => Self::Edit,
            Status::Shipped => Self::Ship,
        }
    }

    /// The step a video in `status` is on, `None` once it shipped.
    fn step_of(status: Status) -> Option<Self> {
        (status != Status::Shipped).then(|| Self::for_status(status))
    }

    /// The step of a video outside a channel, from its folder: Record until it has takes, then
    /// Cut while cuts wait for review, then Edit.
    fn step_of_loose(cut: &Cut, lock: &Lock) -> Self {
        if cut.chapters.iter().all(|chapter| chapter.takes.is_empty()) {
            Self::Record
        } else if cut.review_queue(lock).next().is_some() {
            Self::Cut
        } else {
            Self::Edit
        }
    }

    /// How the nav marks this screen for a video on step `next`: steps before it are done.
    /// Board is no step.
    fn mark(self, next: Option<Self>) -> Option<Mark> {
        const STEPS: [Screen; 5] = [
            Screen::Script,
            Screen::Record,
            Screen::Cut,
            Screen::Edit,
            Screen::Ship,
        ];
        let at = STEPS.iter().position(|&s| s == self)?;
        let next = next.map_or(STEPS.len(), |next| {
            STEPS.iter().position(|&s| s == next).unwrap_or(0)
        });
        match at.cmp(&next) {
            std::cmp::Ordering::Less => Some(Mark::Done),
            std::cmp::Ordering::Equal => Some(Mark::Next),
            std::cmp::Ordering::Greater => None,
        }
    }
}

/// The quiet state of a step in the nav.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mark {
    Done,
    Next,
}

pub struct Studio {
    folder: Entity<VideoFolder>,
    screen: Screen,
    record: Entity<RecordScreen>,
    cut: Entity<CutScreen>,
    edit: Entity<EditScreen>,
    ship: Entity<ShipScreen>,
    /// Present when the video folder sits in a channel repo, `<repo>/videos/<slug>`.
    planning: Option<Planning>,
    /// The status of the video's card, read again on every screen switch and when the window
    /// comes back. `None` outside a channel or when the board cannot be read.
    status: Option<Status>,
    /// Why the last attempt to move the video into a channel or open one failed.
    channel_error: Option<SharedString>,
    /// The nav entry under the pointer. Kept here because a hover style stays lit when the
    /// pointer leaves the window over it.
    hovered: Option<Screen>,
    _subscriptions: Vec<Subscription>,
}

/// The Board and Script screens of a video in a channel.
struct Planning {
    repo: PathBuf,
    slug: String,
    board: Entity<BoardScreen>,
    script: Entity<ScriptScreen>,
}

/// The open studio windows by video folder.
#[derive(Default)]
struct Studios(Vec<(PathBuf, AnyWindowHandle, WeakEntity<Studio>)>);

impl Global for Studios {}

/// The path a window is known by: canonical where it exists, so `/tmp` and `/private/tmp` or a
/// trailing slash open the same window.
pub fn key(dir: &Path) -> PathBuf {
    dir.canonicalize()
        .or_else(|_| std::path::absolute(dir))
        .unwrap_or_else(|_| dir.to_owned())
}

actions!(studio, [Undo, Redo, OpenLibrary]);

/// Whether a take of the cut has no transcript yet.
fn untranscribed(folder: &VideoFolder) -> bool {
    folder
        .project
        .cut
        .chapters
        .iter()
        .flat_map(|chapter| &chapter.takes)
        .any(|edit| !folder.transcripts.contains_key(&edit.take))
}

/// Opens a window on the video folder at `dir` and adds it to the recent videos. Needs
/// [`crate::theme::init`] and [`crate::keymap::bind`].
pub fn open(dir: &Path, cx: &mut App) -> Result<(), String> {
    open_at(dir, None, cx)
}

/// Like [`open`], on `screen` instead of the screen of the video's next step. A folder that is
/// open already comes to the front and shows `screen`. A video of a channel opens in its
/// worktree, given its folder there or in the main checkout (see [`channel::prepare`]).
pub fn open_at(dir: &Path, screen: Option<Screen>, cx: &mut App) -> Result<(), String> {
    let dir = match channel::locate(dir) {
        Some((repo, slug)) => key(&channel::prepare(&repo, &slug)?),
        None => key(dir),
    };
    let open = cx.try_global::<Studios>().and_then(|studios| {
        studios
            .0
            .iter()
            .find(|(d, ..)| *d == dir)
            .and_then(|(_, window, studio)| Some((*window, studio.upgrade()?)))
    });
    if let Some((window, studio)) = open {
        let shown = window.update(cx, |_, window, cx| {
            window.activate_window();
            if let Some(screen) = screen {
                studio.update(cx, |studio, cx| studio.show(screen, window, cx));
            }
        });
        if shown.is_ok() {
            return Ok(());
        }
    }
    let folder = VideoFolder::open(&dir)?;
    let bounds = Bounds::centered(None, size(px(1600.), px(1000.)), cx);
    let mut opened = None;
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(1280.), px(800.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(folder.project.video.title.clone().into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(16.))),
                }),
                // The preview plays at 60 fps, also while another window has focus.
                inactive_frame_interval: None,
                low_power_frame_interval: None,
                thermal_frame_interval: None,
                ..Default::default()
            },
            |window, cx| {
                let folder = cx.new(|_| folder);
                let studio = cx.new(|cx| Studio::new(folder, window, cx));
                let screen = screen.unwrap_or_else(|| studio.read(cx).first_screen(cx));
                studio.update(cx, |studio, cx| studio.show(screen, window, cx));
                opened = Some(studio.downgrade());
                cx.new(|cx| Root::new(studio, window, cx))
            },
        )
        .map_err(|e| e.to_string())?;
    if let Some(studio) = opened {
        let studios = cx.default_global::<Studios>();
        studios
            .0
            .retain(|(d, _, s)| *d != dir && s.upgrade().is_some());
        studios.0.push((dir.clone(), window.into(), studio));
    }
    crate::welcome::remember(&dir);
    cx.activate(true);
    Ok(())
}

impl Studio {
    fn new(folder: Entity<VideoFolder>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let record = cx.new(|cx| RecordScreen::new(folder.clone(), cx));
        let cut = cx.new(|cx| CutScreen::new(folder.clone(), cx));
        let edit = cx.new(|cx| EditScreen::new(folder.clone(), cx));
        let ship = cx.new(|cx| ShipScreen::new(folder.clone(), cx));
        // A finished take stays on Record, so the next part records right away; Review opens
        // the takes.
        let recorded = cx.subscribe(&record, |this, _, _: &Recorded, cx| {
            this.edit.update(cx, |edit, cx| edit.show_last_chapter(cx));
            this.cut.update(cx, |cut, cx| cut.recorded(cx));
            this.refresh_status();
            cx.notify();
        });
        let review = cx.subscribe_in(&record, window, |this, _, _: &ReviewTakes, window, cx| {
            this.cut.update(cx, |cut, cx| cut.show_takes(cx));
            this.show(Screen::Cut, window, cx);
        });
        let continue_to_edit =
            cx.subscribe_in(&cut, window, |this, _, GoTo(screen), window, cx| {
                this.show(*screen, window, cx);
            });
        let edit_to_record = cx.subscribe_in(&edit, window, |this, _, GoTo(screen), window, cx| {
            this.show(*screen, window, cx);
        });
        window.focus(&edit.focus_handle(cx), cx);
        // Focus that drops out of the window, as when the focused element stops rendering,
        // goes back to the shown screen instead of leaving its shortcuts dead.
        let focus_lost = cx.on_focus_lost(window, |this, window, cx| this.focus_screen(window, cx));
        let mut subscriptions = vec![
            recorded,
            review,
            continue_to_edit,
            edit_to_record,
            focus_lost,
        ];
        // Cards move on the board and in other windows: the nav catches up when this comes back.
        // A script saved elsewhere reaches the prompter the same way.
        subscriptions.push(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.refresh_status();
                if this.screen == Screen::Record {
                    this.take_script(cx);
                }
                cx.notify();
            }
        }));
        let planning = channel::locate(&folder.read(cx).dir).map(|(repo, slug)| {
            let full_script =
                folder.read(cx).project.format.prompter == shot_core::PrompterMode::FullScript;
            let board = cx.new(|cx| BoardScreen::new(repo.clone(), Some(slug.clone()), cx));
            subscriptions.push(cx.subscribe_in(
                &board,
                window,
                |this, _, ShowCurrent(screen), window, cx| this.show(*screen, window, cx),
            ));
            let script =
                cx.new(|cx| ScriptScreen::new(repo.clone(), slug.clone(), full_script, window, cx));
            subscriptions.push(cx.subscribe_in(
                &script,
                window,
                |this, _, _: &ReadyToRecord, window, cx| {
                    this.take_script(cx);
                    this.show(Screen::Record, window, cx);
                },
            ));
            Planning {
                repo,
                slug,
                board,
                script,
            }
        });
        let mut studio = Self {
            folder,
            screen: Screen::Edit,
            record,
            cut,
            edit,
            ship,
            planning,
            status: None,
            channel_error: None,
            hovered: None,
            _subscriptions: subscriptions,
        };
        studio.refresh_status();
        studio
    }

    /// Puts the latest script into the folder, so Record prompts what the Script screen or an
    /// editor saved last: the board's copy on the video's branch for a video of a channel,
    /// where Script accepts its changes, else the folder's `script.md`. The prompter starts
    /// over only when the script changed, and never while a take records.
    fn take_script(&mut self, cx: &mut Context<Self>) {
        if self.record.read(cx).is_recording() {
            return;
        }
        let path = self.folder.read(cx).dir.join("script.md");
        let text = match &self.planning {
            Some(planning) => shot_board::Board::open(&planning.repo)
                .ok()
                .and_then(|board| board.read_file(&planning.slug, "script.md").ok().flatten()),
            None => std::fs::read_to_string(&path).ok(),
        };
        let Some(text) = text else {
            return;
        };
        let Ok(script) = shot_core::Script::parse(&text) else {
            return;
        };
        if script == self.folder.read(cx).project.script {
            return;
        }
        // A folder that cannot be written keeps prompting its own copy.
        if self.planning.is_some()
            && std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str())
            && shot_fs::write(&path, &text).is_err()
        {
            return;
        }
        self.folder.update(cx, |folder, cx| {
            folder.project.script = script;
            cx.notify();
        });
        self.record
            .update(cx, |record, cx| record.reload_script(cx));
    }

    fn refresh_status(&mut self) {
        if let Some(planning) = &self.planning {
            self.status = channel::status(&planning.repo, &planning.slug);
        }
    }

    /// The step this video is on, `None` once it shipped: from its card for a video of a
    /// channel, else from what its folder holds. Edit and Ship work on words, so a take of the
    /// cut without a transcript keeps the video on Cut, where it is transcribed.
    fn step(&self, cx: &App) -> Option<Screen> {
        let folder = self.folder.read(cx);
        let project = &folder.project;
        let step = match &self.planning {
            Some(_) => Screen::step_of(self.status.unwrap_or(project.video.status)),
            None => Some(Screen::step_of_loose(&project.cut, folder.lock())),
        };
        match step {
            Some(Screen::Edit | Screen::Ship) if untranscribed(folder) => Some(Screen::Cut),
            step => step,
        }
    }

    /// The screen a new window opens on: the video's next step. A video outside a channel has
    /// no status, so it opens on Record until it has takes, on Cut while a take waits for its
    /// transcript and on Edit after.
    fn first_screen(&self, cx: &App) -> Screen {
        match self.step(cx) {
            None => Screen::Ship,
            Some(Screen::Record) => Screen::Record,
            Some(Screen::Cut) if untranscribed(self.folder.read(cx)) => Screen::Cut,
            Some(_) if self.planning.is_none() => Screen::Edit,
            Some(step) => step,
        }
    }

    fn show(&mut self, screen: Screen, window: &mut Window, cx: &mut Context<Self>) {
        if self.record.read(cx).is_recording() {
            return;
        }
        // A video outside a channel has no Script screen: its next step is in Edit.
        let screen = match (screen, &self.planning) {
            (Screen::Script, None) => Screen::Edit,
            _ => screen,
        };
        self.screen = screen;
        self.refresh_status();
        match screen {
            Screen::Board | Screen::Script => {
                if let Some(Planning { board, script, .. }) = &self.planning {
                    board.update(cx, |board, _| board.reload());
                    script.update(cx, |script, _| script.reload());
                }
            }
            Screen::Record => self.take_script(cx),
            Screen::Cut => {}
            Screen::Edit => self.edit.update(cx, |edit, cx| edit.reload_proposals(cx)),
            Screen::Ship => self.ship.update(cx, |ship, cx| ship.reload(cx)),
        }
        self.focus_screen(window, cx);
        cx.notify();
    }

    /// Gives the shown screen the keyboard, so its single-key shortcuts work right away.
    fn focus_screen(&self, window: &mut Window, cx: &mut App) {
        let focus = match (self.screen, &self.planning) {
            (Screen::Board, Some(planning)) => planning.board.focus_handle(cx),
            (Screen::Script, Some(planning)) => planning.script.focus_handle(cx),
            (Screen::Board | Screen::Script, None) => return,
            (Screen::Record, _) => self.record.focus_handle(cx),
            (Screen::Cut, _) => self.cut.focus_handle(cx),
            (Screen::Edit, _) => self.edit.focus_handle(cx),
            (Screen::Ship, _) => self.ship.focus_handle(cx),
        };
        window.focus(&focus, cx);
    }

    /// Undoes or redoes the last change of `cut.kdl`, while nothing records.
    fn step_history(&mut self, undo: bool, cx: &mut Context<Self>) {
        if self.record.read(cx).is_recording() {
            return;
        }
        let result = self.folder.update(cx, |folder, cx| {
            let result = if undo { folder.undo() } else { folder.redo() };
            cx.notify();
            result
        });
        self.edit
            .update(cx, |edit, cx| edit.stepped_history(result.err(), cx));
    }

    /// Opens the channel's theme window on `section`. A video outside a channel has no brand.
    fn open_brand(&mut self, section: crate::brand::Section, cx: &mut Context<Self>) {
        let Some(planning) = &self.planning else {
            return;
        };
        if let Err(error) = crate::brand::open(&planning.repo, section, cx) {
            self.report(error, cx);
        }
    }

    /// The Board screen of a video outside a channel: why there is no board, and the way into
    /// one.
    fn no_channel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dir = self.folder.read(cx).dir.clone();
        let parent = dir.parent().map(Path::to_owned).unwrap_or_default();
        div().size_full().p(px(8.)).pt(px(0.)).child(
            pane()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(560.))
                        .flex()
                        .flex_col()
                        .gap(px(14.))
                        .child(
                            div()
                                .text_size(px(21.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(STRONG)
                                .child("This video is not in a channel"),
                        )
                        .child(div().text_color(BODY).line_height(px(23.)).child(
                            "The board plans the videos of a channel: a git repo that keeps each \
                             video in videos/<name>, as a card from idea to shipped, with the \
                             Script screen and Claude's planning next to it. A folder on its own \
                             records, cuts, edits and ships just the same.",
                        ))
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .text_size(TEXT_SM)
                                .text_color(MUTED)
                                .child("This folder:")
                                .child(
                                    mono(crate::welcome::shown_path(&dir))
                                        .min_w_0()
                                        .truncate()
                                        .text_color(KBD_TEXT),
                                ),
                        )
                        .when_some(self.channel_error.clone(), |this, error| {
                            this.child(alert(error))
                        })
                        .child(
                            div()
                                .pt(px(6.))
                                .flex()
                                .gap(px(8.))
                                .child(
                                    proposal_action(
                                        "move-into-channel",
                                        "Move into a channel…",
                                        Some("board"),
                                        None,
                                        ActionLook::Plain,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, window, cx| this.move_into_channel(window, cx),
                                    )),
                                )
                                .child(
                                    proposal_action(
                                        "open-channel",
                                        "Open a channel…",
                                        None,
                                        None,
                                        ActionLook::Quiet,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.channel_error = None;
                                            let done = this.reporter(cx);
                                            crate::welcome::pick_channel(done, cx);
                                        },
                                    )),
                                )
                                .child(
                                    proposal_action(
                                        "new-channel",
                                        "Start a channel…",
                                        None,
                                        None,
                                        ActionLook::Quiet,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.channel_error = None;
                                            let done = this.reporter(cx);
                                            crate::welcome::new_channel(&parent, done, cx);
                                        },
                                    )),
                                ),
                        )
                        .child(
                            div()
                                .text_size(TEXT_SM)
                                .text_color(MUTED)
                                .line_height(px(20.))
                                .child(
                                    "Moving puts this folder in the channel's videos/ folder and \
                                     adds it to the board. Recordings move along and stay out of \
                                     git.",
                                ),
                        ),
                ),
        )
    }

    /// Asks for a channel, moves this video into it and opens it again there, on the Board.
    fn move_into_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.channel_error = None;
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Move into this channel".into()),
        });
        let window = window.window_handle();
        cx.spawn(async move |this, cx| {
            let Some(repo) = picked
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
                .and_then(|paths| paths.into_iter().next())
            else {
                return;
            };
            this.update(cx, |this, cx| {
                if this.record.read(cx).is_recording() {
                    return;
                }
                let dir = this.folder.read(cx).dir.clone();
                match channel::move_into(&dir, &repo) {
                    Ok(moved) => {
                        // The old folder is gone: this window closes and the video opens again
                        // from its new place.
                        window
                            .update(cx, |_, window, _| window.remove_window())
                            .ok();
                        if let Err(error) = open_at(&moved, Some(Screen::Board), cx) {
                            crate::welcome::show_error(error, cx);
                        }
                    }
                    Err(error) => this.report(error, cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// Where a channel picker started here reports why it failed.
    fn reporter(
        &self,
        cx: &mut Context<Self>,
    ) -> impl FnOnce(Result<(), String>, &mut App) + use<> {
        let studio = cx.weak_entity();
        move |result, cx| {
            if let Err(error) = result {
                studio
                    .update(cx, |studio, cx| studio.report(error, cx))
                    .ok();
            }
        }
    }

    fn report(&mut self, error: String, cx: &mut Context<Self>) {
        self.channel_error = Some(error.into());
        cx.notify();
    }

    /// `compact` below 1500 px wide, where the workspace switcher shows icons only.
    fn title_bar(&mut self, compact: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let folder = self.folder.read(cx);
        let title = folder.project.video.title.clone();
        let format = board::format_name(folder.project.video.format);
        let recording = self.record.read(cx).is_recording();
        let trailing = match self.screen {
            Screen::Edit => self.edit.update(cx, |edit, cx| {
                edit.title_controls(compact, cx).into_any_element()
            }),
            Screen::Record => div()
                .when(recording, |this| {
                    this.flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(6.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(RECORD_TEXT)
                        .child(
                            div()
                                .size(px(9.))
                                .rounded_full()
                                .bg(RECORD)
                                .border_4()
                                .border_color(gpui::Hsla::from(RECORD).opacity(0.18)),
                        )
                        .child("Recording")
                })
                .into_any_element(),
            Screen::Cut => self
                .cut
                .update(cx, |cut, cx| cut.title_controls(cx).into_any_element()),
            Screen::Board | Screen::Script | Screen::Ship => div().into_any_element(),
        };
        let step = self.step(cx);
        let nav =
            |id: &'static str, name: &'static str, icon_name: &'static str, screen: Screen| {
                let on = self.screen == screen;
                let mark = screen.mark(step);
                // Without a channel the Board entry stays quiet: it only explains.
                let quiet = screen == Screen::Board && self.planning.is_none();
                let hovered = !on && !recording && self.hovered == Some(screen);
                let color = match (on, quiet, hovered) {
                    (true, ..) => STRONG,
                    (false, _, true) => TEXT,
                    (false, true, false) => FAINT,
                    (false, false, false) => MUTED,
                };
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px(px(12.))
                    .py(px(5.))
                    .rounded(RADIUS_SM)
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(color)
                    .when(on, |this| this.bg(RAISED))
                    .when(!on && !recording, |this| this.cursor_pointer())
                    .when(!on && recording, |this| {
                        this.tooltip(|window, cx| {
                            Tooltip::new("Stop recording first").build(window, cx)
                        })
                    })
                    // Only a real move lights an entry: after the pointer left the window, gpui
                    // reports a hover again from where it was last seen.
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.hovered != Some(screen) {
                            this.hovered = Some(screen);
                            cx.notify();
                        }
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if !hovered && this.hovered == Some(screen) {
                            this.hovered = None;
                            cx.notify();
                        }
                    }))
                    .child(icon(icon_name, ICON, color))
                    .child(name)
                    .when(mark == Some(Mark::Done), |this| {
                        this.child(icon("check", ICON_SM, color))
                    })
                    .when(mark == Some(Mark::Next), |this| {
                        this.child(div().size(px(6.)).rounded_full().bg(ACCENT))
                    })
                    .when(!recording, |this| {
                        this.on_click(
                            cx.listener(move |this, _, window, cx| this.show(screen, window, cx)),
                        )
                    })
            };
        div()
            .h(px(46.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(14.))
            .pl(px(92.))
            .pr(px(12.))
            .child(
                div()
                    .flex_1()
                    .flex_basis(px(0.))
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(div().truncate().child(title))
                    .child(
                        div()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(MUTED)
                            .child(format),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(nav("nav-board", "Board", "board", Screen::Board))
                    .when(self.planning.is_some(), |this| {
                        this.child(nav("nav-script", "Script", "script", Screen::Script))
                    })
                    .child(nav("nav-record", "Record", "record", Screen::Record))
                    .child(nav("nav-cut", "Cut", "cut", Screen::Cut))
                    .child(nav("nav-edit", "Edit", "edit", Screen::Edit))
                    .child(nav("nav-ship", "Ship", "export", Screen::Ship)),
            )
            .child(
                div()
                    .flex_1()
                    .flex_basis(px(0.))
                    .min_w_0()
                    .flex()
                    .justify_end()
                    .child(trailing),
            )
    }
}

impl Render for Studio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let compact = window.viewport_size().width < px(1500.);
        let screen = match (self.screen, &self.planning) {
            (Screen::Board, Some(planning)) => planning.board.clone().into_any_element(),
            (Screen::Script, Some(planning)) => planning.script.clone().into_any_element(),
            (Screen::Board | Screen::Script, None) => self.no_channel(cx).into_any_element(),
            (Screen::Record, _) => self.record.clone().into_any_element(),
            (Screen::Cut, _) => self.cut.clone().into_any_element(),
            (Screen::Edit, _) => self.edit.clone().into_any_element(),
            (Screen::Ship, _) => self.ship.clone().into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.step_history(true, cx)))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.step_history(false, cx)))
            .on_action(cx.listener(|this, _: &crate::brand::OpenTheme, _, cx| {
                this.open_brand(crate::brand::Section::Colors, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::brand::OpenOverlays, _, cx| {
                this.open_brand(crate::brand::Section::Overlays, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenLibrary, _, cx| {
                let dir = this.folder.read(cx).dir.clone();
                if let Err(error) = crate::library::open(&dir, this.edit.downgrade(), cx) {
                    this.report(error, cx);
                }
            }))
            .bg(FRAME)
            .text_color(TEXT)
            .font_family(SANS)
            .text_size(TEXT_BASE)
            .child(self.title_bar(compact, cx))
            .child(div().flex_1().min_h_0().flex().flex_col().child(screen))
    }
}

/// Rebuilds the lock of every open video of the channel at `repo` after its brand changed, so
/// their screens draw the new look.
pub fn brand_changed(repo: &Path, cx: &mut App) {
    let repo = key(repo);
    let folders: Vec<Entity<VideoFolder>> = cx
        .try_global::<Studios>()
        .into_iter()
        .flat_map(|studios| &studios.0)
        .filter(|(dir, ..)| shot_fs::channel_root(dir) == Some(repo.as_path()))
        .filter_map(|(_, _, studio)| studio.upgrade())
        .map(|studio| studio.read(cx).folder.clone())
        .collect();
    for folder in folders {
        folder.update(cx, |folder, cx| {
            // A video whose lock cannot be built shows why on its next edit.
            let _ = folder.rebuild_lock();
            cx.notify();
        });
    }
}

/// The channel repo and slug of a video folder, see [`channel::locate`].
pub(crate) fn channel(dir: &Path) -> Option<(PathBuf, String)> {
    channel::locate(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nav_marks_steps_before_the_next_one_done() {
        let next = Some(Screen::Cut);
        assert_eq!(Screen::Script.mark(next), Some(Mark::Done));
        assert_eq!(Screen::Record.mark(next), Some(Mark::Done));
        assert_eq!(Screen::Cut.mark(next), Some(Mark::Next));
        assert_eq!(Screen::Edit.mark(next), None);
        assert_eq!(Screen::Board.mark(next), None);
        assert_eq!(
            Screen::Ship.mark(Screen::step_of(Status::Shipped)),
            Some(Mark::Done)
        );
    }

    #[test]
    fn a_loose_video_is_on_record_until_it_has_takes() {
        assert_eq!(
            Screen::step_of_loose(&Cut::default(), &Lock::default()),
            Screen::Record
        );
    }
}
