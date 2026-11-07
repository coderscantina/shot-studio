//! The style engine: resolves the cascade and applies rules and overrides to elements,
//! producing the lock. Pure and deterministic: the same inputs give a byte-identical lock.

use std::collections::BTreeMap;
use std::fmt;

use crate::lock::{Applied, Asset, ElementRef, Instance, Lock, LockTake, Orphan, Provenance};
use crate::style::ActiveRule;
use crate::timeline::ContentHash;
use crate::{
    Attr, AttrValue, Brand, Cut, Decision, Effect, EffectKey, Element, ElementKind, Elements,
    FormatLayer, Id, Layer, Micros, Override, OverrideEffect, ParamValue, RangeEdit, Selector,
    StyleSheet, Target, TimeRange,
};

/// Everything a build reads. The caller does the IO: parsing the files, fetching the imports
/// at their commit and hashing the asset files.
#[derive(Debug, Clone, Copy)]
pub struct StyleInput<'a> {
    /// The brand file of each `import` of `brand`, in the same order. Their own imports are not
    /// followed.
    pub imported: &'a [Brand],
    /// `brand/brand.kdl` of the channel.
    pub brand: &'a Brand,
    /// The channel's `formats/<format>.kdl` named by `video.kdl`.
    pub format: &'a FormatLayer,
    /// The video's `format.kdl`.
    pub video: &'a FormatLayer,
    pub cut: &'a Cut,
    /// `elements/<take>.jsonl` of each take that has one. A take without a file has no elements.
    pub elements: &'a [Elements],
    /// Content hashes of the files under the channel repo, by path as the brand names them.
    pub assets: &'a BTreeMap<String, ContentHash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleError {
    /// `imported` does not match the brand's `import` nodes.
    ImportCount { expected: usize, found: usize },
    /// The brand names a file that `assets` does not hash.
    MissingAsset(String),
    /// Two element files for one take.
    DuplicateElements(Id),
}

impl fmt::Display for StyleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImportCount { expected, found } => write!(
                f,
                "the brand imports {expected} style(s), {found} were given"
            ),
            Self::MissingAsset(path) => write!(f, "the brand names `{path}`, which has no hash"),
            Self::DuplicateElements(take) => write!(f, "two element files for take {take}"),
        }
    }
}

impl std::error::Error for StyleError {}

/// Builds the lock.
pub fn build(input: &StyleInput<'_>) -> Result<Lock, StyleError> {
    if input.imported.len() != input.brand.imports.len() {
        return Err(StyleError::ImportCount {
            expected: input.brand.imports.len(),
            found: input.imported.len(),
        });
    }
    let mut brand = input
        .imported
        .iter()
        .fold(Brand::default(), |merged, import| merged.merge(import));
    brand = brand.merge(input.brand);
    for patch in [&input.format.brand, &input.video.brand]
        .into_iter()
        .flatten()
    {
        brand = brand.merge(patch);
    }
    let imports = std::mem::take(&mut brand.imports);
    let assets = brand
        .assets()
        .into_iter()
        .map(|path| {
            input
                .assets
                .get(path)
                .map(|hash| Asset {
                    path: path.to_owned(),
                    hash: *hash,
                })
                .ok_or_else(|| StyleError::MissingAsset(path.to_owned()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let imported_rules = input.imported.iter().map(|b| (Layer::Import, &b.rules));
    let rules = StyleSheet::cascade(imported_rules.chain([
        (Layer::Brand, &input.brand.rules),
        (Layer::Format, &input.format.rules),
        (Layer::Video, &input.video.rules),
    ]));

    let mut takes = Vec::new();
    for take in take_order(input.cut) {
        let mut found = input.elements.iter().filter(|e| e.take() == take);
        let elements = found.next();
        if found.next().is_some() {
            return Err(StyleError::DuplicateElements(take.clone()));
        }
        takes.push(evaluate_take(take, elements, input.cut, &rules));
    }
    Ok(Lock {
        imports,
        assets,
        brand,
        takes,
    })
}

/// Takes of the main cut in order of first appearance.
fn take_order(cut: &Cut) -> Vec<&Id> {
    let mut order: Vec<&Id> = Vec::new();
    for block in cut.chapters.iter().flat_map(|c| &c.takes) {
        if !order.contains(&&block.take) {
            order.push(&block.take);
        }
    }
    order
}

/// What a rule is applied to: an element, or a moment of the cut's structure.
struct Subject<'a> {
    target: Target,
    t: Micros,
    /// The end of a span, `None` for a moment.
    to: Option<Micros>,
    element: Option<&'a Element>,
}

impl Subject<'_> {
    fn of(element: &Element) -> Subject<'_> {
        Subject {
            target: Target::Kind(element.kind),
            t: element.t,
            to: element.to,
            element: Some(element),
        }
    }

    fn at(target: Target, t: Micros) -> Subject<'static> {
        Subject {
            target,
            t,
            to: None,
            element: None,
        }
    }

    fn span(&self) -> Option<TimeRange> {
        TimeRange::new(self.t, self.to?)
    }

    /// Facts without a confidence are certain.
    fn confidence(&self) -> u8 {
        self.element.and_then(|e| e.confidence).unwrap_or(100)
    }

    fn matches(&self, selector: &Selector) -> bool {
        use crate::Filter;
        selector.target == self.target
            && selector.filters.iter().all(|filter| match filter {
                Filter::Attr(attr, value) => {
                    self.element.and_then(|e| e.attr(*attr)) == Some(value)
                }
                Filter::Min(min) => self.span().is_some_and(|s| s.duration() >= *min),
                Filter::Confidence(c) => self.confidence() >= *c,
            })
    }
}

