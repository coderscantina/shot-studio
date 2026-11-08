//! The overlay editor: a template with its parameters and timing as controls, checked against
//! the template's spec, over a live preview of a real frame that replays the overlay's enter and
//! exit. It edits an overlay preset of the brand (inside the theme window) or an overlay placed in
//! a video (in a window of its own, see [`open_placed`]).
//!
//! The editor holds a [`Draft`] and shows it at once. It emits [`Commit`] when an edit is done
//! (Enter, focus leaving a field, a switch); its owner writes the files and hands the result
//! back with [`OverlayEditor::set_draft`].

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Bounds, Context, Entity, EventEmitter, FontWeight, Subscription, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use gpui_component::{Root, switch::Switch};
use shot_core::{Brand, Id, Micros, Overlay, ParamValue, SlotType, TakeAt, TimeRange};
use shot_render::overlay::{self, KEYFRAME_PARAMS, ParamKind};
use shot_render::theme::Theme;

use super::fields::{self, Inputs, parse_seconds, seconds};
use super::stage::{Sample, Scene, Stage};
use crate::folder::VideoFolder;
use crate::theme::*;

/// An overlay as the editor shows it: a preset's name, or none for a placed overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub name: Option<Id>,
    pub template: Id,
    pub params: Vec<(Id, ParamValue)>,
    pub length: Micros,
}

impl Draft {
    /// The overlay this draft places at take time `from`.
    pub fn overlay(&self, from: Micros) -> Overlay {
        Overlay {
            template: self.template.clone(),
            range: TimeRange::new(from, Micros(from.0 + self.length.0.max(1)))
                .expect("a length is positive"),
            params: self.params.clone(),
        }
    }

    fn get(&self, name: &str) -> Option<&ParamValue> {
        self.params
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .map(|(_, v)| v)
    }

    /// With `name` set to `value`, or unset for `None`, keeping the order of the others.
    fn set(&mut self, name: &str, value: Option<ParamValue>) {
        let id = Id::new(name).expect("parameter names are ids");
        match (self.params.iter().position(|(n, _)| *n == id), value) {
            (Some(i), Some(value)) => self.params[i].1 = value,
            (Some(i), None) => {
                self.params.remove(i);
            }
            (None, Some(value)) => self.params.push((id, value)),
            (None, None) => {}
        }
    }
}

/// The kind of control a parameter gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Integer,
    Bool,
    Time,
    /// A Lottie color slot, `"#rrggbb"`.
    Color,
}

/// A parameter the editor offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub kind: Kind,
    pub required: bool,
    pub about: String,
}

/// What a template is about and its parameters, besides `enter` and `exit`: a built-in spec's,
/// or a brand Lottie template's slots. Whether it takes `enter` and `exit`.
pub fn fields(template: &str, brand: &Brand) -> (String, Vec<Field>, bool) {
    if let Some(spec) = overlay::spec(template) {
        let fields = spec
            .params
            .iter()
            .map(|p| Field {
                name: p.name.to_owned(),
                kind: match p.kind {
                    ParamKind::Text => Kind::Text,
                    ParamKind::Integer => Kind::Integer,
                    ParamKind::Bool => Kind::Bool,
                    ParamKind::Time => Kind::Time,
                },
                required: p.required,
                about: p.about.to_owned(),
            })
            .collect();
        return (spec.about.to_owned(), fields, spec.animated);
    }
    let Some(declared) = brand.templates.iter().find(|t| t.name.as_str() == template) else {
        return (
            "A template this brand does not declare".into(),
            Vec::new(),
            false,
        );
    };
    let fields = declared
        .slots
        .iter()
        .map(|slot| Field {
            name: slot.name.to_string(),
            kind: match slot.kind {
                SlotType::Text => Kind::Text,
                SlotType::Color => Kind::Color,
                SlotType::Integer => Kind::Integer,
                SlotType::Time => Kind::Time,
            },
            required: slot.required,
            about: match slot.name.as_str() {
                "accent" | "text" | "card" | "background" if slot.kind == SlotType::Color => {
                    format!("defaults to the brand's {} color", slot.name)
                }
                _ => format!("the Lottie's {} slot", slot.name),
            },
        })
        .collect();
    (
        format!("A Lottie template of the brand, {}", declared.file),
        fields,
        false,
    )
}

