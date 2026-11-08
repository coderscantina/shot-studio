//! `shot migrate`: a video written before the lock, turned into content, overrides and the
//! rules of its layers once, mechanically ([0024]). The migrated video renders the same frames:
//! [`migrate`] checks that its timeline equals the old one before it hands anything back.
//!
//! [0024]: ../../../docs/decisions/0024-content-stays-in-cut-kdl-style-moves-to-the-lock.md

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::lock::{Applied, Instance, Lock};
use crate::timeline::Timeline;
use crate::{
    Attr, AttrValue, AutoZoom, Brand, ContentHash, Cut, Decision, Effect, EffectKey, ElementKind,
    Elements, Filter, Format, FormatLayer, Id, Micros, Overlay, Override, OverrideEffect, Project,
    RangeEdit, RemovedBy, Rule, RuleBody, RuleKind, Selector, StyleError, StyleInput, StyleSheet,
    TakeEdit, Target, TimeRange, Zoom, ZoomBy, ZoomTrigger, build,
};

/// The pacing's punch-in on emphasis, and the shortest it holds.
pub const EMPHASIS_ZOOM: u16 = 112;
pub const EMPHASIS_HOLD: Micros = Micros(1_000_000);
/// How long before a click or typing an auto zoom starts.
pub const ZOOM_LEAD: Micros = Micros(500_000);
/// How long a link card shows from the word that says the URL.
pub const LINK_CARD_FOR: Micros = Micros(6_000_000);
/// Idle screen at least this long gets a b-roll slot, a B marker one this long.
pub const B_ROLL_IDLE: Micros = Micros(8_000_000);
pub const B_ROLL_MARKER: Micros = Micros(4_000_000);

/// The overlay templates the overlay rules wrote into `cut.kdl` before the lock.
const RULE_TEMPLATES: [&str; 3] = ["link-card", "highlight", "b-roll"];

/// The style rules a format's settings stand for: the auto-cut rules with the pacing's breath,
/// the punch-in on emphasis, the auto-zoom triggers, link cards, highlights and b-roll slots.
/// The shipped presets carry exactly these.
pub fn implied_rules(format: &Format) -> StyleSheet {
    let auto = &format.auto_cut;
    let breath = format.pacing.breath();
    let cut = |breath| Effect::Cut {
        breath,
        confidence: auto.confidence,
    };
    let at_least = |kind, min| Selector {
        target: Target::Kind(kind),
        filters: vec![Filter::Min(min)],
    };
    let any = |kind| Selector::new(Target::Kind(kind));
    let id = |text: &str| Id::new(text).expect("template names are ids");
    let overlay = |template, length, copies| Effect::Overlay {
        template: id(template),
        length,
        copies,
        params: Vec::new(),
    };

    let mut rules: Vec<(Selector, Effect)> = Vec::new();
    for (rule, kind) in [
        (auto.silence, ElementKind::Silence),
        (auto.pause, ElementKind::Pause),
        (auto.idle_screen, ElementKind::Idle),
    ] {
        if rule.on {
            rules.push((at_least(kind, rule.min), cut(breath)));
        }
    }
    for (on, kind) in [
        (auto.filler, ElementKind::Filler),
        (auto.retake, ElementKind::Retake),
        (auto.misspoken, ElementKind::Misspoken),
    ] {
        if on {
            rules.push((any(kind), cut(Micros(0))));
        }
    }
    rules.push((
        any(ElementKind::Emphasis),
        Effect::Zoom {
            scale: EMPHASIS_ZOOM,
            hold: EMPHASIS_HOLD,
            lead: Micros(0),
        },
    ));
    for trigger in format.auto_zoom.iter().flat_map(|z| &z.triggers) {
        let kind = match trigger {
            ZoomTrigger::Click => ElementKind::Click,
            ZoomTrigger::Typing => ElementKind::Typing,
            // A resting cursor has no element kind yet.
            ZoomTrigger::Cursor => continue,
        };
        let zoom = format.auto_zoom.as_ref().expect("has triggers");
        rules.push((
            any(kind),
            Effect::Zoom {
                scale: zoom.scale,
                hold: Micros(zoom.hold.0.saturating_sub(ZOOM_LEAD.0)),
                lead: ZOOM_LEAD,
            },
        ));
    }
    rules.push((
        any(ElementKind::Url),
        overlay("link-card", Some(LINK_CARD_FOR), vec![Attr::Url]),
    ));
    rules.push((
        any(ElementKind::Ocr),
        overlay(
            "highlight",
            None,
            vec![Attr::Text, Attr::X, Attr::Y, Attr::Width, Attr::Height],
        ),
    ));
    rules.push((
        at_least(ElementKind::Idle, B_ROLL_IDLE),
        overlay("b-roll", None, Vec::new()),
    ));
    rules.push((
        Selector {
            target: Target::Kind(ElementKind::Marker),
            filters: vec![Filter::Attr(Attr::Label, AttrValue::Text("B".into()))],
        },
        overlay("b-roll", Some(B_ROLL_MARKER), Vec::new()),
    ));

    // One rule per selector: an idle cut and the b-roll slot may share one.
    let mut sheet = StyleSheet::default();
    for (selector, effect) in rules {
        match sheet.rules.iter_mut().find(|r| r.selector == selector) {
            Some(Rule {
                body: RuleBody::Effects(effects),
                ..
            }) => {
                effects.push(effect);
                effects.sort_by_key(Effect::key);
            }
            _ => sheet.rules.push(Rule {
                selector,
                body: RuleBody::Effects(vec![effect]),
            }),
        }
    }
    sheet
}

