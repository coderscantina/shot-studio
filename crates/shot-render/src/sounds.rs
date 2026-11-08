//! The brand's automatic sounds in an output ([0021], [0023]): the `sounds` node's keys played
//! per motion phase on every card, pop, zoom, click with a ripple and transition. Pure.
//!
//! An overlay is a `pop` or a card (every other template that moves: link cards, lower thirds,
//! chapter bands, callouts, highlights, code diffs, end cards and the brand's Lottie
//! templates); redactions and b-roll have no sound. Its phases are its first part's on the
//! preset's springs ([`crate::motion::Motion::times`]), a zoom's are its spring's
//! ([`crate::zoom::phases`]), a click has all of them at once and plays only where a ripple
//! draws: with `cursor ripple=#true` in a layout showing the screen.
//!
//! Each phase lands on a frame: the first frame drawn at or after its time, the first that shows
//! the move in that phase. Overlays, zooms and clicks are in take time, so that frame plays
//! wherever the output shows it and nowhere the cut leaves it out. Transitions are in output
//! frames already: the bare key at the window's start, `-travel` and `-impact` on the cut,
//! `-settle` at the window's end.
//!
//! [0021]: ../../../docs/decisions/0021-sound-transitions-and-bookends.md
//! [0023]: ../../../docs/decisions/0023-motion-system-and-lottie-templates.md

use std::collections::HashSet;

use shot_audio::Anchor;
use shot_audio::phases::{Move, move_sounds};
use shot_core::{Effects, Id, Micros, SoundEffect, SoundKey};

use crate::motion::Part;
use crate::overlay::Params;
use crate::plan::ExportPlan;
use crate::theme::Theme;

/// The sound an overlay template plays on, if any.
fn effect_of(template: &str) -> Option<SoundEffect> {
    match template {
        crate::overlay::pop::TEMPLATE => Some(SoundEffect::Pop),
        "blur" | "bleep" | "b-roll" => None,
        _ => Some(SoundEffect::Card),
    }
}