/// Every template an overlay or preset can use, in the inspector's order, then the brand's own
/// Lottie templates.
pub fn templates(brand: &Brand) -> Vec<Id> {
    let builtin = overlay::TEMPLATES
        .iter()
        .filter(|s| s.animated && s.name != overlay::highlight::TEMPLATE)
        .map(|s| Id::new(s.name).expect("template names are ids"));
    let lottie = brand
        .templates
        .iter()
        .filter(|t| overlay::spec(t.name.as_str()).is_none())
        .map(|t| t.name.clone());
    builtin.chain(lottie).collect()
}

/// A starting draft for `template`: example values for what it requires, so it draws at once.
pub fn starter(name: Option<Id>, template: &Id) -> Draft {
    let text = |s: &str| ParamValue::Text(s.to_owned());
    let params: Vec<(&str, ParamValue)> = match template.as_str() {
        overlay::link_card::TEMPLATE => {
            vec![("url", text("https://github.com/coderscantina/shot-studio"))]
        }
        overlay::lower_third::TEMPLATE => vec![
            ("text", text("Your name")),
            ("subtitle", text("What you do")),
        ],
        overlay::callout::TEMPLATE => vec![
            ("x", ParamValue::Integer(960)),
            ("y", ParamValue::Integer(540)),
            ("text", text("Look here")),
        ],
        overlay::pop::TEMPLATE => vec![("text", text("🔥"))],
        shot_render::code_diff::TEMPLATE => vec![("diff", text("sources/change.diff"))],
        _ => Vec::new(),
    };
    let length = match template.as_str() {
        overlay::pop::TEMPLATE => crate::place::POP_LENGTH,
        overlay::chapter_band::TEMPLATE => Micros(2_500_000),
        overlay::end_card::TEMPLATE => Micros(12_000_000),
        overlay::link_card::TEMPLATE => Micros(6_000_000),
        _ => Micros(5_000_000),
    };
    Draft {
        name,
        template: template.clone(),
        params: params
            .into_iter()
            .map(|(n, v)| (Id::new(n).expect("an id"), v))
            .collect(),
        length,
    }
}

/// A field of the editor.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Name,
    Length,
    Param(String),
}

/// An edit is done; the owner writes `0`.
pub struct Commit(pub Draft);

/// Where the preview takes its frame and theme from.
pub struct Preview {
    pub sample: Option<Sample>,
    pub brand: Brand,
    pub theme: Result<Arc<Theme>, String>,
    /// The chapter title a chapter band shows.
    pub chapter: String,
}

/// Time before the overlay starts in the preview, so its entrance shows.
const LEAD: Micros = Micros(600_000);
/// Time after it ends.
const TAIL: Micros = Micros(800_000);

pub struct OverlayEditor {
    pub draft: Draft,
    preview: Preview,
    pub stage: Entity<Stage>,
    inputs: Inputs<Key>,
    /// Fields whose text was refused, with why.
    refused: Vec<(Key, String)>,
    /// Shows the preset's name field.
    named: bool,
}

impl EventEmitter<Commit> for OverlayEditor {}

impl OverlayEditor {
    pub fn new(draft: Draft, preview: Preview, cx: &mut Context<Self>) -> Self {
        let stage = cx.new(|_| Stage::new());
        let mut editor = Self {
            named: draft.name.is_some(),
            draft,
            preview,
            stage,
            inputs: Inputs::default(),
            refused: Vec::new(),
        };
        editor.show(cx);
        editor
    }

