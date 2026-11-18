//! Scene switches on the Timeline's Layout lane: a mark on each edge where the layout changes
//! inside a take, named by how it moves, and the popover that sets its kind, length and sound.
//!
//! A click or right-click on a mark opens the popover, T opens the switch nearest the playhead.
//! Inside it 1 to 7 pick the kind, `[` and `]` step the length, Space plays the switch again,
//! Backspace or Delete gives the edge back to the format's `layout-switch`, Esc closes. Every
//! change writes `cut.kdl` with undo and plays the switch from just before its edge.

use std::ops::Range;

use gpui::{
    Context, Div, FocusHandle, FontWeight, Hsla, MouseButton, MouseDownEvent, SharedString,
    Stateful, Window, actions, div, prelude::*, px,
};
use gpui_component::tooltip::Tooltip;
use shot_core::{Cut, Id, Micros, TakeAt, Transition, TransitionKind, TransitionSound};

use super::EditScreen;
use super::parts::layout_name;
use super::timeline::{Lit, choice};
use crate::model::{FPS, SwitchMark, seconds};
use crate::theme::*;
use crate::ui::{icon, kbd, label};

actions!(
    scene_switch,
    [
        MorphSwitch,
        DissolveSwitch,
        ZoomThroughSwitch,
        CutSwitch,
        DipSwitch,
        StingerSwitch,
        MaskWipeSwitch,
        ShorterSwitch,
        LongerSwitch,
        PlaySwitch,
        FormatSwitch,
        CloseSwitch,
    ]
);

pub(crate) const CONTEXT: &str = "SceneSwitch";

/// The kinds in the order of their keys.
const KINDS: [(TransitionKind, &str, &str); 7] = [
    (TransitionKind::Morph, "Morph", "1"),
    (TransitionKind::Dissolve, "Dissolve", "2"),
    (TransitionKind::ZoomThrough, "Zoom through", "3"),
    (TransitionKind::Cut, "Cut", "4"),
    (TransitionKind::Dip, "Dip", "5"),
    (TransitionKind::Stinger, "Stinger", "6"),
    (TransitionKind::MaskWipe, "Mask wipe", "7"),
];

/// The lengths `[` and `]` step through, in microseconds.
const LENGTHS: [u64; 5] = [200_000, 300_000, 500_000, 800_000, 1_200_000];

/// How much of the take the preview plays before the edge and after the window.
const LEAD: u64 = FPS as u64 * 3 / 4;

/// The switch whose popover is open.
pub struct OpenSwitch {
    pub at: TakeAt,
    pub edge: Micros,
    focus: FocusHandle,
    /// The sounds the video can name, read when the popover opens.
    sounds: Vec<Id>,
}

/// What `cut.kdl` writes on the edge at `edge` of the block at `at`, as
/// [`Cut::set_switch`] reads it: the change starting there, else the one ending there.
fn written(cut: &Cut, at: TakeAt, edge: Micros) -> Option<Transition> {
    let block = cut.chapters.get(at.chapter)?.takes.get(at.take)?;
    let starts = block.layouts.iter().find(|c| c.range.from() == edge);
    match starts {
        Some(change) => change.enter.clone(),
        None => block
            .layouts
            .iter()
            .find(|c| c.range.to() == edge)
            .and_then(|c| c.exit.clone()),
    }
}

fn kind_name(kind: TransitionKind) -> &'static str {
    KINDS
        .iter()
        .find(|(k, ..)| *k == kind)
        .map_or("Morph", |(_, name, _)| name)
}

fn sound_name(sound: &Option<TransitionSound>) -> String {
    match sound {
        None => "No sound".into(),
        Some(TransitionSound::Brand) => "Brand sounds".into(),
        Some(TransitionSound::Named(name)) => name.to_string(),
    }
}

impl EditScreen {
    /// The mark of the open popover's switch, as the output shows it now.
    fn open_mark(&self) -> Option<SwitchMark> {
        let open = self.lanes.switch.as_ref()?;
        self.map
            .switch_marks()
            .into_iter()
            .find(|m| m.at == open.at && m.switch.at == open.edge)
    }

