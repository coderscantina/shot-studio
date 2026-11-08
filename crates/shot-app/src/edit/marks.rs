//! The effects the Text workspace marks on the transcript: zooms, overlays and sounds of a take
//! block, a person's and the style's, each with the words it covers, and where the layout
//! switches. Read from the shown cut and the lock; the transcript draws them in the lanes' hues.

use gpui::{App, Hsla, rgb};
use shot_core::{Applied, Id, Layer, Layout, Micros, OverrideEffect, Placed, TakeAt, TimeRange};
use shot_render::overlay::{self, broll, highlight, link_card};

use super::EditScreen;
use crate::model::LayoutRun;
use crate::theme::*;

/// The Overlays lane's violet from design.md.
pub const OVERLAY: u32 = 0xB69CFF;

/// One effect by its block, kind, start and place among its kind, stable across renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkId {
    pub at: TakeAt,
    pub kind: KindTag,
    pub from: Micros,
    pub index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KindTag {
    Zoom,
    Overlay,
    Sound,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Zoom { scale: u16 },
    Overlay { template: String },
    Sound { name: Id },
}

/// What a click on a person's effect picks, and Delete removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Own {
    Placed(Placed),
    /// A sound placed with an override on the word at `at`.
    Sound {
        at: Micros,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub id: MarkId,
    pub kind: Kind,
    /// Media time of the take. A sound covers its word.
    pub range: TimeRange,
    /// A few words on the chip: the scale, what the overlay shows, the sound's name.
    pub label: String,
    /// A person's effect; `None` for one the style rules made.
    pub own: Option<Own>,
    /// Where an effect the style made comes from.
    pub style: Option<String>,
}

impl Mark {
    /// The lane hue: zoom blue, highlight lime, other overlays violet, sounds neutral.
    pub fn hue(&self) -> Hsla {
        match &self.kind {
            Kind::Zoom { .. } => ZOOM.into(),
            Kind::Overlay { template } if template == highlight::TEMPLATE => ACCENT.into(),
            Kind::Overlay { .. } => rgb(OVERLAY).into(),
            Kind::Sound { .. } => MUTED.into(),
        }
    }

    /// A highlight washes its words instead of underlining them.
    pub fn washes(&self) -> bool {
        matches!(&self.kind, Kind::Overlay { template } if template == highlight::TEMPLATE)
    }

    pub fn icon(&self) -> &'static str {
        match &self.kind {
            Kind::Zoom { .. } => "zoom",
            Kind::Sound { .. } => "system-audio",
            Kind::Overlay { template } => match template.as_str() {
                highlight::TEMPLATE => "edit",
                broll::TEMPLATE => "picture",
                link_card::TEMPLATE => "link",
                overlay::pop::TEMPLATE => "spark",
                overlay::callout::TEMPLATE => "cursor",
                _ => "layout",
            },
        }
    }

    /// What it is, in a word or two.
    pub fn name(&self) -> String {
        match &self.kind {
            Kind::Zoom { .. } => "Zoom".into(),
            Kind::Sound { .. } => "Sound".into(),
            Kind::Overlay { template } => match template.as_str() {
                highlight::TEMPLATE => "Highlight".into(),
                broll::TEMPLATE => "B-roll".into(),
                link_card::TEMPLATE => "Link card".into(),
                overlay::pop::TEMPLATE => "Pop".into(),
                overlay::callout::TEMPLATE => "Callout".into(),
                other => {
                    let mut name = other.replace('-', " ");
                    if let Some(first) = name.get_mut(..1) {
                        first.make_ascii_uppercase();
                    }
                    name
                }
            },
        }
    }

    /// Whether the effect covers media time `t`.
    pub fn covers(&self, t: Micros) -> bool {
        self.range.from() <= t && t < self.range.to()
    }
}

/// How far from its word a sound may sit and still mark it.
const SOUND_REACH: Micros = Micros(300_000);

