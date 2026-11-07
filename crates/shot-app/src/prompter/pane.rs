//! The Prompter pane of the Record screen: what the prompter shows, its settings, and the
//! hotkey legend and retake toast the stage shows while recording.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, Context, Div, Entity, FontWeight, Render, SharedString, Window, WindowHandle, div,
    prelude::*, px, rgb,
};
use shot_capture::events::Hotkey;
use shot_core::PrompterMode;

use super::output::{self, PrompterOutput};
use super::settings::{ScrollMode, Settings};
use super::{Live, bindings, log_filter, native};
use crate::folder::VideoFolder;
use crate::theme::*;
use crate::ui::{Segment, hint, label, mono, pane, segmented};

/// How long the retake toast stays.
const TOAST: Duration = Duration::from_secs(4);

pub struct PrompterPane {
    folder: Entity<VideoFolder>,
    live: Arc<Live>,
    settings: Settings,
    displays: Vec<(u32, String)>,
    output: Option<(u32, WindowHandle<PrompterOutput>)>,
    hotkeys: Option<native::Hotkeys>,
    /// The retake toast on screen: its sequence number and when it appeared.
    toast: Option<(u32, Instant)>,
}

impl PrompterPane {
    pub fn new(folder: Entity<VideoFolder>, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let live = Live::new(&folder.read(cx).project, settings.scroll.into());
        // Opening a window while the main window is being built is not allowed.
        cx.spawn(async move |this, cx| this.update(cx, |this, cx| this.place_output(cx)))
            .detach();
        Self {
            folder,
            live,
            settings,
            displays: native::display_names(),
            output: None,
            hotkeys: None,
            toast: None,
        }
    }

