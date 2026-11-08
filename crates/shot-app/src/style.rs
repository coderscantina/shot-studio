//! A migrated video in the app: what the screens show of its lock, and the overrides a person's
//! answers write. Pure functions over `shot-core` types; [`crate::folder`] does the IO.
//!
//! The screens were built on take blocks that carry the rough cut, the review queue and rule
//! zooms. [`shown`] folds the lock back into that shape for display only, so the Edit, Cut,
//! Picture and Timeline views read one cut, and `Timeline::evaluate` of it gives the frames
//! `Timeline::styled` gives export. Edits go to the real `cut.kdl`, never to the shown cut.

use shot_core::{
    Applied, AutoZoom, Cut, Decision, EditError, ElementKind, Format, FormatLayer, Id, Lock,
    LockTake, Overlay, Override, RangeEdit, RemovedBy, Review, RuleKind, TakeAt, Target, TimeRange,
    Zoom,
};

/// The auto-cut rule a pacing element kind stands for.
fn rule_kind(kind: ElementKind) -> Option<RuleKind> {
    Some(match kind {
        ElementKind::Silence => RuleKind::Silence,
        ElementKind::Pause => RuleKind::Pause,
        ElementKind::Filler => RuleKind::Filler,
        ElementKind::Retake => RuleKind::Retake,
        ElementKind::Misspoken => RuleKind::Misspoken,
        ElementKind::Idle => RuleKind::IdleScreen,
        _ => return None,
    })
}

/// Who a lock cut shows as. A cut on a kind outside the pacing kinds has no rule name in the
/// screens and shows as a person's.
fn removed_by(target: Target) -> RemovedBy {
    match target {
        Target::Kind(kind) => rule_kind(kind).map_or(RemovedBy::Manual, RemovedBy::Rule),
        _ => RemovedBy::Manual,
    }
}

/// The cut as the screens show a migrated video: each block's kept ranges less the lock's cuts
/// of its take, removed by the rule of their element's kind; the lock's review items that play
/// in the block; and the lock's zooms and overlays after a person's.
pub fn shown(cut: &Cut, lock: &Lock) -> Cut {
    let mut shown = cut.clone();
    for block in shown.chapters.iter_mut().flat_map(|c| &mut c.takes) {
        let Some(styled) = lock.take(&block.take) else {
            continue;
        };
        let kept: Vec<TimeRange> = block
            .ranges
            .iter()
            .filter_map(|r| match r {
                RangeEdit::Keep(range) => Some(*range),
                RangeEdit::Remove { .. } => None,
            })
            .collect();
        block.ranges = fold_cuts(&block.ranges, styled);
        for instance in &styled.instances {
            match &instance.applied {
                Applied::Review(range) => {
                    let by = match instance.element.target {
                        Target::Kind(kind) => rule_kind(kind),
                        _ => None,
                    };
                    if let Some(by) = by
                        && kept.iter().any(|k| k.overlaps(*range))
                    {
                        block.reviews.push(Review {
                            range: *range,
                            by,
                            kept: false,
                        });
                    }
                }
                Applied::Zoom {
                    scale,
                    range,
                    center,
                } => block.zooms.push(Zoom {
                    scale: *scale,
                    range: *range,
                    center: *center,
                    by: None,
                }),
                Applied::Overlay {
                    template,
                    range,
                    params,
                } => block.overlays.push(Overlay {
                    template: template.clone(),
                    range: *range,
                    params: params.clone(),
                }),
                _ => {}
            }
        }
        block.zooms.sort_by_key(|z| z.range.from());
    }
    shown
}

/// `ranges` with the lock's cuts taken out of the kept ones. Overlapping cuts count once, the
/// earlier one names the removal.
fn fold_cuts(ranges: &[RangeEdit], styled: &LockTake) -> Vec<RangeEdit> {
    let mut cuts: Vec<(TimeRange, RemovedBy)> = styled
        .instances
        .iter()
        .filter_map(|i| match i.applied {
            Applied::Cut(range) => Some((range, removed_by(i.element.target))),
            _ => None,
        })
        .collect();
    cuts.sort_by_key(|(range, _)| range.from());
    let mut out: Vec<RangeEdit> = Vec::with_capacity(ranges.len() + cuts.len() * 2);
    let mut push = |piece: RangeEdit| {
        if let (
            Some(RangeEdit::Remove { range: last, by }),
            RangeEdit::Remove {
                range,
                by: piece_by,
            },
        ) = (out.last_mut(), piece)
            && *by == piece_by
            && last.to() == range.from()
        {
            *last = TimeRange::new(last.from(), range.to()).expect("adjacent ranges join");
            return;
        }
        out.push(piece);
    };
    for piece in ranges {
        let RangeEdit::Keep(kept) = *piece else {
            push(*piece);
            continue;
        };
        let mut from = kept.from();
        for &(cut, by) in cuts.iter().filter(|(c, _)| c.overlaps(kept)) {
            if let Some(before) = TimeRange::new(from, cut.from()) {
                push(RangeEdit::Keep(before));
            }
            if let Some(range) = TimeRange::new(from.max(cut.from()), cut.to().min(kept.to())) {
                push(RangeEdit::Remove { range, by });
            }
            from = from.max(cut.to());
        }
        if let Some(after) = TimeRange::new(from, kept.to()) {
            push(RangeEdit::Keep(after));
        }
    }
    out
}