/// Whether `cut.kdl` still holds what rules wrote: rule removals, the review queue or rule
/// zooms. Such a video is migrated before a build may write its lock.
pub fn has_rule_nodes(cut: &Cut) -> bool {
    cut.chapters.iter().flat_map(|c| &c.takes).any(|block| {
        !block.reviews.is_empty()
            || block.zooms.iter().any(|z| z.by.is_some())
            || block.ranges.iter().any(|r| {
                matches!(
                    r,
                    RangeEdit::Remove {
                        by: RemovedBy::Rule(_),
                        ..
                    }
                )
            })
    })
}

/// What [`migrate`] reads. The project is the video as written before the lock; the rest is
/// what a build reads, with the channel's format as it is on disk.
#[derive(Debug, Clone, Copy)]
pub struct MigrateInput<'a> {
    pub project: &'a Project,
    pub imported: &'a [Brand],
    pub brand: &'a Brand,
    /// The channel's `formats/<format>.kdl`.
    pub channel: &'a FormatLayer,
    pub elements: &'a [Elements],
    pub assets: &'a BTreeMap<String, ContentHash>,
}

/// The migrated files and the lock they build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    pub cut: Cut,
    /// The video's `format.kdl`: only what differs from the channel's.
    pub video: FormatLayer,
    /// The channel's format with the default rules of its settings, when it had no rules yet.
    pub channel: Option<FormatLayer>,
    /// The video's settings after the cascade, for its [`Project`].
    pub format: Format,
    pub lock: Lock,
    pub report: Report,
}

/// What became of the nodes the rules wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Rule removals, review items, zooms and overlays the layers' rules now produce.
    pub reproduced: usize,
    /// Overrides written to keep what the old cut showed.
    pub overrides: usize,
    /// Rule removals no rule or override reproduces, kept as a person's cut.
    pub manual: usize,
    /// Rule zooms and overlays no rule or override reproduces, kept as a person's.
    pub kept: usize,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} reproduced by rules, {} overrides, {} manual cuts, {} zooms and overlays kept",
            self.reproduced, self.overrides, self.manual, self.kept
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrateError {
    Style(StyleError),
    /// The migrated video would not show what the old one did.
    Differs(String),
}

impl fmt::Display for MigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Style(e) => e.fmt(f),
            Self::Differs(what) => write!(f, "the migrated video would differ: {what}"),
        }
    }
}

impl std::error::Error for MigrateError {}

impl From<StyleError> for MigrateError {
    fn from(e: StyleError) -> Self {
        Self::Style(e)
    }
}