/// The moments of the cut's structure in one take: chapter and video edges.
fn structure_subjects(cut: &Cut, take: &Id) -> Vec<Subject<'static>> {
    let mut subjects = Vec::new();
    let edges: Vec<(&Id, Micros, &Id, Micros)> = cut
        .chapters
        .iter()
        .filter_map(|chapter| {
            let first = chapter.takes.first()?;
            let last = chapter.takes.last()?;
            let start = first.ranges.iter().find_map(|r| match r {
                RangeEdit::Keep(range) => Some(range.from()),
                RangeEdit::Remove { .. } => None,
            })?;
            let end = last.ranges.iter().rev().find_map(|r| match r {
                RangeEdit::Keep(range) => Some(range.to()),
                RangeEdit::Remove { .. } => None,
            })?;
            Some((&first.take, start, &last.take, end))
        })
        .collect();
    for (i, (start_take, start, end_take, end)) in edges.iter().enumerate() {
        if *start_take == take {
            subjects.push(Subject::at(Target::ChapterStart, *start));
            if i == 0 {
                subjects.push(Subject::at(Target::VideoStart, *start));
            }
        }
        if *end_take == take {
            subjects.push(Subject::at(Target::ChapterEnd, *end));
            if i == edges.len() - 1 {
                subjects.push(Subject::at(Target::VideoEnd, *end));
            }
        }
    }
    subjects
}

/// The kept ranges of a take across the cut, sorted.
fn kept_ranges(cut: &Cut, take: &Id) -> Vec<TimeRange> {
    let mut kept: Vec<TimeRange> = cut
        .chapters
        .iter()
        .flat_map(|c| &c.takes)
        .filter(|block| &block.take == take)
        .flat_map(|block| &block.ranges)
        .filter_map(|r| match r {
            RangeEdit::Keep(range) => Some(*range),
            RangeEdit::Remove { .. } => None,
        })
        .collect();
    kept.sort_by_key(|r| r.from());
    kept
}

fn is_kept(kept: &[TimeRange], t: Micros) -> bool {
    let i = kept.partition_point(|r| r.from() <= t);
    i > 0 && t < kept[i - 1].to()
}

/// The effect that won an element's [`EffectKey`] and where it came from.
struct Winner<'a> {
    effect: &'a Effect,
    provenance: Provenance,
}

