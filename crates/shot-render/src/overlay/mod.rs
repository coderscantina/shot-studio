//! Overlay templates: the typed meaning of `overlay <template> from=... to=... { ... }` in
//! `cut.kdl` and `cut.lock`.
//!
//! Each template is a [`TemplateSpec`]: its name, its parameters with their kinds and whether
//! they are required. The spec is the template's contract with the files and is independent of
//! how the renderer draws it: a renderer change bumps [`shot_core::RENDERER_VERSION`] and leaves
//! every spec and every file as it is, and a spec only grows by optional parameters, so an older
//! `cut.kdl` always reads the same.
//!
//! Every template that draws moves on the motion system ([`crate::motion`]) in the video's
//! preset with the brand's feel, colors and fonts ([`crate::theme`]), and takes two duration
//! hints besides its own parameters: `enter` and `exit`, the microseconds its springs take to
//! come in after `from` and to settle out before `to`. The templates:
//!
//! - `link-card` ([`link_card`]): a card for a URL said in the transcript.
//! - `lower-third` ([`lower_third`]): a name or line at the bottom left.
//! - `highlight` ([`highlight`]): a box on the screen recording around text you react to.
//! - `b-roll` ([`broll`]): a cutaway to a still, a clip or a diff over idle screen or a B marker.
//! - `code-diff` ([`crate::code_diff`]): a diff snapshot typed out, one line highlighted.
//! - `chapter-band` ([`chapter_band`]): the chapter's title on a band as the chapter starts.
//! - `end-card` ([`end_card`]): the end screen with YouTube's element areas kept free.
//! - `callout` ([`callout`]): a label pointing at a click or a box on the screen.
//! - `pop` ([`pop`]): an emoji, a word or a channel sticker popping in.
//! - `blur` and `bleep` ([`redact`]): a blurred rectangle of the screen, a tone over words. They
//!   switch hard: a fade would reveal what they hide.
//!
//! A brand re-skins `link-card`, `lower-third`, `chapter-band` and `end-card` by declaring a
//! Lottie template of the same name ([`reskin_slots`] lists what each fills), and declares new
//! templates the same way; [`crate::theme::Theme::check`] checks overlays against both.

pub mod broll;
pub mod callout;
mod card;
pub mod chapter_band;
pub mod end_card;
pub mod highlight;
pub mod link_card;
pub mod lower_third;
pub mod pop;
pub mod redact;

use shot_core::{Id, Micros, Overlay, ParamValue, TimeRange};

use crate::code_diff;
use crate::drawing::{Drawing, Shape};
use crate::frame::{Look, ScreenView};
use crate::layout::{Canvas, Layer, Scene};
use crate::lottie::{SlotKind, SlotValue};
use crate::motion::Motion;
use crate::theme::Theme;

/// The kind of value a parameter holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    Text,
    Integer,
    Bool,
    /// Microseconds, an integer.
    Time,
}

impl ParamKind {
    fn accepts(self, value: &ParamValue) -> bool {
        matches!(
            (self, value),
            (Self::Text, ParamValue::Text(_))
                | (Self::Integer | Self::Time, ParamValue::Integer(_))
                | (Self::Bool, ParamValue::Bool(_))
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Integer => "an integer",
            Self::Bool => "a boolean",
            Self::Time => "a time in microseconds",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub kind: ParamKind,
    pub required: bool,
    /// One line for the inspector.
    pub about: &'static str,
}

impl ParamSpec {
    pub const fn required(name: &'static str, kind: ParamKind, about: &'static str) -> Self {
        Self {
            name,
            kind,
            required: true,
            about,
        }
    }

    pub const fn optional(name: &'static str, kind: ParamKind, about: &'static str) -> Self {
        Self {
            name,
            kind,
            required: false,
            about,
        }
    }
}

/// A template's name and parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateSpec {
    pub name: &'static str,
    /// One line for the inspector.
    pub about: &'static str,
    pub params: &'static [ParamSpec],
    /// Whether it moves on springs and takes `enter` and `exit`. Blur and bleep switch hard.
    pub animated: bool,
}

/// The duration hints of every animated template.
pub const KEYFRAME_PARAMS: [ParamSpec; 2] = [
    ParamSpec::optional("enter", ParamKind::Time, "how long it takes to come in"),
    ParamSpec::optional("exit", ParamKind::Time, "how long it takes to go out"),
];

/// Every template, in the order the inspector offers them.
pub const TEMPLATES: [&TemplateSpec; 11] = [
    &link_card::SPEC,
    &lower_third::SPEC,
    &highlight::SPEC,
    &broll::SPEC,
    &code_diff::SPEC,
    &chapter_band::SPEC,
    &end_card::SPEC,
    &callout::SPEC,
    &pop::SPEC,
    &redact::BLUR,
    &redact::BLEEP,
];

/// The spec of the built-in template `name`.
pub fn spec(name: &str) -> Option<&'static TemplateSpec> {
    TEMPLATES.into_iter().find(|spec| spec.name == name)
}

