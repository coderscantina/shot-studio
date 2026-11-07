//! Overlays and derived cuts in the Timeline workspace: the B-roll and Highlight lanes, and a
//! pane listing the overlays around the playhead with their template parameters, the overlay
//! rules, and the Shorts and X clips with their export.
//!
//! Every action writes `cut.kdl`: Find runs [`overlay_rules::run`], picking b-roll fills a slot
//! from the files the video can use, a boolean parameter toggles in place, and remove drops the
//! overlay.

use std::path::Path;

use gpui::{Context, FontWeight, div, prelude::*, px, rgb};
use shot_core::{Cut, DerivedKind, Overlay, ParamValue, TakeAt};
use shot_render::overlay::{self, Params, broll, highlight, link_card};
use shot_render::overlay_rules;
use shot_render::plan::snap;

use super::EditScreen;
use crate::model::{FPS, timecode};
use crate::theme::*;
use crate::ui::{self, icon, label, mono};

/// The b-roll lane color from design.md. Not a theme token yet.
const BROLL: u32 = 0xB69CFF;

/// Where an overlay sits: its block in the cut, its index there, and its output frames.
struct Placed<'a> {
    at: TakeAt,
    index: usize,
    overlay: &'a Overlay,
    frames: std::ops::Range<u64>,
}

/// Every overlay the edit shows, in output order, once per block.
fn placed(this: &EditScreen) -> Vec<Placed<'_>> {
    let mut out: Vec<Placed> = Vec::new();
    for span in &this.map.spans {
        for overlay in &span.overlays {
            let snapped = snap(overlay.range, FPS);
            let from = snapped.start.max(span.source_frames.start);
            let to = snapped.end.min(span.source_frames.end);
            if from >= to {
                continue;
            }
            let out_frame = |f: u64| span.output_frame + (f - span.source_frames.start);
            let frames = out_frame(from)..out_frame(to);
            match out
                .iter_mut()
                .find(|p| p.at == span.at && std::ptr::eq(p.overlay, overlay))
            {
                Some(p) => p.frames.end = p.frames.end.max(frames.end),
                None => out.push(Placed {
                    at: span.at,
                    index: 0,
                    overlay,
                    frames,
                }),
            }
        }
    }
    out
}

/// The B-roll or Highlight lane: blocks of `template` overlays in the lane window.
pub fn lane(
    this: &EditScreen,
    window: &std::ops::Range<u64>,
    x: impl Fn(u64) -> f32,
    template: &str,
) -> impl IntoElement {
    let (fill, text) = if template == broll::TEMPLATE {
        (rgb(BROLL).alpha(0.3), rgb(BROLL))
    } else {
        (SELECTION, ACCENT)
    };
    div().size_full().children(
        placed(this)
            .into_iter()
            .filter(|p| p.overlay.template.as_str() == template)
            .filter(|p| p.frames.end > window.start && p.frames.start < window.end)
            .map(move |p| {
                let left = x(p.frames.start.max(window.start));
                let right = x(p.frames.end.min(window.end));
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui::relative(left))
                    .w(gpui::relative((right - left).max(0.002)))
                    .rounded(px(6.))
                    .px(px(8.))
                    .bg(fill)
                    .text_color(text)
                    .text_size(TEXT_XS)
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(px(36.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(summary(p.overlay))
            }),
    )
}

/// One line about an overlay: what it shows.
fn summary(overlay: &Overlay) -> String {
    let p = Params(overlay);
    match overlay.template.as_str() {
        link_card::TEMPLATE => link_card::LinkCard::of(overlay)
            .map(|card| card.text().1)
            .unwrap_or_else(|| "Link".into()),
        highlight::TEMPLATE => p.text("text").unwrap_or("Highlight").to_owned(),
        broll::TEMPLATE => match p.text("file") {
            Some(file) => file.rsplit('/').next().unwrap_or(file).to_owned(),
            None => "Empty slot".into(),
        },
        _ => p
            .text("text")
            .or(p.text("diff"))
            .unwrap_or(overlay.template.as_str())
            .to_owned(),
    }
}

/// Stills, clips and diffs a b-roll slot can show: the video's `sources/`, the channel's
/// `library/` two folders up, and the screen recordings of the video's takes.
fn candidates(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut scan = |folder: &Path, prefix: &str| {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return;
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|name| broll::Media::of(name).is_some())
            .map(|name| format!("{prefix}{name}"))
            .collect();
        names.sort();
        files.extend(names);
    };
    scan(&dir.join("sources"), "sources/");
    scan(&dir.join("../../library"), "../../library/");
    if let Ok(takes) = std::fs::read_dir(dir.join("media")) {
        let mut takes: Vec<String> = takes
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|take| dir.join("media").join(take).join("screen.mp4").exists())
            .collect();
        takes.sort();
        files.extend(takes.into_iter().map(|t| format!("media/{t}/screen.mp4")));
    }
    files
}

