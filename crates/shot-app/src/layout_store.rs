//! Pane widths of the Edit workspaces. Each workspace is a row of a pane on either side of a
//! flexible center, and the 8 px gap beside each side pane is the handle that resizes it: drag
//! to resize, double-click to reset. The widths are a preference of this machine, like the
//! recent videos, in `~/Library/Application Support/Shot Studio/layout.json`.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use gpui::{
    App, ClickEvent, Context, Div, DragMoveEvent, Global, IntoElement, Stateful, Window, div,
    prelude::*, px,
};
use serde::{Deserialize, Serialize};

use crate::theme::*;

/// The space between panes, which is also the handle that resizes them.
const GAP: f32 = 8.;

/// A side pane that resizes, named by its workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pane {
    TextOutline,
    TextSide,
    PictureLayouts,
    PictureLayers,
}

/// A pane's width before it was resized, and how narrow and wide it may get.
struct Limits {
    default: f32,
    min: f32,
    max: f32,
}

impl Pane {
    fn limits(self) -> Limits {
        let (default, min, max) = match self {
            Pane::TextOutline => (236., 180., 360.),
            Pane::TextSide => (520., 400., 760.),
            Pane::PictureLayouts => (176., 140., 260.),
            Pane::PictureLayers => (236., 200., 360.),
        };
        Limits { default, min, max }
    }
}

/// A workspace's row: a pane on each side of a center that takes the rest.
#[derive(Clone, Copy)]
pub struct Row {
    left: Pane,
    right: Pane,
    /// The center's smallest width, kept at every window width down to the 1280 px minimum.
    center: f32,
}

/// Chapters, the transcript, and the preview column.
pub const TEXT: Row = Row {
    left: Pane::TextOutline,
    right: Pane::TextSide,
    center: 480.,
};

/// Layouts, the stage, and the layers.
pub const PICTURE: Row = Row {
    left: Pane::PictureLayouts,
    right: Pane::PictureLayers,
    center: 560.,
};

impl Row {
    /// The side panes' widths in a row `width` wide: each within its limits, and narrowed,
    /// the right one first, when the center would get less than its minimum.
    fn fit(self, width: f32, widths: &Widths) -> (f32, f32) {
        let left = widths.of(self.left);
        let right = widths.of(self.right);
        let over = (left + right + 2. * GAP + self.center - width).max(0.);
        let right_min = self.right.limits().min;
        let from_right = over.min((right - right_min).max(0.));
        let left = (left - (over - from_right)).max(self.left.limits().min);
        (left, right - from_right)
    }

    /// Sets `pane` to `width`, as wide as the limits and the row leave room for.
    fn resize(self, pane: Pane, width: f32, row: f32, widths: &mut Widths) {
        let (left, right) = self.fit(row, widths);
        let other = if pane == self.left { right } else { left };
        let limits = pane.limits();
        let max = (row - other - 2. * GAP - self.center).min(limits.max);
        widths.saved.insert(pane, width.min(max).max(limits.min));
    }

    /// The row: `left`, the `center` and `right` with a resize handle between each side pane
    /// and the center. The side panes get their width from a wrapper, a flex column, so each
    /// should fill it with `flex_1`.
    pub fn render<V: 'static>(
        self,
        left: impl IntoElement,
        center: impl IntoElement,
        right: impl IntoElement,
        window: &Window,
        cx: &mut Context<V>,
    ) -> Stateful<Div> {
        let row = f32::from(window.viewport_size().width) - 2. * GAP;
        let dragging = cx.has_active_drag();
        let saved = widths(cx);
        // A drag released outside the row has no drop here; its width is saved now.
        if !dragging {
            saved.finish_drag();
        }
        let (left_width, right_width) = self.fit(row, saved);
        let side = |width: f32| div().w(px(width)).flex_none().min_h_0().flex().flex_col();
        div()
            .id(("pane-row", self.left as usize))
            .flex_1()
            .min_h_0()
            .flex()
            .mx(px(GAP))
            .child(side(left_width).child(left))
            .child(handle(self.left, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(center),
            )
            .child(handle(self.right, cx))
            .child(side(right_width).child(right))
            .on_drag_move(cx.listener(move |_, event: &DragMoveEvent<Resize>, _, cx| {
                let pane = event.drag(cx).0;
                let bounds = event.bounds;
                let x = event.event.position.x;
                let width = if pane == self.left {
                    x - bounds.left()
                } else if pane == self.right {
                    bounds.right() - x
                } else {
                    return;
                };
                let row = f32::from(bounds.size.width);
                self.resize(pane, f32::from(width) - GAP / 2., row, widths(cx));
                cx.notify();
            }))
            .on_drop(cx.listener(|_, _: &Resize, _, cx| {
                widths(cx).finish_drag();
                cx.notify();
            }))
    }
}

