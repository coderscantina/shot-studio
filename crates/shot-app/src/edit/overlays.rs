//! Overlays and derived cuts in the Timeline workspace: the Overlays and Highlight lanes, and a
//! pane listing every overlay with its template parameters, the one at the playhead lit, and
//! the Shorts and X clips with their export.
//!
//! The lanes and the pane show a person's overlays and the lock's alike. A person's can be
//! changed: every action writes `cut.kdl`. Picking b-roll fills a slot from the files the video
//! can use, a boolean parameter toggles in place, and
//! remove drops the overlay. The lock's say which rule or override made them, and remove turns
//! one off on its element's override.
//!
//! Add places an overlay at the playhead: one of the channel's overlay presets, written as a
//! normal `overlay` node with its parameters, or a template with example values. Edit opens a
//! person's overlay in the overlay editor ([`crate::brand::overlay`]).

use std::path::Path;

use gpui::{App, Context, Entity, FontWeight, Hsla, Window, div, prelude::*, px, rgb};
use gpui_component::Sizable;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use shot_core::{
    Applied, Cut, DerivedKind, Id, Instance, Lock, Overlay, OverlayPreset, ParamValue, TakeAt,
};
use shot_render::overlay::{self, Params, broll, highlight, link_card};

use super::EditScreen;
use crate::model::timecode;
use crate::theme::*;
use crate::ui::{self, icon, label, mono};

/// The b-roll lane color from design.md. Not a theme token yet.
const BROLL: u32 = 0xB69CFF;
/// The Overlays pane's width in the Timeline's top row.
pub const PANE_WIDTH: f32 = 400.;

/// Where an overlay sits: its block in the cut, its index there, and its output frames.
struct Placed {
    at: TakeAt,
    index: usize,
    overlay: Overlay,
    frames: std::ops::Range<u64>,
}

/// Every overlay the edit shows, a person's and the lock's, in output order.
fn placed(this: &EditScreen, cx: &App) -> Vec<Placed> {
    let folder = this.folder.read(cx);
    let mut out: Vec<Placed> = crate::model::blocks(&folder.project.cut)
        .flat_map(|(at, block)| {
            crate::model::overlays(block, folder.lock())
                .into_iter()
                .filter_map(move |overlay| {
                    Some(Placed {
                        at,
                        index: 0,
                        frames: this.map.output_range(at, overlay.range)?,
                        overlay,
                    })
                })
        })
        .collect();
    out.sort_by_key(|p| p.frames.start);
    out
}

/// The lock's instance of `take` that draws `overlay`.
pub(super) fn lock_instance<'a>(
    lock: &'a Lock,
    take: &Id,
    overlay: &Overlay,
) -> Option<&'a Instance> {
    lock.take(take)?.instances.iter().find(|i| {
        matches!(&i.applied, Applied::Overlay { template, range, params }
            if *template == overlay.template
                && *range == overlay.range
                && *params == overlay.params)
    })
}

/// Every overlay of the Overlays lane, or with `highlight` of the Highlight lane, a person's
/// with its index in `cut.kdl`, the lock's with its instance.
pub fn lane_items(
    this: &EditScreen,
    highlight: bool,
    cx: &Context<EditScreen>,
) -> Vec<super::timeline::Item> {
    let folder = this.folder.read(cx);
    let mut items = Vec::new();
    for (at, block) in crate::model::blocks(&folder.project.cut) {
        for overlay in crate::model::overlays(block, folder.lock()) {
            if super::timeline::is_highlight(overlay.template.as_str()) != highlight {
                continue;
            }
            let Some(frames) = this.map.output_range(at, overlay.range) else {
                continue;
            };
            let placed = block
                .overlays
                .iter()
                .position(|o| *o == overlay)
                .map(shot_core::Placed::Overlay);
            let lock = match placed {
                Some(_) => None,
                None => lock_instance(folder.lock(), &block.take, &overlay)
                    .map(|i| (block.take.clone(), i.clone())),
            };
            items.push(super::timeline::Item {
                at,
                placed,
                lock,
                frames,
                text: summary(&overlay),
            });
        }
    }
    items
}

