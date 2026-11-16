//! The Prompter pane of the Record screen: a live preview of the output, where it shows, its
//! look and scrolling, and the hotkey legend and retake toast the stage shows while recording.

use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, Context, DisplayId, Div, Entity, FontWeight, Render, SharedString, Subscription, Window,
    WindowHandle, WindowId, div, prelude::*, px,
};
use gpui_component::Sizable;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use gpui_component::switch::Switch;
use shot_capture::events::Hotkey;
use shot_core::PrompterMode;

use super::output::{self, PrompterOutput};
use super::settings::{self, COUNTDOWNS, Colors, ScrollMode, Settings};
use super::{Live, bindings, log_filter, native};
use crate::folder::VideoFolder;
use crate::theme::*;
use crate::ui::{self, Segment, hint, icon, kbd, mono, pane, segmented};

/// How long the retake toast stays.
const TOAST: Duration = Duration::from_secs(4);
/// Width of the preview: the pane's width inside its padding.
const PREVIEW_WIDTH: f32 = 384.;
/// The show and hide shortcut, as the menu shows it.
const TOGGLE_KEYS: &str = "⌘⇧P";

/// A slider of the pane: what it changes and how its value reads.
struct Knob {
    name: &'static str,
    range: RangeInclusive<f32>,
    step: f32,
    get: fn(&Settings) -> f32,
    set: fn(&mut Settings, f32),
    show: fn(f32) -> String,
}

/// The sliders: text size in view, speed with steady scroll, the rest under "More settings".
const KNOBS: [Knob; 6] = [
    Knob {
        name: "Text size",
        range: settings::TEXT_SIZE,
        step: 2.,
        get: |s| s.text_size,
        set: |s, v| s.text_size = v,
        show: |v| format!("{v:.0} pt"),
    },
    Knob {
        name: "Line height",
        range: settings::LINE_HEIGHT,
        step: 0.05,
        get: |s| s.line_height,
        set: |s, v| s.line_height = v,
        show: |v| format!("{v:.2}"),
    },
    Knob {
        name: "Side margins",
        range: settings::MARGIN,
        step: 4.,
        get: |s| s.margin,
        set: |s, v| s.margin = v,
        show: |v| format!("{v:.0} pt"),
    },
    Knob {
        name: "Text width",
        range: settings::TEXT_WIDTH,
        step: 5.,
        get: |s| s.text_width,
        set: |s, v| s.text_width = v,
        show: |v| format!("{v:.0}%"),
    },
    Knob {
        name: "Reading line",
        range: settings::READING_LINE,
        step: 5.,
        get: |s| s.reading_line,
        set: |s, v| s.reading_line = v,
        show: |v| format!("{v:.0}% down"),
    },
    Knob {
        name: "Speed",
        range: settings::SPEED,
        step: 5.,
        get: |s| s.speed,
        set: |s, v| s.speed = v,
        show: |v| format!("{v:.0} wpm"),
    },
];
const TEXT_SIZE: usize = 0;
const SPEED: usize = 5;
/// Line height, margins, width and reading line.
const MORE: [usize; 4] = [1, 2, 3, 4];

/// Where the output is, or why it is not on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// The Record screen is not showing.
    Away,
    /// The chosen display, or an Elgato Prompter, is not connected.
    Missing,
    /// Hidden with Esc, the close control, ⌘⇧P or the menu.
    Hidden(u32),
    /// Held back because the app's window is on that display, until asked for.
    OverApp(u32),
    Shown(u32),
}

/// Where the output goes for the `target` display, given the Record screen, whether Mike hid
/// it, the display the app's window is on, and whether he asked for that display anyway.
fn placement(
    target: Option<u32>,
    on_screen: bool,
    hidden: bool,
    app_display: Option<u32>,
    asked: bool,
) -> Placement {
    match target {
        _ if !on_screen => Placement::Away,
        None => Placement::Missing,
        Some(display) if hidden => Placement::Hidden(display),
        Some(display) if Some(display) == app_display && !asked => Placement::OverApp(display),
        Some(display) => Placement::Shown(display),
    }
}