fn block_take(cut: &Cut, at: TakeAt) -> Result<&Id, EditError> {
    cut.chapters
        .get(at.chapter)
        .and_then(|c| c.takes.get(at.take))
        .map(|b| &b.take)
        .ok_or(EditError::UnknownTake(at))
}

/// `cut` with the override of (`take`, `kind`, `at`) changed by `change`, added when there is
/// none, in canonical order.
pub(crate) fn with_override(
    cut: &Cut,
    take: &Id,
    kind: ElementKind,
    at: shot_core::Micros,
    change: impl FnOnce(&mut Override),
) -> Cut {
    let mut cut = cut.clone();
    let index = match cut
        .overrides
        .iter()
        .position(|o| o.take == *take && o.kind == kind && o.at == at)
    {
        Some(index) => index,
        None => {
            cut.overrides.push(Override {
                take: take.clone(),
                kind,
                at,
                decision: None,
                effects: Vec::new(),
            });
            cut.overrides.len() - 1
        }
    };
    change(&mut cut.overrides[index]);
    let order: Vec<Id> = cut
        .chapters
        .iter()
        .flat_map(|c| &c.takes)
        .map(|b| b.take.clone())
        .collect();
    cut.overrides
        .sort_by_key(|o| (order.iter().position(|t| *t == o.take), o.at, o.kind));
    cut
}

/// K or C on the lock's review item at `range` in the block at `at`: `override { keep }` or
/// `override { cut }` on its element.
pub fn decide(
    cut: &Cut,
    lock: &Lock,
    at: TakeAt,
    range: TimeRange,
    decision: Decision,
) -> Result<Cut, EditError> {
    let take = block_take(cut, at)?;
    let element = lock
        .take(take)
        .and_then(|t| {
            t.instances
                .iter()
                .find(|i| i.applied == Applied::Review(range))
        })
        .map(|i| i.element)
        .ok_or(EditError::NotInReview(range))?;
    let Target::Kind(kind) = element.target else {
        return Err(EditError::NotInReview(range));
    };
    Ok(with_override(cut, take, kind, element.t, |o| {
        o.decision = Some(decision);
    }))
}

/// Restores `range` of the block at `at`: a person's removals there turn back into kept media,
/// and each lock cut there gets `override { keep }` on its element, so the rules leave it.
pub fn restore(cut: &Cut, lock: &Lock, at: TakeAt, range: TimeRange) -> Result<Cut, EditError> {
    let mut restored = cut.restore(at, range)?;
    let take = block_take(cut, at)?.clone();
    for instance in lock.take(&take).iter().flat_map(|t| &t.instances) {
        if let (Applied::Cut(cut_range), Target::Kind(kind)) =
            (&instance.applied, instance.element.target)
            && cut_range.overlaps(range)
        {
            restored = with_override(&restored, &take, kind, instance.element.t, |o| {
                o.decision = Some(Decision::Keep);
            });
        }
    }
    Ok(restored)
}

