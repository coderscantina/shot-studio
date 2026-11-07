//! The semantic video files of the contract: `fixtures/examples/channel/` is a channel repo
//! with a brand, a format layer and one video whose `cut.lock` is what the style engine builds
//! from the other files.

use std::collections::BTreeMap;
use std::time::Instant;

use shot_core::{
    Applied, Attr, Brand, ContentHash, Cut, Element, ElementKind, Elements, Format, FormatKind,
    FormatLayer, Id, Layer, Lock, Micros, Pacing, StyleInput, Target, TimeRange, build,
};

const BRAND: &str = include_str!("../../../fixtures/examples/channel/brand/brand.kdl");
const CHANNEL_FORMAT: &str = include_str!("../../../fixtures/examples/channel/formats/opinion.kdl");
const FORMAT: &str = include_str!("../../../fixtures/examples/channel/videos/fridays/format.kdl");
const CUT: &str = include_str!("../../../fixtures/examples/channel/videos/fridays/cut.kdl");
const LOCK: &str = include_str!("../../../fixtures/examples/channel/videos/fridays/cut.lock");
const T01: &str =
    include_str!("../../../fixtures/examples/channel/videos/fridays/elements/t01.jsonl");
const T02: &str =
    include_str!("../../../fixtures/examples/channel/videos/fridays/elements/t02.jsonl");
const CONTRACT: &str = include_str!("../../../docs/contracts/project-files.md");

/// The style the brand imports, as the build fetched it at the pinned commit.
const IMPORTED: &str = "colors background=\"#0b0d10\"\non chapter-start {\n    transition cut\n}\non chapter-end {\n    sfx tick\n}\n";

fn id(text: &str) -> Id {
    Id::new(text).unwrap()
}

fn range(from: u64, to: u64) -> TimeRange {
    TimeRange::new(Micros(from), Micros(to)).unwrap()
}

/// The parsed files of the example channel.
struct Channel {
    imported: Vec<Brand>,
    brand: Brand,
    format: FormatLayer,
    video: FormatLayer,
    cut: Cut,
    elements: Vec<Elements>,
    assets: BTreeMap<String, ContentHash>,
}

impl Channel {
    fn example() -> Self {
        let brand = Brand::parse(BRAND).unwrap();
        let assets = brand
            .assets()
            .into_iter()
            .map(|path| (path.to_owned(), ContentHash::of(path.as_bytes())))
            .collect();
        Self {
            imported: vec![Brand::parse(IMPORTED).unwrap()],
            brand,
            format: FormatLayer::parse(CHANNEL_FORMAT).unwrap(),
            video: FormatLayer::parse(FORMAT).unwrap(),
            cut: Cut::parse(CUT).unwrap(),
            elements: vec![
                Elements::parse(id("t01"), T01).unwrap(),
                Elements::parse(id("t02"), T02).unwrap(),
            ],
            assets,
        }
    }

    fn input(&self) -> StyleInput<'_> {
        StyleInput {
            imported: &self.imported,
            brand: &self.brand,
            format: &self.format,
            video: &self.video,
            cut: &self.cut,
            elements: &self.elements,
            assets: &self.assets,
        }
    }

    fn lock(&self) -> Lock {
        build(&self.input()).unwrap()
    }

    /// The instances of one take of the lock on one element.
    fn instances_on(&self, take: &str, target: Target, t: u64) -> Vec<(Applied, Layer)> {
        self.lock()
            .take(&id(take))
            .unwrap()
            .instances
            .iter()
            .filter(|i| i.element.target == target && i.element.t == Micros(t))
            .map(|i| (i.applied.clone(), i.provenance.layer))
            .collect()
    }
}

fn assert_in_contract(example: &str) {
    assert!(
        CONTRACT.contains(example),
        "the contract is missing this example:\n{example}"
    );
}

#[test]
fn channel_examples_round_trip() {
    for (text, written) in [
        (BRAND, Brand::parse(BRAND).unwrap().to_kdl()),
        (
            CHANNEL_FORMAT,
            FormatLayer::parse(CHANNEL_FORMAT).unwrap().to_kdl(),
        ),
        (FORMAT, FormatLayer::parse(FORMAT).unwrap().to_kdl()),
        (CUT, Cut::parse(CUT).unwrap().to_kdl()),
        (T01, Elements::parse(id("t01"), T01).unwrap().to_jsonl()),
        (LOCK, Lock::parse(LOCK).unwrap().to_kdl()),
    ] {
        assert_in_contract(text);
        assert_eq!(written, text);
    }
    let t02 = Elements::parse(id("t02"), T02).unwrap();
    assert_eq!(t02.to_jsonl(), T02);
}