/// Why an overlay does not fit its template.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    #[error("there is no overlay template `{0}`")]
    Unknown(String),
    #[error("`{template}` has no parameter `{param}`")]
    UnknownParam { template: String, param: String },
    #[error("`{template}` needs `{param}`")]
    Missing { template: String, param: String },
    #[error("`{template}` takes {kind} for `{param}`")]
    Kind {
        template: String,
        param: String,
        kind: &'static str,
    },
    #[error("brand template `{template}`: {error}")]
    Lottie { template: String, error: String },
}

/// Checks `overlay` against its built-in template's spec: a known template, known parameters of
/// the right kind, and every required one. Brand templates are checked by
/// [`Theme::check`](crate::theme::Theme::check).
pub fn check(overlay: &Overlay) -> Result<&'static TemplateSpec, TemplateError> {
    let name = overlay.template.as_str();
    let spec = spec(name).ok_or_else(|| TemplateError::Unknown(name.to_owned()))?;
    let keyframes: &[ParamSpec] = if spec.animated { &KEYFRAME_PARAMS } else { &[] };
    for (param, value) in &overlay.params {
        let known = spec
            .params
            .iter()
            .chain(keyframes)
            .find(|p| p.name == param.as_str())
            .ok_or_else(|| TemplateError::UnknownParam {
                template: name.to_owned(),
                param: param.to_string(),
            })?;
        if !known.kind.accepts(value) {
            return Err(TemplateError::Kind {
                template: name.to_owned(),
                param: param.to_string(),
                kind: known.kind.as_str(),
            });
        }
    }
    if let Some(missing) = spec
        .params
        .iter()
        .find(|p| p.required && Params(overlay).get(p.name).is_none())
    {
        return Err(TemplateError::Missing {
            template: name.to_owned(),
            param: missing.name.to_owned(),
        });
    }
    if name == pop::TEMPLATE {
        pop::check(overlay)?;
    }
    Ok(spec)
}

/// The slots a brand Lottie that re-skins template `name` may declare, `None` for templates
/// drawn by code alone.
pub fn reskin_slots(name: &str) -> Option<&'static [(&'static str, SlotKind)]> {
    const ACCENT: (&str, SlotKind) = ("accent", SlotKind::Color);
    Some(match name {
        link_card::TEMPLATE => &[
            ("label", SlotKind::Text),
            ("title", SlotKind::Text),
            ("url", SlotKind::Text),
            ACCENT,
        ],
        lower_third::TEMPLATE | end_card::TEMPLATE => &[
            ("text", SlotKind::Text),
            ("subtitle", SlotKind::Text),
            ACCENT,
        ],
        chapter_band::TEMPLATE => &[("text", SlotKind::Text), ("label", SlotKind::Text), ACCENT],
        _ => return None,
    })
}

/// The values a re-skinned overlay gives its Lottie's slots, by [`reskin_slots`] name.
pub(crate) fn reskin_values(
    overlay: &Overlay,
    theme: &Theme,
    take: &Id,
) -> Vec<(&'static str, SlotValue)> {
    let p = Params(overlay);
    let text = |s: &str| SlotValue::Text(s.to_owned());
    let mut values = vec![("accent", SlotValue::Color(theme.palette.accent))];
    match overlay.template.as_str() {
        link_card::TEMPLATE => {
            if let Some(card) = link_card::LinkCard::of(overlay) {
                let (label, title) = card.text();
                values.push(("label", text(&label)));
                values.push(("title", text(&title)));
                values.push(("url", text(card.shown_url())));
            }
        }
        lower_third::TEMPLATE | end_card::TEMPLATE => {
            let line = match overlay.template.as_str() {
                end_card::TEMPLATE => p.text("text").unwrap_or(end_card::HEADLINE),
                _ => p.text("text").unwrap_or_default(),
            };
            values.push(("text", text(line)));
            values.extend(p.text("subtitle").map(|s| ("subtitle", text(s))));
        }
        chapter_band::TEMPLATE => {
            let title = chapter_band::title(overlay, theme, Some(take)).unwrap_or_default();
            values.push(("text", text(title)));
            values.extend(p.text("label").map(|s| ("label", text(s))));
        }
        _ => {}
    }
    values
}

/// Whether the overlay draws anything of the brand: its colors, fonts or motion. `blur` and
/// `bleep` do not, and neither do a b-roll clip, which cuts in hard, or an empty b-roll slot.
pub fn branded(overlay: &Overlay) -> bool {
    match overlay.template.as_str() {
        redact::BLUR_TEMPLATE | redact::BLEEP_TEMPLATE => false,
        broll::TEMPLATE => {
            broll::file(overlay).is_some_and(|(_, media)| media != broll::Media::Clip)
        }
        _ => true,
    }
}

