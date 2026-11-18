//! The stage of the Record screen: what the next take records, the inputs, the monitor, the
//! scenes and the transport.

use std::sync::atomic::Ordering;
use std::time::Duration;

use gpui::{Context, Div, FontWeight, div, prelude::*, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::progress::Progress;
use gpui_component::{Disableable, Sizable};
use shot_capture::Preview;

use super::outline::{Part, length};
use super::{Phase, RecordScreen, ReviewTakes, live_camera, live_screen, styled_screen};
use crate::edit::parts::layout_name;
use crate::edit::picture::thumbnail;
use crate::takes::take_name;
use crate::theme::*;
use crate::ui::{self, alert, hint, kbd, label, mono};

/// Width of a scene's drawing.
const SCENE: f32 = 52.;

impl RecordScreen {
    pub(super) fn stage(&self, phase: &Phase, busy: bool, cx: &mut Context<Self>) -> Div {
        let live = self.shared.preview.lock().expect("preview").clone();
        let screen = live.as_ref().and_then(Preview::screen);
        let camera = live.as_ref().and_then(Preview::camera);
        let problem = self
            .preview_problem(screen.is_some(), camera.is_some())
            .filter(|_| !busy);
        let blocked = self.choice.nothing_to_record();
        let script = &self.folder.read(cx).project.script;
        let part = self.part.beats(script).map(|_| match self.part {
            Part::Script => "Whole script, retakes inline".to_owned(),
            Part::Chapter { chapter, .. }
                if self
                    .outline
                    .chapters
                    .get(chapter)
                    .is_some_and(|c| !c.takes.is_empty()) =>
            {
                format!("Retake {}", self.part.name(script))
            }
            Part::Chapter { .. } => self.part.name(script),
        });
        let recording = matches!(phase, Phase::Recording { .. });
        let (title, detail): (String, String) = match phase {
            Phase::Idle | Phase::Failed(_) | Phase::Finished { .. } => (
                crate::takes::take_name(&self.next_take(cx)),
                if let Some(reason) = blocked {
                    reason.into()
                } else if !self.choice.inputs.screen && self.choice.camera().is_none() {
                    "Sound only: this take records no picture. Press Space to start.".into()
                } else if self.standby_starting() {
                    "Starting the preview.".into()
                } else if problem.is_some() {
                    "Check the problem above first. Space still records the inputs that started."
                        .into()
                } else {
                    "Press Space or the record button to start.".into()
                },
            ),
            Phase::Downloading { .. } => (
                "Downloading the speech model".into(),
                "Once, on first use. Recording starts when it is ready.".into(),
            ),
            Phase::Loading => (
                "Starting".into(),
                "Loading the speech model and the capture devices.".into(),
            ),
            Phase::Recording { take, .. } => (
                format!("Recording {}", take_name(take).to_lowercase()),
                "Space stops. The take joins the outline.".into(),
            ),
            Phase::Finishing => (
                "Finishing the take".into(),
                "Writing the last fragments and the transcript.".into(),
            ),
        };
        let elapsed = match phase {
            Phase::Recording { started, .. } => started.elapsed(),
            _ => Duration::ZERO,
        };
        let styled = self.shared.styled.load(Ordering::Relaxed);
        let monitor = self.shared.monitor.lock().expect("monitor").clone();
        let monitor_error = monitor.as_ref().and_then(|m| m.error()).filter(|_| styled);
        let monitored = monitor.and_then(|m| m.frame()).filter(|_| styled);
        let start = if problem.is_some() {
            "Record anyway"
        } else {
            "Start"
        };
        ui::pane()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(17.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .when(recording, |title| title.text_color(RECORD_TEXT))
                            .child(title),
                    )
                    .child(div().flex_1().min_w_0().truncate().text_color(MUTED).child(
                        part.unwrap_or_else(|| self.folder.read(cx).project.video.title.clone()),
                    )),
            )
            .child(self.inputs_bar(busy, cx))
            .children(monitor_error.map(|e| {
                div()
                    .text_size(TEXT_SM)
                    .text_color(MUTED)
                    .truncate()
                    .child(format!("Showing the raw feed: {e}"))
            }))
            .children(self.failure(cx))
            .children(problem.map(|problem| {
                alert(problem).child(
                    Button::new("retry-preview")
                        .small()
                        .label("Try again")
                        .on_click(cx.listener(|this, _, _, cx| this.restart(cx))),
                )
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .rounded(px(12.))
                    .bg(gpui::rgb(0x050607))
                    .when(recording, |stage| {
                        stage
                            .border_2()
                            .border_color(gpui::Hsla::from(RECORD).opacity(0.55))
                    })
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(14.))
                    .map(|stage| match (monitored, screen.clone(), camera.clone()) {
                        (Some(frame), _, _) => stage.child(styled_screen(frame)),
                        (None, Some(screen), camera) => stage.child(live_screen(screen, camera)),
                        (None, None, Some(camera)) => stage.child(live_camera(camera)),
                        (None, None, None) => stage,
                    })
                    .when(recording && screen.is_none() && camera.is_none(), |stage| {
                        stage.child(
                            div()
                                .size(px(14.))
                                .rounded_full()
                                .bg(RECORD)
                                .border_4()
                                .border_color(gpui::Hsla::from(RECORD).opacity(0.18)),
                        )
                    })
                    .when_some(
                        match phase {
                            Phase::Downloading { done, total } => Some((*done, *total)),
                            _ => None,
                        },
                        |stage, (done, total)| {
                            stage.child(
                                div()
                                    .w(px(320.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.))
                                    .child(
                                        Progress::new("model")
                                            .value(done as f32 / total.max(1) as f32 * 100.0),
                                    )
                                    .child(
                                        mono(format!(
                                            "{} of {} MB",
                                            done / 1_000_000,
                                            total / 1_000_000
                                        ))
                                        .text_size(TEXT_SM)
                                        .text_color(MUTED),
                                    ),
                            )
                        },
                    )
                    .when(screen.is_none() && camera.is_none(), |stage| {
                        stage.child(div().text_color(MUTED).child(detail))
                    })
                    .children(
                        self.prompter
                            .update(cx, |prompter, _| prompter.toast())
                            .map(|toast| {
                                div()
                                    .absolute()
                                    .bottom(px(14.))
                                    .left_0()
                                    .right_0()
                                    .flex()
                                    .justify_center()
                                    .child(toast)
                            }),
                    ),
            )
            .child(self.scene_strip(styled, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .child(
                        Button::new("record-button")
                            .danger()
                            .disabled(blocked.is_some() && !recording)
                            .when_some(blocked.filter(|_| !recording), |button, reason| {
                                button.tooltip(reason)
                            })
                            .accessibility_label(if recording {
                                "Stop recording"
                            } else {
                                "Start recording"
                            })
                            .w(px(46.))
                            .h(px(46.))
                            .rounded(px(12.))
                            .child(
                                div()
                                    .size(px(14.))
                                    .bg(ON_ACCENT)
                                    .when(recording, |d| d.rounded(px(3.)))
                                    .when(!recording, |d| d.rounded_full()),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle(&super::ToggleRecording, window, cx)
                            })),
                    )
                    .child(
                        mono(format!(
                            "{:02}:{:02}",
                            elapsed.as_secs() / 60,
                            elapsed.as_secs() % 60
                        ))
                        .text_size(px(26.))
                        .font_weight(FontWeight::MEDIUM),
                    )
                    .child(div().flex_1().min_w_0().flex().justify_center().map(|row| {
                        if recording {
                            row.child(self.prompter.read(cx).legend())
                        } else {
                            row.children(self.saved_note(cx))
                        }
                    }))
                    .when(blocked.is_none() || recording, |row| {
                        row.child(hint("Space", if recording { "Stop" } else { start }))
                    }),
            )
    }

    /// What the last take did, with a way to its review.
    fn saved_note(&self, cx: &mut Context<Self>) -> Option<Div> {
        let saved = self.saved.as_ref()?;
        let replaced = match saved.replaced {
            0 => String::new(),
            1 => ", in place of an earlier take at 1 beat".into(),
            n => format!(", in place of earlier takes at {n} beats"),
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .min_w_0()
                .child(
                    gpui::svg()
                        .path(icon_path("check"))
                        .size(ICON_SM)
                        .text_color(ACCENT),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(TEXT_SM)
                        .text_color(BODY)
                        .child(format!(
                            "{} saved, {}{replaced}",
                            take_name(&saved.take),
                            length(saved.length)
                        )),
                )
                .child(
                    Button::new("review-take")
                        .small()
                        .ghost()
                        .label("Review")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ReviewTakes))),
                ),
        )
    }

    /// The scenes the take can show, the picked one lit: before a take the one it starts in,
    /// while recording a switch. The styled and raw monitor switch sits at its end.
    fn scene_strip(&self, styled: bool, cx: &mut Context<Self>) -> Div {
        let scenes = self.scenes(cx);
        let picked = self
            .prompter
            .read(cx)
            .live()
            .scene()
            .or_else(|| scenes.first().copied());
        let folder = self.folder.read(cx);
        let custom = folder.theme.as_ref().map_or(&[][..], |t| &t.layouts[..]);
        let shared = self.shared.clone();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .child(label("Scene").pr(px(6.)))
            .children(scenes.iter().enumerate().map(|(index, &layout)| {
                let on = picked == Some(layout);
                div()
                    .id(("scene", index))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .p(px(5.))
                    .pr(px(10.))
                    .rounded(RADIUS_SM)
                    .cursor_pointer()
                    .bg(if on { FLOAT } else { RAISED })
                    .when(on, |chip| chip.border_1().border_color(ACCENT))
                    .hover(|s| s.bg(FLOAT))
                    .child(div().w(px(SCENE)).child(thumbnail(layout, custom)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if on { STRONG } else { MUTED })
                                    .child(layout_name(layout)),
                            )
                            .child(kbd((index + 1).to_string())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_scene(index, cx)))
            }))
            .child(div().flex_1())
            .child(
                ui::segmented(
                    "monitor",
                    WELL,
                    [("Styled", true), ("Raw", false)]
                        .into_iter()
                        .map(|(name, on)| {
                            let shared = shared.clone();
                            ui::Segment::new(name, styled == on, move |_, window, _| {
                                shared.styled.store(on, Ordering::Relaxed);
                                window.refresh();
                            })
                        })
                        .collect(),
                )
                .w(px(150.)),
            )
    }
}
