//! The Board: the channel repo as a Kanban board, in a window of its own per channel and as a
//! screen of the studio for a video of the channel. Columns are `video.kdl` statuses, moving a
//! card (by drag and drop or ⌥← ⌥→) is one commit through `shot-board`, a new video is a branch
//! and a folder, and opening a card opens its folder in the studio on the screen of its next
//! step. The bottom line is the git log of the video branches, shipped videos show their tag.
//!
//! The board holds no state of its own: it rebuilds from git after every change and whenever
//! its window comes to the front. "Plan the week with Claude" runs the planning prompt through
//! the `claude` bridge.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{
    AnyWindowHandle, App, Bounds, ClickEvent, Context, Div, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, Global, Hsla, KeyBinding, Rgba, SharedString, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, point, prelude::*, px,
    size,
};
use gpui_component::Root;
use gpui_component::input::{Escape, Input, InputEvent, InputState};
use shot_board::{Activity, Board, Card, ReleaseTag};
use shot_core::{Cut, FormatKind, Lock, Project, Script, Status, Timeline, Video};
use shot_mcp::bridge::{Claude, Reach};
use shot_ship::lock::LOCK_FILE;
use shot_ship::script_check;

pub mod channel;

use crate::agent::{self, AgentRun};
use crate::studio::{self, Screen};
use crate::theme::*;
use crate::ui::{ActionLook, Segment, alert, hint, kbd, mono, pane, proposal_action, segmented};

/// Videos published per week, the planning target.
const CADENCE: u8 = 2;
/// Commits shown in the activity line.
const ACTIVITY: usize = 3;
const CONTEXT: &str = "Board";

/// The formats in the order the new video picker lists them.
pub const FORMATS: [FormatKind; 4] = [
    FormatKind::Tutorial,
    FormatKind::ProductTour,
    FormatKind::Opinion,
    FormatKind::Reaction,
];

actions!(
    board,
    [
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        MoveLeft,
        MoveRight,
        OpenSelected,
        NewVideo,
        Deselect
    ]
);

/// Binds the board's keys. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("left", SelectLeft, context),
        KeyBinding::new("right", SelectRight, context),
        KeyBinding::new("up", SelectUp, context),
        KeyBinding::new("down", SelectDown, context),
        KeyBinding::new("alt-left", MoveLeft, context),
        KeyBinding::new("alt-right", MoveRight, context),
        KeyBinding::new("enter", OpenSelected, context),
        KeyBinding::new("n", NewVideo, context),
        KeyBinding::new("escape", Deselect, context),
    ]);
}

/// The open board windows by channel repo.
#[derive(Default)]
struct Boards(Vec<(PathBuf, AnyWindowHandle)>);

impl Global for Boards {}

/// Opens a window on the board of the channel at `repo`, or brings its open one to the front,
/// and adds the channel to the recent channels.
pub fn open_window(repo: &Path, cx: &mut App) -> Result<(), String> {
    let repo = studio::key(repo);
    Board::open(&repo).map_err(|_| {
        format!(
            "{} is not a channel: a channel is the root folder of a git repo",
            repo.display()
        )
    })?;
    crate::welcome::remember_channel(&repo);
    let open = cx
        .try_global::<Boards>()
        .and_then(|boards| boards.0.iter().find(|(r, _)| *r == repo).map(|(_, w)| *w));
    if let Some(window) = open
        && window
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return Ok(());
    }
    let name = channel::name(&repo);
    let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(1280.), px(800.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(name.into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(16.), px(16.))),
                }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| BoardWindow::new(repo.clone(), window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .map_err(|e| e.to_string())?;
    let boards = cx.default_global::<Boards>();
    boards.0.retain(|(r, _)| *r != repo);
    boards.0.push((repo, window.into()));
    cx.activate(true);
    Ok(())
}

/// A board window: the channel's name in the title bar over its board.
pub struct BoardWindow {
    repo: PathBuf,
    board: Entity<BoardScreen>,
    _activation: Subscription,
}