pub struct PrompterPane {
    folder: Entity<VideoFolder>,
    live: Arc<Live>,
    settings: Settings,
    displays: Vec<(u32, String)>,
    output: Option<(u32, WindowHandle<PrompterOutput>)>,
    hotkeys: Option<native::Hotkeys>,
    /// The retake toast on screen: its sequence number and when it appeared.
    toast: Option<(u32, Instant)>,
    on_screen: bool,
    app_display: Option<u32>,
    hidden: bool,
    /// Mike asked for the output, so it may cover the app's own display.
    asked: bool,
    /// The display list is open.
    picking: bool,
    sliders: Vec<Entity<SliderState>>,
    /// The preview's scroll, see [`output::Flow`].
    flow: output::Flow,
    _subscriptions: Vec<Subscription>,
}

impl PrompterPane {
    pub fn new(folder: Entity<VideoFolder>, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let live = Live::new(&folder.read(cx).project, settings.pace());
        let sliders: Vec<_> = KNOBS
            .iter()
            .map(|knob| {
                cx.new(|_| {
                    SliderState::new()
                        .min(*knob.range.start())
                        .max(*knob.range.end())
                        .step(knob.step)
                        .default_value((knob.get)(&settings))
                })
            })
            .collect();
        let mut subscriptions: Vec<Subscription> = sliders
            .iter()
            .enumerate()
            .map(|(index, slider)| {
                cx.subscribe(slider, move |pane, _, event: &SliderEvent, cx| {
                    let set = KNOBS[index].set;
                    match *event {
                        SliderEvent::Change(value) => pane.apply(cx, |s| set(s, value.start())),
                        SliderEvent::Release(value) => pane.change(cx, |s| set(s, value.start())),
                    }
                })
            })
            .collect();
        let this = cx.weak_entity();
        subscriptions.push(cx.observe_displays(move |_, cx| {
            let _ = this.update(cx, |pane, cx| {
                pane.displays = native::display_names();
                pane.sync(cx);
            });
        }));
        let this = cx.weak_entity();
        subscriptions.push(cx.on_window_closed(move |cx, id| {
            // The pane closes the output itself while it is being updated, so hear about it
            // after that update ends.
            let this = this.clone();
            cx.defer(move |cx| {
                let _ = this.update(cx, |pane, cx| pane.closed(id, cx));
            });
        }));
        subscriptions.push(cx.on_release(|pane, cx| {
            if let Some((_, handle)) = pane.output.take() {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
        }));
        Self {
            folder,
            live,
            settings,
            displays: native::display_names(),
            output: None,
            hotkeys: None,
            toast: None,
            on_screen: false,
            app_display: None,
            hidden: false,
            asked: false,
            picking: false,
            sliders,
            flow: output::Flow::default(),
            _subscriptions: subscriptions,
        }
    }

    /// The prompter state the recorder thread and voice follow move.
    pub fn live(&self) -> Arc<Live> {
        self.live.clone()
    }

    /// The Record screen shows, its window on `app_display`: the output opens on the prompter
    /// display, unless that is where the app is.
    pub fn enter(&mut self, app_display: Option<u32>, cx: &mut Context<Self>) {
        self.on_screen = true;
        self.app_display = app_display;
        self.hidden = false;
        self.asked = false;
        self.displays = native::display_names();
        // The screen is mid-render; windows open after it.
        cx.spawn(async move |this, cx| this.update(cx, |pane, cx| pane.sync(cx)))
            .detach();
    }

    /// The Record screen went away: the output closes with it.
    pub fn leave(&mut self, cx: &mut Context<Self>) {
        self.on_screen = false;
        self.picking = false;
        self.sync(cx);
    }

    /// ⌘⇧P, the menu or the pane's button: hides a shown output, or shows it even over the app.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.output.is_some() {
            self.hidden = true;
        } else {
            self.hidden = false;
            self.asked = true;
        }
        self.sync(cx);
    }

    /// Starts the prompter over from the script, for a new take.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.live.reset(&self.folder.read(cx).project);
        self.toast = None;
        cx.notify();
    }

    /// Registers the record hotkeys, from start to stop of a take.
    pub fn arm(&mut self) {
        let keys = bindings(&self.settings);
        let codes: Vec<(u16, u32)> = keys.iter().map(|&(_, code, mods)| (code, mods)).collect();
        let live = self.live.clone();
        self.hotkeys = Some(native::Hotkeys::register(&codes, move |index| {
            live.press(keys[index].0)
        }));
    }

    pub fn disarm(&mut self) {
        self.hotkeys = None;
    }

    /// Presses the event log keeps out of `key` events.
    pub fn log_filter(&self) -> Vec<Hotkey> {
        log_filter(&self.settings)
    }

    /// The hotkey legend under the stage.
    pub fn legend(&self) -> Div {
        let chord = if self.settings.plain_keys {
            ""
        } else {
            "⌃⌥"
        };
        let keys = [
            ("A", "Again"),
            ("N", "Next beat"),
            ("M", "Marker"),
            ("Z", "Zoom here"),
            ("B", "B-roll here"),
            ("1–4", "Layout"),
        ];
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(14.))
            .children(
                keys.into_iter()
                    .map(|(key, what)| hint(format!("{chord}{key}"), what)),
            )
            .when(self.settings.scroll == ScrollMode::Pedal, |row| {
                row.child(hint(self.settings.advance.label(), "Pedal"))
            })
    }

    /// The inline retake toast, while one is fresh.
    pub fn toast(&mut self) -> Option<Div> {
        let prompter = self.live.lock();
        let (seq, retake) = prompter.last_retake()?;
        if self.toast.is_none_or(|(shown, _)| shown != seq) {
            self.toast = Some((seq, Instant::now()));
        }
        let (_, since) = self.toast?;
        (since.elapsed() < TOAST).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(14.))
                .py(px(10.))
                .rounded(RADIUS_MD)
                .bg(FLOAT)
                .shadow_lg()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Inline retake caught"),
                )
                .child(div().text_color(MUTED).child(format!(
                    "\"{}\" → keeping \"{}\"",
                    retake.heard, retake.kept
                )))
        })
    }

    fn placement(&self) -> Placement {
        placement(
            self.settings.pick_display(&self.displays),
            self.on_screen,
            self.hidden,
            self.app_display,
            self.asked,
        )
    }

    /// Opens, moves or closes the output window to match the placement.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let want = match self.placement() {
            Placement::Shown(display) => Some(display),
            _ => None,
        };
        if self.output.as_ref().map(|(id, _)| *id) != want {
            if let Some((_, handle)) = self.output.take() {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }
            self.output = want.and_then(|display| {
                let over_menu_bar = Some(display) != self.app_display;
                let handle = output::open(
                    self.live.clone(),
                    display,
                    self.settings.clone(),
                    over_menu_bar,
                    cx,
                )?;
                Some((display, handle))
            });
        }
        cx.notify();
    }

    /// A window closed. When it was the output and its display is still there, Mike closed it,
    /// so it stays hidden; when the display went away, it comes back with the display.
    fn closed(&mut self, id: WindowId, cx: &mut Context<Self>) {
        let Some((display, handle)) = &self.output else {
            return;
        };
        if handle.window_id() != id {
            return;
        }
        let display = *display;
        self.output = None;
        self.displays = native::display_names();
        if self.settings.pick_display(&self.displays) == Some(display) {
            self.hidden = true;
        }
        cx.notify();
    }

    /// Changes the settings without saving them, as a slider moves.
    fn apply(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        edit(&mut self.settings);
        if before == self.settings {
            return;
        }
        self.live.lock().set_pace(self.settings.pace());
        if let Some((_, handle)) = &self.output {
            let settings = self.settings.clone();
            let _ = handle.update(cx, |output, window, cx| {
                native::set_mirror(window, settings.mirror);
                output.settings = settings;
                cx.notify();
            });
        }
        if self.hotkeys.is_some()
            && (before.plain_keys, before.scroll, before.advance)
                != (
                    self.settings.plain_keys,
                    self.settings.scroll,
                    self.settings.advance,
                )
        {
            self.arm();
        }
        if before.display != self.settings.display {
            self.sync(cx);
        }
        cx.notify();
    }

    fn change(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Settings)) {
        self.apply(cx, edit);
        self.settings.save();
    }

    /// Picks the output display. Picking is asking for it, also on the app's display.
    fn pick(&mut self, display: Option<String>, cx: &mut Context<Self>) {
        self.picking = false;
        self.hidden = false;
        self.asked = true;
        self.change(cx, |s| s.display = display);
        self.sync(cx);
    }

    fn reset_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.change(cx, Settings::reset);
        for (knob, slider) in KNOBS.iter().zip(&self.sliders) {
            let value = (knob.get)(&self.settings);
            slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
        }
    }

    fn set_mode(&mut self, mode: PrompterMode, cx: &mut Context<Self>) {
        let result = self.folder.update(cx, |folder, _| {
            let mut format = folder.project.format.clone();
            format.prompter = mode;
            folder.write_format(format)
        });
        if result.is_ok() {
            self.reset(cx);
        }
    }

    /// The "More settings" row that opens and closes the settings set once in a while.
    fn disclosure(&self, open: bool, cx: &mut Context<Self>) -> impl IntoElement {
        ui::keyboard_reachable(
            div()
                .id("prompter-more")
                .flex()
                .items_center()
                .gap(px(6.))
                .mt(px(6.))
                .py(px(6.))
                .px(px(2.))
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .text_size(TEXT_SM)
                .text_color(MUTED)
                .hover(|s| s.text_color(TEXT))
                .child(icon(
                    if open {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                    ICON_SM,
                    MUTED,
                ))
                .child("More settings")
                .on_click(cx.listener(|pane, _, _, cx| pane.change(cx, |s| s.more = !s.more))),
        )
    }

    fn display_name(&self, id: u32) -> String {
        self.displays.iter().find(|(d, _)| *d == id).map_or_else(
            || "the prompter display".to_owned(),
            |(_, name)| name.clone(),
        )
    }

    /// What the output does now, and whether the button shows or hides it.
    fn status(&self) -> (String, Option<&'static str>) {
        let mirrored = if self.settings.mirror {
            ", mirrored"
        } else {
            ""
        };
        match self.placement() {
            Placement::Shown(id) => (
                format!("On {}{mirrored}", self.display_name(id)),
                Some("Hide"),
            ),
            Placement::Hidden(_) => ("Hidden".into(), Some("Show")),
            Placement::OverApp(id) => (
                format!(
                    "Held back because this window is on {}",
                    self.display_name(id)
                ),
                Some("Show"),
            ),
            Placement::Missing => (
                match &self.settings.display {
                    Some(name) => format!("{name} is not connected"),
                    None => "No Elgato Prompter connected. Pick a display below.".into(),
                },
                None,
            ),
            Placement::Away => (String::new(), None),
        }
    }

    fn segment(
        &self,
        text: impl Into<SharedString>,
        on: bool,
        cx: &mut Context<Self>,
        apply: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Segment {
        let this = cx.entity().downgrade();
        Segment::new(text, on, move |_, _, cx: &mut App| {
            let _ = this.update(cx, |pane, cx| apply(pane, cx));
        })
    }

    fn slider_row(&self, index: usize) -> Div {
        let knob = &KNOBS[index];
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(6.))
            .px(px(2.))
            .child(name(knob.name))
            .child(
                Slider::new(&self.sliders[index])
                    .accessibility_label(knob.name)
                    .flex_1(),
            )
            .child(
                div()
                    .child((knob.show)((knob.get)(&self.settings)))
                    .w(px(76.))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .text_size(TEXT_SM),
            )
    }

    fn display_picker(&self, cx: &mut Context<Self>) -> Div {
        let chosen = self.settings.display.clone();
        let current = chosen.clone().unwrap_or_else(|| "Automatic".to_owned());
        let mut options: Vec<(Option<String>, String)> =
            vec![(None, "Automatic: the Elgato Prompter".to_owned())];
        options.extend(self.displays.iter().map(|(id, name)| {
            let here = if Some(*id) == self.app_display {
                ", this window"
            } else {
                ""
            };
            (Some(name.clone()), format!("{name}{here}"))
        }));
        if let Some(name) = chosen.as_ref()
            && !self.displays.iter().any(|(_, n)| n == name)
        {
            options.push((Some(name.clone()), format!("{name}, not connected")));
        }
        div()
            .flex()
            .flex_col()
            .child(row(
                "Display",
                div()
                    .id("prompter-display")
                    .max_w(px(260.))
                    .px(px(10.))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .rounded(RADIUS_SM)
                    .bg(RAISED)
                    .cursor_pointer()
                    .hover(|s| s.bg(FLOAT))
                    .text_size(TEXT_SM)
                    .child(div().truncate().child(current))
                    .child(icon("chevron-down", ICON_SM, MUTED))
                    .on_click(cx.listener(|pane, _, _, cx| {
                        pane.picking = !pane.picking;
                        cx.notify();
                    })),
            ))
            .when(self.picking, |list| {
                list.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .p(px(4.))
                        .mb(px(6.))
                        .rounded(RADIUS_MD)
                        .bg(CARD)
                        .children(
                            options
                                .into_iter()
                                .enumerate()
                                .map(|(index, (value, text))| {
                                    let on = value == chosen;
                                    div()
                                        .id(("prompter-display-option", index))
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
                                        .on_click(cx.listener(move |pane, _, _, cx| {
                                            pane.pick(value.clone(), cx)
                                        }))
                                }),
                        ),
                )
            })
    }
}