/// The fill and text of the Overlays lane in violet, or of the Highlight lane in lime.
pub fn lane_colors(highlight: bool) -> (Hsla, Hsla) {
    if highlight {
        (SELECTION.into(), ACCENT.into())
    } else {
        (rgb(BROLL).alpha(0.3).into(), rgb(BROLL).into())
    }
}

/// One line about an overlay: what it shows.
pub fn summary(overlay: &Overlay) -> String {
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
        overlay::callout::TEMPLATE => p.text("text").unwrap_or("Callout").to_owned(),
        overlay::pop::TEMPLATE => p
            .text("sticker")
            .or(p.text("text"))
            .unwrap_or("Pop")
            .to_owned(),
        overlay::chapter_band::TEMPLATE => p.text("text").unwrap_or("Chapter band").to_owned(),
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
    /// Removes the lock's overlay or sound `instance` of `take`, see
    /// [`crate::folder::VideoFolder::remove_instance`].
    pub(super) fn remove_instance(
        &mut self,
        take: &Id,
        instance: &Instance,
        cx: &mut Context<Self>,
    ) {
        let result = self.folder.update(cx, |folder, cx| {
            folder.remove_instance(take, instance)?;
            cx.notify();
            Ok(())
        });
        self.wrote(result, cx);
    }

    /// Puts what `change` makes of the overlay at `index` of the block at `at` in its place,
    /// or with `None` removes it. Through the core, so the overlay's copies across a splice
    /// change with it ([`Cut::set_overlay`]).
    fn change_overlay(
        &mut self,
        at: TakeAt,
        index: usize,
        change: impl FnOnce(Overlay) -> Option<Overlay>,
        cx: &mut Context<Self>,
    ) {
        self.edit(cx, |cut| {
            let block = cut
                .chapters
                .get(at.chapter)
                .and_then(|c| c.takes.get(at.take));
            Ok(match block.and_then(|block| block.overlays.get(index)) {
                Some(overlay) => cut.set_overlay(at, index, change(overlay.clone())),
                None => cut.clone(),
            })
        });
    }

    /// Shows `file` in the b-roll slot `index` of the block at `at`.
    fn set_broll_file(&mut self, at: TakeAt, index: usize, file: String, cx: &mut Context<Self>) {
        self.change_overlay(
            at,
            index,
            |mut overlay| {
                let id = shot_core::Id::new("file").expect("an id");
                overlay.params.retain(|(n, _)| *n != id);
                overlay.params.insert(0, (id, ParamValue::Text(file)));
                Some(overlay)
            },
            cx,
        );
    }

    /// Switches the boolean parameter `name`, now `on`, of the overlay `index` at `at`.
    fn toggle_param(
        &mut self,
        at: TakeAt,
        index: usize,
        name: shot_core::Id,
        on: bool,
        cx: &mut Context<Self>,
    ) {
        self.change_overlay(
            at,
            index,
            |mut overlay| {
                if let Some((_, v)) = overlay.params.iter_mut().find(|(n, _)| *n == name) {
                    *v = ParamValue::Bool(!on);
                }
                Some(overlay)
            },
            cx,
        );
    }

    /// Places `draft` at the playhead and opens it in the overlay editor.
    fn place_draft(&mut self, draft: crate::brand::overlay::Draft, cx: &mut Context<Self>) {
        let Some((at, t)) = self.map.source_at(self.playhead) else {
            self.overlay_note = Some("Move the playhead onto the cut to place an overlay".into());
            cx.notify();
            return;
        };
        let overlay = draft.overlay(t);
        self.edit(cx, |cut| cut.add_overlay(at, overlay));
        let index = self
            .folder
            .read(cx)
            .project
            .cut
            .chapters
            .get(at.chapter)
            .and_then(|c| c.takes.get(at.take))
            .map_or(0, |b| b.overlays.len().saturating_sub(1));
        self.edit_overlay(at, index, cx);
    }

    pub(super) fn place_preset(&mut self, preset: OverlayPreset, cx: &mut Context<Self>) {
        self.overlay_note = Some(format!("Placed {}", preset.name));
        self.place_draft(
            crate::brand::overlay::Draft {
                name: None,
                template: preset.template,
                params: preset.params,
                length: preset.length,
            },
            cx,
        );
    }

    fn place_template(&mut self, template: Id, cx: &mut Context<Self>) {
        self.overlay_note = None;
        self.place_draft(crate::brand::overlay::starter(None, &template), cx);
    }

    /// Opens a person's overlay `index` of the block at `at` in the overlay editor.
    pub fn edit_overlay(&mut self, at: TakeAt, index: usize, cx: &mut Context<Self>) {
        if let Err(error) = crate::brand::overlay::open_placed(self.folder.clone(), at, index, cx) {
            self.error = Some(error);
            cx.notify();
        }
    }
}