/// Migrates the video, per the [migration mapping] of the contract:
///
/// - The content keeps a person's cuts, zooms and overlays. Kept ranges that only a rule's
///   removal split merge into one.
/// - A rule removal, review item, zoom or overlay the rules reproduce is dropped.
/// - Where the rules would do more than the old cut showed, an override takes it back: `keep`
///   on a cut, `zoom off`, `overlay <template> off`. `ignore` becomes `keep`.
/// - Where they do less, an override on the element adds it (`cut` on a queued finding, a zoom
///   or overlay placed exactly), and what no element explains stays as a person's.
///
/// [migration mapping]: ../../../docs/contracts/project-files.md#migration-of-todays-nodes
pub fn migrate(input: &MigrateInput<'_>) -> Result<Migration, MigrateError> {
    let old = input.project;
    let kind = old.video.format;
    let channel = input.channel.rules.rules.is_empty().then(|| {
        let mut layer = input.channel.clone();
        layer.rules = implied_rules(&kind.preset().resolve([input.channel]));
        layer
    });
    let channel_layer = channel.as_ref().unwrap_or(input.channel);
    let channel_format = kind.preset().resolve([channel_layer]);
    let mut video = settings_diff(&old.format, &channel_format);
    video.rules = rules_diff(&implied_rules(&old.format), &channel_layer.rules);
    let format = channel_format.resolve([&video]);

    let mut m = Migrator {
        input,
        channel: channel_layer,
        video: &video,
        old: &old.cut,
        cut: content(&old.cut),
        overrides: BTreeMap::new(),
        report: Report::default(),
    };
    m.decisions();
    m.cuts()?;
    m.zooms()?;
    m.overlays()?;
    let (cut, report) = m.finish();

    let lock = build(&style_input(input, channel_layer, &video, &cut))?;
    let migrated = Project {
        cut: cut.clone(),
        format: format.clone(),
        ..old.clone()
    };
    same_frames(
        &Timeline::evaluate(old),
        &Timeline::styled(&migrated, &lock),
    )
    .map_err(MigrateError::Differs)?;
    Ok(Migration {
        cut,
        video,
        channel,
        format,
        lock,
        report,
    })
}

fn style_input<'a>(
    input: &'a MigrateInput<'_>,
    channel: &'a FormatLayer,
    video: &'a FormatLayer,
    cut: &'a Cut,
) -> StyleInput<'a> {
    StyleInput {
        imported: input.imported,
        brand: input.brand,
        format: channel,
        video,
        cut,
        elements: input.elements,
        assets: input.assets,
    }
}

/// The settings of `old` that differ from the channel's, written so the cascade gives them
/// back. An optional setting the old file left at its default is written out, since a layer
/// that leaves it out inherits the channel's.
fn settings_diff(old: &Format, channel: &Format) -> FormatLayer {
    fn differs<T: PartialEq>(a: T, b: T) -> Option<T> {
        (a != b).then_some(a)
    }
    let layouts = |f: &Format| {
        std::iter::once(f.default_layout)
            .chain(f.other_layouts.iter().copied())
            .collect::<Vec<_>>()
    };
    let effects = old.effects();
    FormatLayer {
        prompter: differs(old.prompter, channel.prompter),
        pacing: differs(old.pacing, channel.pacing),
        layouts: differs(layouts(old), layouts(channel)),
        captions: differs(old.captions, channel.captions),
        auto_cut: differs(old.auto_cut.clone(), channel.auto_cut.clone()),
        auto_zoom: (old.auto_zoom != channel.auto_zoom).then(|| {
            old.auto_zoom.clone().unwrap_or_else(|| AutoZoom {
                triggers: Vec::new(),
                ..channel.auto_zoom.clone().expect("differs from none")
            })
        }),
        zoom_motion: (old.zoom_motion != channel.zoom_motion).then_some(effects.zoom_motion),
        cursor: (old.cursor != channel.cursor).then_some(effects.cursor),
        keystrokes: (old.keystrokes != channel.keystrokes).then_some(effects.keystrokes),
        window: (old.window != channel.window).then_some(effects.window),
        brand: None,
        rules: StyleSheet::default(),
    }
}

/// The video's rules where its own settings stood for other rules than the channel's: its
/// rules that differ, and `off` for the channel's cut and zoom rules it never had.
fn rules_diff(video: &StyleSheet, channel: &StyleSheet) -> StyleSheet {
    let mut sheet = StyleSheet::default();
    for rule in &video.rules {
        if !channel.rules.contains(rule) {
            sheet.rules.push(rule.clone());
        }
    }
    for rule in &channel.rules {
        let RuleBody::Effects(effects) = &rule.body else {
            continue;
        };
        let paces = effects
            .iter()
            .any(|e| matches!(e, Effect::Cut { .. } | Effect::Zoom { .. }));
        if paces && !video.rules.iter().any(|r| r.selector == rule.selector) {
            sheet.rules.push(Rule {
                selector: rule.selector.clone(),
                body: RuleBody::Off,
            });
        }
    }
    sheet
}