impl BoardWindow {
    fn new(repo: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let board = cx.new(|cx| BoardScreen::new(repo.clone(), None, cx));
        window.focus(&board.focus_handle(cx), cx);
        // Videos change in their studio windows: the board catches up when it comes back.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.board.update(cx, |board, cx| {
                    board.reload();
                    cx.notify();
                });
            }
        });
        Self {
            repo,
            board,
            _activation: activation,
        }
    }
}

impl Render for BoardWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(FRAME)
            .text_color(TEXT)
            .font_family(SANS)
            .text_size(TEXT_BASE)
            .child(
                div()
                    .h(px(46.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pl(px(92.))
                    .pr(px(12.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(channel::name(&self.repo)),
                    )
                    .child(
                        mono(crate::welcome::shown_path(&self.repo))
                            .min_w_0()
                            .truncate()
                            .text_size(TEXT_SM)
                            .text_color(KBD_TEXT)
                            .px(px(8.))
                            .py(px(3.))
                            .rounded(px(6.))
                            .bg(CARD),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(self.board.clone()),
            )
    }
}

/// Asks the studio of the video the board belongs to to show the screen of a card's next step.
pub struct ShowCurrent(pub Screen);

/// What a card says besides its title and format.
struct CardInfo {
    /// The edited length once there is a cut, else the spoken length of the script.
    length: Option<String>,
    next: Option<Next>,
}

/// The next step of a video, and whether it waits on the person.
#[derive(Debug, PartialEq, Eq)]
struct Next {
    text: String,
    waiting: bool,
}

/// The new video being named, above the columns.
struct Composer {
    input: Entity<InputState>,
    format: FormatKind,
    _events: Subscription,
}

pub struct BoardScreen {
    repo: PathBuf,
    /// The video of the studio window the board is part of, highlighted. `None` in a board
    /// window.
    current: Option<String>,
    cards: Vec<Card>,
    info: HashMap<String, CardInfo>,
    activity: Vec<Activity>,
    releases: Vec<ReleaseTag>,
    filter: Option<FormatKind>,
    /// The card the arrow keys, Enter and ⌥← ⌥→ act on.
    selected: Option<String>,
    composer: Option<Composer>,
    error: Option<String>,
    claude: Option<Claude>,
    plan: Option<AgentRun>,
    plan_open: bool,
    focus: FocusHandle,
}

impl EventEmitter<ShowCurrent> for BoardScreen {}

/// A card being dragged to another column.
#[derive(Clone)]
struct Dragged {
    slug: String,
    title: SharedString,
}

impl Render for Dragged {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(220.))
            .p(px(12.))
            .rounded(RADIUS_MD)
            .bg(FLOAT)
            .shadow_lg()
            .text_color(STRONG)
            .font_family(SANS)
            .child(self.title.clone())
    }
}

impl Focusable for BoardScreen {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl BoardScreen {
    pub fn new(repo: PathBuf, current: Option<String>, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            repo,
            selected: current.clone(),
            current,
            cards: Vec::new(),
            info: HashMap::new(),
            activity: Vec::new(),
            releases: Vec::new(),
            filter: None,
            composer: None,
            error: None,
            claude: Claude::find(),
            plan: None,
            plan_open: false,
            focus: cx.focus_handle(),
        };
        screen.reload();
        screen
    }