/// The video's `format.kdl` layer that gives `format` over `base`, the settings below it: the
/// settings that differ, with the layer's brand fields and rules kept. An optional setting
/// `format` leaves at its default is written out, since leaving it out inherits `base`'s.
pub fn layer(format: &Format, base: &Format, current: &FormatLayer) -> FormatLayer {
    fn differs<T: PartialEq>(a: T, b: T) -> Option<T> {
        (a != b).then_some(a)
    }
    let layouts = |f: &Format| {
        std::iter::once(f.default_layout)
            .chain(f.other_layouts.iter().copied())
            .collect::<Vec<_>>()
    };
    let effects = format.effects();
    FormatLayer {
        prompter: differs(format.prompter, base.prompter),
        pacing: differs(format.pacing, base.pacing),
        layouts: differs(layouts(format), layouts(base)),
        captions: differs(format.captions, base.captions),
        auto_cut: differs(format.auto_cut.clone(), base.auto_cut.clone()),
        auto_zoom: (format.auto_zoom != base.auto_zoom).then(|| {
            format.auto_zoom.clone().unwrap_or_else(|| AutoZoom {
                triggers: Vec::new(),
                ..base.auto_zoom.clone().expect("differs from none")
            })
        }),
        zoom_motion: (format.zoom_motion != base.zoom_motion).then_some(effects.zoom_motion),
        cursor: (format.cursor != base.cursor).then_some(effects.cursor),
        keystrokes: (format.keystrokes != base.keystrokes).then_some(effects.keystrokes),
        window: (format.window != base.window).then_some(effects.window),
        brand: current.brand.clone(),
        rules: current.rules.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::{FormatKind, Micros, Project, PrompterMode, Script, Timeline, Video};

    const CUT: &str = "chapter \"A\" {\n    take t01 {\n        keep from=0 to=10_000_000\n        remove from=10_000_000 to=11_000_000 by=manual\n        keep from=11_000_000 to=20_000_000\n    }\n}\n";
    const LOCK: &str = "lock 1\n\ntake t01 {\n    cut from=2_000_000 to=3_000_000 on=silence t=2_000_000 layer=format rule=\"silence\"\n    cut from=2_500_000 to=4_000_000 on=filler t=2_500_000 layer=format rule=\"filler\"\n    review from=6_000_000 to=7_000_000 on=pause t=6_000_000 layer=format rule=\"pause\"\n    zoom 112 from=8_000_000 to=9_000_000 on=emphasis t=8_000_000 layer=format rule=\"emphasis\"\n    cut from=10_500_000 to=12_000_000 on=silence t=10_500_000 layer=format rule=\"silence\"\n}\n";

    fn range(from: u64, to: u64) -> TimeRange {
        TimeRange::new(Micros(from), Micros(to)).unwrap()
    }

    fn project(cut: Cut) -> Project {
        Project {
            video: Video::parse("title \"A\"\nstatus editing\nformat opinion\n").unwrap(),
            script: Script::parse("# A\n").unwrap(),
            format: FormatKind::Opinion.preset(),
            cut,
        }
    }

    const AT: TakeAt = TakeAt {
        chapter: 0,
        take: 0,
    };

    #[test]
    fn the_shown_cut_plays_the_frames_of_the_styled_timeline() {
        let (cut, lock) = (Cut::parse(CUT).unwrap(), Lock::parse(LOCK).unwrap());
        let shown = shown(&cut, &lock);
        let silence = RemovedBy::Rule(RuleKind::Silence);
        assert_eq!(
            shown.chapters[0].takes[0].ranges,
            [
                RangeEdit::Keep(range(0, 2_000_000)),
                RangeEdit::Remove {
                    range: range(2_000_000, 3_000_000),
                    by: silence
                },
                RangeEdit::Remove {
                    range: range(3_000_000, 4_000_000),
                    by: RemovedBy::Rule(RuleKind::Filler)
                },
                RangeEdit::Keep(range(4_000_000, 10_000_000)),
                RangeEdit::Remove {
                    range: range(10_000_000, 11_000_000),
                    by: RemovedBy::Manual
                },
                RangeEdit::Remove {
                    range: range(11_000_000, 12_000_000),
                    by: silence
                },
                RangeEdit::Keep(range(12_000_000, 20_000_000)),
            ]
        );
        assert_eq!(
            shown
                .review_queue()
                .map(|(_, r)| r.range)
                .collect::<Vec<_>>(),
            [range(6_000_000, 7_000_000)]
        );
        assert_eq!(
            Timeline::evaluate(&project(shown)),
            Timeline::styled(&project(cut), &lock)
        );
    }

    #[test]
    fn answers_and_restores_write_overrides_on_the_element() {
        let (cut, lock) = (Cut::parse(CUT).unwrap(), Lock::parse(LOCK).unwrap());
        let review = range(6_000_000, 7_000_000);
        let cut_it = decide(&cut, &lock, AT, review, Decision::Cut).unwrap();
        let kept = decide(&cut_it, &lock, AT, review, Decision::Keep).unwrap();
        assert_eq!(kept.overrides.len(), 1, "one override per element");
        assert!(
            kept.to_kdl()
                .ends_with("override t01 pause at=6_000_000 {\n    keep\n}\n"),
            "{}",
            kept.to_kdl()
        );
        assert!(decide(&cut, &lock, AT, range(1, 2), Decision::Keep).is_err());

        let restored = restore(&cut, &lock, AT, range(9_000_000, 12_000_000)).unwrap();
        assert_eq!(
            restored.chapters[0].takes[0].ranges,
            [RangeEdit::Keep(range(0, 20_000_000))]
        );
        let kinds: Vec<_> = restored.overrides.iter().map(|o| (o.kind, o.at)).collect();
        assert_eq!(kinds, [(ElementKind::Silence, Micros(10_500_000))]);
        assert_eq!(Cut::parse(&restored.to_kdl()).unwrap(), restored);
    }

    #[test]
    fn a_format_change_writes_only_what_differs_from_the_channel() {
        let base = FormatKind::Opinion.preset();
        let current = FormatLayer::parse("on filler off\n").unwrap();
        let mut format = base.clone();
        assert_eq!(layer(&format, &base, &current), current);
        format.prompter = PrompterMode::FullScript;
        assert_eq!(
            layer(&format, &base, &current).to_kdl(),
            "prompter full-script\non filler off\n"
        );
    }
}