/// What the Overlays pane offers for a person's overlay `index` of the block at `at`, as menu
/// items: the file of a b-roll slot and the switches of its parameters.
pub fn menu_items(
    menu: PopupMenu,
    screen: &Entity<EditScreen>,
    at: TakeAt,
    index: usize,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some((overlay, dir)) = overlay_at(screen.read(cx), at, index, cx) else {
        return menu;
    };
    let mut menu = menu;
    let editing = screen.clone();
    menu =
        menu.item(PopupMenuItem::new("Edit…").on_click(move |_, _, cx| {
            editing.update(cx, |this, cx| this.edit_overlay(at, index, cx))
        }));
    if overlay.template.as_str() == broll::TEMPLATE {
        let files = candidates(&dir);
        let current = Params(&overlay).text("file").map(str::to_owned);
        if !files.is_empty() {
            let screen = screen.clone();
            menu = menu.submenu("File", window, cx, move |menu, _, _| {
                files.iter().fold(menu, |menu, file| {
                    let (screen, file) = (screen.clone(), file.clone());
                    menu.item(
                        PopupMenuItem::new(file.clone())
                            .checked(current.as_deref() == Some(file.as_str()))
                            .on_click(move |_, _, cx| {
                                let file = file.clone();
                                screen
                                    .update(cx, |this, cx| this.set_broll_file(at, index, file, cx))
                            }),
                    )
                })
            });
        }
    }
    for (name, value) in &overlay.params {
        let ParamValue::Bool(on) = *value else {
            continue;
        };
        let (screen, name) = (screen.clone(), name.clone());
        menu = menu.item(
            PopupMenuItem::new(name.as_str().replace('-', " "))
                .checked(on)
                .on_click(move |_, _, cx| {
                    let name = name.clone();
                    screen.update(cx, |this, cx| this.toggle_param(at, index, name, on, cx))
                }),
        );
    }
    menu
}

fn overlay_at(
    this: &EditScreen,
    at: TakeAt,
    index: usize,
    cx: &App,
) -> Option<(Overlay, std::path::PathBuf)> {
    let folder = this.folder.read(cx);
    let overlay = folder
        .project
        .cut
        .chapters
        .get(at.chapter)?
        .takes
        .get(at.take)?
        .overlays
        .get(index)?
        .clone();
    Some((overlay, folder.dir.clone()))
}

