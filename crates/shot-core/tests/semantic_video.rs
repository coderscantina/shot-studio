//! The semantic video files of the contract: `fixtures/examples/channel/` is a channel repo
//! with a brand, a format layer and one video whose `cut.lock` is what the style engine builds
//! from the other files.

use std::collections::BTreeMap;
use std::time::Instant;

use shot_core::{
    Applied, Attr, Brand, ContentHash, Cut, Element, ElementKind, Elements, FormatKind,
    FormatLayer, Framing, Id, Layer, Lock, Micros, StyleInput, Target, TimeRange, ZoomShape, build,
};

const BRAND: &str = include_str!("../../../fixtures/examples/channel/brand/brand.kdl");
const DARK: &str = include_str!("../../../fixtures/examples/channel/brand/themes/cantina-dark.kdl");
const THEME: &str =
    include_str!("../../../fixtures/examples/channel/brand/themes/cantina-light.kdl");
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
        let brand = Brand::parse(BRAND)
            .unwrap()
            .with_look(&Brand::parse_theme(DARK).unwrap());
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
        (DARK, Brand::parse_theme(DARK).unwrap().to_kdl()),
        (THEME, Brand::parse_theme(THEME).unwrap().to_kdl()),
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
    // The theme name and the overlay presets are the channel's, not part of the style.
    assert_eq!(channel.brand.theme, Some(id("cantina-dark")));
    assert_eq!(channel.brand.overlay_presets[0].template, id("lower-third"));
    assert!(lock.brand.theme.is_none() && lock.brand.overlay_presets.is_empty());
}