/// The edge of `pane` that faces the center: shows a grip on hover and while it is dragged.
fn handle<V: 'static>(pane: Pane, cx: &mut Context<V>) -> Stateful<Div> {
    let dragging = cx.has_active_drag() && widths(cx).dragging == Some(pane);
    div()
        .id(("pane-edge", pane as usize))
        .group("pane-edge")
        .w(px(GAP))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor_col_resize()
        .child(
            div()
                .w(px(3.))
                .h(px(if dragging { 48. } else { 28. }))
                .rounded_full()
                .when(dragging, |grip| grip.bg(MUTED))
                .when(!dragging, |grip| {
                    grip.group_hover("pane-edge", |s| s.bg(FAINT))
                }),
        )
        .on_drag(Resize(pane), |drag, _, _, cx| {
            widths(cx).dragging = Some(drag.0);
            cx.new(|_| drag.clone())
        })
        .on_click(cx.listener(move |_, event: &ClickEvent, _, cx| {
            if event.click_count() == 2 {
                let widths = widths(cx);
                widths.saved.remove(&pane);
                widths.save();
                cx.notify();
            }
        }))
}

/// What a pane edge drags. It draws nothing: the pane itself follows the pointer.
#[derive(Clone)]
struct Resize(Pane);

impl Render for Resize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// The saved widths, loaded on first use.
#[derive(Default)]
struct Widths {
    saved: BTreeMap<Pane, f32>,
    /// The pane whose edge is dragged, for its grip, until the drag ends and its width is saved.
    dragging: Option<Pane>,
}

impl Global for Widths {}

impl Widths {
    fn of(&self, pane: Pane) -> f32 {
        let limits = pane.limits();
        self.saved
            .get(&pane)
            .map_or(limits.default, |w| w.clamp(limits.min, limits.max))
    }

    fn path() -> PathBuf {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home).join("Library/Application Support/Shot Studio/layout.json")
    }

    fn load() -> Self {
        let saved = fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            saved,
            dragging: None,
        }
    }

    /// Saves the widths once a drag has ended.
    fn finish_drag(&mut self) {
        if self.dragging.take().is_some() {
            self.save();
        }
    }

    fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(
            path,
            serde_json::to_string_pretty(&self.saved).expect("widths serialize"),
        );
    }
}

fn widths(cx: &mut App) -> &mut Widths {
    if !cx.has_global::<Widths>() {
        cx.set_global(Widths::load());
    }
    cx.global_mut::<Widths>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(pairs: &[(Pane, f32)]) -> Widths {
        Widths {
            saved: pairs.iter().copied().collect(),
            dragging: None,
        }
    }

    #[test]
    fn panes_keep_their_width_while_the_center_has_room() {
        let widths = saved(&[(Pane::TextOutline, 300.)]);
        assert_eq!(TEXT.fit(1584., &widths), (300., 520.));
        let widths = saved(&[(Pane::TextOutline, 9000.), (Pane::TextSide, 1.)]);
        assert_eq!(TEXT.fit(3000., &widths), (360., 400.), "limits hold");
    }

    #[test]
    fn a_narrow_window_narrows_the_right_pane_first_and_keeps_the_center() {
        let widths = saved(&[(Pane::TextOutline, 360.), (Pane::TextSide, 760.)]);
        let (left, right) = TEXT.fit(1264., &widths);
        assert_eq!((left, right), (360., 408.));
        assert_eq!(1264. - left - right - 2. * GAP, TEXT.center);
        let (left, right) = TEXT.fit(1000., &widths);
        assert_eq!((left, right), (180., 400.), "both at their minimum");
    }

    #[test]
    fn a_drag_stops_where_the_center_would_get_too_narrow() {
        let mut widths = Widths::default();
        PICTURE.resize(Pane::PictureLayers, 340., 1264., &mut widths);
        assert_eq!(widths.of(Pane::PictureLayers), 340.);
        TEXT.resize(Pane::TextSide, 2000., 1264., &mut widths);
        assert_eq!(widths.of(Pane::TextSide), 1264. - 236. - 16. - 480.);
        TEXT.resize(Pane::TextOutline, 10., 1264., &mut widths);
        assert_eq!(widths.of(Pane::TextOutline), 180.);
    }

    #[test]
    fn widths_save_as_one_entry_per_pane() {
        let widths = saved(&[(Pane::PictureLayers, 280.), (Pane::TextSide, 600.)]);
        let text = serde_json::to_string(&widths.saved).unwrap();
        assert_eq!(text, r#"{"text-side":600.0,"picture-layers":280.0}"#);
    }
}