/// A setting's name in the left column.
fn name(text: &'static str) -> Div {
    div().w(px(96.)).flex_none().text_color(MUTED).child(text)
}

fn row(text: &'static str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .py(px(6.))
        .px(px(2.))
        .child(name(text))
        .child(div().flex_1().min_w_0().flex().justify_end().child(control))
}

impl Render for PrompterPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.folder.read(cx).project.format.prompter;
        let s = self.settings.clone();
        let (status, button) = self.status();
        let shown = self.output.is_some();
        // The preview has the output's shape, its text at the output's scale.
        let (width, height) = self
            .settings
            .pick_display(&self.displays)
            .and_then(|id| cx.find_display(DisplayId::new(u64::from(id))))
            .map_or(output::ELGATO, |display| {
                let size = display.bounds().size;
                (f32::from(size.width), f32::from(size.height))
            });
        let live = self.live.clone();
        let prompter = live.lock();
        let view = prompter.view();
        let left = view.time_left.as_secs();
        let preview_height = PREVIEW_WIDTH * height / width;
        let preview = div()
            .w(px(PREVIEW_WIDTH))
            .h(px(preview_height))
            .flex_none()
            .rounded(RADIUS_MD)
            .overflow_hidden()
            .child(output::page(
                &view,
                prompter.lines(),
                &s,
                PREVIEW_WIDTH / width,
                (PREVIEW_WIDTH, preview_height),
                &mut self.flow,
                window,
            ));
        let chapter = format!(
            "{} · beat {} of {}",
            view.chapter, view.part_beat, view.part_beats
        );
        let retakes = view.retakes;
        drop(prompter);

        let shows = segmented(
            "prompter-shows",
            WELL,
            vec![
                self.segment("Beats", mode == PrompterMode::Beats, cx, |p, cx| {
                    p.set_mode(PrompterMode::Beats, cx)
                }),
                self.segment(
                    "Full script",
                    mode == PrompterMode::FullScript,
                    cx,
                    |p, cx| p.set_mode(PrompterMode::FullScript, cx),
                ),
            ],
        );
        let scroll = |text, mode: ScrollMode, cx: &mut Context<Self>| {
            self.segment(text, s.scroll == mode, cx, move |p, cx| {
                p.change(cx, |s| s.scroll = mode)
            })
        };
        let scroll = segmented(
            "prompter-scroll",
            WELL,
            vec![
                scroll("Voice", ScrollMode::Voice, cx),
                scroll("Steady", ScrollMode::Steady, cx),
                scroll("Pedal", ScrollMode::Pedal, cx),
            ],
        );
        let colors = segmented(
            "prompter-colors",
            WELL,
            Colors::ALL
                .into_iter()
                .map(|colors| {
                    self.segment(colors.label(), s.colors == colors, cx, move |p, cx| {
                        p.change(cx, |s| s.colors = colors)
                    })
                })
                .collect(),
        );
        let countdown = segmented(
            "prompter-countdown",
            WELL,
            COUNTDOWNS
                .into_iter()
                .map(|seconds| {
                    let text = if seconds == 0 {
                        "Off".to_owned()
                    } else {
                        format!("{seconds} s")
                    };
                    self.segment(text, s.countdown == seconds, cx, move |p, cx| {
                        p.change(cx, |s| s.countdown = seconds)
                    })
                })
                .collect(),
        );
        let keys = segmented(
            "prompter-keys",
            WELL,
            vec![
                self.segment("⌃⌥ + key", !s.plain_keys, cx, |p, cx| {
                    p.change(cx, |s| s.plain_keys = false)
                }),
                self.segment("Key alone", s.plain_keys, cx, |p, cx| {
                    p.change(cx, |s| s.plain_keys = true)
                }),
            ],
        );

        pane()
            .id("prompter")
            .w(px(412.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(14.))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Elgato Prompter"),
                    )
                    .children(button.map(|text| {
                        div()
                            .id("prompter-toggle")
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(10.))
                            .h(px(28.))
                            .rounded(RADIUS_SM)
                            .bg(RAISED)
                            .cursor_pointer()
                            .hover(|s| s.bg(FLOAT))
                            .text_size(TEXT_SM)
                            .font_weight(FontWeight::MEDIUM)
                            .child(text)
                            .child(kbd(TOGGLE_KEYS))
                            .on_click(cx.listener(|pane, _, _, cx| pane.toggle(cx)))
                    })),
            )
            .child(preview)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child(
                        div()
                            .mt(px(5.))
                            .size(px(7.))
                            .flex_none()
                            .rounded_full()
                            .bg(if shown { ACCENT } else { FAINT }),
                    )
                    .child(div().flex_1().min_w_0().child(status)),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap(px(8.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child(div().truncate().child(chapter))
                    .child(
                        mono(format!(
                            "{retakes} {} · ~{}:{:02} left",
                            if retakes == 1 { "retake" } else { "retakes" },
                            left / 60,
                            left % 60
                        ))
                        .flex_none(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(row("Shows", shows.w_full()))
                    .child(row("Scroll", scroll.w_full()))
                    .child(self.slider_row(TEXT_SIZE))
                    .when(s.scroll == ScrollMode::Steady, |rows| {
                        rows.child(self.slider_row(SPEED))
                    })
                    .child(self.disclosure(s.more, cx))
                    .when(s.more, |rows| {
                        rows.child(self.display_picker(cx))
                            .child(row(
                                "Mirror",
                                Switch::new("prompter-mirror")
                                    .checked(s.mirror)
                                    .accessibility_label("Mirror the output")
                                    .on_click(cx.listener(|pane, on: &bool, _, cx| {
                                        let on = *on;
                                        pane.change(cx, |s| s.mirror = on)
                                    })),
                            ))
                            .children(MORE.map(|index| self.slider_row(index)))
                            .child(row("Countdown", countdown.w_full()))
                            .child(row("Colors", colors.w_full()))
                            .when(s.scroll == ScrollMode::Pedal, |rows| {
                                rows.child(row(
                                    "Pedal key",
                                    div()
                                        .id("prompter-pedal")
                                        .px(px(10.))
                                        .h(px(28.))
                                        .flex()
                                        .items_center()
                                        .rounded(RADIUS_SM)
                                        .bg(RAISED)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(FLOAT))
                                        .text_size(TEXT_SM)
                                        .child(s.advance.label())
                                        .on_click(cx.listener(|pane, _, _, cx| {
                                            pane.change(cx, |s| s.advance = s.advance.following())
                                        })),
                                ))
                            })
                            .child(row("Hotkeys", keys.w_full()))
                            .child(
                                div().flex().justify_end().pt(px(6.)).child(
                                    Button::new("prompter-reset")
                                        .small()
                                        .ghost()
                                        .label("Reset to defaults")
                                        .tooltip("Keeps the display and mirroring")
                                        .on_click(cx.listener(|pane, _, window, cx| {
                                            pane.reset_settings(window, cx)
                                        })),
                                ),
                            )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_never_covers_the_app_display_unless_asked() {
        let elgato = Some(3);
        assert_eq!(
            placement(elgato, true, false, Some(1), false),
            Placement::Shown(3)
        );
        assert_eq!(
            placement(elgato, false, false, Some(1), true),
            Placement::Away
        );
        assert_eq!(
            placement(elgato, true, true, Some(1), true),
            Placement::Hidden(3)
        );
        assert_eq!(
            placement(Some(1), true, false, Some(1), false),
            Placement::OverApp(1)
        );
        assert_eq!(
            placement(Some(1), true, false, Some(1), true),
            Placement::Shown(1)
        );
        assert_eq!(
            placement(None, true, false, Some(1), true),
            Placement::Missing
        );
    }
}