/// The brand's sounds (`brand`) for every move in `plan`, with where each plays in output
/// samples, in no particular order. `clicks` gives a take's click times; it is asked only when
/// clicks can sound.
pub(crate) fn automatic<'a>(
    plan: &ExportPlan,
    theme: &Theme,
    effects: &Effects,
    brand: &'a [(SoundKey, Id)],
    clicks: &mut dyn FnMut(&Id) -> Vec<Micros>,
) -> Vec<(&'a Id, Anchor)> {
    let mut out = Vec::new();
    if brand.is_empty() {
        return out;
    }
    let per_frame = u64::from(shot_media::audio::SAMPLE_RATE / plan.fps);
    let sample = |frame: u64| frame * per_frame;
    let has = |effect: SoundEffect| brand.iter().any(|(key, _)| key.effect == effect);

    for transition in &plan.transitions {
        let mv = Move {
            effect: SoundEffect::Transition,
            start: sample(transition.window.start),
            impact: sample(transition.edge),
            settle: sample(transition.window.end),
            exit: None,
        };
        out.extend(move_sounds(&mv, brand));
    }

    // Moves in take time, in frames of their take rather than samples, placed wherever the
    // output shows those frames.
    let frame = |t: Micros| (t.0 * u64::from(plan.fps)).div_ceil(1_000_000);
    let mut place = |take: &Id, mv: Move, screen_only: bool| {
        for (sound, anchor) in move_sounds(&mv, brand) {
            let (at, peak) = match anchor {
                Anchor::Start(at) => (at, false),
                Anchor::Peak(at) => (at, true),
            };
            for planned in &plan.segments {
                let source = &planned.source_frames;
                if planned.segment.take != *take
                    || !source.contains(&at)
                    || (screen_only && !planned.segment.layout.shows_screen())
                {
                    continue;
                }
                let output = sample(planned.output_frame + at - source.start);
                out.push((
                    sound,
                    if peak {
                        Anchor::Peak(output)
                    } else {
                        Anchor::Start(output)
                    },
                ));
            }
        }
    };
    let (mut seen, mut zooms) = (HashSet::new(), HashSet::new());
    let mut takes = Vec::new();
    for planned in &plan.segments {
        let take = &planned.segment.take;
        if !takes.contains(take) {
            takes.push(take.clone());
        }
        for overlay in &planned.segment.overlays {
            let Some(effect) = effect_of(overlay.template.as_str()) else {
                continue;
            };
            if !has(effect) || !seen.insert((take.clone(), overlay.template.clone(), overlay.range))
            {
                continue;
            }
            let times = Params(overlay)
                .motion(theme)
                .times(overlay.range, Part::WHOLE);
            let mv = Move {
                effect,
                start: frame(times.start),
                impact: frame(times.impact),
                settle: frame(times.settle),
                exit: Some(frame(times.exit)),
            };
            place(take, mv, false);
        }
        if has(SoundEffect::Zoom) {
            for (start, rest, exit) in
                crate::zoom::phases(&planned.segment.zooms, effects.zoom_motion)
            {
                if !zooms.insert((take.clone(), start)) {
                    continue;
                }
                let mv = Move {
                    effect: SoundEffect::Zoom,
                    start: frame(start),
                    impact: frame(rest),
                    settle: frame(rest),
                    exit: exit.map(frame),
                };
                place(take, mv, false);
            }
        }
    }
    if effects.cursor.ripple && has(SoundEffect::Click) {
        for take in &takes {
            for t in clicks(take) {
                let at = frame(t);
                let mv = Move {
                    effect: SoundEffect::Click,
                    start: at,
                    impact: at,
                    settle: at,
                    exit: None,
                };
                place(take, mv, true);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{Cut, FormatKind, Phase, Project, Script, Timeline, Video, ZoomMotion};

    const FRAME: u64 = 800;

    /// A take kept from 0 to 4 s in the face layout and from 6 to 10 s, with a lower third whose
    /// exit falls in the cut, a pop and a zoom after it.
    fn plan() -> ExportPlan {
        let project = Project {
            video: Video::parse("title \"t\"\nstatus editing\nformat tutorial\n").unwrap(),
            script: Script::parse("# t\n").unwrap(),
            format: FormatKind::Tutorial.preset(),
            cut: Cut::parse(
                "chapter \"One\" {\n    take t01 {\n        keep from=0 to=4_000_000\n        \
                 keep from=6_000_000 to=10_000_000\n        \
                 layout face from=0 to=4_000_000\n        \
                 zoom 150 from=8_000_000 to=9_500_000\n        \
                 overlay lower-third from=1_000_000 to=5_000_000 {\n            text \"x\"\n        }\n        \
                 overlay pop from=7_000_000 to=8_000_000 {\n            text \"x\"\n        }\n    }\n}\n",
            )
            .unwrap(),
        };
        ExportPlan::new(Timeline::evaluate(&project).main(), 60, |_| {
            shot_core::MediaId::default()
        })
    }

    fn brand() -> Vec<(SoundKey, Id)> {
        let key = |effect, phase| SoundKey { effect, phase };
        let id = |s: &str| Id::new(s).unwrap();
        vec![
            (key(SoundEffect::Card, Some(Phase::Travel)), id("whoosh")),
            (key(SoundEffect::Card, Some(Phase::Settle)), id("tick")),
            (key(SoundEffect::Card, Some(Phase::Exit)), id("swoosh")),
            (key(SoundEffect::Pop, None), id("pop")),
            (key(SoundEffect::Zoom, Some(Phase::Settle)), id("thud")),
            (key(SoundEffect::Zoom, Some(Phase::Exit)), id("swish")),
            (key(SoundEffect::Click, None), id("click")),
        ]
    }

    fn placed(effects: &Effects) -> Vec<(String, Anchor)> {
        let brand = brand();
        let mut clicks = |_: &Id| vec![Micros(2_000_000), Micros(7_500_000)];
        let mut out: Vec<_> = automatic(&plan(), &Theme::default(), effects, &brand, &mut clicks)
            .into_iter()
            .map(|(name, anchor)| (name.to_string(), anchor))
            .collect();
        out.sort_by_key(|(name, _)| name.clone());
        out
    }

    #[test]
    fn each_phase_plays_on_the_first_frame_that_shows_it_where_the_cut_keeps_it() {
        let theme = Theme::default();
        let card = theme.motion.times(
            shot_core::TimeRange::new(Micros(1_000_000), Micros(5_000_000)).unwrap(),
            Part::WHOLE,
        );
        let frame = |t: Micros| (t.0 * 60).div_ceil(1_000_000);
        let mut effects = Effects {
            zoom_motion: ZoomMotion::Smooth,
            ..Effects::default()
        };
        effects.cursor.ripple = true;
        // The take plays from 6 s at output 4 s, 120 frames earlier. The smooth zoom rests and
        // starts out 450 ms into and before its end; the lower third's exit is cut.
        let expected = [
            ("click", Anchor::Start((450 - 120) * FRAME)),
            ("pop", Anchor::Start((420 - 120) * FRAME)),
            ("swish", Anchor::Start((543 - 120) * FRAME)),
            ("thud", Anchor::Peak((507 - 120) * FRAME)),
            ("tick", Anchor::Peak(frame(card.settle) * FRAME)),
            ("whoosh", Anchor::Peak(frame(card.impact) * FRAME)),
        ];
        assert!(card.exit > Micros(4_000_000) && card.settle < Micros(4_000_000));
        let expected: Vec<_> = expected.map(|(n, a)| (n.to_owned(), a)).into();
        assert_eq!(placed(&effects), expected);

        // Without a ripple clicks are silent; the click at 2 s is in the face layout anyway.
        effects.cursor.ripple = false;
        assert!(placed(&effects).iter().all(|(name, _)| name != "click"));
    }
}