    /// Rebuilds the board from git.
    pub fn reload(&mut self) {
        let result = Board::open(&self.repo).and_then(|board| {
            let cards = board.cards()?;
            let proposals = board.proposals(None).unwrap_or_default();
            let info = cards
                .iter()
                .filter_map(|card| {
                    let video = card.video.as_ref().ok()?;
                    let open = proposals.iter().filter(|p| p.slug == card.slug).count();
                    Some((
                        card.slug.clone(),
                        card_info(&board, &card.slug, video, open),
                    ))
                })
                .collect();
            Ok((cards, info, board.activity(ACTIVITY)?, board.releases()?))
        });
        match result {
            Ok((cards, info, activity, releases)) => {
                (self.cards, self.info, self.activity, self.releases) =
                    (cards, info, activity, releases);
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn move_card(&mut self, slug: &str, status: Status, cx: &mut Context<Self>) {
        let moved = Board::open(&self.repo).and_then(|b| b.move_card(slug, status));
        self.reload();
        if let Err(e) = moved {
            self.error = Some(e.to_string());
        }
        cx.notify();
    }

    /// Opens a card's video in the studio on the screen of its next step: here when it is this
    /// window's video, else in its own window.
    fn open(&mut self, slug: &str, status: Status, cx: &mut Context<Self>) {
        let screen = Screen::for_status(status);
        if self.current.as_deref() == Some(slug) {
            cx.emit(ShowCurrent(screen));
            return;
        }
        let dir = self.repo.join("videos").join(slug);
        if let Err(e) = studio::open_at(&dir, Some(screen), cx) {
            self.error = Some(e);
            cx.notify();
        }
    }

    fn status_of(&self, slug: &str) -> Option<Status> {
        self.cards
            .iter()
            .find(|c| c.slug == slug)
            .and_then(|c| c.video.as_ref().ok())
            .map(|v| v.status)
    }

    /// The slugs of the cards shown in each column, in column order.
    fn grid(&self) -> Vec<Vec<String>> {
        Status::ALL
            .iter()
            .map(|&status| {
                self.cards
                    .iter()
                    .filter(|c| {
                        c.video.as_ref().is_ok_and(|v| {
                            v.status == status && self.filter.is_none_or(|f| v.format == f)
                        })
                    })
                    .map(|c| c.slug.clone())
                    .collect()
            })
            .collect()
    }

    fn select(&mut self, step: Step, cx: &mut Context<Self>) {
        self.selected = stepped(&self.grid(), self.selected.as_deref(), step);
        cx.notify();
    }

    fn move_selected(&mut self, by: isize, cx: &mut Context<Self>) {
        let Some(slug) = self.selected.clone() else {
            return;
        };
        let Some(status) = self.status_of(&slug) else {
            return;
        };
        let index = Status::ALL.iter().position(|&s| s == status).unwrap_or(0);
        if let Some(&to) = index
            .checked_add_signed(by)
            .and_then(|i| Status::ALL.get(i))
        {
            self.move_card(&slug, to, cx);
        }
    }

    fn open_selected(&mut self, _: &OpenSelected, _: &mut Window, cx: &mut Context<Self>) {
        let Some(slug) = self.selected.clone() else {
            return;
        };
        if let Some(status) = self.status_of(&slug) {
            self.open(&slug, status, cx);
        }
    }

    fn deselect(&mut self, _: &Deselect, _: &mut Window, cx: &mut Context<Self>) {
        if self.plan_open {
            self.plan_open = false;
        } else {
            self.selected = None;
        }
        cx.notify();
    }

    fn start_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(composer) = &self.composer {
            composer
                .input
                .update(cx, |input, cx| input.focus(window, cx));
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Title of the new video"));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.create(window, cx);
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.composer = Some(Composer {
            input,
            format: self.filter.unwrap_or(FormatKind::Tutorial),
            _events: events,
        });
        cx.notify();
    }

    fn close_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(composer) = &self.composer else {
            return;
        };
        let title = composer.input.read(cx).value().trim().to_owned();
        if title.is_empty() {
            return;
        }
        match channel::new_video(&self.repo, &title, composer.format) {
            Ok(slug) => {
                self.filter = self.filter.filter(|&f| f == composer.format);
                self.selected = Some(slug);
                self.reload();
                self.close_composer(window, cx);
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn plan_week(&mut self, cx: &mut Context<Self>) {
        self.plan_open = true;
        let Some(claude) = self.claude.clone() else {
            cx.notify();
            return;
        };
        if self.plan.as_ref().is_some_and(AgentRun::running) {
            cx.notify();
            return;
        }
        match AgentRun::start(
            claude,
            &self.repo,
            "Plan the week".to_owned(),
            shot_mcp::agents::plan_week(CADENCE),
            Reach::Project,
        ) {
            Ok(run) => {
                self.plan = Some(run);
                agent::watch(
                    cx,
                    |this: &mut Self| this.plan.as_mut(),
                    |this, _| this.reload(),
                );
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let next = self
            .cards
            .iter()
            .filter_map(|c| c.video.as_ref().ok())
            .filter(|v| v.status != Status::Shipped)
            .filter_map(|v| v.target_date)
            .min_by_key(|d| (d.year(), d.month(), d.day()));
        let filters = [
            ("All", None),
            ("Reaction", Some(FormatKind::Reaction)),
            ("Opinion", Some(FormatKind::Opinion)),
            ("Product", Some(FormatKind::ProductTour)),
            ("Tutorial", Some(FormatKind::Tutorial)),
        ];
        let segments = filters
            .into_iter()
            .map(|(name, kind)| {
                let entity = cx.entity();
                Segment::new(name, self.filter == kind, move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.filter = kind;
                        cx.notify();
                    });
                })
            })
            .collect();
        let running = self.plan.as_ref().is_some_and(AgentRun::running);
        div()
            .flex()
            .items_center()
            .gap(px(20.))
            .px(px(18.))
            .pt(px(16.))
            .pb(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w(px(96.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .whitespace_nowrap()
                            .text_size(px(21.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(STRONG)
                            .child("Videos"),
                    )
                    .child(
                        div().truncate().text_size(px(14.)).text_color(MUTED).child(
                            "Every card is a folder and a branch. Moving a card is a commit.",
                        ),
                    ),
            )
            .child(stat(
                next.map_or("None set".to_owned(), |d| {
                    format!("{} {} {}", weekday(d), d.day(), month(d.month()))
                }),
                "Next publish",
            ))
            .child(stat(format!("{CADENCE} per week"), "Cadence"))
            .child(segmented("board-filter", SUNK, segments).flex_none())
            .when(!self.plan_open, |this| {
                this.child(
                    proposal_action(
                        "plan-week",
                        if running {
                            "Claude is planning"
                        } else {
                            "Plan the week with Claude"
                        },
                        Some("spark"),
                        None,
                        ActionLook::Plain,
                    )
                    .h(px(34.))
                    .flex_none()
                    .on_click(cx.listener(|this, _, _, cx| this.plan_week(cx))),
                )
            })
            .child(
                proposal_action(
                    "new-video",
                    "New video",
                    None,
                    Some("N"),
                    ActionLook::Accent,
                )
                .h(px(34.))
                .flex_none()
                .on_click(cx.listener(|this, _, window, cx| this.start_composer(window, cx))),
            )
    }

    /// The row where a new video gets its title and format.
    fn composer_row(&self, composer: &Composer, cx: &mut Context<Self>) -> Div {
        let formats = FORMATS
            .iter()
            .map(|&format| {
                let entity = cx.entity();
                Segment::new(
                    format_name(format),
                    composer.format == format,
                    move |_, _, cx| {
                        entity.update(cx, |this, cx| {
                            if let Some(composer) = &mut this.composer {
                                composer.format = format;
                            }
                            cx.notify();
                        });
                    },
                )
            })
            .collect();
        div()
            .mx(px(18.))
            .mb(px(12.))
            .p(px(8.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(RADIUS_MD)
            .bg(RAISED)
            .on_action(cx.listener(|this, _: &Escape, window, cx| this.close_composer(window, cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(4.))
                    .rounded(RADIUS_SM)
                    .bg(SUNK)
                    .child(Input::new(&composer.input).appearance(false)),
            )
            .child(segmented("new-video-format", SUNK, formats).flex_none())
            .child(
                proposal_action(
                    "create-video",
                    "Add idea",
                    None,
                    Some("↩"),
                    ActionLook::Accent,
                )
                .flex_none()
                .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
            )
            .child(
                proposal_action(
                    "cancel-video",
                    "Cancel",
                    None,
                    Some("Esc"),
                    ActionLook::Quiet,
                )
                .flex_none()
                .on_click(cx.listener(|this, _, window, cx| this.close_composer(window, cx))),
            )
    }

    fn column(&self, status: Status, slugs: &[String], cx: &mut Context<Self>) -> impl IntoElement {
        let cards: Vec<&Card> = slugs
            .iter()
            .filter_map(|slug| self.cards.iter().find(|c| &c.slug == slug))
            .collect();
        div()
            .id(SharedString::from(format!("column-{status}")))
            .flex_1()
            .flex_basis(px(0.))
            .min_w(px(150.))
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .rounded(RADIUS_MD)
            .bg(SUNK)
            .drag_over::<Dragged>(|style, _, _, _| style.bg(RAISED))
            .on_drop(cx.listener(move |this, dragged: &Dragged, _, cx| {
                this.selected = Some(dragged.slug.clone());
                this.move_card(&dragged.slug, status, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .px(px(4.))
                    .pb(px(4.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(STRONG)
                            .child(column_name(status)),
                    )
                    .child(
                        mono(cards.len().to_string())
                            .text_size(TEXT_SM)
                            .text_color(MUTED),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("cards-{status}")))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .children(cards.into_iter().map(|card| self.card(card, cx)))
                    .when(status == Status::Ideas, |this| {
                        this.child(
                            div()
                                .id("add-idea")
                                .flex()
                                .items_center()
                                .h(px(34.))
                                .px(px(10.))
                                .rounded(RADIUS_SM)
                                .text_size(px(14.))
                                .text_color(MUTED)
                                .cursor_pointer()
                                .hover(|s| s.text_color(TEXT).bg(CARD))
                                .child("+ New video")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.start_composer(window, cx)
                                })),
                        )
                    }),
            )
    }

    fn card(&self, card: &Card, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let video = card.video.as_ref().expect("the grid holds readable cards");
        let info = self.info.get(&card.slug);
        let current = self.current.as_deref() == Some(card.slug.as_str());
        let selected = self.selected.as_deref() == Some(card.slug.as_str());
        let shipped = video.status == Status::Shipped;
        let tag = self
            .releases
            .iter()
            .find(|t| t.slug.as_deref() == Some(card.slug.as_str()))
            .map(|t| format!("Tagged {}", t.name));
        let date = video
            .target_date
            .map(|d| format!("{} {}", d.day(), month(d.month())));
        let when = tag.or(date);
        let next = info.and_then(|i| i.next.as_ref());
        let length = info.and_then(|i| i.length.clone());
        let dragged = Dragged {
            slug: card.slug.clone(),
            title: video.title.clone().into(),
        };
        let (slug, status) = (card.slug.clone(), video.status);
        div()
            .id(SharedString::from(format!("card-{}", card.slug)))
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .rounded(RADIUS_MD)
            .bg(if shipped { CARD } else { RAISED })
            .shadow_sm()
            .border_1()
            .border_color(if selected {
                Hsla::from(ACCENT).opacity(0.6)
            } else if current {
                Hsla::from(ACCENT).opacity(0.25)
            } else {
                gpui::transparent_black()
            })
            .cursor_grab()
            .hover(|s| s.bg(CARD_HOVER))
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.selected = Some(slug.clone());
                window.focus(&this.focus, cx);
                if event.click_count() >= 2 {
                    this.open(&slug, status, cx);
                }
                cx.notify();
            }))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if shipped { BODY } else { STRONG })
                    .child(video.title.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(TEXT_SM)
                    .text_color(KBD_TEXT)
                    .child(
                        div()
                            .size(px(7.))
                            .rounded_full()
                            .bg(format_color(video.format)),
                    )
                    .child(format_name(video.format))
                    .when(current, |this| {
                        this.child(div().flex_1())
                            .child(div().text_color(ACCENT).child("This video"))
                    }),
            )
            .when_some(next, |this, next| {
                this.child(
                    div()
                        .text_size(TEXT_SM)
                        .text_color(if next.waiting { WARN } else { BODY })
                        .child(next.text.clone()),
                )
            })
            .when(when.is_some() || length.is_some(), |this| {
                this.child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(8.))
                        .text_size(TEXT_SM)
                        .text_color(MUTED)
                        .child(div().min_w_0().truncate().child(when.unwrap_or_default()))
                        .when_some(length, |this, length| this.child(mono(length).flex_none())),
                )
            })
    }

    fn unreadable(&self) -> Option<Div> {
        let broken: Vec<String> = self
            .cards
            .iter()
            .filter_map(|c| c.video.as_ref().err().map(|e| e.to_string()))
            .collect();
        (!broken.is_empty()).then(|| {
            div()
                .px(px(18.))
                .pb(px(8.))
                .text_size(TEXT_SM)
                .text_color(CUT_TEXT)
                .children(broken)
        })
    }

    fn footer(&self) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(18.))
            .px(px(18.))
            .py(px(12.))
            .text_size(TEXT_SM)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(18.))
                    .overflow_hidden()
                    .when(!self.activity.is_empty(), |this| {
                        this.child(mono("git log --oneline").text_color(MUTED).flex_none())
                    })
                    .children(self.activity.iter().map(|entry| {
                        div()
                            .flex()
                            .gap(px(6.))
                            .min_w_0()
                            .child(
                                mono(entry.sha[..7].to_owned())
                                    .text_color(KBD_TEXT)
                                    .flex_none(),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_color(MUTED)
                                    .child(entry.subject.clone()),
                            )
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .gap(px(14.))
                    .child(hint("↩", "Open"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .text_size(px(14.))
                            .text_color(KBD_TEXT)
                            .child(kbd("⌥←"))
                            .child(kbd("⌥→"))
                            .child(div().pl(px(3.)).child("Move")),
                    ),
            )
    }

    fn plan_pane(&self, cx: &mut Context<Self>) -> Div {
        let body = match (&self.claude, &self.plan) {
            (None, _) => agent::setup_view(cx.listener(|this, _, _, cx| {
                this.claude = Claude::find();
                cx.notify();
            })),
            (Some(_), Some(run)) => agent::steps_view(run),
            (Some(_), None) => div(),
        };
        pane()
            .w(px(392.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(STRONG)
                            .child("Plan the week"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child("Claude · moves cards as commits"),
                    )
                    .child(
                        proposal_action(
                            "plan-close",
                            "Close",
                            None,
                            Some("Esc"),
                            ActionLook::Quiet,
                        )
                        .flex_none()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.plan_open = false;
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .id("plan-steps")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}

impl Render for BoardScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let grid = self.grid();
        let columns: Vec<_> = Status::ALL
            .iter()
            .zip(&grid)
            .map(|(&status, slugs)| self.column(status, slugs, cx).into_any_element())
            .collect();
        let composer = self
            .composer
            .as_ref()
            .map(|composer| self.composer_row(composer, cx));
        let board = pane()
            .flex_1()
            .flex()
            .flex_col()
            .child(self.header(cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(div().px(px(18.)).pb(px(12.)).child(alert(error)))
            })
            .children(self.unreadable())
            .children(composer)
            .child(
                div()
                    .id("columns")
                    .key_context(CONTEXT)
                    .track_focus(&self.focus)
                    .on_action(
                        cx.listener(|this, _: &SelectLeft, _, cx| this.select(Step::Left, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &SelectRight, _, cx| this.select(Step::Right, cx)),
                    )
                    .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.select(Step::Up, cx)))
                    .on_action(
                        cx.listener(|this, _: &SelectDown, _, cx| this.select(Step::Down, cx)),
                    )
                    .on_action(cx.listener(|this, _: &MoveLeft, _, cx| this.move_selected(-1, cx)))
                    .on_action(cx.listener(|this, _: &MoveRight, _, cx| this.move_selected(1, cx)))
                    .on_action(cx.listener(Self::open_selected))
                    .on_action(cx.listener(Self::deselect))
                    .on_action(
                        cx.listener(|this, _: &NewVideo, window, cx| {
                            this.start_composer(window, cx)
                        }),
                    )
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(8.))
                    .px(px(18.))
                    .overflow_x_scroll()
                    .children(columns),
            )
            .child(self.footer());
        div()
            .id("board")
            .size_full()
            .flex()
            .gap(px(8.))
            .p(px(8.))
            .pt(px(0.))
            .child(board)
            .when(self.plan_open, |this| this.child(self.plan_pane(cx)))
    }
}

/// A move of the selection on the board.
#[derive(Clone, Copy, Debug)]
enum Step {
    Left,
    Right,
    Up,
    Down,
}

/// The card selected after `step` from `selected` on a board whose columns hold `grid`. Left
/// and right go to the nearest column with cards, at the same height or its last card. With
/// nothing selected, or the selection hidden by the filter, any step selects the first card.
fn stepped(grid: &[Vec<String>], selected: Option<&str>, step: Step) -> Option<String> {
    let at = selected.and_then(|slug| {
        grid.iter().enumerate().find_map(|(column, cards)| {
            cards
                .iter()
                .position(|c| c == slug)
                .map(|row| (column, row))
        })
    });
    let Some((column, row)) = at else {
        return grid.iter().flatten().next().cloned();
    };
    let cards = &grid[column];
    let pick = |column: usize| {
        let cards = &grid[column];
        cards.get(row.min(cards.len() - 1)).cloned()
    };
    match step {
        Step::Up => Some(cards[row.saturating_sub(1)].clone()),
        Step::Down => Some(cards[(row + 1).min(cards.len() - 1)].clone()),
        Step::Left => (0..column)
            .rev()
            .find(|&c| !grid[c].is_empty())
            .and_then(pick)
            .or_else(|| selected.map(str::to_owned)),
        Step::Right => (column + 1..grid.len())
            .find(|&c| !grid[c].is_empty())
            .and_then(pick)
            .or_else(|| selected.map(str::to_owned)),
    }
}

/// Reads what a card shows besides its `video.kdl`: the edited length from `cut.kdl` (less the
/// lock's cuts on a migrated video), else the spoken length of `script.md`, and the next step.
fn card_info(board: &Board, slug: &str, video: &Video, proposals: usize) -> CardInfo {
    let read = |file: &str| board.read_file(slug, file).ok().flatten();
    let script = read("script.md").and_then(|text| Script::parse(&text).ok());
    let cut = read("cut.kdl")
        .and_then(|text| Cut::parse(&text).ok())
        .filter(|cut| !cut.chapters.is_empty());
    let review = cut.as_ref().map_or(0, |cut| cut.review_queue().count());
    let length = match cut {
        Some(cut) => {
            let project = Project {
                video: video.clone(),
                script: script.unwrap_or(Script {
                    title: video.title.clone(),
                    chapters: Vec::new(),
                }),
                format: video.format.preset(),
                cut,
            };
            let lock = read(LOCK_FILE).and_then(|text| Lock::parse(&text).ok());
            let timeline = match &lock {
                Some(lock) => Timeline::styled(&project, lock),
                None => Timeline::evaluate(&project),
            };
            let edited = timeline
                .main()
                .chapters
                .iter()
                .flat_map(|c| &c.segments)
                .map(|s| s.source.duration().0)
                .sum::<u64>();
            (edited > 0).then(|| shot_ship::clock(shot_core::Micros(edited)))
        }
        None => script
            .filter(|s| !s.chapters.is_empty())
            .map(|s| script_check::check_script(&s).total)
            .filter(|total| total.0 > 0)
            .map(|total| format!("~{} min", total.0.div_ceil(60_000_000))),
    };
    CardInfo {
        length,
        next: next_step(video.status, proposals, review),
    }
}

/// What a video waits for, from its column, its open proposals and its review queue.
fn next_step(status: Status, proposals: usize, review: usize) -> Option<Next> {
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let (text, waiting) = match status {
        Status::Ideas => ("Write the script".to_owned(), false),
        Status::Scripting if proposals > 0 => (
            format!(
                "{} to review",
                plural(proposals, "suggestion", "suggestions")
            ),
            true,
        ),
        Status::Scripting => ("Finish the script".to_owned(), false),
        Status::Ready => ("Record the takes".to_owned(), false),
        Status::Recorded if review > 0 => (
            format!("{} your call", plural(review, "cut needs", "cuts need")),
            true,
        ),
        Status::Recorded => ("Edit the cut".to_owned(), false),
        Status::Editing if proposals > 0 => (
            format!("{} to review", plural(proposals, "proposal", "proposals")),
            true,
        ),
        Status::Editing => ("Export and ship".to_owned(), false),
        Status::Shipped => return None,
    };
    Some(Next { text, waiting })
}

fn stat(value: impl Into<SharedString>, name: &'static str) -> Div {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(STRONG)
                .child(value.into()),
        )
        .child(div().text_size(TEXT_SM).text_color(MUTED).child(name))
}

/// A hovered card, between raised and float.
const CARD_HOVER: Rgba = Rgba {
    r: 0x1d as f32 / 255.,
    g: 0x20 as f32 / 255.,
    b: 0x26 as f32 / 255.,
    a: 1.,
};

/// The day of the week, by Sakamoto's method.
fn weekday(date: shot_core::Date) -> &'static str {
    const OFFSETS: [u32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let (month, day) = (u32::from(date.month()), u32::from(date.day()));
    let year = u32::from(date.year()) - u32::from(month < 3);
    let index = (year + year / 4 - year / 100 + year / 400 + OFFSETS[month as usize - 1] + day) % 7;
    ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][index as usize]
}

fn column_name(status: Status) -> &'static str {
    match status {
        Status::Ideas => "Ideas",
        Status::Scripting => "Scripting",
        Status::Ready => "Ready to record",
        Status::Recorded => "Recorded",
        Status::Editing => "Editing",
        Status::Shipped => "Shipped",
    }
}

pub fn format_name(format: FormatKind) -> &'static str {
    match format {
        FormatKind::Reaction => "Reaction",
        FormatKind::Opinion => "Opinion",
        FormatKind::ProductTour => "Product tour",
        FormatKind::Tutorial => "Tutorial",
    }
}