    /// Shows `draft`, as written by the owner, with the preview's frame and theme.
    pub fn set_draft(&mut self, draft: Draft, preview: Option<Preview>, cx: &mut Context<Self>) {
        let other = draft.name != self.draft.name || draft.template != self.draft.template;
        if other {
            self.inputs.clear();
            self.refused.clear();
        }
        self.named = draft.name.is_some();
        let changed = draft != self.draft || preview.is_some();
        self.draft = draft;
        if let Some(preview) = preview {
            self.preview = preview;
        }
        if changed {
            self.show(cx);
        }
        cx.notify();
    }

    /// The overlay's place in the preview scene.
    fn span(&self) -> (Micros, Micros) {
        (LEAD, Micros(LEAD.0 + self.draft.length.0))
    }

    fn time(&self, name: &str) -> Option<u64> {
        match self.draft.get(name) {
            Some(ParamValue::Integer(n)) => u64::try_from(*n).ok(),
            _ => None,
        }
    }

    /// Redraws the preview from the draft, resting on the overlay fully in.
    fn show(&mut self, cx: &mut Context<Self>) {
        let (from, to) = self.span();
        let rest = Micros(from.0 + self.time("enter").unwrap_or(700_000) + 500_000)
            .min(Micros((from.0 + to.0) / 2 + 1));
        let scene = match &self.preview.sample {
            None => {
                Err("Record a take in this channel to preview overlays on a real frame".to_owned())
            }
            Some(sample) => {
                let overlay = self.draft.overlay(from);
                let project =
                    sample.project(&self.preview.chapter, Micros(to.0 + TAIL.0), vec![overlay]);
                Ok(Scene {
                    dir: sample.dir.clone(),
                    project,
                    theme: self.preview.theme.clone(),
                    captions: false,
                })
            }
        };
        self.stage
            .update(cx, |stage, cx| stage.set_scene(scene, rest, cx));
    }

    fn replay(&mut self, exit: bool, cx: &mut Context<Self>) {
        let (from, to) = self.span();
        let rest = self.stage.read(cx).time().unwrap_or(from);
        let (start, until) = if exit {
            let exit = self.time("exit").unwrap_or(600_000);
            (
                Micros(to.0.saturating_sub(exit + 700_000)),
                Micros(to.0 + TAIL.0),
            )
        } else {
            let enter = self.time("enter").unwrap_or(700_000);
            (Micros(0), Micros(from.0 + enter + 900_000))
        };
        self.stage.update(cx, |stage, cx| {
            stage.play(start, until.min(Micros(to.0 + TAIL.0)), rest, cx)
        });
    }

    /// Why the draft does not fit its template, from the same check export runs.
    fn problem(&self) -> Option<String> {
        let theme = self.preview.theme.as_ref().ok()?;
        let take = self
            .preview
            .sample
            .as_ref()
            .map_or_else(|| Id::new("t01").expect("an id"), |s| s.take.clone());
        theme
            .check(&self.draft.overlay(Micros(0)), &take)
            .err()
            .map(|e| e.to_string())
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        cx.emit(Commit(self.draft.clone()));
    }