#[test]
fn a_saved_theme_holds_a_look_only() {
    let theme = Brand::parse_theme(THEME).unwrap();
    assert_eq!(
        theme.colors.card,
        Some(shot_core::Color([0xff, 0xff, 0xff]))
    );
    for (node, line) in [
        ("theme cantina-light", 1),
        ("overlay-preset a template=pop length=1_000_000", 1),
        ("on punchline off", 1),
    ] {
        let err = Brand::parse_theme(&format!("{node}\n")).unwrap_err();
        assert_eq!(err.line, line, "{err}");
        assert!(err.message.contains("belongs to the channel"), "{err}");
    }
    // A brand that names a theme keeps no look of its own.
    let inline = BRAND.replace(
        "theme cantina-dark\n",
        "theme cantina-dark\ncolors accent=\"#000000\"\n",
    );
    let err = Brand::parse(&inline).unwrap_err();
    assert_eq!(err.line, 3, "{err}");
    assert!(err.message.contains("brand/themes/"), "{err}");
    let no_length = BRAND.replace(" length=5_000_000 {", " {");
    let err = Brand::parse(&no_length).unwrap_err();
    assert!(err.message.contains("length"), "{err}");
    let twice = BRAND.replace(
        "overlay-preset speaker",
        "overlay-preset speaker template=pop length=1_000_000\noverlay-preset speaker",
    );
    let err = Brand::parse(&twice).unwrap_err();
    assert!(err.message.contains("duplicate overlay preset"), "{err}");
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
                center: None,
                shape: Default::default(),
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
    assert_eq!(format.captions, FormatKind::Opinion.preset().captions);

    // The settings from before style rules are refused, with what to do instead.
    let err = FormatLayer::parse(&format!("prompter beats\npacing tight\n{FORMAT}")).unwrap_err();
    assert_eq!((err.line, err.column), (2, 1), "{err}");
    assert!(
        err.message.contains("`pacing` is from before style rules"),
        "{err}"
    );
    // A preset is also a layer, and comes back the same.
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
    assert_eq!((err.line, err.column), (5, 5), "{err}");

    let wrong_attr = FORMAT.replace("on silence min=500_000", "on silence host=github");
    let err = FormatLayer::parse(&wrong_attr).unwrap_err();
    assert_eq!(err.line, 4, "{err}");
    assert!(err.message.contains("no attribute `host`"), "{err}");

    let min_on_a_moment = CHANNEL_FORMAT.replace("on click {", "on click min=100 {");
    let err = FormatLayer::parse(&min_on_a_moment).unwrap_err();
    assert!(err.message.contains("moment"), "{err}");

    let transition_on_a_url = BRAND.replace("    sfx pop\n", "    transition dip\n");
    let err = Brand::parse(&transition_on_a_url).unwrap_err();
    assert_eq!(err.line, 27, "{err}");

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

/// Elements of take `t01` in the first chapter's kept 0 to 12 s, one JSON line each.
fn t01_elements(lines: &[String]) -> Vec<Elements> {
    vec![Elements::parse(id("t01"), &format!("{}\n", lines.join("\n"))).unwrap()]
}

fn click(t: u64) -> String {
    format!(r#"{{"kind":"click","t":{t},"by":"events","x":100,"y":100}}"#)
}

fn silence(t: u64, length: u64) -> String {
    format!(
        r#"{{"kind":"silence","t":{t},"to":{},"by":"vad@silero-5"}}"#,
        t + length
    )
}

/// The starts of the instances an effect named `node` has in take `t01`.
fn starts(channel: &Channel, node: fn(&Applied) -> Option<u64>) -> Vec<u64> {
    let lock = channel.lock();
    lock.take(&id("t01"))
        .unwrap()
        .instances
        .iter()
        .filter_map(|i| node(&i.applied))
        .collect()
}

fn sfx_at(applied: &Applied) -> Option<u64> {
    match applied {
        Applied::Sfx { at, .. } if at.0 < 12_000_000 && at.0 != 0 => Some(at.0),
        _ => None,
    }
}

fn zoom_from(applied: &Applied) -> Option<u64> {
    match applied {
        Applied::Zoom { range, .. } => Some(range.from().0),
        _ => None,
    }
}

#[test]
fn every_selects_the_first_and_each_nth_instance_in_time_order() {
    let mut channel = Channel::example();
    channel.elements = t01_elements(&[
        click(1_000_000),
        click(2_000_000),
        click(3_000_000),
        click(4_000_000),
        click(5_000_000),
    ]);
    for (every, expected) in [
        (1, vec![1, 2, 3, 4, 5]),
        (2, vec![1, 3, 5]),
        (3, vec![1, 4]),
        (9, vec![1]),
    ] {
        channel.video =
            FormatLayer::parse(&format!("on click every={every} {{\n    sfx pop\n}}\n")).unwrap();
        let seconds: Vec<u64> = starts(&channel, sfx_at)
            .iter()
            .map(|t| t / 1_000_000)
            .collect();
        assert_eq!(seconds, expected, "every={every}");
    }
}

#[test]
fn every_counts_after_the_other_filters() {
    let mut channel = Channel::example();
    // The 0.3 s silence fails `min`, so it is not counted: the 1st, 3rd of the three left.
    channel.elements = t01_elements(&[
        silence(1_000_000, 900_000),
        silence(3_000_000, 300_000),
        silence(5_000_000, 800_000),
        silence(7_000_000, 900_000),
    ]);
    channel.video =
        FormatLayer::parse("on silence min=500_000 every=2 {\n    sfx pop\n}\n").unwrap();
    assert_eq!(starts(&channel, sfx_at), [1_000_000, 7_000_000]);
}

#[test]
fn the_talking_presets_punch_in_on_every_other_silence() {
    let mut channel = Channel::example();
    channel.elements = t01_elements(&[
        silence(1_000_000, 900_000),
        silence(3_000_000, 300_000),
        silence(5_000_000, 700_000),
        silence(7_000_000, 900_000),
    ]);
    channel.video = FormatLayer::default();
    for kind in FormatKind::ALL {
        channel.format = kind.preset_layer();
        let expected: &[u64] = match kind {
            FormatKind::Reaction | FormatKind::Opinion => &[1_000_000, 7_000_000],
            FormatKind::ProductTour | FormatKind::Tutorial => &[],
        };
        assert_eq!(starts(&channel, zoom_from), expected, "{kind}");
        assert_eq!(channel.lock().to_kdl(), channel.lock().to_kdl());
    }
}

#[test]
fn a_zoom_node_keeps_its_framing_and_ramps_and_writes_them_back() {
    let line = "zoom 200 from=2_000_000 to=6_000_000 framing=top-right ramp-in=300_000 ramp-out=0";
    assert_in_contract(line);
    let text = format!(
        "chapter \"Intro\" {{\n    take t01 {{\n        keep from=0 to=20_000_000\n        {line}\n        zoom 150 from=6_000_000 to=9_000_000 x=10 y=20 framing=center\n    }}\n}}\n"
    );
    let cut = Cut::parse(&text).unwrap();
    let zooms = &cut.chapters[0].takes[0].zooms;
    assert_eq!(
        zooms[0].shape,
        ZoomShape {
            framing: Framing::TopRight,
            ramp_in: Some(Micros(300_000)),
            ramp_out: Some(Micros(0)),
        }
    );
    assert_eq!(zooms[1].shape.framing, Framing::Center);
    assert_eq!(zooms[1].shape.ramp_in, None);
    assert_eq!(cut.to_kdl(), text);

    let unknown = text.replace("framing=center", "framing=middle");
    let err = Cut::parse(&unknown).unwrap_err();
    assert_eq!(err.line, 5, "{err}");
}

#[test]
fn a_zoom_effect_carries_framing_and_ramps_into_the_lock() {
    let mut channel = Channel::example();
    let rule = "on emphasis {\n    zoom 120 framing=bottom-left ramp-in=200_000\n}\n";
    channel.video = FormatLayer::parse(&format!("{FORMAT}{rule}")).unwrap();
    assert!(channel.video.to_kdl().ends_with(rule));
    let shape = ZoomShape {
        framing: Framing::BottomLeft,
        ramp_in: Some(Micros(200_000)),
        ramp_out: None,
    };
    let emphasis = channel.instances_on("t01", Target::Kind(ElementKind::Emphasis), 8_000_000);
    assert!(matches!(emphasis[0].0, Applied::Zoom { shape: s, .. } if s == shape));
    let lock = channel.lock().to_kdl();
    assert!(
        lock.contains("framing=bottom-left ramp-in=200_000 on=emphasis"),
        "{lock}"
    );
    assert_eq!(Lock::parse(&lock).unwrap().to_kdl(), lock);
}

#[test]
fn the_camera_node_sets_its_frame_and_hand_held_strength_apart() {
    let both = "camera shape=rounded corners=24 border=0 shadow=soft handheld=40\n";
    let brand = Brand::parse(both).unwrap();
    assert_eq!(brand.handheld, Some(40));
    assert!(brand.camera.is_some());
    assert_eq!(brand.to_kdl(), both);

    let alone = "camera handheld=30\n";
    let video = Brand::parse(alone).unwrap();
    assert_eq!((video.camera, video.handheld), (None, Some(30)));
    assert_eq!(video.to_kdl(), alone);
    // The video's strength replaces the channel's, and the channel's frame stays.
    let merged = brand.merge(&video);
    assert_eq!((merged.camera, merged.handheld), (brand.camera, Some(30)));

    assert!(Brand::parse("camera shape=rounded corners=24\n").is_err());
    assert!(Brand::parse("camera handheld=101\n").is_err());
    assert!(Brand::parse("camera\n").is_err());
    let layer = FormatLayer::parse("brand {\n    camera handheld=0\n}\n").unwrap();
    assert_eq!(layer.brand.unwrap().handheld, Some(0));
}

#[test]
fn the_camera_node_cuts_the_person_out_on_its_own_or_with_the_frame() {
    let channel = "camera shape=circle corners=0 border=8 shadow=soft handheld=20 cutout=#true\n";
    let brand = Brand::parse(channel).unwrap();
    assert_eq!(brand.cutout, Some(true));
    assert_eq!(brand.to_kdl(), channel);

    // A video turns it off in its format layer and keeps the channel's frame and drift.
    let layer = FormatLayer::parse("brand {\n    camera cutout=#false\n}\n").unwrap();
    let video = layer.brand.unwrap();
    assert_eq!((video.camera, video.cutout), (None, Some(false)));
    let merged = brand.merge(&video);
    assert_eq!(
        (merged.camera, merged.handheld, merged.cutout),
        (brand.camera, Some(20), Some(false))
    );
    assert_eq!(
        Brand::parse("camera cutout=#false\n").unwrap().to_kdl(),
        "camera cutout=#false\n"
    );
    assert!(Brand::parse("camera cutout=1\n").is_err());
}

#[test]
fn the_nearest_layer_naming_a_theme_picks_the_look_and_its_brand_block_lays_over_it() {
    let mut channel = Channel::example();
    let file = Brand::parse(BRAND).unwrap();
    channel.format.theme = Some(id("cantina-light"));
    let picked = shot_core::picked_theme(&file, &channel.format, &channel.video).cloned();
    assert_eq!(picked, Some(id("cantina-light")));
    channel.brand = file.with_look(&Brand::parse_theme(THEME).unwrap());
    let lock = channel.lock();
    // The light theme replaces the dark look: its card and bundled fonts, no font file.
    assert_eq!(lock.brand.colors.card, Some(shot_core::Color([0xff; 3])));
    assert_eq!(
        lock.brand.fonts.heading,
        Some(shot_core::Font::Bundled(shot_core::BundledFont::Inter))
    );
    assert!(lock.assets.iter().all(|a| !a.path.contains("fonts/")));
    // The video's brand block lays over the theme the layer below picked.
    assert_eq!(
        lock.brand.colors.accent,
        Some(shot_core::Color([0xff, 0x7a, 0x59]))
    );
    // A video naming a theme is nearer than its channel format.
    channel.video.theme = Some(id("cantina-dark"));
    let picked = shot_core::picked_theme(&file, &channel.format, &channel.video);
    assert_eq!(picked, Some(&id("cantina-dark")));
}

#[test]
fn a_rule_naming_a_preset_writes_its_overlay_into_the_lock() {
    let mut channel = Channel::example();
    let rule = "on chapter-end {\n    overlay preset=speaker\n}\n";
    channel.video = FormatLayer::parse(&format!("{FORMAT}{rule}")).unwrap();
    assert!(channel.video.to_kdl().ends_with(rule), "it writes back");
    let lock = channel.lock();
    let placed: Vec<_> = lock
        .takes
        .iter()
        .flat_map(|t| &t.instances)
        .filter(|i| i.element.target == Target::ChapterEnd)
        .filter_map(|i| match &i.applied {
            Applied::Overlay {
                template,
                range,
                params,
            } => Some((
                template.clone(),
                range.to().0 - range.from().0,
                params.len(),
            )),
            _ => None,
        })
        .collect();
    assert!(!placed.is_empty());
    assert!(
        placed
            .iter()
            .all(|p| *p == (id("lower-third"), 5_000_000, 3))
    );

    channel.video = FormatLayer::parse(&rule.replace("speaker", "host")).unwrap();
    let err = build(&channel.input()).unwrap_err().to_string();
    assert!(err.contains("`host`"), "{err}");
}