    pub(super) fn open_switch(
        &mut self,
        mark: &SwitchMark,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let dir = self.folder.read(cx).dir.clone();
        self.lanes.switch = Some(OpenSwitch {
            at: mark.at,
            edge: mark.switch.at,
            focus,
            sounds: crate::place::sounds(&dir)
                .into_iter()
                .map(|(name, _)| name)
                .collect(),
        });
        cx.notify();
    }

    /// Opens the switch nearest the playhead, T.
    pub(super) fn open_switch_near_playhead(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let playhead = self.playhead;
        let nearest = self
            .map
            .switch_marks()
            .into_iter()
            .min_by_key(|m| m.frame.abs_diff(playhead));
        if let Some(mark) = nearest {
            self.open_switch(&mark, window, cx);
        }
    }

    pub(super) fn close_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.lanes.switch.take().is_some() {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Writes the open switch as `change` makes it from the current one, `None` for the
    /// format's, then plays it.
    fn change_switch(
        &mut self,
        change: impl FnOnce(Transition) -> Option<Transition>,
        cx: &mut Context<Self>,
    ) {
        let Some(mark) = self.open_mark() else {
            return;
        };
        let switch = change(mark.switch.transition.clone());
        let (at, edge) = (mark.at, mark.switch.at);
        self.edit(cx, |cut| cut.set_switch(at, edge, switch));
        self.play_switch(&mark);
        cx.notify();
    }

    /// Plays from just before the switch's edge to just after its window.
    fn play_switch(&mut self, mark: &SwitchMark) {
        let window = mark.switch.window.duration().0 * u64::from(FPS) / 1_000_000;
        self.seek(mark.frame.saturating_sub(LEAD));
        self.set_playing(true);
        self.play_until = Some(mark.frame + window + LEAD);
    }

    fn step_length(&mut self, longer: bool, cx: &mut Context<Self>) {
        self.change_switch(
            |s| {
                let length = s.length.0;
                let next = if longer {
                    LENGTHS.into_iter().find(|&l| l > length)
                } else {
                    LENGTHS.into_iter().rev().find(|&l| l < length)
                };
                Some(Transition {
                    length: Micros(next.unwrap_or(length)),
                    ..s
                })
            },
            cx,
        );
    }

    fn set_kind(&mut self, kind: TransitionKind, cx: &mut Context<Self>) {
        self.change_switch(|s| Some(Transition { kind, ..s }), cx);
    }
}

/// A mark on each switch's edge in `window`, its kind's name, dim where the format decides. The
/// mark ends on the edge, so the new layout's name stays readable, and a faint band shows the
/// window it moves over.
pub fn marks(
    this: &EditScreen,
    window: &Range<u64>,
    x: &impl Fn(u64) -> f32,
    cx: &mut Context<EditScreen>,
) -> Vec<Div> {
    let cut = &this.folder.read(cx).project.cut;
    let open = this.lanes.switch.as_ref().map(|o| (o.at, o.edge));
    this.map
        .switch_marks()
        .into_iter()
        .filter(|m| window.contains(&m.frame))
        .enumerate()
        .map(|(index, mark)| {
            let own = written(cut, mark.at, mark.switch.at).is_some();
            let on = open == Some((mark.at, mark.switch.at));
            let switch = &mark.switch.transition;
            let tip: SharedString = format!(
                "{} into {}, {}{}",
                kind_name(switch.kind),
                layout_name(mark.switch.to),
                seconds(switch.length),
                if own { "" } else { ", from the format" }
            )
            .into();
            let pill = div()
                .id(("switch-mark", index))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(4.))
                .h(px(20.))
                .px(px(6.))
                .rounded(px(5.))
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .whitespace_nowrap()
                .cursor_pointer()
                .border_1()
                .map(|p| match (on, own) {
                    (true, _) => p.bg(TEXT).text_color(FRAME).border_color(TEXT),
                    (false, true) => p.bg(FLOAT).text_color(TEXT).border_color(MUTED),
                    (false, false) => p.bg(WELL).text_color(MUTED).border_color(FAINT),
                })
                .hover(|p| p.border_color(TEXT))
                .child(match switch.kind {
                    TransitionKind::ZoomThrough => "Zoom",
                    kind => kind_name(kind),
                })
                .when(switch.sound.is_some(), |p| {
                    p.child(icon(
                        "system-audio",
                        px(11.),
                        if on { FRAME } else { MUTED },
                    ))
                })
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx));
            let open_it = |mark: SwitchMark| {
                move |this: &mut EditScreen,
                      _: &MouseDownEvent,
                      window: &mut Window,
                      cx: &mut Context<EditScreen>| {
                    cx.stop_propagation();
                    this.open_switch(&mark, window, cx);
                }
            };
            let frames = mark.switch.window.duration().0 * u64::from(FPS) / 1_000_000;
            let (from, to) = (x(mark.frame), x(mark.frame + frames));
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .right_0()
                .when(switch.kind != TransitionKind::Cut, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(gpui::relative(from))
                            .w(gpui::relative((to - from).max(0.0)))
                            .bg(Hsla::from(TEXT).opacity(if on { 0.16 } else { 0.07 })),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(gpui::relative(from))
                        .w(px(2.))
                        .ml(px(-1.))
                        .bg(if own { TEXT } else { MUTED }),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(gpui::relative(1.0 - from))
                        .pr(px(3.))
                        .flex()
                        .items_center()
                        .child(
                            pill.on_mouse_down(
                                MouseButton::Left,
                                cx.listener(open_it(mark.clone())),
                            )
                            .on_mouse_down(MouseButton::Right, cx.listener(open_it(mark))),
                        ),
                )
        })
        .collect()
}