    fn typed(
        this: &mut Self,
        key: &Key,
        text: &str,
        done: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        this.refused.retain(|(k, _)| k != key);
        let refuse = |this: &mut Self, why: &str| this.refused.push((key.clone(), why.to_owned()));
        match key {
            Key::Name => {
                if done {
                    match super::store::slug(text) {
                        Some(id) => this.draft.name = Some(id),
                        None => refuse(this, "A name needs a letter"),
                    }
                }
            }
            Key::Length => match parse_seconds(text).filter(|&t| t >= 100_000) {
                Some(t) => this.draft.length = Micros(t),
                None => refuse(this, "Seconds, at least 0.1"),
            },
            Key::Param(name) => {
                let kind = this.kind_of(name);
                let value = match (kind, text.trim()) {
                    (_, "") => Ok(None),
                    (Some(Kind::Text), _) => Ok(Some(ParamValue::Text(text.to_owned()))),
                    (Some(Kind::Color), t) => crate::brand::parse_hex(t)
                        .map(|_| Some(ParamValue::Text(t.to_lowercase())))
                        .ok_or("A color as #rrggbb"),
                    (Some(Kind::Integer), t) => t
                        .replace('_', "")
                        .parse::<i64>()
                        .map(|n| Some(ParamValue::Integer(n)))
                        .map_err(|_| "A whole number"),
                    (Some(Kind::Time), t) => parse_seconds(t)
                        .map(|t| Some(ParamValue::Integer(t as i64)))
                        .ok_or("Seconds, such as 0.4"),
                    (Some(Kind::Bool) | None, _) => Err("Not a parameter of this template"),
                };
                match value {
                    Ok(value) => this.draft.set(name, value),
                    Err(why) => refuse(this, why),
                }
            }
        }
        this.show(cx);
        if done && this.refused.is_empty() {
            this.commit(cx);
        }
        cx.notify();
    }

    fn kind_of(&self, name: &str) -> Option<Kind> {
        if KEYFRAME_PARAMS.iter().any(|p| p.name == name) {
            return Some(Kind::Time);
        }
        fields(self.draft.template.as_str(), &self.preview.brand)
            .1
            .into_iter()
            .find(|f| f.name == name)
            .map(|f| f.kind)
    }

    fn text_of(&self, name: &str, kind: Kind) -> String {
        match (self.draft.get(name), kind) {
            (Some(ParamValue::Text(t)), _) => t.clone(),
            (Some(ParamValue::Integer(n)), Kind::Time) => seconds(u64::try_from(*n).unwrap_or(0)),
            (Some(ParamValue::Integer(n)), _) => n.to_string(),
            _ => String::new(),
        }
    }

