//! The brand's automatic sounds on a move, one per phase ([0023]).
//!
//! The motion system gives the phase times; this places the sounds the brand's `sounds` node
//! names for the move's effect. Arrivals land on their frame: a travel or impact sound peaks on
//! the frame the spring first reaches its place, a settle sound on the frame it rests. A sound
//! without a phase starts with the move, an exit sound with the exit.
//!
//! [0023]: ../../../docs/decisions/0023-motion-system-and-lottie-templates.md

use shot_core::{Id, Phase, SoundEffect, SoundKey};

use crate::mix::Anchor;

/// A move's phases in output samples, each at the start of its frame. A move without a spring,
/// such as a click, has its impact and settle at its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Move {
    pub effect: SoundEffect,
    pub start: u64,
    pub impact: u64,
    pub settle: u64,
    /// Where the exit starts, `None` for a move that does not leave.
    pub exit: Option<u64>,
}

/// The sounds of `sounds` (the brand's `sounds` node) for `mv`, with where each plays.
pub fn move_sounds<'a>(
    mv: &Move,
    sounds: &'a [(SoundKey, Id)],
) -> impl Iterator<Item = (&'a Id, Anchor)> {
    let mv = *mv;
    sounds
        .iter()
        .filter(move |(key, _)| key.effect == mv.effect)
        .filter_map(move |(key, sound)| {
            let anchor = match key.phase {
                None => Anchor::Start(mv.start),
                Some(Phase::Travel | Phase::Impact) => Anchor::Peak(mv.impact),
                Some(Phase::Settle) => Anchor::Peak(mv.settle),
                Some(Phase::Exit) => Anchor::Start(mv.exit?),
            };
            Some((sound, anchor))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_phase_lands_on_its_frame() {
        let key = |effect, phase| SoundKey { effect, phase };
        let id = |s: &str| Id::new(s).unwrap();
        let sounds = vec![
            (key(SoundEffect::Card, None), id("swish")),
            (key(SoundEffect::Card, Some(Phase::Travel)), id("whoosh")),
            (key(SoundEffect::Card, Some(Phase::Settle)), id("tick")),
            (key(SoundEffect::Card, Some(Phase::Exit)), id("swoosh")),
            (key(SoundEffect::Zoom, None), id("riser")),
        ];
        let mv = Move {
            effect: SoundEffect::Card,
            start: 1_000,
            impact: 9_000,
            settle: 16_000,
            exit: None,
        };
        let placed: Vec<_> = move_sounds(&mv, &sounds)
            .map(|(s, a)| (s.as_str(), a))
            .collect();
        assert_eq!(
            placed,
            [
                ("swish", Anchor::Start(1_000)),
                ("whoosh", Anchor::Peak(9_000)),
                ("tick", Anchor::Peak(16_000)),
            ]
        );
        let leaving = Move {
            exit: Some(90_000),
            ..mv
        };
        assert_eq!(
            move_sounds(&leaving, &sounds)
                .last()
                .map(|(s, a)| (s.as_str(), a)),
            Some(("swoosh", Anchor::Start(90_000)))
        );
    }
}