pub fn format_color(format: FormatKind) -> Rgba {
    match format {
        FormatKind::Reaction => FORMAT_REACTION,
        FormatKind::Opinion => FORMAT_OPINION,
        FormatKind::ProductTour => FORMAT_PRODUCT,
        FormatKind::Tutorial => FORMAT_TUTORIAL,
    }
}

fn month(m: u8) -> &'static str {
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][usize::from(m.clamp(1, 12)) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_weekday_of_a_target_date() {
        assert_eq!(weekday(shot_core::Date::new(2026, 11, 13).unwrap()), "Fri");
        assert_eq!(weekday(shot_core::Date::new(2024, 2, 29).unwrap()), "Thu");
    }

    #[test]
    fn arrow_keys_walk_the_columns_and_skip_empty_ones() {
        let grid: Vec<Vec<String>> = [vec!["a", "b", "c"], vec![], vec!["d"], vec!["e", "f"]]
            .into_iter()
            .map(|c| c.into_iter().map(str::to_owned).collect())
            .collect();
        let step = |from: Option<&str>, step| stepped(&grid, from, step);
        assert_eq!(step(None, Step::Down).as_deref(), Some("a"));
        assert_eq!(step(Some("gone"), Step::Right).as_deref(), Some("a"));
        assert_eq!(step(Some("a"), Step::Down).as_deref(), Some("b"));
        assert_eq!(step(Some("c"), Step::Down).as_deref(), Some("c"));
        assert_eq!(step(Some("a"), Step::Up).as_deref(), Some("a"));
        assert_eq!(
            step(Some("c"), Step::Right).as_deref(),
            Some("d"),
            "skips the empty column"
        );
        assert_eq!(step(Some("d"), Step::Right).as_deref(), Some("e"));
        assert_eq!(
            step(Some("f"), Step::Right).as_deref(),
            Some("f"),
            "stays at the edge"
        );
        assert_eq!(step(Some("f"), Step::Left).as_deref(), Some("d"));
        assert_eq!(step(Some("d"), Step::Left).as_deref(), Some("a"));
    }

    #[test]
    fn the_next_step_waits_on_you_only_when_something_is_open() {
        let next = |status, proposals, review| next_step(status, proposals, review).unwrap();
        assert_eq!(next(Status::Ideas, 0, 0).text, "Write the script");
        let suggestions = next(Status::Scripting, 2, 0);
        assert_eq!(suggestions.text, "2 suggestions to review");
        assert!(suggestions.waiting);
        assert_eq!(next(Status::Recorded, 0, 1).text, "1 cut needs your call");
        assert_eq!(next(Status::Recorded, 0, 6).text, "6 cuts need your call");
        assert!(!next(Status::Editing, 0, 3).waiting);
        assert!(next_step(Status::Shipped, 1, 0).is_none());
    }
}