/// The Overlays pane: rules, every overlay with the one at the playhead lit, and the derived
/// cuts.
pub fn pane(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let placed = placed(this, cx);
    let folder = this.folder.read(cx);
    let dir = folder.dir.clone();
    let cut = folder.project.cut.clone();
    let lock = folder.lock();
    let mut entries = Vec::new();
    for mut p in placed {
        let Some(block) = cut
            .chapters
            .get(p.at.chapter)
            .and_then(|c| c.takes.get(p.at.take))
        else {
            continue;
        };
        let made = match block.overlays.iter().position(|o| *o == p.overlay) {
            Some(index) => {
                p.index = index;
                Made::Yours
            }
            None => {
                let instance = lock_instance(lock, &block.take, &p.overlay);
                Made::Lock {
                    provenance: instance.map_or_else(
                        || "From the style rules".to_owned(),
                        |i| crate::place::provenance(&i.provenance),
                    ),
                    instance: instance.map(|i| (block.take.clone(), i.clone())),
                }
            }
        };
        let checked = match &folder.theme {
            Ok(theme) => theme
                .check(&p.overlay, &block.take)
                .map(|()| about(&p.overlay))
                .map_err(|e| e.to_string()),
            Err(error) => Err(error.clone()),
        };
        entries.push((p, made, checked));
    }
    let files = candidates(&dir);
    let playhead = this.playhead;
    let rows: Vec<_> = entries
        .into_iter()
        .enumerate()
        .map(|(key, (p, made, checked))| {
            let now = p.frames.contains(&playhead);
            overlay_row(&p, key, now, &files, &made, checked, cx)
        })
        .collect();
    let empty = rows.is_empty().then(|| {
        label(
            "No overlays yet. This video's link cards, highlights and b-roll come from its style rules.",
        )
        .px(px(10.))
    });
    ui::pane()
        .w(px(PANE_WIDTH))
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
                .child(add_menu(this, cx)),
        )
        .children(
            this.overlay_note
                .clone()
                .map(|note| label(note).px(px(10.))),
        )
        .children(empty)
        .child(ui::scrolling_list(
            "overlay-list",
            &this.overlays_scroll,
            div().flex().flex_col().gap(px(4.)).children(rows),
        ))
        .child(derived_cuts(&cut, cx))
}

/// The Add menu: the channel's overlay presets, the templates, and the way to the presets.
fn add_menu(this: &EditScreen, cx: &mut Context<EditScreen>) -> impl IntoElement {
    let folder = this.folder.read(cx);
    let repo = crate::brand::overlay::repo_of(&folder.dir);
    let channel = repo
        .as_deref()
        .map(crate::brand::channel_brand)
        .unwrap_or_default();
    let brand = folder.lock().brand.clone();
    let presets = channel.overlay_presets;
    let templates = crate::brand::overlay::templates(&brand);
    let screen = cx.entity();
    Button::new("add-overlay")
        .label("Add")
        .small()
        .ghost()
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu.label("Presets");
            if presets.is_empty() {
                menu = menu.item(PopupMenuItem::new("None yet").disabled(true));
            }
            for preset in &presets {
                let (screen, preset) = (screen.clone(), preset.clone());
                menu = menu.item(
                    PopupMenuItem::new(format!(
                        "{}  ·  {}, {}",
                        preset.name,
                        preset.template,
                        crate::model::seconds(preset.length)
                    ))
                    .on_click(move |_, _, cx| {
                        let preset = preset.clone();
                        screen.update(cx, |this, cx| this.place_preset(preset, cx))
                    }),
                );
            }
            menu = menu.separator().label("Templates");
            for template in &templates {
                let (screen, template) = (screen.clone(), template.clone());
                menu = menu.item(PopupMenuItem::new(template.to_string()).on_click(
                    move |_, _, cx| {
                        let template = template.clone();
                        screen.update(cx, |this, cx| this.place_template(template, cx))
                    },
                ));
            }
            if let Some(repo) = repo.clone() {
                menu = menu
                    .separator()
                    .item(
                        PopupMenuItem::new("Edit presets…").on_click(move |_, _, cx| {
                            if let Err(error) =
                                crate::brand::open_overlay_preset(&repo, None, None, cx)
                            {
                                eprintln!("{error}");
                            }
                        }),
                    );
            }
            menu
        })
}

/// Who made an overlay the pane lists.
enum Made {
    /// A person's overlay in the take block, which the pane can change.
    Yours,
    /// The lock's, with the rule or override that made it and the instance to remove.
    Lock {
        provenance: String,
        instance: Option<(Id, Instance)>,
    },
}

/// What an overlay's template is, for a built-in one or a brand Lottie.
fn about(overlay: &Overlay) -> &'static str {
    overlay::spec(overlay.template.as_str()).map_or("a Lottie template of the brand", |s| s.about)
}