/// `cut` without what the rules wrote: rule removals become kept material, the queue goes,
/// and so do rule zooms and the overlays of the overlay rules' templates.
fn content(cut: &Cut) -> Cut {
    let mut cut = cut.clone();
    for block in cut.chapters.iter_mut().flat_map(|c| &mut c.takes) {
        let ranges = block.ranges.drain(..).map(|r| match r {
            RangeEdit::Remove {
                range,
                by: RemovedBy::Rule(_),
            } => RangeEdit::Keep(range),
            other => other,
        });
        block.ranges = merge_keeps(ranges);
        block.reviews.clear();
        block.zooms.retain(|z| z.by.is_none());
        block
            .overlays
            .retain(|o| !RULE_TEMPLATES.contains(&o.template.as_str()));
    }
    cut
}

fn merge_keeps(ranges: impl IntoIterator<Item = RangeEdit>) -> Vec<RangeEdit> {
    let mut out: Vec<RangeEdit> = Vec::new();
    for range in ranges {
        if let (Some(RangeEdit::Keep(last)), RangeEdit::Keep(next)) = (out.last_mut(), range)
            && last.to() == next.from()
        {
            *last = TimeRange::new(last.from(), next.to()).expect("joined ranges are not empty");
            continue;
        }
        out.push(range);
    }
    out
}

/// The element kind a rule's findings are.
fn element_kind(rule: RuleKind) -> ElementKind {
    match rule {
        RuleKind::Silence => ElementKind::Silence,
        RuleKind::Retake => ElementKind::Retake,
        RuleKind::Misspoken => ElementKind::Misspoken,
        RuleKind::Pause => ElementKind::Pause,
        RuleKind::Filler => ElementKind::Filler,
        RuleKind::IdleScreen => ElementKind::Idle,
    }
}

/// The element kinds a rule zoom may hang on, by what wrote it.
fn zoom_kinds(by: Option<ZoomBy>) -> &'static [ElementKind] {
    match by {
        Some(ZoomBy::Click) => &[ElementKind::Click],
        Some(ZoomBy::Typing) => &[ElementKind::Typing],
        Some(ZoomBy::Emphasis) => &[ElementKind::Emphasis, ElementKind::Word],
        Some(ZoomBy::Marker) => &[ElementKind::Marker],
        Some(ZoomBy::JumpCut) => &[ElementKind::Word],
        Some(ZoomBy::Cursor) | None => &[],
    }
}

fn overlay_kinds(template: &str) -> &'static [ElementKind] {
    match template {
        "link-card" => &[ElementKind::Url],
        "highlight" => &[ElementKind::Ocr],
        "b-roll" => &[ElementKind::Idle, ElementKind::Marker],
        _ => &[],
    }
}

/// An override's identity: take, element kind, `at`.
type Key = (Id, ElementKind, Micros);

struct Migrator<'a> {
    input: &'a MigrateInput<'a>,
    channel: &'a FormatLayer,
    video: &'a FormatLayer,
    old: &'a Cut,
    cut: Cut,
    overrides: BTreeMap<Key, Override>,
    report: Report,
}

impl<'a> Migrator<'a> {
    fn takes(&self) -> Vec<Id> {
        let mut order: Vec<Id> = Vec::new();
        for block in self.old.chapters.iter().flat_map(|c| &c.takes) {
            if !order.contains(&block.take) {
                order.push(block.take.clone());
            }
        }
        order
    }