#[test]
fn the_lock_is_what_the_engine_builds() {
    let channel = Channel::example();
    let lock = channel.lock();
    assert_eq!(lock.to_kdl(), LOCK);
    assert_eq!(lock, Lock::parse(LOCK).unwrap());

    // The merged brand: the video's accent over the channel's, the import's background under it.
    assert_eq!(
        lock.brand.colors.accent,
        Some(shot_core::Color([0xff, 0x7a, 0x59]))
    );
    assert_eq!(
        lock.brand.colors.background,
        Some(shot_core::Color([0x10, 0x14, 0x18]))
    );
    assert!(lock.brand.rules.rules.is_empty() && lock.brand.imports.is_empty());
    assert_eq!(lock.imports, channel.brand.imports);
}

#[test]
fn two_builds_write_byte_identical_locks() {
    let mut channel = Channel::example();
    let first = channel.lock().to_kdl();
    channel.elements.reverse();
    assert_eq!(channel.lock().to_kdl(), first);
    assert_eq!(Channel::example().lock().to_kdl(), first);
}

#[test]
fn a_later_layer_replaces_a_rule_with_the_same_selector_or_switches_it_off() {
    let channel = Channel::example();
    // The video's `on filler` replaced the channel format's: no breath, 80 percent.
    assert_eq!(
        channel.instances_on("t02", Target::Kind(ElementKind::Filler), 20_000_000),
        [(Applied::Cut(range(20_000_000, 20_500_000)), Layer::Video)]
    );
    // `on punchline off` in the video removed the brand's pop.
    assert!(
        channel
            .instances_on("t01", Target::Kind(ElementKind::Punchline), 40_100_000)
            .is_empty()
    );
    // The brand's `on chapter-start` replaced the import's whole rule, `transition cut` included.
    let at_hook = channel.instances_on("t01", Target::ChapterStart, 0);
    assert_eq!(at_hook.len(), 3);
    assert!(at_hook.iter().all(|(_, layer)| *layer == Layer::Brand));
    assert!(at_hook.iter().any(|(a, _)| matches!(
        a,
        Applied::Transition {
            kind: shot_core::TransitionKind::Dip,
            ..
        }
    )));
}

#[test]
fn a_more_specific_selector_wins_then_the_later_layer() {
    let mut channel = Channel::example();
    // Two video rules select the 5 s silence; the one with two filters wins.
    let long = channel.instances_on("t01", Target::Kind(ElementKind::Silence), 52_000_000);
    assert_eq!(
        long,
        [(Applied::Cut(range(52_250_000, 56_750_000)), Layer::Video)]
    );
    let lock = channel.lock();
    let silence = lock
        .take(&id("t01"))
        .unwrap()
        .instances
        .iter()
        .find(|i| i.element.t == Micros(52_000_000));
    assert_eq!(
        silence
            .unwrap()
            .provenance
            .rule
            .as_ref()
            .unwrap()
            .to_string(),
        "silence min=4_000_000 confidence=50"
    );

    // Same specificity: the video's zoom beats the channel format's.
    let emphasis = channel.instances_on("t01", Target::Kind(ElementKind::Emphasis), 8_000_000);
    assert_eq!(emphasis[0].1, Layer::Format);
    channel.video =
        FormatLayer::parse(&format!("{FORMAT}on emphasis {{\n    zoom 120\n}}\n")).unwrap();
    let emphasis = channel.instances_on("t01", Target::Kind(ElementKind::Emphasis), 8_000_000);
    assert_eq!(
        emphasis,
        [(
            Applied::Zoom {
                scale: 120,
                range: range(8_000_000, 9_500_000),
                center: None
            },
            Layer::Video
        )]
    );
}