impl EditScreen {
    /// Changes the overlay at `index` of the block at `at`.
    fn change_overlay(
        &mut self,
        at: TakeAt,
        index: usize,
        change: impl FnOnce(&mut Vec<Overlay>, usize),
        cx: &mut Context<Self>,
    ) {
        self.edit(cx, |cut| {
            let mut cut: Cut = cut.clone();
            if let Some(block) = cut
                .chapters
                .get_mut(at.chapter)
                .and_then(|c| c.takes.get_mut(at.take))
                && index < block.overlays.len()
            {
                change(&mut block.overlays, index);
            }
            Ok(cut)
        });
    }

    /// Runs the overlay rules off the UI thread, since Vision reads the screen, then writes the
    /// cut they return unless it changed meanwhile.
    fn find_overlays(&mut self, cx: &mut Context<Self>) {
        let folder = self.folder.read(cx);
        let (dir, before) = (folder.dir.clone(), folder.project.cut.clone());
        self.overlay_note = Some("Looking for links, idle screen and text on screen".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let input = before.clone();
            let found = cx
                .background_spawn(async move { overlay_rules::run(&dir, &input) })
                .await;
            this.update(cx, |this, cx| {
                let result = found.map_err(|e| e.to_string()).and_then(|(cut, added)| {
                    this.folder.update(cx, |folder, cx| {
                        if folder.project.cut != before {
                            return Err("the cut changed while looking; run Find again".into());
                        }
                        folder.write_cut(cut)?;
                        cx.notify();
                        Ok(added)
                    })
                });
                match result {
                    Ok(added) => {
                        this.overlay_note = Some(format!(
                            "Added {} link cards, {} b-roll slots, {} highlights",
                            added.link_cards, added.b_roll, added.highlights
                        ));
                    }
                    Err(error) => {
                        this.overlay_note = None;
                        this.error = Some(error);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// The Overlays pane: rules, the overlays around the playhead, and the derived cuts.
pub fn pane(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let folder = this.folder.read(cx);
    let dir = folder.dir.clone();
    let cut = folder.project.cut.clone();
    let window =
        this.playhead.saturating_sub(u64::from(FPS) * 5)..this.playhead + u64::from(FPS) * 20;
    let mut near: Vec<(Placed, usize)> = Vec::new();
    for p in placed(this) {
        if p.frames.end <= window.start || p.frames.start >= window.end {
            continue;
        }
        let Some(block) = cut
            .chapters
            .get(p.at.chapter)
            .and_then(|c| c.takes.get(p.at.take))
        else {
            continue;
        };
        if let Some(index) = block.overlays.iter().position(|o| o == p.overlay) {
            near.push((Placed { index, ..p }, near.len()));
        }
    }
    let files = candidates(&dir);
    let mut rows = Vec::with_capacity(near.len());
    for (p, key) in &near {
        rows.push(overlay_row(p, *key, &files, cx));
    }
    ui::pane()
        .w(px(400.))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(12.))
        .py(px(16.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .pb(px(6.))
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Overlays"),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("find-overlays")
                        .px(px(10.))
                        .py(px(4.))
                        .rounded(RADIUS_SM)
                        .bg(FLOAT)
                        .hover(|s| s.bg(FLOAT_ON))
                        .cursor_pointer()
                        .text_size(TEXT_SM)
                        .child("Find")
                        .on_click(cx.listener(|this, _, _, cx| this.find_overlays(cx))),
                ),
        )
        .children(
            this.overlay_note
                .clone()
                .map(|note| label(note).px(px(10.))),
        )
        .child(
            div()
                .id("overlay-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(rows),
        )
        .child(derived_cuts(&cut, cx))
}

fn overlay_row(
    p: &Placed,
    key: usize,
    files: &[String],
    cx: &mut Context<EditScreen>,
) -> gpui::AnyElement {
    let overlay = p.overlay;
    let template = overlay.template.as_str();
    let color = match template {
        broll::TEMPLATE => rgb(BROLL),
        highlight::TEMPLATE => ACCENT,
        _ => BODY,
    };
    let checked = overlay::check(overlay);
    let (at, index) = (p.at, p.index);
    let mut params = div().flex().flex_col().gap(px(2.)).pl(px(26.));
    for (name, value) in &overlay.params {
        let shown = match value {
            ParamValue::Text(text) => text.clone(),
            ParamValue::Integer(n) => n.to_string(),
            ParamValue::Bool(b) => if *b { "on" } else { "off" }.to_owned(),
        };
        let mut line = div()
            .id(("param", key * 64 + params_index(overlay, name.as_str())))
            .flex()
            .gap(px(8.))
            .text_size(TEXT_XS)
            .child(div().w(px(84.)).text_color(MUTED).child(name.to_string()))
            .child(
                mono(shown)
                    .text_color(BODY)
                    .overflow_hidden()
                    .whitespace_nowrap(),
            );
        if let ParamValue::Bool(on) = value {
            let name = name.clone();
            let on = *on;
            line = line
                .cursor_pointer()
                .hover(|s| s.bg(FLOAT))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let name = name.clone();
                    this.change_overlay(
                        at,
                        index,
                        |overlays, i| {
                            if let Some((_, v)) =
                                overlays[i].params.iter_mut().find(|(n, _)| *n == name)
                            {
                                *v = ParamValue::Bool(!on);
                            }
                        },
                        cx,
                    );
                }));
        }
        params = params.child(line);
    }
    let slot = template == broll::TEMPLATE && !files.is_empty();
    let next_file = {
        let current = Params(overlay).text("file");
        let position = current.and_then(|c| files.iter().position(|f| f == c));
        files[position.map_or(0, |i| (i + 1) % files.len().max(1))..]
            .first()
            .cloned()
    };
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .px(px(10.))
        .py(px(8.))
        .rounded(RADIUS_SM)
        .bg(CARD)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(icon(icon_of(template), ICON_SM, color))
                .child(
                    div()
                        .text_size(TEXT_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .child(template.to_owned()),
                )
                .child(
                    mono(timecode(p.frames.start))
                        .text_size(TEXT_XS)
                        .text_color(MUTED),
                )
                .child(div().flex_1())
                .children(slot.then(|| {
                    div()
                        .id(("pick", key))
                        .px(px(8.))
                        .rounded(px(6.))
                        .bg(FLOAT)
                        .hover(|s| s.bg(FLOAT_ON))
                        .cursor_pointer()
                        .text_size(TEXT_XS)
                        .child(if Params(overlay).text("file").is_some() {
                            "Next file"
                        } else {
                            "Pick"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let Some(file) = next_file.clone() else {
                                return;
                            };
                            this.change_overlay(
                                at,
                                index,
                                |overlays, i| {
                                    let id = shot_core::Id::new("file").expect("an id");
                                    overlays[i].params.retain(|(n, _)| *n != id);
                                    overlays[i].params.insert(0, (id, ParamValue::Text(file)));
                                },
                                cx,
                            );
                        }))
                }))
                .child(
                    div()
                        .id(("remove", key))
                        .px(px(6.))
                        .rounded(px(6.))
                        .hover(|s| s.bg(FLOAT))
                        .cursor_pointer()
                        .text_size(TEXT_XS)
                        .text_color(MUTED)
                        .child("Remove")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_overlay(
                                at,
                                index,
                                |overlays, i| {
                                    overlays.remove(i);
                                },
                                cx,
                            );
                        })),
                ),
        )
        .child(
            div()
                .pl(px(26.))
                .text_size(TEXT_XS)
                .text_color(MUTED)
                .child(match &checked {
                    Ok(spec) => spec.about.to_owned(),
                    Err(e) => e.to_string(),
                }),
        )
        .child(params)
        .into_any_element()
}

