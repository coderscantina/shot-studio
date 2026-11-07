//! The Board screen: the channel repo as a Kanban board. Columns are `video.kdl` statuses,
//! dragging a card to another column is one commit through `shot-board`, the bottom line is the
//! git log of the video branches, and shipped videos show their release tag.
//!
//! The board holds no state of its own: it rebuilds from git after every change.
//! "Plan the week with Claude" runs the planning prompt through the `claude` bridge.

use std::path::PathBuf;

use gpui::{
    App, Context, Div, FocusHandle, Focusable, FontWeight, Hsla, Rgba, SharedString, Window, div,
    prelude::*, px,
};
use shot_board::{Activity, Board, Card, ReleaseTag};
use shot_core::{FormatKind, Status};
use shot_mcp::bridge::Claude;

use crate::agent::{self, AgentRun};
use crate::theme::*;
use crate::ui::{Segment, mono, pane, segmented};

/// Videos published per week, the planning target.
const CADENCE: u8 = 2;
/// Commits shown in the activity line.
const ACTIVITY: usize = 3;

pub struct BoardScreen {
    repo: PathBuf,
    /// The video the window opened, highlighted.
    current: String,
    cards: Vec<Card>,
    activity: Vec<Activity>,
    releases: Vec<ReleaseTag>,
    filter: Option<FormatKind>,
    error: Option<String>,
    claude: Option<Claude>,
    plan: Option<AgentRun>,
    plan_open: bool,
    focus: FocusHandle,
}

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
    pub fn new(repo: PathBuf, current: String, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            repo,
            current,
            cards: Vec::new(),
            activity: Vec::new(),
            releases: Vec::new(),
            filter: None,
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
        let result = Board::open(&self.repo)
            .and_then(|board| Ok((board.cards()?, board.activity(ACTIVITY)?, board.releases()?)));
        match result {
            Ok((cards, activity, releases)) => {
                (self.cards, self.activity, self.releases) = (cards, activity, releases);
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn move_card(&mut self, slug: &str, status: Status, cx: &mut Context<Self>) {
        let moved = Board::open(&self.repo).and_then(|b| b.move_card(slug, status));
        if let Err(e) = moved {
            self.error = Some(e.to_string());
        }
        self.reload();
        cx.notify();
    }

    fn plan_week(&mut self, cx: &mut Context<Self>) {
        self.plan_open = true;
        let Some(claude) = self.claude.clone() else {
            cx.notify();
            return;
        };
        match AgentRun::start(
            claude,
            &self.repo,
            "Plan the week".to_owned(),
            shot_mcp::agents::plan_week(CADENCE),
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
                    .min_w_0()
                    .flex_shrink(1.)
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
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
                next.map_or("None".to_owned(), |d| {
                    format!("{} {} {}", weekday(d), d.day(), month(d.month()))
                }),
                "Next publish",
            ))
            .child(stat(format!("{CADENCE} per week"), "Cadence"))
            .child(div().flex_1())
            .child(segmented("board-filter", SURFACE, segments))
            .child(
                button("plan-week", "Plan the week with Claude", running)
                    .on_click(cx.listener(|this, _, _, cx| this.plan_week(cx))),
            )
    }

    fn column(&self, status: Status, cx: &mut Context<Self>) -> impl IntoElement {
        let cards: Vec<&Card> = self
            .cards
            .iter()
            .filter(|c| {
                c.video
                    .as_ref()
                    .is_ok_and(|v| v.status == status && self.filter.is_none_or(|f| v.format == f))
            })
            .collect();
        div()
            .id(SharedString::from(format!("column-{status}")))
            .flex_1()
            .flex_basis(px(0.))
            .min_w(px(180.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .rounded(RADIUS_MD)
            .bg(SUNK)
            .drag_over::<Dragged>(|style, _, _, _| style.bg(RAISED))
            .on_drop(cx.listener(move |this, dragged: &Dragged, _, cx| {
                this.move_card(&dragged.slug, status, cx);
            }))
            .child(
                div()
                    .flex()
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
                        div()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child(cards.len().to_string()),
                    ),
            )
            .children(cards.into_iter().map(|card| self.card(card)))
    }

    fn card(&self, card: &Card) -> impl IntoElement + use<> {
        let video = card.video.as_ref().expect("filtered to readable cards");
        let current = card.slug == self.current;
        let tag = self
            .releases
            .iter()
            .rev()
            .find(|t| t.slug.as_deref() == Some(card.slug.as_str()))
            .map(|t| t.name.clone());
        let meta = match (video.status, tag) {
            (Status::Shipped, Some(tag)) => format!("Tagged {tag}"),
            _ if !card.has_branch => "No branch yet".to_owned(),
            _ => format!("video/{}", card.slug),
        };
        let date = video
            .target_date
            .map(|d| format!("{} {}", d.day(), month(d.month())));
        let dragged = Dragged {
            slug: card.slug.clone(),
            title: video.title.clone().into(),
        };
        div()
            .id(SharedString::from(format!("card-{}", card.slug)))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .rounded(RADIUS_MD)
            .bg(RAISED)
            .shadow_sm()
            .cursor_grab()
            .hover(|s| s.bg(CARD_HOVER))
            .when(current, |this| {
                this.border_1()
                    .border_color(Hsla::from(ACCENT).opacity(0.35))
            })
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .child(div().text_color(STRONG).child(video.title.clone()))
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
                    .child(format_name(video.format)),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap(px(8.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .when(card.has_branch && video.status != Status::Shipped, |this| {
                                this.font_family(MONO)
                            })
                            .child(meta),
                    )
                    .when_some(date, |this, date| this.child(div().flex_none().child(date))),
            )
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
                .text_size(TEXT_SM)
                .text_color(CUT_TEXT)
                .children(broken)
        })
    }

    fn activity_line(&self) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(18.))
            .px(px(18.))
            .py(px(12.))
            .text_size(TEXT_SM)
            .overflow_hidden()
            .child(mono("git log --oneline").text_color(MUTED).flex_none())
            .children(self.activity.iter().map(|entry| {
                div()
                    .flex()
                    .gap(px(6.))
                    .min_w_0()
                    .child(mono(entry.sha[..7].to_owned()).text_color(KBD_TEXT))
                    .child(
                        div()
                            .truncate()
                            .text_color(MUTED)
                            .child(entry.subject.clone()),
                    )
            }))
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
                    .items_baseline()
                    .gap(px(10.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(STRONG)
                            .child("Plan the week"),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .child("Claude · moves cards as commits"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("plan-close")
                            .text_size(TEXT_SM)
                            .text_color(MUTED)
                            .cursor_pointer()
                            .hover(|s| s.text_color(TEXT))
                            .child("Close")
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
        let columns: Vec<_> = Status::ALL
            .iter()
            .map(|&status| self.column(status, cx).into_any_element())
            .collect();
        let board = pane()
            .flex_1()
            .flex()
            .flex_col()
            .child(self.header(cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .px(px(18.))
                        .pb(px(8.))
                        .text_color(CUT_TEXT)
                        .child(error),
                )
            })
            .children(self.unreadable())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .gap(px(8.))
                    .px(px(18.))
                    .children(columns),
            )
            .child(self.activity_line());
        div()
            .id("board")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .gap(px(8.))
            .p(px(8.))
            .pt(px(0.))
            .child(board)
            .when(self.plan_open, |this| this.child(self.plan_pane(cx)))
    }
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
        .child(div().text_size(px(14.)).text_color(MUTED).child(name))
}

fn button(id: &'static str, text: &'static str, busy: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(34.))
        .px(px(14.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(9.))
        .bg(RAISED)
        .font_weight(FontWeight::MEDIUM)
        .text_size(px(14.))
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT))
        .when(busy, |this| {
            this.child(div().size(px(7.)).rounded_full().bg(ACCENT))
        })
        .child(text)
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

fn format_color(format: FormatKind) -> Rgba {
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
}