#[test]
fn unsure_cuts_queue_and_overrides_answer_them() {
    let mut channel = Channel::example();
    let filler = Target::Kind(ElementKind::Filler);
    // With the `keep` override the 60 percent filler leaves nothing behind.
    assert!(channel.instances_on("t01", filler, 9_800_000).is_empty());

    channel.cut =
        Cut::parse(&CUT.replace("override t01 filler at=9_800_000 {\n    keep\n}\n", "")).unwrap();
    assert_eq!(
        channel.instances_on("t01", filler, 9_800_000),
        [(Applied::Review(range(9_800_000, 10_400_000)), Layer::Video)]
    );

    channel.cut =
        Cut::parse(&CUT.replace("at=9_800_000 {\n    keep", "at=9_800_000 {\n    cut")).unwrap();
    assert_eq!(
        channel.instances_on("t01", filler, 9_800_000),
        [(Applied::Cut(range(9_800_000, 10_400_000)), Layer::Override)]
    );
}

#[test]
fn an_override_follows_its_element_within_the_tolerance() {
    let mut channel = Channel::example();
    let url = Target::Kind(ElementKind::Url);
    let moved = |by: u64| {
        let mut elements = Elements::new(id("t01"));
        for element in Elements::parse(id("t01"), T01).unwrap().iter() {
            let mut element = element.clone();
            if element.kind == ElementKind::Url {
                element.t = Micros(element.t.0 + by);
                element.to = element.to.map(|to| Micros(to.0 + by));
            }
            elements.push(element).unwrap();
        }
        elements
    };

    channel.elements[0] = moved(300_000);
    let after_small_move = channel.instances_on("t01", url, 22_300_000);
    assert!(after_small_move.iter().any(|(a, layer)| {
        *layer == Layer::Override && matches!(a, Applied::Overlay { range, .. } if *range == range_from(22_300_000, 6_000_000))
    }));
    assert!(channel.lock().take(&id("t01")).unwrap().orphans.is_empty());

    channel.elements[0] = moved(600_000);
    let after_big_move = channel.instances_on("t01", url, 22_600_000);
    let (overlay, layer) = after_big_move
        .iter()
        .find(|(a, _)| matches!(a, Applied::Overlay { .. }))
        .unwrap();
    assert_eq!(*layer, Layer::Brand);
    let Applied::Overlay { params, .. } = overlay else {
        unreachable!()
    };
    assert_eq!(params.len(), 1, "the rule copies only the url");
    let orphans = channel.lock().take(&id("t01")).unwrap().orphans.clone();
    assert_eq!(orphans.len(), 1);
    assert_eq!(
        (orphans[0].kind, orphans[0].at),
        (ElementKind::Url, Micros(22_000_000))
    );
}

fn range_from(from: u64, length: u64) -> TimeRange {
    range(from, from + length)
}

#[test]
fn format_layers_resolve_over_the_preset() {
    let channel = FormatLayer::parse(CHANNEL_FORMAT).unwrap();
    let video = FormatLayer::parse(FORMAT).unwrap();
    let format = FormatKind::Opinion.preset().resolve([&channel, &video]);
    assert_eq!(format.pacing, Pacing::Natural);
    assert_eq!(format.auto_cut.confidence, 85);
    assert_eq!(format.captions, FormatKind::Opinion.preset().captions);

    // A settings-only `Format` stays strict: rules and brand blocks belong to a layer.
    let err = Format::parse(CHANNEL_FORMAT).unwrap_err();
    assert_eq!((err.line, err.column), (13, 1), "{err}");
    assert!(err.message.contains("unknown node `on`"));
    // A full file is also a layer, and comes back the same.
    let preset = FormatKind::Opinion.preset_source();
    assert_eq!(FormatLayer::parse(preset).unwrap().to_kdl(), preset);
}