/// One overlay of the pane, lit when `now`, at the playhead.
fn overlay_row(
    p: &Placed,
    key: usize,
    now: bool,
    files: &[String],
    made: &Made,
    checked: Result<&'static str, String>,
    cx: &mut Context<EditScreen>,
) -> gpui::AnyElement {
    let overlay = &p.overlay;
    let template = overlay.template.as_str();
    let color = match template {
        highlight::TEMPLATE => ACCENT,
        _ => rgb(BROLL),
    };
    let yours = matches!(made, Made::Yours);
    let lock_instance = match made {
        Made::Lock { instance, .. } => instance.clone(),
        Made::Yours => None,
    };
    let (at, index) = (p.at, p.index);
    let mut params = div().flex().flex_col().gap(px(2.)).pl(px(26.));
    for (name, value) in &overlay.params {
        let shown = match value {
            ParamValue::Text(text) => text.clone(),
            ParamValue::Integer(n) if is_time(template, name.as_str()) => {
                format!("{:.2} s", *n as f64 / 1e6)
            }
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
                div()
                    // Geist Mono is for file names; labels and values stay in Geist.
                    .when(name.as_str() == "file", |value| value.font_family(MONO))
                    .text_color(BODY)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(shown),
            );
        if let (ParamValue::Bool(on), true) = (value, yours) {
            let name = name.clone();
            let on = *on;
            line = line
                .cursor_pointer()
                .hover(|s| s.bg(FLOAT))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_param(at, index, name.clone(), on, cx)
                }));
        }
        params = params.child(line);
    }
    let slot = yours && template == broll::TEMPLATE && !files.is_empty();
    let next_file = {
        let current = Params(overlay).text("file");
        let position = current.and_then(|c| files.iter().position(|f| f == c));
        files[position.map_or(0, |i| (i + 1) % files.len().max(1))..]
            .first()
            .cloned()
    };
    div()
        .flex()
        .flex_none()
        .flex_col()
        .gap(px(4.))
        .px(px(10.))
        .py(px(8.))
        .rounded(RADIUS_SM)
        .border_1()
        .border_color(if now {
            Hsla::from(color)
        } else {
            Hsla::from(CARD)
        })
        .bg(if now { FLOAT } else { CARD })
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
                            if let Some(file) = next_file.clone() {
                                this.set_broll_file(at, index, file, cx);
                            }
                        }))
                }))
                .when(yours, |row| {
                    row.child(
                        div()
                            .id(("edit", key))
                            .px(px(6.))
                            .rounded(px(6.))
                            .hover(|s| s.bg(FLOAT))
                            .cursor_pointer()
                            .text_size(TEXT_XS)
                            .text_color(BODY)
                            .child("Edit")
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.edit_overlay(at, index, cx)),
                            ),
                    )
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
                                this.change_overlay(at, index, |_| None, cx);
                            })),
                    )
                })
                .children(lock_instance.map(|(take, instance)| {
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
                            this.remove_instance(&take, &instance, cx);
                        }))
                })),
        )
        .child(
            div()
                .pl(px(26.))
                .text_size(TEXT_XS)
                .text_color(MUTED)
                .child(match checked {
                    Ok(about) => about.to_owned(),
                    Err(e) => e,
                }),
        )
        .child(
            div()
                .pl(px(26.))
                .text_size(TEXT_XS)
                .text_color(MUTED)
                .child(match made {
                    Made::Yours => "Added by you".to_owned(),
                    Made::Lock { provenance, .. } => provenance.clone(),
                }),
        )
        .child(params)
        .into_any_element()
}

/// Whether parameter `name` of `template` is a time in microseconds, shown in seconds.
fn is_time(template: &str, name: &str) -> bool {
    let keyframes = overlay::KEYFRAME_PARAMS.iter();
    overlay::spec(template).is_some_and(|spec| {
        spec.params
            .iter()
            .chain(keyframes)
            .any(|p| p.name == name && p.kind == overlay::ParamKind::Time)
    })
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
