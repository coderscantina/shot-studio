//! Kinetic type: text whose words or glyphs come in on their own springs, staggered in reading
//! order, and word pop captions whose spoken word scales up and settles. Pure; the vector pass
//! lays the text out with parley and draws each unit at its pose.

use shot_core::{Micros, TimeRange};

use crate::drawing::{Font, Rgba};
use crate::layout::Rect;
use crate::motion::{Motion, Part, Reveal, Spring, Unit};

/// Text whose units move on springs over `range`, drawn as it is at `t`.
#[derive(Debug, Clone, PartialEq)]
pub struct Kinetic {
    /// Top left of the first line.
    pub origin: [f32; 2],
    pub text: String,
    pub size: f32,
    pub font: Font,
    pub color: Rgba,
    /// Lines break to fit, when set.
    pub width: Option<f32>,
    pub reveal: Reveal,
    pub unit: Unit,
    pub motion: Motion,
    pub range: TimeRange,
    pub t: Micros,
}

/// How one unit draws: moved by `offset`, scaled by `scale` about its center, faded to
/// `opacity`, and with `masked` clipped to its line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitPose {
    pub offset: [f32; 2],
    pub scale: f32,
    pub opacity: f32,
    pub masked: bool,
}

impl Kinetic {
    /// The pose of `part`, the unit's place in reading order, on lines `line_height` tall.
    pub fn pose(&self, part: Part, line_height: f32) -> UnitPose {
        let pose = self.motion.pose(self.range, part, self.t);
        let travel = self.motion.travel;
        let rise = |share: f32| UnitPose {
            offset: [0.0, pose.offset() * self.size * share * travel],
            scale: 1.0,
            opacity: pose.presence(),
            masked: false,
        };
        match self.reveal {
            Reveal::Rise => rise(0.45),
            Reveal::Fade => rise(0.12),
            // Up out of the line's slot, and on up out of it again on the way out.
            Reveal::Mask => UnitPose {
                offset: [0.0, (1.0 - pose.enter - pose.exit) * line_height],
                scale: 1.0,
                opacity: if pose.enter > 0.0 && pose.exit < 1.0 {
                    1.0
                } else {
                    0.0
                },
                masked: true,
            },
            Reveal::Pop => UnitPose {
                offset: [0.0, 0.0],
                scale: ((0.35 + 0.65 * pose.enter) * (1.0 - 0.5 * pose.exit)).max(0.0),
                opacity: (pose.enter * 3.0).clamp(0.0, 1.0) * (1.0 - pose.exit),
                masked: false,
            },
        }
    }

    /// A rectangle that holds the text in every pose.
    pub fn bounds(&self) -> Rect {
        let chars = self.text.chars().count() as f32;
        let width = self.width.unwrap_or(chars * self.size);
        let lines = (chars * self.size * 0.7 / width).ceil().max(1.0) + 1.0;
        let reach = self.size * (1.0 + self.motion.travel);
        Rect::new(
            self.origin[0] - reach,
            self.origin[1] - reach,
            self.origin[0] + width + reach,
            self.origin[1] + lines * self.size * 1.3 + reach,
        )
    }
}

/// The scale of a word pop caption's spoken word `since` it started: it pops up past full size
/// from a little smaller and settles.
pub fn word_pop(since: Micros) -> f32 {
    0.6 + 0.4 * WORD_POP.value(since.0 as f32 / 1e6)
}

/// How long a word pop takes to settle; captions hold still from then on.
pub fn word_pop_settle() -> Micros {
    Micros((WORD_POP.settle() * 1e6).ceil() as u64)
}

const WORD_POP: Spring = Spring::new(0.35, 0.32);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::Preset;

    fn text(reveal: Reveal, t: u64) -> Kinetic {
        Kinetic {
            origin: [100.0, 100.0],
            text: "ship it today".into(),
            size: 100.0,
            font: Font::Sans,
            color: Rgba::hex(0xFFFFFF, 1.0),
            width: None,
            reveal,
            unit: Unit::Word,
            motion: Preset::Editorial.motion(),
            range: TimeRange::new(Micros(0), Micros(3_000_000)).unwrap(),
            t: Micros(t),
        }
    }

    #[test]
    fn units_come_in_in_reading_order_and_rest_in_place() {
        for reveal in [Reveal::Rise, Reveal::Mask, Reveal::Pop, Reveal::Fade] {
            let early = text(reveal, 60_000);
            let first = early.pose(Part::new(0, 3), 130.0);
            let last = early.pose(Part::new(2, 3), 130.0);
            assert!(first.opacity >= last.opacity, "{reveal:?}");
            assert!(first.offset[1] <= last.offset[1] || first.scale >= last.scale);
            let rest = text(reveal, 1_500_000).pose(Part::new(2, 3), 130.0);
            assert_eq!(
                (rest.offset, rest.scale, rest.opacity),
                ([0.0, 0.0], 1.0, 1.0),
                "{reveal:?}"
            );
        }
        // A mask reveal starts a full line below its slot, hidden by the mask.
        let start = text(Reveal::Mask, 0).pose(Part::new(0, 3), 130.0);
        assert!(start.masked && start.offset[1] == 130.0);
    }

    #[test]
    fn word_pop_overshoots_and_settles_at_full_size() {
        assert_eq!(word_pop(Micros(0)), 0.6);
        let peak = (0..400)
            .map(|ms| word_pop(Micros(ms * 1000)))
            .fold(0.0, f32::max);
        assert!(peak > 1.08 && peak < 1.2, "{peak}");
        assert_eq!(word_pop(word_pop_settle()), 1.0);
    }
}