fn evaluate_take(
    take: &Id,
    elements: Option<&Elements>,
    cut: &Cut,
    rules: &[ActiveRule],
) -> LockTake {
    let kept = kept_ranges(cut, take);
    let mut orphans = Vec::new();
    // Overrides bound to their element, by the element's identity.
    let mut bound: BTreeMap<(ElementKind, Micros), Vec<&Override>> = BTreeMap::new();
    for o in cut.overrides.iter().filter(|o| &o.take == take) {
        match elements.and_then(|e| e.nearest(o.kind, o.at, Override::TOLERANCE)) {
            Some(element) => bound.entry((element.kind, element.t)).or_default().push(o),
            None => orphans.push(Orphan {
                at: o.at,
                kind: o.kind,
            }),
        }
    }

    let element_subjects = elements
        .into_iter()
        .flat_map(Elements::iter)
        .filter(|e| is_kept(&kept, e.t))
        .map(Subject::of);
    let mut instances = Vec::new();
    for subject in element_subjects.chain(structure_subjects(cut, take)) {
        let mut winners: BTreeMap<EffectKey, (usize, usize, Winner<'_>)> = BTreeMap::new();
        for (order, rule) in rules.iter().enumerate() {
            if !subject.matches(&rule.selector) {
                continue;
            }
            let rank = (rule.selector.specificity(), order);
            for effect in &rule.effects {
                let better = winners
                    .get(&effect.key())
                    .is_none_or(|(s, o, _)| rank > (*s, *o));
                if better {
                    let winner = Winner {
                        effect,
                        provenance: Provenance {
                            layer: rule.layer,
                            rule: Some(rule.selector.clone()),
                            override_at: None,
                        },
                    };
                    winners.insert(effect.key(), (rank.0, rank.1, winner));
                }
            }
        }
        let mut winners: BTreeMap<EffectKey, Winner<'_>> =
            winners.into_iter().map(|(k, (_, _, w))| (k, w)).collect();

        let overrides = match subject.target {
            Target::Kind(kind) => bound.get(&(kind, subject.t)).map_or(&[][..], Vec::as_slice),
            _ => &[],
        };
        let mut decision = None;
        for o in overrides {
            if o.decision.is_some() {
                decision = o.decision.map(|d| (d, o.at));
            }
            for effect in &o.effects {
                match effect {
                    OverrideEffect::Set(effect) => {
                        let winner = Winner {
                            effect,
                            provenance: Provenance {
                                layer: Layer::Override,
                                rule: None,
                                override_at: Some(o.at),
                            },
                        };
                        winners.insert(effect.key(), winner);
                    }
                    OverrideEffect::Off(key) => {
                        winners.remove(key);
                    }
                }
            }
        }

        let element = ElementRef {
            target: subject.target,
            t: subject.t,
        };
        let cut_winner = winners.remove(&EffectKey::Cut);
        if let Some((applied, provenance)) = apply_cut(&subject, cut_winner, decision) {
            instances.push(Instance {
                applied,
                element,
                provenance,
            });
        }
        for winner in winners.into_values() {
            if let Some(applied) = apply(winner.effect, &subject) {
                instances.push(Instance {
                    applied,
                    element,
                    provenance: winner.provenance,
                });
            }
        }
    }
    instances.sort_by_key(Instance::order_key);
    orphans.sort_unstable();
    LockTake {
        take: take.clone(),
        instances,
        orphans,
    }
}

/// The pacing's answer for one element: a cut, a review item or nothing.
fn apply_cut(
    subject: &Subject<'_>,
    winner: Option<Winner<'_>>,
    decision: Option<(Decision, Micros)>,
) -> Option<(Applied, Provenance)> {
    let span = subject.span()?;
    let (breath, threshold) = match winner.as_ref().map(|w| w.effect) {
        Some(Effect::Cut { breath, confidence }) => (*breath, *confidence),
        _ => (Micros(0), 0),
    };
    let trimmed = || {
        let from = if subject.t.0 == 0 {
            0
        } else {
            span.from().0 + breath.0
        };
        TimeRange::new(Micros(from), Micros(span.to().0.saturating_sub(breath.0)))
    };
    match decision {
        Some((Decision::Keep, _)) => None,
        Some((Decision::Cut, at)) => {
            let provenance = Provenance {
                layer: Layer::Override,
                rule: None,
                override_at: Some(at),
            };
            Some((Applied::Cut(trimmed()?), provenance))
        }
        None => {
            let winner = winner?;
            if subject.confidence() >= threshold {
                Some((Applied::Cut(trimmed()?), winner.provenance))
            } else {
                Some((Applied::Review(span), winner.provenance))
            }
        }
    }
}

/// One effect's instance on a subject, or `None` when it would be empty.
fn apply(effect: &Effect, subject: &Subject<'_>) -> Option<Applied> {
    let t = subject.t;
    Some(match effect {
        Effect::Cut { .. } => unreachable!("cuts go through apply_cut"),
        Effect::Layout(layout) => Applied::Layout {
            layout: *layout,
            range: subject.span()?,
        },
        Effect::Zoom { scale, hold, lead } => {
            let from = Micros(t.0.saturating_sub(lead.0));
            let to = Micros(subject.to.unwrap_or(t).0.max(t.0 + hold.0));
            Applied::Zoom {
                scale: *scale,
                range: TimeRange::new(from, to)?,
                center: subject.element.and_then(Element::point),
            }
        }
        Effect::Overlay {
            template,
            length,
            copies,
            params,
        } => {
            let range = match length {
                Some(length) => TimeRange::new(t, Micros(t.0 + length.0))?,
                None => subject.span()?,
            };
            let copied = copies.iter().filter_map(|attr| {
                let value = match subject.element?.attr(*attr)? {
                    AttrValue::Text(s) => ParamValue::Text(s.clone()),
                    AttrValue::Integer(n) => ParamValue::Integer(*n),
                };
                Some((param_name(*attr), value))
            });
            Applied::Overlay {
                template: template.clone(),
                range,
                params: copied.chain(params.iter().cloned()).collect(),
            }
        }
        Effect::Sfx { sound, gain } => Applied::Sfx {
            sound: sound.clone(),
            at: t,
            gain: *gain,
        },
        Effect::Transition { kind, length } => Applied::Transition {
            kind: *kind,
            at: t,
            length: *length,
        },
        Effect::Music { file, gain, duck } => Applied::Music {
            file: file.clone(),
            at: t,
            gain: *gain,
            duck: *duck,
        },
        Effect::Captions(style) => Applied::Captions {
            style: *style,
            range: subject.span()?,
        },
    })
}

fn param_name(attr: Attr) -> Id {
    Id::new(attr.as_str()).expect("attribute names are ids")
}