/// Typed access to an overlay's parameters. A value of the wrong kind reads as missing; [`check`]
/// reports it.
#[derive(Debug, Clone, Copy)]
pub struct Params<'a>(pub &'a Overlay);

impl<'a> Params<'a> {
    pub fn get(self, name: &str) -> Option<&'a ParamValue> {
        self.0
            .params
            .iter()
            .find(|(n, _)| n.as_str() == name)
            .map(|(_, v)| v)
    }

    pub fn text(self, name: &str) -> Option<&'a str> {
        match self.get(name)? {
            ParamValue::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn int(self, name: &str) -> Option<i64> {
        match self.get(name)? {
            ParamValue::Integer(n) => Some(*n),
            _ => None,
        }
    }

    pub fn bool(self, name: &str) -> Option<bool> {
        match self.get(name)? {
            ParamValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn time(self, name: &str) -> Option<Micros> {
        self.int(name)
            .and_then(|n| u64::try_from(n).ok())
            .map(Micros)
    }

    /// The theme's springs fitted to the overlay's `enter` and `exit` hints.
    pub fn motion(self, theme: &Theme) -> Motion {
        theme
            .motion
            .with_hints(self.time("enter"), self.time("exit"))
    }
}

/// Builds an overlay from a template name and parameters, in the given order.
pub fn overlay(template: &str, range: TimeRange, params: Vec<(&str, ParamValue)>) -> Overlay {
    let id = |name: &str| Id::new(name).expect("template and parameter names are ids");
    Overlay {
        template: id(template),
        range,
        params: params.into_iter().map(|(n, v)| (id(n), v)).collect(),
    }
}

/// The file an overlay draws from, by its path in the video folder: a code-diff snapshot or a
/// b-roll file.
pub fn file(overlay: &Overlay) -> Option<&str> {
    match overlay.template.as_str() {
        code_diff::TEMPLATE => Params(overlay).text("diff"),
        broll::TEMPLATE => broll::file(overlay).map(|(file, _)| file),
        _ => None,
    }
}

/// Extends the key of every chunk with what it draws beyond its segment's own values:
/// `identity` of each file an overlay names, so a file replaced under the same name renders
/// again, and where a branded template draws, the theme's preset, colors and fonts, a brand
/// Lottie's file hash and a chapter band's title. Where the brand frames the camera beside the
/// screen, the camera's frame. A chunk where nothing branded draws keeps its key when the brand
/// changes.
pub fn key_chunks(
    plan: &mut crate::plan::ExportPlan,
    identity: &dyn Fn(&str) -> Vec<u8>,
    theme: &Theme,
) {
    let fps = plan.fps;
    for planned in &mut plan.segments {
        for chunk in &mut planned.chunks {
            let Some(span) = TimeRange::new(
                crate::plan::frame_time(chunk.frames.start, fps),
                crate::plan::frame_time(chunk.frames.end, fps),
            ) else {
                continue;
            };
            let input = span_input(&planned.segment, span, identity, theme);
            if !input.is_empty() {
                chunk.hash = chunk.hash.extend(&input);
            }
        }
    }
}

/// What [`key_chunks`] adds to the key of `segment`'s frames over `span`, empty for nothing.
pub(crate) fn span_input(
    segment: &shot_core::Segment,
    span: TimeRange,
    identity: &dyn Fn(&str) -> Vec<u8>,
    theme: &Theme,
) -> Vec<u8> {
    let mut input = Vec::new();
    let mut branded = false;
    for overlay in segment.overlays.iter().filter(|o| o.range.overlaps(span)) {
        if let Some(file) = file(overlay) {
            let id = identity(file);
            input.extend_from_slice(&(id.len() as u64).to_le_bytes());
            input.extend_from_slice(&id);
        }
        if let Some(key) = crate::theme::overlay_key(theme, overlay, &segment.take) {
            branded = true;
            input.extend_from_slice(&(key.len() as u64).to_le_bytes());
            input.extend_from_slice(&key);
        }
    }
    if branded {
        input.extend_from_slice(theme.key());
    }
    if let Some(camera) = theme.camera_key(segment.layout) {
        input.extend_from_slice(b"camera");
        input.extend_from_slice(&camera);
    }
    input
}

/// A clip a b-roll overlay cuts away to: the screen layer shows `file` at `time` instead of the
/// take's screen. Export and preview decode it like a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cutaway {
    /// Relative to the video folder.
    pub file: String,
    /// Media time of the clip.
    pub time: Micros,
}

/// What one overlay can change of a frame, besides drawing on top.
pub(crate) struct Stage<'a> {
    pub t: Micros,
    pub canvas: Canvas,
    pub look: &'a Look,
    /// The screen's layer and how its points map to the canvas, when the layout shows it.
    pub screen: &'a mut Option<(Layer, ScreenView)>,
    pub scene: &'a mut Scene,
    pub drawing: &'a mut Drawing,
    pub cutaway: &'a mut Option<Cutaway>,
}

