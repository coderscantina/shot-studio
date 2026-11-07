//! Overlay templates: the typed meaning of `overlay <template> from=... to=... { ... }` in
//! `cut.kdl`.
//!
//! Each template is a [`TemplateSpec`]: its name, its parameters with their kinds and whether
//! they are required, and the keyframes it animates by. The spec is the template's contract with
//! `cut.kdl` and is independent of how the renderer draws it: a renderer change bumps
//! [`shot_core::RENDERER_VERSION`] and leaves every spec and every file as it is, and a spec only
//! grows by optional parameters, so an older `cut.kdl` always reads the same.
//!
//! Every template that draws takes two keyframe parameters besides its own: `enter` and `exit`,
//! the time in microseconds it takes to come in after `from` and to go out before `to`
//! ([`Keyframes::presence`]). The templates:
//!
//! - `code-diff` ([`crate::code_diff`]): a diff snapshot typed out, one line highlighted.
//! - `link-card` ([`link_card`]): a card for a URL said in the transcript.
//! - `lower-third` ([`lower_third`]): a name or line at the bottom left.
//! - `highlight` ([`highlight`]): a box on the screen recording around text you react to.
//! - `b-roll` ([`broll`]): a cutaway to a still, a clip or a diff over idle screen or a B marker.
//! - `blur` and `bleep` ([`redact`]): a blurred rectangle of the screen, a tone over words.

pub mod broll;
pub mod highlight;
pub mod link_card;
pub mod lower_third;
pub mod redact;

use shot_core::{Id, Micros, Overlay, ParamValue, TimeRange};

use crate::code_diff;
use crate::drawing::Drawing;
use crate::frame::{Look, ScreenView};
use crate::layout::{Canvas, Layer, Scene};

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
    /// Whether `enter` and `exit` apply. Bleep has no picture to fade.
    pub animated: bool,
}

/// The keyframe parameters of every animated template.
pub const KEYFRAME_PARAMS: [ParamSpec; 2] = [
    ParamSpec::optional("enter", ParamKind::Time, "how long it takes to come in"),
    ParamSpec::optional("exit", ParamKind::Time, "how long it takes to go out"),
];

/// Every template, in the order the inspector offers them.
pub const TEMPLATES: [&TemplateSpec; 7] = [
    &link_card::SPEC,
    &lower_third::SPEC,
    &highlight::SPEC,
    &broll::SPEC,
    &code_diff::SPEC,
    &redact::BLUR,
    &redact::BLEEP,
];

/// The spec of the template `name`.
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
}