fn params_index(overlay: &Overlay, name: &str) -> usize {
    overlay
        .params
        .iter()
        .position(|(n, _)| n.as_str() == name)
        .unwrap_or(0)
}

fn icon_of(template: &str) -> &'static str {
    match template {
        broll::TEMPLATE => "picture",
        highlight::TEMPLATE => "edit",
        overlay::redact::BLEEP_TEMPLATE => "mic",
        overlay::redact::BLUR_TEMPLATE => "display",
        _ => "layout",
    }
}

/// The Shorts and X clips with their length and an export button each.
fn derived_cuts(cut: &Cut, cx: &mut Context<EditScreen>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .pt(px(8.))
        .child(label("Shorts and X clips").px(px(10.)))
        .children(cut.derived.is_empty().then(|| {
            div()
                .px(px(10.))
                .text_size(TEXT_SM)
                .text_color(MUTED)
                .child("None yet. Add one with shot derived add.")
        }))
        .children(cut.derived.iter().enumerate().map(|(i, derived)| {
            let seconds: u64 = derived.clips.iter().map(|c| c.range.duration().0).sum();
            let name = derived.name.to_string();
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(6.))
                .rounded(RADIUS_SM)
                .bg(CARD)
                .child(
                    div()
                        .text_size(TEXT_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .child(name.clone()),
                )
                .child(
                    div()
                        .text_size(TEXT_XS)
                        .text_color(MUTED)
                        .child(match derived.kind {
                            DerivedKind::Short => "9:16 Short",
                            DerivedKind::XClip => "1:1 X clip",
                        }),
                )
                .child(
                    mono(format!("{:.0} s", seconds as f64 / 1e6))
                        .text_size(TEXT_XS)
                        .text_color(MUTED),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id(("export-derived", i))
                        .px(px(8.))
                        .rounded(px(6.))
                        .bg(FLOAT)
                        .hover(|s| s.bg(FLOAT_ON))
                        .cursor_pointer()
                        .text_size(TEXT_XS)
                        .child("Export")
                        .on_click(cx.listener(move |this, _, _, cx| this.start_export(&name, cx))),
                )
        }))
}