#[test]
fn grammar_errors_name_the_line() {
    let unknown_effect = FORMAT.replace(
        "    cut breath=250_000 confidence=80",
        "    trim breath=250_000",
    );
    let err = FormatLayer::parse(&unknown_effect).unwrap_err();
    assert_eq!((err.line, err.column), (6, 5), "{err}");

    let wrong_attr = FORMAT.replace("on silence min=500_000", "on silence host=github");
    let err = FormatLayer::parse(&wrong_attr).unwrap_err();
    assert_eq!(err.line, 5, "{err}");
    assert!(err.message.contains("no attribute `host`"), "{err}");

    let min_on_a_moment = CHANNEL_FORMAT.replace("on click {", "on click min=100 {");
    let err = FormatLayer::parse(&min_on_a_moment).unwrap_err();
    assert!(err.message.contains("moment"), "{err}");

    let transition_on_a_url = BRAND.replace("    sfx pop\n", "    transition dip\n");
    let err = Brand::parse(&transition_on_a_url).unwrap_err();
    assert_eq!(err.line, 28, "{err}");

    let twice = CUT.replace(
        "override t02 filler at=30_000_000",
        "override t01 filler at=9_800_000",
    );
    let err = Cut::parse(&twice).unwrap_err();
    assert!(err.message.contains("duplicate override"), "{err}");

    let unknown_take = CUT.replace("override t02 filler", "override t03 filler");
    let err = Cut::parse(&unknown_take).unwrap_err();
    assert_eq!(err.line, 36, "{err}");

    let bad_field = T01.replace("\"host\":\"github\"", "\"domain\":\"github\"");
    let err = Elements::parse(id("t01"), &bad_field).unwrap_err();
    assert_eq!((err.file, err.line), ("elements.jsonl", 6), "{err}");
    assert!(err.message.contains("unknown field `domain`"), "{err}");

    let old_lock = LOCK.replace("lock 1", "lock 7");
    let err = Lock::parse(&old_lock).unwrap_err();
    assert_eq!(err.line, 1, "{err}");
}

/// About 17 minutes of a dense take: words, faces, clicks, OCR and the rest, with the example
/// channel's rules. Style evaluation has to fit inside the 5 s rough-cut budget with room to
/// spare; this asserts well under it and prints the time for the evidence report.
#[test]
fn evaluating_seventeen_minutes_of_elements_is_fast() {
    const LENGTH: u64 = 1_020_000_000;
    let mut channel = Channel::example();
    let take = id("t01");
    let mut elements = Elements::new(take.clone());
    let span = |kind, t: u64, len: u64, by: &str| Element::over(kind, range(t, t + len), by);
    let mut push = |e: Element| elements.push(e).unwrap();
    for t in (0..LENGTH).step_by(400_000) {
        push(
            span(ElementKind::Word, t, 300_000, "whisper")
                .with_text(Attr::Text, "word")
                .with_confidence(95),
        );
    }
    for t in (0..LENGTH).step_by(500_000) {
        push(
            span(ElementKind::Face, t, 500_000, "vision")
                .with_integer(Attr::X, 800)
                .with_integer(Attr::Y, 300)
                .with_integer(Attr::Width, 400)
                .with_integer(Attr::Height, 400),
        );
    }
    for t in (0..LENGTH).step_by(2_000_000) {
        push(
            span(ElementKind::Ocr, t, 1_500_000, "vision")
                .with_text(Attr::Text, "never deploy on Fridays")
                .with_integer(Attr::X, 820)
                .with_integer(Attr::Y, 412)
                .with_integer(Attr::Width, 1_340)
                .with_integer(Attr::Height, 64),
        );
    }
    for t in (100_000..LENGTH).step_by(3_000_000) {
        push(
            Element::at(ElementKind::Click, Micros(t), "events")
                .with_integer(Attr::X, 2210)
                .with_integer(Attr::Y, 640),
        );
    }
    for t in (200_000..LENGTH).step_by(5_000_000) {
        push(span(ElementKind::Silence, t, 700_000, "vad").with_confidence(90));
    }
    for t in (300_000..LENGTH).step_by(6_000_000) {
        push(span(ElementKind::Sentence, t, 5_000_000, "whisper"));
    }
    for t in (250_000..LENGTH).step_by(20_000_000) {
        push(span(ElementKind::Filler, t, 400_000, "whisper").with_confidence(70));
    }
    for t in (350_000..LENGTH).step_by(60_000_000) {
        push(
            span(ElementKind::Url, t, 1_400_000, "transcript")
                .with_text(Attr::Url, "https://github.com/coderscantina/shot-studio")
                .with_text(Attr::Host, "github"),
        );
    }
    let count = elements.len();
    channel.elements = vec![elements];
    channel.cut = Cut::parse(&format!(
        "chapter \"All\" {{\n    take t01 {{\n        keep from=0 to={LENGTH}\n    }}\n}}\n"
    ))
    .unwrap();

    let started = Instant::now();
    let lock = channel.lock();
    let built = started.elapsed();
    let text = lock.to_kdl();
    let written = started.elapsed();
    println!(
        "{count} elements -> {} instances: built in {built:?}, written in {written:?} ({} bytes)",
        lock.takes[0].instances.len(),
        text.len()
    );
    assert!(count > 5_000);
    assert!(written.as_millis() < 500, "{written:?}");
}