    fn refused(&self, key: &Key) -> Option<String> {
        self.refused
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, why)| why.clone())
    }

    /// A row for parameter `field`.
    fn param_row(
        &mut self,
        field: &Field,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let name = field.name.clone();
        let label = format!(
            "{}{}",
            name.replace('-', " "),
            if field.required { " *" } else { "" }
        );
        let control = match field.kind {
            Kind::Bool => {
                let on = matches!(self.draft.get(&name), Some(ParamValue::Bool(true)));
                let n = name.clone();
                div().child(
                    Switch::new(gpui::ElementId::Name(format!("param-{name}").into()))
                        .checked(on)
                        .accessibility_label(label.clone())
                        .on_click(cx.listener(move |this, on: &bool, _, cx| {
                            this.draft.set(&n, Some(ParamValue::Bool(*on)));
                            this.show(cx);
                            this.commit(cx);
                            cx.notify();
                        })),
                )
            }
            kind => {
                let key = Key::Param(name.clone());
                let value = self.text_of(&name, kind);
                let placeholder = match kind {
                    Kind::Time => "seconds",
                    Kind::Color => "#rrggbb",
                    Kind::Integer => "a number",
                    _ => "",
                };
                let refused = self.refused(&key);
                let state = self
                    .inputs
                    .get(key, &value, placeholder, window, cx, Self::typed);
                let swatch = (kind == Kind::Color)
                    .then(|| crate::brand::parse_hex(&value))
                    .flatten()
                    .map(|c| crate::brand::swatch(c, 20.));
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .children(swatch)
                    .child(fields::input(&state, refused.is_some()).flex_1())
            }
        };
        let note = self
            .refused(&Key::Param(name.clone()))
            .map(|why| div().text_color(CUT_TEXT).child(why))
            .unwrap_or_else(|| div().text_color(MUTED).child(field.about.clone()));
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(fields::row(label, control))
            .child(div().pl(px(108.)).text_size(TEXT_XS).child(note))
    }

    /// The controls without the preview, for the theme window's Overlays section.
    pub fn controls(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let (about, params, animated) = fields(self.draft.template.as_str(), &self.preview.brand);
        let mut out = div().flex().flex_col().gap(px(8.));
        out = out.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(TEXT_BASE)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.draft.template.to_string()),
                )
                .child(div().text_size(TEXT_SM).text_color(MUTED).child(about)),
        );
        if self.named {
            let name = self
                .draft
                .name
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            let refused = self.refused(&Key::Name);
            let state = self
                .inputs
                .get(Key::Name, &name, "speaker", window, cx, Self::typed);
            out = out.child(fields::row(
                "Name",
                fields::input(&state, refused.is_some()),
            ));
            if let Some(why) = refused {
                out = out.child(
                    div()
                        .pl(px(108.))
                        .text_size(TEXT_XS)
                        .text_color(CUT_TEXT)
                        .child(why),
                );
            }
        }
        out = out.child(fields::group("Timing"));
        let length = seconds(self.draft.length.0);
        let refused = self.refused(&Key::Length);
        let state = self
            .inputs
            .get(Key::Length, &length, "seconds", window, cx, Self::typed);
        out = out.child(fields::row(
            "Length",
            fields::input(&state, refused.is_some()),
        ));
        if let Some(why) = refused {
            out = out.child(
                div()
                    .pl(px(108.))
                    .text_size(TEXT_XS)
                    .text_color(CUT_TEXT)
                    .child(why),
            );
        }
        if animated {
            for spec in KEYFRAME_PARAMS {
                let field = Field {
                    name: spec.name.to_owned(),
                    kind: Kind::Time,
                    required: false,
                    about: format!("{}, seconds; empty fits the motion preset", spec.about),
                };
                out = out.child(self.param_row(&field, window, cx));
            }
        }
        if !params.is_empty() {
            out = out.child(fields::group("Parameters"));
        }
        for field in &params {
            out = out.child(self.param_row(field, window, cx));
        }
        if let Some(problem) = self.problem() {
            out = out.child(crate::ui::alert(problem));
        }
        out
    }

    /// The preview with its replay buttons.
    pub fn preview(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(self.stage.clone())
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        fields::button("replay-enter", "Play enter")
                            .on_click(cx.listener(|this, _, _, cx| this.replay(false, cx))),
                    )
                    .child(
                        fields::button("replay-exit", "Play exit")
                            .on_click(cx.listener(|this, _, _, cx| this.replay(true, cx))),
                    ),
            )
    }
}

impl Render for OverlayEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .gap(px(20.))
            .child(
                div()
                    .id("overlay-controls")
                    .w(px(380.))
                    .flex_none()
                    .overflow_y_scroll()
                    .child(self.controls(window, cx)),
            )
            .child(div().flex_1().min_w_0().child(self.preview(cx)))
    }
}