    /// The prompter state the recorder thread and voice follow move.
    pub fn live(&self) -> Arc<Live> {
        self.live.clone()
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

    /// Opens, moves or closes the output window to match the settings and the displays.
    fn place_output(&mut self, cx: &mut Context<Self>) {
        let target = self.settings.pick_display(&self.displays);
        if self.output.as_ref().map(|(id, _)| *id) == target {
            return;
        }
        if let Some((_, handle)) = self.output.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        self.output = target.and_then(|display| {
            let handle = output::open(
                self.live.clone(),
                display,
                f32::from(self.settings.text_size),
                self.settings.mirror,
                cx,
            )?;
            Some((display, handle))
        });
        cx.notify();
    }

    fn change(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        edit(&mut self.settings);
        self.settings.save();
        if before.scroll != self.settings.scroll {
            self.live.lock().set_scroll(self.settings.scroll.into());
        }
        if before.display != self.settings.display {
            self.place_output(cx);
        }
        if let Some((_, handle)) = &self.output {
            let (mirror, size) = (self.settings.mirror, f32::from(self.settings.text_size));
            let _ = handle.update(cx, |output, window, cx| {
                native::set_mirror(window, mirror);
                output.text_size = size;
                cx.notify();
            });
        }
        if self.hotkeys.is_some() {
            self.arm();
        }
        cx.notify();
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

    /// The next display to show the prompter on, cycling through the displays and off.
    fn next_display(&mut self, cx: &mut Context<Self>) {
        self.displays = native::display_names();
        let names: Vec<Option<String>> = self
            .displays
            .iter()
            .map(|(_, name)| Some(name.clone()))
            .chain([Some(String::new())])
            .collect();
        let current = self
            .settings
            .pick_display(&self.displays)
            .and_then(|id| self.displays.iter().find(|(d, _)| *d == id))
            .map(|(_, name)| name.clone())
            .unwrap_or_default();
        let index = names
            .iter()
            .position(|n| n.as_deref() == Some(current.as_str()))
            .unwrap_or(0);
        let next = names[(index + 1) % names.len()].clone();
        self.change(cx, |s| s.display = next);
    }

    fn segment(
        &self,
        text: &'static str,
        on: bool,
        cx: &mut Context<Self>,
        apply: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Segment {
        let this = cx.entity().downgrade();
        Segment::new(text, on, move |_, _, cx: &mut App| {
            let _ = this.update(cx, |pane, cx| apply(pane, cx));
        })
    }
}

fn row(name: &'static str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .py(px(6.))
        .px(px(2.))
        .child(div().text_color(MUTED).flex_none().child(name))
        .child(control)
}

fn value_button(id: &'static str, text: impl Into<SharedString>) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .px(px(10.))
        .h(px(28.))
        .flex()
        .items_center()
        .rounded(RADIUS_SM)
        .bg(RAISED)
        .cursor_pointer()
        .hover(|s| s.bg(FLOAT))
        .text_size(TEXT_SM)
        .child(text.into())
}

impl Render for PrompterPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = self.folder.read(cx).project.format.prompter;
        let s = self.settings.clone();
        let display = self
            .output
            .as_ref()
            .and_then(|(id, _)| self.displays.iter().find(|(d, _)| d == id))
            .map(|(_, name)| name.clone());
        let status = match (&display, s.mirror) {
            (Some(_), true) => "connected · mirrored",
            (Some(_), false) => "connected",
            (None, _) => "not connected",
        };
        let follow = match s.scroll {
            ScrollMode::Voice => "following your voice".to_owned(),
            ScrollMode::Steady => "steady scroll".to_owned(),
            ScrollMode::Pedal => format!("pedal on {}", s.advance.label()),
        };
        let prompter = self.live.lock();
        let view = prompter.view();
        let left = view.time_left.as_secs();
        let preview = div()
            .w_full()
            .aspect_ratio(1024. / 600.)
            .rounded(RADIUS_MD)
            .bg(rgb(0x000000))
            .p(px(14.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(output::lines(&view, f32::from(s.text_size) * 0.36));
        let chapter = format!("{} · beat {} of {}", view.chapter, view.beat, view.beats);
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
        let mirror = segmented(
            "prompter-mirror",
            WELL,
            vec![
                self.segment("Off", !s.mirror, cx, |p, cx| {
                    p.change(cx, |s| s.mirror = false)
                }),
                self.segment("On", s.mirror, cx, |p, cx| {
                    p.change(cx, |s| s.mirror = true)
                }),
            ],
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
        let size = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                value_button("text-smaller", "−").on_click(cx.listener(|p, _, _, cx| {
                    p.change(cx, |s| s.text_size = s.text_size.saturating_sub(4).max(28))
                })),
            )
            .child(
                mono(format!("{} pt", s.text_size))
                    .w(px(52.))
                    .flex()
                    .justify_center(),
            )
            .child(
                value_button("text-larger", "+").on_click(cx.listener(|p, _, _, cx| {
                    p.change(cx, |s| s.text_size = (s.text_size + 4).min(96))
                })),
            );

        pane()
            .id("prompter")
            .w(px(412.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(14.))
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Elgato Prompter"),
                    )
                    .child(div().text_size(TEXT_SM).text_color(MUTED).child(status)),
            )
            .child(preview)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .child(div().size(px(7.)).rounded_full().bg(ACCENT))
                    .child(follow),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(row("Shows", shows.w(px(220.))))
                    .child(row("Scroll", scroll.w(px(220.))))
                    .child(row("Text size", size))
                    .child(row("Mirror", mirror.w(px(220.))))
                    .child(row(
                        "Display",
                        value_button("prompter-display", display.unwrap_or_else(|| "Off".into()))
                            .on_click(cx.listener(|p, _, _, cx| p.next_display(cx))),
                    ))
                    .when(s.scroll == ScrollMode::Pedal, |rows| {
                        rows.child(row(
                            "Pedal key",
                            value_button("prompter-pedal", s.advance.label()).on_click(
                                cx.listener(|p, _, _, cx| {
                                    p.change(cx, |s| s.advance = s.advance.following())
                                }),
                            ),
                        ))
                    })
                    .child(row("Hotkeys", keys.w(px(220.)))),
            )
            .child(
                div()
                    .mt_auto()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(label("Progress"))
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
                    ),
            )
    }
}