impl<'a> Stage<'a> {
    pub fn theme(&self) -> &'a Theme {
        &self.look.theme
    }

    /// Canvas pixels per 4K pixel.
    pub fn scale(&self) -> f32 {
        self.canvas.width as f32 / 3840.0
    }
}

/// Applies `overlay` to the frame at `stage.t`. Overlays draw in file order, later on top.
pub(crate) fn apply(overlay: &Overlay, stage: &mut Stage) {
    if stage.t < overlay.range.from() || stage.t >= overlay.range.to() {
        return;
    }
    if let Some(take) = &stage.look.take
        && let Ok(Some(lottie)) = stage.theme().lottie(overlay, take)
    {
        draw_full_frame(lottie, overlay, stage);
        return;
    }
    match overlay.template.as_str() {
        code_diff::TEMPLATE => code_diff::apply(overlay, stage),
        link_card::TEMPLATE => link_card::apply(overlay, stage),
        lower_third::TEMPLATE => lower_third::apply(overlay, stage),
        highlight::TEMPLATE => highlight::apply(overlay, stage),
        broll::TEMPLATE => broll::apply(overlay, stage),
        chapter_band::TEMPLATE => chapter_band::apply(overlay, stage),
        end_card::TEMPLATE => end_card::apply(overlay, stage),
        callout::TEMPLATE => callout::apply(overlay, stage),
        pop::TEMPLATE => pop::apply(overlay, stage),
        redact::BLUR_TEMPLATE => redact::apply_blur(overlay, stage),
        _ => {}
    }
}

/// Draws `overlay` at `t` over `frame` on its own, for an overlay the plan places rather than a
/// segment: the end card over the last seconds.
pub(crate) fn draw_alone(
    overlay: &Overlay,
    t: Micros,
    canvas: Canvas,
    look: &Look,
    frame: &mut crate::frame::Frame,
) {
    apply(
        overlay,
        &mut Stage {
            t,
            canvas,
            look,
            screen: &mut None,
            scene: &mut frame.scene,
            drawing: &mut frame.drawing,
            cutaway: &mut frame.cutaway,
        },
    );
}

/// A brand Lottie template drawn over the whole frame: its composition scaled to the canvas
/// width and centered, playing from the overlay's start and holding its last frame.
fn draw_full_frame(
    lottie: std::sync::Arc<crate::lottie::Filled>,
    overlay: &Overlay,
    stage: &mut Stage,
) {
    let (width, height) = lottie.size();
    let scale = stage.canvas.width as f32 / width as f32;
    let top = (stage.canvas.height as f32 - height as f32 * scale) / 2.0;
    stage.drawing.over.push(Shape::Lottie {
        lottie,
        origin: [0.0, top],
        scale,
        elapsed: Micros(stage.t.0 - overlay.range.from().0),
        opacity: 1.0,
        font: stage.theme().fonts.body(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    #[test]
    fn check_follows_the_spec() {
        let card = |params| overlay("link-card", range(0, 1_000_000), params);
        let url = ("url", ParamValue::Text("https://x.com/a/status/1".into()));
        assert!(check(&card(vec![url.clone()])).is_ok());
        assert!(
            check(&card(vec![
                url.clone(),
                ("enter", ParamValue::Integer(100_000))
            ]))
            .is_ok()
        );
        assert_eq!(
            check(&card(vec![])).unwrap_err().to_string(),
            "`link-card` needs `url`"
        );
        assert!(matches!(
            check(&card(vec![url.clone(), ("size", ParamValue::Integer(2))])),
            Err(TemplateError::UnknownParam { .. })
        ));
        assert!(matches!(
            check(&card(vec![url, ("accent", ParamValue::Integer(1))])),
            Err(TemplateError::Kind { .. })
        ));
        assert!(matches!(
            check(&overlay("sparkles", range(0, 1), vec![])),
            Err(TemplateError::Unknown(_))
        ));
        let blur = overlay(
            "blur",
            range(0, 1),
            vec![
                ("x", ParamValue::Integer(0)),
                ("y", ParamValue::Integer(0)),
                ("width", ParamValue::Integer(10)),
                ("height", ParamValue::Integer(10)),
                ("enter", ParamValue::Integer(100_000)),
            ],
        );
        assert!(matches!(
            check(&blur),
            Err(TemplateError::UnknownParam { .. })
        ));
    }
}