/// The window that edits an overlay placed in a video: the overlay `index` of the block `at`.
struct PlacedWindow {
    folder: Entity<VideoFolder>,
    at: TakeAt,
    index: usize,
    /// The overlay's start, which the editor does not change.
    from: Micros,
    editor: Entity<OverlayEditor>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

/// The placed overlay and the chapter title it shows under, if it is there.
fn placed(folder: &VideoFolder, at: TakeAt, index: usize) -> Option<(Overlay, Id, String)> {
    let chapter = folder.project.cut.chapters.get(at.chapter)?;
    let block = chapter.takes.get(at.take)?;
    Some((
        block.overlays.get(index)?.clone(),
        block.take.clone(),
        chapter.title.clone(),
    ))
}

fn draft_of(overlay: &Overlay) -> Draft {
    Draft {
        name: None,
        template: overlay.template.clone(),
        params: overlay.params.clone(),
        length: overlay.range.duration(),
    }
}

fn preview_of(folder: &VideoFolder, take: Id, overlay: &Overlay, chapter: String) -> Preview {
    let brand = folder.lock().map(|l| l.brand.clone()).unwrap_or_default();
    Preview {
        sample: Sample::of_video(
            &folder.dir,
            take,
            Micros(overlay.range.from().0.saturating_sub(LEAD.0)),
        ),
        brand,
        theme: folder.theme.clone(),
        chapter,
    }
}

/// Opens an editor on the overlay `index` of the block `at` of the video in `folder`. Edits
/// write `cut.kdl` and undo in the studio.
pub fn open_placed(
    folder: Entity<VideoFolder>,
    at: TakeAt,
    index: usize,
    cx: &mut App,
) -> Result<(), String> {
    let (overlay, take, chapter) =
        placed(folder.read(cx), at, index).ok_or("That overlay is gone")?;
    let preview = preview_of(folder.read(cx), take, &overlay, chapter);
    let bounds = Bounds::centered(None, size(px(1240.), px(760.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(980.), px(600.))),
            titlebar: Some(TitlebarOptions {
                title: Some(format!("Overlay: {}", overlay.template).into()),
                ..Default::default()
            }),
            inactive_frame_interval: None,
            ..Default::default()
        },
        |window, cx| {
            let editor = cx.new(|cx| OverlayEditor::new(draft_of(&overlay), preview, cx));
            let view = cx.new(|cx| {
                let commits =
                    cx.subscribe(&editor, |this: &mut PlacedWindow, _, Commit(draft), cx| {
                        this.write(draft.clone(), cx);
                    });
                let follows = cx.observe(&folder, |this: &mut PlacedWindow, _, cx| this.follow(cx));
                PlacedWindow {
                    folder: folder.clone(),
                    at,
                    index,
                    from: overlay.range.from(),
                    editor,
                    error: None,
                    _subscriptions: vec![commits, follows],
                }
            });
            cx.new(|cx| Root::new(view, window, cx))
        },
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

impl PlacedWindow {
    fn write(&mut self, draft: Draft, cx: &mut Context<Self>) {
        let (at, index, from) = (self.at, self.index, self.from);
        let result = self.folder.update(cx, |folder, cx| {
            let mut cut = folder.project.cut.clone();
            let slot = cut
                .chapters
                .get_mut(at.chapter)
                .and_then(|c| c.takes.get_mut(at.take))
                .and_then(|b| b.overlays.get_mut(index))
                .ok_or("That overlay is gone")?;
            *slot = draft.overlay(from);
            folder.write_cut(cut)?;
            cx.notify();
            Ok::<_, String>(())
        });
        self.error = result.err();
        cx.notify();
    }

    /// Shows the overlay as the files have it now, after an undo in the studio.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let Some((overlay, take, chapter)) = placed(self.folder.read(cx), self.at, self.index)
        else {
            self.error = Some("This overlay is no longer in the cut".into());
            cx.notify();
            return;
        };
        let preview = preview_of(self.folder.read(cx), take, &overlay, chapter);
        self.from = overlay.range.from();
        self.editor.update(cx, |editor, cx| {
            editor.set_draft(draft_of(&overlay), Some(preview), cx)
        });
    }

    fn step_history(&mut self, undo: bool, cx: &mut Context<Self>) {
        let result = self.folder.update(cx, |folder, cx| {
            let stepped = if undo { folder.undo() } else { folder.redo() };
            cx.notify();
            stepped
        });
        if let Err(error) = result {
            self.error = Some(error);
        }
    }
}

impl Render for PlacedWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(20.))
            .bg(SURFACE)
            .text_color(TEXT)
            .font_family(SANS)
            .on_action(
                cx.listener(|this, _: &crate::studio::Undo, _, cx| this.step_history(true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &crate::studio::Redo, _, cx| this.step_history(false, cx)),
            )
            .children(self.error.clone().map(crate::ui::alert))
            .child(div().flex_1().min_h_0().child(self.editor.clone()))
    }
}

/// A channel's overlay presets are edited in its theme window.
pub fn repo_of(dir: &std::path::Path) -> Option<PathBuf> {
    shot_render::theme::channel_root(dir)
        .filter(|root| root.join("brand").is_dir() || root.join("videos").is_dir())
        .map(ToOwned::to_owned)
}
