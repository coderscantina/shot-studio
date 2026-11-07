//! The app window for one video folder: the title bar with the Record, Cut and Edit screens, and
//! the Board and Script screens when the folder is a video of a channel repo.

use std::path::Path;

use gpui::{
    App, Bounds, Context, Entity, Focusable, FontWeight, Subscription, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, point, prelude::*, px, size,
};
use gpui_component::Root;

use crate::board::BoardScreen;
use crate::cut::{self, ContinueToEdit, CutScreen};
use crate::edit::{self, EditScreen};
use crate::folder::VideoFolder;
use crate::record::{self, RecordScreen, Recorded};
use crate::script::{self, ScriptScreen};
use crate::ship::{self, ShipScreen};
use crate::theme::*;
use crate::ui::icon;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Board,
    Script,
    Record,
    Cut,
    Edit,
    Ship,
}

pub struct Studio {
    folder: Entity<VideoFolder>,
    screen: Screen,
    record: Entity<RecordScreen>,
    cut: Entity<CutScreen>,
    edit: Entity<EditScreen>,
    ship: Entity<ShipScreen>,
    /// Present when the video folder sits in a channel repo, `<repo>/videos/<slug>`.
    planning: Option<(Entity<BoardScreen>, Entity<ScriptScreen>)>,
    _recorded: Subscription,
    _continue: Subscription,
}

pub fn open(dir: &Path, cx: &mut App) -> Result<(), String> {
    let folder = VideoFolder::open(dir)?;
    crate::theme::init(cx);
    edit::bind_keys(cx);
    record::bind_keys(cx);
    cut::bind_keys(cx);
    script::bind_keys(cx);
    ship::bind_keys(cx);
    let bounds = Bounds::centered(None, size(px(1600.), px(1000.)), cx);
    cx.open_window(
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
            cx.new(|cx| Root::new(studio, window, cx))
        },
    )
    .map_err(|e| e.to_string())?;
    cx.activate(true);
    Ok(())
}

impl Studio {
    fn new(folder: Entity<VideoFolder>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let record = cx.new(|cx| RecordScreen::new(folder.clone(), cx));
        let cut = cx.new(|cx| CutScreen::new(folder.clone(), cx));
        let edit = cx.new(|cx| EditScreen::new(folder.clone(), cx));
        let ship = cx.new(|cx| ShipScreen::new(folder.clone(), cx));
        let recorded = cx.subscribe_in(&record, window, |this, _, _: &Recorded, window, cx| {
            this.edit.update(cx, |edit, cx| edit.show_last_chapter(cx));
            this.show(Screen::Cut, window, cx);
        });
        let continue_to_edit =
            cx.subscribe_in(&cut, window, |this, _, _: &ContinueToEdit, window, cx| {
                this.show(Screen::Edit, window, cx);
            });
        window.focus(&edit.focus_handle(cx), cx);
        let planning = channel(&folder.read(cx).dir).map(|(repo, slug)| {
            let full_script =
                folder.read(cx).project.format.prompter == shot_core::PrompterMode::FullScript;
            (
                cx.new(|cx| BoardScreen::new(repo.clone(), slug.clone(), cx)),
                cx.new(|cx| ScriptScreen::new(repo, slug, full_script, window, cx)),
            )
        });
        Self {
            folder,
            screen: Screen::Edit,
            record,
            cut,
            edit,
            ship,
            planning,
            _recorded: recorded,
            _continue: continue_to_edit,
        }
    }

    fn show(&mut self, screen: Screen, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = screen;
        match screen {
            Screen::Board | Screen::Script => {
                if let Some((board, script)) = &self.planning {
                    board.update(cx, |board, _| board.reload());
                    script.update(cx, |script, _| script.reload());
                    let focus = match screen {
                        Screen::Board => board.focus_handle(cx),
                        _ => script.focus_handle(cx),
                    };
                    window.focus(&focus, cx);
                }
            }
            Screen::Record => window.focus(&self.record.focus_handle(cx), cx),
            Screen::Cut => window.focus(&self.cut.focus_handle(cx), cx),
            Screen::Edit => window.focus(&self.edit.focus_handle(cx), cx),
            Screen::Ship => {
                self.ship.update(cx, |ship, cx| ship.reload(cx));
                window.focus(&self.ship.focus_handle(cx), cx);
            }
        }
        cx.notify();
    }

    fn title_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let folder = self.folder.read(cx);
        let title = folder.project.video.title.clone();
        let format = match folder.project.video.format {
            shot_core::FormatKind::Reaction => "Reaction",
            shot_core::FormatKind::Opinion => "Opinion",
            shot_core::FormatKind::ProductTour => "Product tour",
            shot_core::FormatKind::Tutorial => "Tutorial",
        };
        let recording = self.record.read(cx).is_recording();
        let trailing = match self.screen {
            Screen::Edit => self
                .edit
                .update(cx, |edit, cx| edit.title_controls(cx).into_any_element()),
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
            Screen::Cut | Screen::Board | Screen::Script | Screen::Ship => div().into_any_element(),
        };
        let nav =
            |id: &'static str, name: &'static str, icon_name: &'static str, screen: Screen| {
                let on = self.screen == screen;
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
                    .text_color(if on { STRONG } else { MUTED })
                    .when(on, |this| this.bg(RAISED))
                    .when(!on && !recording, |this| {
                        this.cursor_pointer().hover(|s| s.text_color(TEXT))
                    })
                    .child(icon(icon_name, ICON, if on { STRONG } else { MUTED }))
                    .child(name)
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
                    .when(self.planning.is_some(), |this| {
                        this.child(nav("nav-board", "Board", "board", Screen::Board))
                            .child(nav("nav-script", "Script", "script", Screen::Script))
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let screen = match (self.screen, &self.planning) {
            (Screen::Board, Some((board, _))) => board.clone().into_any_element(),
            (Screen::Script, Some((_, script))) => script.clone().into_any_element(),
            (Screen::Board | Screen::Script, None) => div().into_any_element(),
            (Screen::Record, _) => self.record.clone().into_any_element(),
            (Screen::Cut, _) => self.cut.clone().into_any_element(),
            (Screen::Edit, _) => self.edit.clone().into_any_element(),
            (Screen::Ship, _) => self.ship.clone().into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(FRAME)
            .text_color(TEXT)
            .font_family(SANS)
            .text_size(TEXT_BASE)
            .child(self.title_bar(cx))
            .child(div().flex_1().min_h_0().flex().flex_col().child(screen))
    }
}

/// The channel repo and slug of a video folder at `<repo>/videos/<slug>`, if the repo opens as
/// a board.
fn channel(dir: &Path) -> Option<(std::path::PathBuf, String)> {
    let dir = std::path::absolute(dir).ok()?;
    let slug = dir.file_name()?.to_str()?.to_owned();
    let videos = dir.parent()?;
    if videos.file_name()? != "videos" {
        return None;
    }
    let repo = videos.parent()?.to_owned();
    shot_board::Board::open(&repo).ok()?;
    Some((repo, slug))
}