/// The open switch's popover, under the Layout lane at its mark.
pub fn popover(
    this: &EditScreen,
    window: &Range<u64>,
    x: &impl Fn(u64) -> f32,
    cx: &mut Context<EditScreen>,
) -> Option<Stateful<Div>> {
    let open = this.lanes.switch.as_ref()?;
    let mark = this.open_mark()?;
    let cut = &this.folder.read(cx).project.cut;
    let own = written(cut, mark.at, mark.switch.at);
    let format = this.folder.read(cx).project.format.layout_switch();
    let switch = mark.switch.transition.clone();
    let at = x(mark.frame.clamp(window.start, window.end));

    let row = |name: &'static str| {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(label(name).w(px(64.)).flex_none())
    };
    let kinds = div()
        .flex()
        .items_start()
        .gap(px(4.))
        .child(label("Kind").w(px(64.)).flex_none().pt(px(5.)))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .children(KINDS.iter().map(|&(kind, name, key)| {
                    choice(("switch-kind", key.as_bytes()[0] as usize), name, Some(key))
                        .on(switch.kind == kind)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_kind(kind, cx)))
                })),
        );
    let lengths = row("Length")
        .children(LENGTHS.iter().map(|&length| {
            choice(
                ("switch-length", length as usize),
                seconds(Micros(length)),
                None,
            )
            .on(switch.length.0 == length)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.change_switch(
                    |s| {
                        Some(Transition {
                            length: Micros(length),
                            ..s
                        })
                    },
                    cx,
                )
            }))
        }))
        .child(
            div()
                .pl(px(4.))
                .flex()
                .gap(px(2.))
                .child(kbd("["))
                .child(kbd("]")),
        );
    let mut sounds: Vec<Option<TransitionSound>> = vec![None, Some(TransitionSound::Brand)];
    sounds.extend(
        open.sounds
            .iter()
            .cloned()
            .map(|n| Some(TransitionSound::Named(n))),
    );
    let sound_row = div()
        .flex()
        .items_start()
        .gap(px(4.))
        .child(label("Sound").w(px(64.)).flex_none().pt(px(5.)))
        .child(
            div()
                .id("switch-sounds")
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .max_h(px(96.))
                .overflow_y_scroll()
                .children(sounds.into_iter().enumerate().map(|(index, sound)| {
                    let on = switch.sound == sound;
                    choice(("switch-sound", index), sound_name(&sound), None)
                        .on(on)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let sound = sound.clone();
                            this.change_switch(|s| Some(Transition { sound, ..s }), cx)
                        }))
                })),
        );
    let footer = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(TEXT_XS)
        .text_color(MUTED)
        .child(match &own {
            Some(_) => format!(
                "The format switches with {}, {}",
                kind_name(format.kind).to_lowercase(),
                seconds(format.length)
            ),
            None => "From the format".into(),
        })
        .child(div().flex_1())
        .when(own.is_some(), |f| {
            f.child(
                choice("switch-format", "Use the format's", Some("⌫"))
                    .on_click(cx.listener(|this, _, _, cx| this.change_switch(|_| None, cx))),
            )
        })
        .child(
            choice("switch-play", "Play", Some("space")).on_click(cx.listener(|this, _, _, cx| {
                if let Some(mark) = this.open_mark() {
                    this.play_switch(&mark);
                }
                cx.notify();
            })),
        );
    let panel = div()
        .id("switch-popover")
        .key_context(CONTEXT)
        .track_focus(&open.focus)
        .absolute()
        .top(px(40.))
        .w(px(508.))
        .map(|p| {
            if at > 0.55 {
                p.right(gpui::relative(1.0 - at))
            } else {
                p.left(gpui::relative(at))
            }
        })
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(12.))
        .rounded(px(11.))
        .bg(FLOAT)
        .shadow_lg()
        .text_size(TEXT_SM)
        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
        .on_action(
            cx.listener(|this, _: &MorphSwitch, _, cx| this.set_kind(TransitionKind::Morph, cx)),
        )
        .on_action(cx.listener(|this, _: &DissolveSwitch, _, cx| {
            this.set_kind(TransitionKind::Dissolve, cx)
        }))
        .on_action(cx.listener(|this, _: &ZoomThroughSwitch, _, cx| {
            this.set_kind(TransitionKind::ZoomThrough, cx)
        }))
        .on_action(cx.listener(|this, _: &CutSwitch, _, cx| this.set_kind(TransitionKind::Cut, cx)))
        .on_action(cx.listener(|this, _: &DipSwitch, _, cx| this.set_kind(TransitionKind::Dip, cx)))
        .on_action(
            cx.listener(|this, _: &StingerSwitch, _, cx| {
                this.set_kind(TransitionKind::Stinger, cx)
            }),
        )
        .on_action(cx.listener(|this, _: &MaskWipeSwitch, _, cx| {
            this.set_kind(TransitionKind::MaskWipe, cx)
        }))
        .on_action(cx.listener(|this, _: &ShorterSwitch, _, cx| this.step_length(false, cx)))
        .on_action(cx.listener(|this, _: &LongerSwitch, _, cx| this.step_length(true, cx)))
        .on_action(cx.listener(|this, _: &PlaySwitch, _, cx| {
            if let Some(mark) = this.open_mark() {
                this.play_switch(&mark);
            }
            cx.notify();
        }))
        .on_action(cx.listener(|this, _: &FormatSwitch, _, cx| this.change_switch(|_| None, cx)))
        .on_action(cx.listener(|this, _: &CloseSwitch, window, cx| this.close_switch(window, cx)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .font_weight(FontWeight::MEDIUM)
                .child(format!(
                    "Switch from {} to {}",
                    layout_name(mark.switch.from),
                    layout_name(mark.switch.to)
                ))
                .child(div().flex_1())
                .child(kbd("esc")),
        )
        .child(kinds)
        .child(lengths)
        .child(sound_row)
        .child(footer);
    Some(panel)
}