/// Checks `overlay` against its template's spec: a known template, known parameters of the
/// right kind, and every required one.
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
    Ok(spec)
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

    /// The overlay's presence over its range, from its `enter` and `exit`.
    pub fn presence(self, default: Micros) -> Keyframes {
        Keyframes::presence(
            self.0.range,
            self.time("enter").unwrap_or(default),
            self.time("exit").unwrap_or(default),
        )
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

/// Extends the key of every chunk that shows an overlay drawing from a file with `identity` of
/// that file, so a file replaced under the same name renders again. `cut.kdl` only names the
/// file, and the contract's hash covers what it names.
pub fn key_assets(plan: &mut crate::plan::ExportPlan, identity: &dyn Fn(&str) -> Vec<u8>) {
    let fps = plan.fps;
    for planned in &mut plan.segments {
        for chunk in &mut planned.chunks {
            let Some(span) = TimeRange::new(
                crate::plan::frame_time(chunk.frames.start, fps),
                crate::plan::frame_time(chunk.frames.end, fps),
            ) else {
                continue;
            };
            let mut input = Vec::new();
            for file in planned
                .segment
                .overlays
                .iter()
                .filter(|o| o.range.overlaps(span))
                .filter_map(file)
            {
                let id = identity(file);
                input.extend_from_slice(&(id.len() as u64).to_le_bytes());
                input.extend_from_slice(&id);
            }
            if !input.is_empty() {
                chunk.hash = chunk.hash.extend(&input);
            }
        }
    }
}

/// A value over media time: keyframes eased into each other, held before the first and after
/// the last.
#[derive(Debug, Clone, PartialEq)]
pub struct Keyframes(Vec<(Micros, f32)>);

impl Keyframes {
    /// Keyframes in time order.
    pub fn new(mut frames: Vec<(Micros, f32)>) -> Self {
        frames.sort_by_key(|(t, _)| *t);
        Self(frames)
    }

    /// 0 at the range's start rising to 1 over `enter`, 1 until `exit` before its end, then
    /// back to 0. A range too short for both meets in the middle.
    pub fn presence(range: TimeRange, enter: Micros, exit: Micros) -> Self {
        let (from, to) = (range.from().0, range.to().0);
        let middle = from + (to - from) / 2;
        let rise = (from + enter.0).min(middle);
        let fall = to.saturating_sub(exit.0).max(middle);
        Self::new(vec![
            (Micros(from), 0.0),
            (Micros(rise), 1.0),
            (Micros(fall), 1.0),
            (Micros(to), 0.0),
        ])
    }

    /// The value at `t`, eased with smoothstep between keyframes.
    pub fn at(&self, t: Micros) -> f32 {
        let Some(&(first_t, first)) = self.0.first() else {
            return 0.0;
        };
        if t <= first_t {
            return first;
        }
        for pair in self.0.windows(2) {
            let ((a_t, a), (b_t, b)) = (pair[0], pair[1]);
            if t < b_t {
                let p = (t.0 - a_t.0) as f32 / (b_t.0 - a_t.0).max(1) as f32;
                let eased = p * p * (3.0 - 2.0 * p);
                return a + (b - a) * eased;
            }
        }
        self.0.last().map_or(0.0, |(_, v)| *v)
    }
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

/// Applies `overlay` to the frame at `stage.t`. Overlays draw in file order, later on top.
pub(crate) fn apply(overlay: &Overlay, stage: &mut Stage) {
    if stage.t < overlay.range.from() || stage.t >= overlay.range.to() {
        return;
    }
    match overlay.template.as_str() {
        code_diff::TEMPLATE => {
            if let Some(code) = code_diff::CodeDiffOverlay::of(overlay)
                && let Some(diff) = stage.look.diffs.get(&code.diff)
            {
                stage
                    .drawing
                    .over
                    .extend(code.draw(diff, stage.t, stage.canvas));
            }
        }
        link_card::TEMPLATE => {
            if let Some(card) = link_card::LinkCard::of(overlay) {
                stage
                    .drawing
                    .over
                    .extend(card.draw(Params(overlay), stage.t, stage.canvas));
            }
        }
        lower_third::TEMPLATE => {
            stage
                .drawing
                .over
                .extend(lower_third::draw(Params(overlay), stage.t, stage.canvas));
        }
        highlight::TEMPLATE => highlight::apply(overlay, stage),
        broll::TEMPLATE => broll::apply(overlay, stage),
        redact::BLUR_TEMPLATE => redact::apply_blur(overlay, stage),
        _ => {}
    }
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
    }

    #[test]
    fn presence_rises_holds_and_falls() {
        let k = Keyframes::presence(
            range(1_000_000, 3_000_000),
            Micros(500_000),
            Micros(250_000),
        );
        assert_eq!(k.at(Micros(1_000_000)), 0.0);
        assert_eq!(k.at(Micros(1_250_000)), 0.5);
        assert_eq!(k.at(Micros(2_000_000)), 1.0);
        assert_eq!(k.at(Micros(2_875_000)), 0.5);
        assert_eq!(k.at(Micros(3_000_000)), 0.0);
        let short = Keyframes::presence(range(0, 400_000), Micros(500_000), Micros(500_000));
        assert_eq!(short.at(Micros(200_000)), 1.0);
    }
}