fn shorten(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// Every effect of the block at `at`, in the order they start.
pub fn marks(this: &EditScreen, at: TakeAt, cx: &App) -> Vec<Mark> {
    let folder = this.folder.read(cx);
    let block = |cut: &shot_core::Cut| {
        cut.chapters
            .get(at.chapter)
            .and_then(|c| c.takes.get(at.take))
            .cloned()
    };
    let (Some(shown), own) = (block(&folder.shown.cut), block(&folder.project.cut)) else {
        return Vec::new();
    };
    let lock_take = folder.lock().and_then(|l| l.take(&shown.take));
    let made_by = |kind: KindTag, from: Micros| -> Option<String> {
        let instance = lock_take?
            .instances
            .iter()
            .find(|i| match (&i.applied, kind) {
                (Applied::Zoom { range, .. }, KindTag::Zoom) => range.from() == from,
                (Applied::Overlay { range, .. }, KindTag::Overlay) => range.from() == from,
                _ => false,
            })?;
        Some(crate::place::provenance(&instance.provenance))
    };
    let mut out = Vec::new();
    for (index, zoom) in shown.zooms.iter().enumerate() {
        let mine = own
            .as_ref()
            .and_then(|b| b.zooms.iter().position(|z| z == zoom));
        out.push(Mark {
            id: MarkId {
                at,
                kind: KindTag::Zoom,
                from: zoom.range.from(),
                index,
            },
            kind: Kind::Zoom { scale: zoom.scale },
            range: zoom.range,
            label: format!("{}%", zoom.scale),
            own: mine.map(|i| Own::Placed(Placed::Zoom(i))),
            style: mine
                .is_none()
                .then(|| made_by(KindTag::Zoom, zoom.range.from()))
                .flatten()
                .or_else(|| mine.is_none().then(|| "From the style rules".into())),
        });
    }
    for (index, placed) in shown.overlays.iter().enumerate() {
        let mine = own
            .as_ref()
            .and_then(|b| b.overlays.iter().position(|o| o == placed));
        out.push(Mark {
            id: MarkId {
                at,
                kind: KindTag::Overlay,
                from: placed.range.from(),
                index,
            },
            kind: Kind::Overlay {
                template: placed.template.to_string(),
            },
            range: placed.range,
            label: shorten(&super::overlays::summary(placed), 18),
            own: mine.map(|i| Own::Placed(Placed::Overlay(i))),
            style: mine
                .is_none()
                .then(|| made_by(KindTag::Overlay, placed.range.from()))
                .flatten()
                .or_else(|| mine.is_none().then(|| "From the style rules".into())),
        });
    }
    // Sounds play from the lock only, a person's from an override on a word.
    let kept = |t: Micros| {
        shown.ranges.iter().any(|r| {
            matches!(r, shot_core::RangeEdit::Keep(_))
                && r.range().from() <= t
                && t < r.range().to()
        })
    };
    if let Some(take) = lock_take {
        for (index, instance) in take.instances.iter().enumerate() {
            let Applied::Sfx { sound, at: t, .. } = &instance.applied else {
                continue;
            };
            if !kept(*t) {
                continue;
            }
            let p = &instance.provenance;
            let mine = p.layer == Layer::Override;
            let own = p
                .override_at
                .filter(|_| mine)
                .filter(|o| {
                    folder.project.cut.overrides.iter().any(|over| {
                        over.take == shown.take
                            && over.at == *o
                            && over.effects.iter().any(|e| {
                                matches!(e, OverrideEffect::Set(shot_core::Effect::Sfx { .. }))
                            })
                    })
                })
                .map(|o| Own::Sound { at: o });
            out.push(Mark {
                id: MarkId {
                    at,
                    kind: KindTag::Sound,
                    from: *t,
                    index,
                },
                kind: Kind::Sound {
                    name: sound.clone(),
                },
                range: TimeRange::new(*t, Micros(t.0 + SOUND_REACH.0)).expect("a length"),
                label: sound.to_string(),
                own,
                style: (!mine).then(|| crate::place::provenance(p)),
            });
        }
    }
    out.sort_by_key(|m| (m.range.from(), m.id.kind as u8));
    out
}

/// A layout switch in the transcript: the stretch it starts and the layout before it.
#[derive(Debug, Clone, PartialEq)]
pub struct Switch {
    pub run: LayoutRun,
    /// The layout the switch replaces, which removing it brings back.
    pub before: Layout,
}

/// Where the layout changes in the block at `at`, `default` being the layout before the
/// first block.
pub fn switches(this: &EditScreen, at: TakeAt, default: Layout) -> Vec<Switch> {
    let runs = this.map.layout_runs();
    let mut before = default;
    let mut out = Vec::new();
    for run in runs {
        if run.at == at && run.layout != before {
            out.push(Switch {
                run: run.clone(),
                before,
            });
        }
        before = run.layout;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_labels_end_in_an_ellipsis() {
        assert_eq!(shorten("never deploy on fridays", 12), "never deploy…");
        assert_eq!(shorten("150%", 12), "150%");
    }
}