    fn elements(&self, take: &Id) -> Option<&'a Elements> {
        let all: &'a [Elements] = self.input.elements;
        all.iter().find(|e| e.take() == take)
    }

    fn lock(&mut self) -> Result<Lock, MigrateError> {
        self.cut.overrides = self.overrides.values().cloned().collect();
        let input = style_input(self.input, self.channel, self.video, &self.cut);
        Ok(build(&input)?)
    }

    fn entry(&mut self, take: &Id, kind: ElementKind, at: Micros) -> &mut Override {
        self.overrides
            .entry((take.clone(), kind, at))
            .or_insert_with(|| Override {
                take: take.clone(),
                kind,
                at,
                decision: None,
                effects: Vec::new(),
            })
    }

    fn set_effect(&mut self, take: &Id, kind: ElementKind, at: Micros, effect: OverrideEffect) {
        let o = self.entry(take, kind, at);
        o.effects.retain(|e| e.key() != effect.key());
        o.effects.push(effect);
        o.effects.sort_by_key(OverrideEffect::key);
    }

    fn unset_effect(&mut self, key: &Key, effect: &EffectKey) {
        if let Some(o) = self.overrides.get_mut(key) {
            o.effects.retain(|e| e.key() != *effect);
            if o.effects.is_empty() && o.decision.is_none() {
                self.overrides.remove(key);
            }
        }
    }

    /// `ignore` items become `keep` on their element; the queue's open items are the lock's.
    fn decisions(&mut self) {
        let mut kept = Vec::new();
        for block in self.old.chapters.iter().flat_map(|c| &c.takes) {
            for review in &block.reviews {
                if !review.kept {
                    self.report.reproduced += 1;
                    continue;
                }
                let kind = element_kind(review.by);
                let found = self
                    .elements(&block.take)
                    .and_then(|e| e.nearest(kind, review.range.from(), Override::TOLERANCE));
                if let Some(element) = found {
                    kept.push((block.take.clone(), kind, element.t));
                }
            }
        }
        for (take, kind, at) in kept {
            self.entry(&take, kind, at).decision = Some(Decision::Keep);
        }
    }

    /// Makes the lock's cuts remove exactly what the old rule removals did.
    fn cuts(&mut self) -> Result<(), MigrateError> {
        let takes = self.takes();
        let lock = self.lock()?;
        for take in &takes {
            let kept = old_kept(self.old, take);
            let Some(styled) = lock.take(take) else {
                continue;
            };
            for instance in &styled.instances {
                let Applied::Cut(range) = instance.applied else {
                    continue;
                };
                if kept.iter().any(|k| k.overlaps(range))
                    && let Target::Kind(kind) = instance.element.target
                {
                    self.entry(take, kind, instance.element.t).decision = Some(Decision::Keep);
                }
            }
        }

        // A removal of a finding the pacing only queues: C in the old queue.
        let lock = self.lock()?;
        for take in &takes {
            let cuts = lock_cuts(&lock, take);
            for (removed, rule) in rule_removals(self.old, take) {
                if subtract(removed, &cuts).is_empty() {
                    continue;
                }
                let kind = element_kind(rule);
                let queued = lock
                    .take(take)
                    .into_iter()
                    .flat_map(|t| &t.instances)
                    .find(|i| {
                        matches!(i.applied, Applied::Review(_))
                            && i.element.target == Target::Kind(kind)
                            && i.element.t.0.abs_diff(removed.from().0) <= Override::TOLERANCE.0
                    });
                if let Some(queued) = queued {
                    let key = (take.clone(), kind, queued.element.t);
                    if !self.overrides.contains_key(&key) {
                        self.entry(take, kind, queued.element.t).decision = Some(Decision::Cut);
                    }
                }
            }
        }
        let lock = self.lock()?;
        for take in &takes {
            let kept = old_kept(self.old, take);
            let wrong: Vec<Key> = lock
                .take(take)
                .into_iter()
                .flat_map(|t| &t.instances)
                .filter(|i| {
                    i.provenance.override_at.is_some()
                        && matches!(i.applied, Applied::Cut(r) if kept.iter().any(|k| k.overlaps(r)))
                })
                .filter_map(|i| match i.element.target {
                    Target::Kind(kind) => Some((take.clone(), kind, i.element.t)),
                    _ => None,
                })
                .collect();
            for key in wrong {
                self.overrides.remove(&key);
            }
        }

        // What still is not removed stays a person's cut. A manual cut silences every element
        // inside it, which can uncover another removal, so this runs until nothing changes.
        loop {
            let lock = self.lock()?;
            let mut changed = false;
            for take in &takes {
                let cuts = lock_cuts(&lock, take);
                let manual = manual_removals(&self.cut, take);
                for (removed, _) in rule_removals(self.old, take) {
                    let open = subtract(removed, &cuts);
                    if open.iter().all(|o| manual.iter().any(|m| contains(*m, *o))) {
                        continue;
                    }
                    remove_manually(&mut self.cut, take, removed);
                    self.overrides.retain(|(t, _, at), _| {
                        t != take || !(removed.from() <= *at && *at < removed.to())
                    });
                    self.report.manual += 1;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        self.report.reproduced += self
            .takes()
            .iter()
            .map(|t| rule_removals(self.old, t).len())
            .sum::<usize>()
            .saturating_sub(self.report.manual);
        Ok(())
    }

    /// Makes the lock's zooms in kept material equal the old rule zooms.
    fn zooms(&mut self) -> Result<(), MigrateError> {
        let lock = self.lock()?;
        let mut placed: Vec<(Key, Zoom)> = Vec::new();
        for take in self.takes() {
            let kept = old_kept(self.old, &take);
            let mut wanted: Vec<Zoom> = rule_zooms(self.old, &take);
            for instance in lock.take(&take).into_iter().flat_map(|t| &t.instances) {
                let Some(zoom) = zoom_of(instance) else {
                    continue;
                };
                if !kept.iter().any(|k| k.overlaps(zoom.range)) {
                    continue;
                }
                match wanted.iter().position(|w| same_zoom(w, &zoom)) {
                    Some(i) => {
                        wanted.remove(i);
                        self.report.reproduced += 1;
                    }
                    None => {
                        if let Target::Kind(kind) = instance.element.target {
                            let off = OverrideEffect::Off(EffectKey::Zoom);
                            self.set_effect(&take, kind, instance.element.t, off);
                        }
                    }
                }
            }
            for zoom in wanted {
                let element = self.elements(&take).and_then(|elements| {
                    zoom_kinds(zoom.by).iter().find_map(|kind| {
                        elements.of_kind(*kind).find(|e| {
                            zoom.range.from() <= e.t
                                && e.range().to() <= zoom.range.to()
                                && e.point() == zoom.center
                                && !self.overrides.contains_key(&(take.clone(), e.kind, e.t))
                        })
                    })
                });
                match element {
                    Some(e) => {
                        let key = (take.clone(), e.kind, e.t);
                        let effect = Effect::Zoom {
                            scale: zoom.scale,
                            hold: Micros(zoom.range.to().0 - e.t.0),
                            lead: Micros(e.t.0 - zoom.range.from().0),
                        };
                        self.set_effect(&take, e.kind, e.t, OverrideEffect::Set(effect));
                        placed.push((key, zoom));
                    }
                    None => self.keep_zoom(&take, zoom),
                }
            }
        }
        let lock = self.lock()?;
        for (key, zoom) in placed {
            let shown = lock
                .take(&key.0)
                .into_iter()
                .flat_map(|t| &t.instances)
                .any(|i| {
                    i.element.target == Target::Kind(key.1)
                        && i.element.t == key.2
                        && zoom_of(i).is_some_and(|z| same_zoom(&z, &zoom))
                });
            if !shown {
                self.unset_effect(&key, &EffectKey::Zoom);
                self.keep_zoom(&key.0, zoom);
            }
        }
        Ok(())
    }

    /// Puts a rule zoom back into the block it came from, as a person's.
    fn keep_zoom(&mut self, take: &Id, zoom: Zoom) {
        let blocks = self.cut.chapters.iter_mut().flat_map(|c| &mut c.takes);
        let olds = self.old.chapters.iter().flat_map(|c| &c.takes);
        for (block, old) in blocks.zip(olds) {
            if &block.take == take && old.zooms.contains(&zoom) {
                let mine = Zoom { by: None, ..zoom };
                let at = block
                    .zooms
                    .partition_point(|z| z.range.from() < mine.range.from());
                block.zooms.insert(at, mine);
            }
        }
        self.report.kept += 1;
    }

    /// Makes the lock's overlays in kept material equal the old overlay-rule overlays.
    fn overlays(&mut self) -> Result<(), MigrateError> {
        let lock = self.lock()?;
        let mut placed: Vec<(Key, Overlay)> = Vec::new();
        let mut keep: BTreeSet<(Id, usize)> = BTreeSet::new();
        for take in self.takes() {
            let kept = old_kept(self.old, &take);
            let mut wanted: Vec<(usize, Overlay)> = rule_overlays(self.old, &take);
            for instance in lock.take(&take).into_iter().flat_map(|t| &t.instances) {
                let Applied::Overlay {
                    template,
                    range,
                    params,
                } = &instance.applied
                else {
                    continue;
                };
                if !kept.iter().any(|k| k.overlaps(*range)) {
                    continue;
                }
                let shown = Overlay {
                    template: template.clone(),
                    range: *range,
                    params: params.clone(),
                };
                match wanted.iter().position(|(_, w)| *w == shown) {
                    Some(i) => {
                        wanted.remove(i);
                        self.report.reproduced += 1;
                    }
                    None => {
                        if let Target::Kind(kind) = instance.element.target {
                            let off = OverrideEffect::Off(EffectKey::Overlay(template.clone()));
                            self.set_effect(&take, kind, instance.element.t, off);
                        }
                    }
                }
            }
            for (index, overlay) in wanted {
                let element = self.elements(&take).and_then(|elements| {
                    overlay_kinds(overlay.template.as_str())
                        .iter()
                        .find_map(|kind| {
                            elements
                                .of_kind(*kind)
                                .find(|e| e.t == overlay.range.from())
                        })
                });
                match element {
                    Some(e) => {
                        let effect = Effect::Overlay {
                            template: overlay.template.clone(),
                            length: Some(overlay.range.duration()),
                            copies: Vec::new(),
                            params: overlay.params.clone(),
                        };
                        self.set_effect(&take, e.kind, e.t, OverrideEffect::Set(effect));
                        placed.push(((take.clone(), e.kind, e.t), overlay));
                    }
                    None => {
                        keep.insert((take.clone(), index));
                    }
                }
            }
        }
        let lock = self.lock()?;
        for (key, overlay) in placed {
            let shown = lock
                .take(&key.0)
                .into_iter()
                .flat_map(|t| &t.instances)
                .any(|i| {
                    i.element.t == key.2
                        && matches!(&i.applied, Applied::Overlay { template, range, params }
                            if *template == overlay.template && *range == overlay.range
                                && *params == overlay.params)
                });
            if !shown {
                self.unset_effect(&key, &EffectKey::Overlay(overlay.template.clone()));
                if let Some((index, _)) = rule_overlays(self.old, &key.0)
                    .into_iter()
                    .find(|(_, o)| *o == overlay)
                {
                    keep.insert((key.0.clone(), index));
                }
            }
        }
        self.report.kept += keep.len();
        // Overlays kept as a person's go back in their old drawing order.
        let blocks = self.cut.chapters.iter_mut().flat_map(|c| &mut c.takes);
        let olds = self.old.chapters.iter().flat_map(|c| &c.takes);
        let mut seen: BTreeMap<Id, usize> = BTreeMap::new();
        for (block, old) in blocks.zip(olds) {
            let start = *seen.entry(block.take.clone()).or_default();
            block.overlays = old
                .overlays
                .iter()
                .enumerate()
                .filter(|(i, o)| {
                    !RULE_TEMPLATES.contains(&o.template.as_str())
                        || keep.contains(&(block.take.clone(), start + i))
                })
                .map(|(_, o)| o.clone())
                .collect();
            *seen.get_mut(&block.take).expect("entered above") += old.overlays.len();
        }
        Ok(())
    }

    fn finish(mut self) -> (Cut, Report) {
        let order = self.takes();
        let mut overrides: Vec<Override> = self.overrides.into_values().collect();
        overrides.sort_by_key(|o| {
            let take = order
                .iter()
                .position(|t| *t == o.take)
                .unwrap_or(usize::MAX);
            (take, o.at, o.kind)
        });
        self.report.overrides = overrides.len();
        self.cut.overrides = overrides;
        (self.cut, self.report)
    }
}

/// The kept ranges of `take` in the old cut, in order.
fn old_kept(cut: &Cut, take: &Id) -> Vec<TimeRange> {
    let mut kept: Vec<TimeRange> = blocks(cut, take)
        .flat_map(|b| &b.ranges)
        .filter_map(|r| match r {
            RangeEdit::Keep(range) => Some(*range),
            RangeEdit::Remove { .. } => None,
        })
        .collect();
    kept.sort_by_key(|r| r.from());
    kept
}

fn blocks<'a>(cut: &'a Cut, take: &'a Id) -> impl Iterator<Item = &'a TakeEdit> + 'a {
    cut.chapters
        .iter()
        .flat_map(|c| &c.takes)
        .filter(move |b| b.take == *take)
}

fn rule_removals(cut: &Cut, take: &Id) -> Vec<(TimeRange, RuleKind)> {
    blocks(cut, take)
        .flat_map(|b| &b.ranges)
        .filter_map(|r| match *r {
            RangeEdit::Remove {
                range,
                by: RemovedBy::Rule(rule),
            } => Some((range, rule)),
            _ => None,
        })
        .collect()
}

fn manual_removals(cut: &Cut, take: &Id) -> Vec<TimeRange> {
    blocks(cut, take)
        .flat_map(|b| &b.ranges)
        .filter_map(|r| match *r {
            RangeEdit::Remove {
                range,
                by: RemovedBy::Manual,
            } => Some(range),
            _ => None,
        })
        .collect()
}

fn rule_zooms(cut: &Cut, take: &Id) -> Vec<Zoom> {
    let mut zooms: Vec<Zoom> = blocks(cut, take)
        .flat_map(|b| &b.zooms)
        .filter(|z| z.by.is_some())
        .copied()
        .collect();
    zooms.dedup();
    zooms
}

/// The overlay-rule overlays of `take` with their index among all of the take's overlays in
/// block order.
fn rule_overlays(cut: &Cut, take: &Id) -> Vec<(usize, Overlay)> {
    blocks(cut, take)
        .flat_map(|b| &b.overlays)
        .enumerate()
        .filter(|(_, o)| RULE_TEMPLATES.contains(&o.template.as_str()))
        .map(|(i, o)| (i, o.clone()))
        .collect()
}

fn zoom_of(instance: &Instance) -> Option<Zoom> {
    match instance.applied {
        Applied::Zoom {
            scale,
            range,
            center,
        } => Some(Zoom {
            scale,
            range,
            center,
            by: None,
        }),
        _ => None,
    }
}

/// Zooms that draw the same: provenance aside.
fn same_zoom(a: &Zoom, b: &Zoom) -> bool {
    (a.scale, a.range, a.center) == (b.scale, b.range, b.center)
}

fn lock_cuts(lock: &Lock, take: &Id) -> Vec<TimeRange> {
    let mut cuts: Vec<TimeRange> = lock.take(take).into_iter().flat_map(|t| t.cuts()).collect();
    cuts.sort_by_key(|r| r.from());
    let mut out: Vec<TimeRange> = Vec::with_capacity(cuts.len());
    for cut in cuts {
        match out.last_mut() {
            Some(last) if cut.from() <= last.to() => {
                *last = TimeRange::new(last.from(), last.to().max(cut.to())).expect("not empty");
            }
            _ => out.push(cut),
        }
    }
    out
}

/// The parts of `range` outside the sorted, disjoint `cuts`.
fn subtract(range: TimeRange, cuts: &[TimeRange]) -> Vec<TimeRange> {
    let mut out = Vec::new();
    let mut from = range.from();
    for cut in cuts.iter().filter(|c| c.overlaps(range)) {
        out.extend(TimeRange::new(from, cut.from()));
        from = from.max(cut.to());
    }
    out.extend(TimeRange::new(from, range.to()));
    out
}

fn contains(outer: TimeRange, inner: TimeRange) -> bool {
    outer.from() <= inner.from() && inner.to() <= outer.to()
}

/// Turns `range`, kept material of a block of `take`, into a person's cut.
fn remove_manually(cut: &mut Cut, take: &Id, range: TimeRange) {
    for block in cut
        .chapters
        .iter_mut()
        .flat_map(|c| &mut c.takes)
        .filter(|b| b.take == *take)
    {
        let mut out = Vec::with_capacity(block.ranges.len() + 2);
        for piece in block.ranges.drain(..) {
            let RangeEdit::Keep(kept) = piece else {
                out.push(piece);
                continue;
            };
            let Some(inside) = kept.intersect(range) else {
                out.push(piece);
                continue;
            };
            out.extend(TimeRange::new(kept.from(), inside.from()).map(RangeEdit::Keep));
            out.push(RangeEdit::Remove {
                range: inside,
                by: RemovedBy::Manual,
            });
            out.extend(TimeRange::new(inside.to(), kept.to()).map(RangeEdit::Keep));
        }
        block.ranges = out;
    }
}

/// Whether two timelines show the same frames: the same segments with the same layouts, zooms
/// and overlays, provenance aside. Names the first difference.
fn same_frames(old: &Timeline, new: &Timeline) -> Result<(), String> {
    for (a, b) in old.outputs.iter().zip(&new.outputs) {
        if (a.captions, a.effects) != (b.captions, b.effects) {
            return Err(format!("{}: the captions or render settings", a.name));
        }
        let segments = |o: &crate::Output| -> Vec<crate::Segment> {
            o.chapters
                .iter()
                .flat_map(|c| &c.segments)
                .map(|s| crate::Segment {
                    zooms: s.zooms.iter().map(|z| Zoom { by: None, ..*z }).collect(),
                    ..s.clone()
                })
                .collect()
        };
        let (sa, sb) = (segments(a), segments(b));
        if let Some((x, y)) = sa.iter().zip(&sb).find(|(x, y)| x != y) {
            return Err(format!(
                "{}: {} at {} s shows {:?} instead of {:?}",
                a.name,
                x.take,
                x.source.from().0 as f64 / 1e6,
                y,
                x
            ));
        }
        if sa.len() != sb.len() {
            return Err(format!(
                "{}: {} segments instead of {}",
                a.name,
                sb.len(),
                sa.len()
            ));
        }
    }
    if old.outputs.len() != new.outputs.len() {
        return Err("the derived cuts".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FormatKind;

    #[test]
    fn presets_carry_the_rules_their_settings_stand_for() {
        for &kind in FormatKind::ALL {
            assert_eq!(
                kind.preset_layer().rules,
                implied_rules(&kind.preset()),
                "{kind}"
            );
        }
    }
}
