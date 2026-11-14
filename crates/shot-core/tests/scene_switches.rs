//! Scene switches and channel layouts: the contract's `fixtures/examples/scenes/` files, the
//! switches a timeline finds in a take and the chunks they key.

use shot_core::{
    Brand, ContentHash, Cut, FormatKind, FormatLayer, Layout, LayoutName, LayoutSwitch, Lock,
    MediaId, Micros, Project, Script, Segment, SwitchKind, SwitchSound, TimeRange, Timeline, Track,
    Video,
};

const BRAND: &str = include_str!("../../../fixtures/examples/scenes/brand.kdl");
const FORMAT: &str = include_str!("../../../fixtures/examples/scenes/format.kdl");
const CUT: &str = include_str!("../../../fixtures/examples/scenes/cut.kdl");
const CONTRACT: &str = include_str!("../../../docs/contracts/project-files.md");

/// The timeline of a video whose lock applies nothing.
fn evaluate(project: &Project) -> Timeline {
    Timeline::evaluate(project, &Lock::default())
}

fn custom(name: &str) -> Layout {
    Layout::Custom(LayoutName::new(name).unwrap())
}

fn project(cut: &str) -> Project {
    let layer = FormatLayer::parse(FORMAT).unwrap();
    Project {
        video: Video::parse("title \"Scenes\"\nstatus editing\nformat opinion\n").unwrap(),
        script: Script::parse("# Scenes\n").unwrap(),
        format: layer.apply(&FormatKind::Opinion.preset()),
        cut: Cut::parse(cut).unwrap(),
    }
}

fn segments(timeline: &Timeline) -> Vec<&Segment> {
    timeline.main().chapters[0].segments.iter().collect()
}

fn keys(timeline: &Timeline) -> Vec<ContentHash> {
    let main = timeline.main();
    segments(timeline)
        .into_iter()
        .flat_map(|s| {
            let first = s.source.from().0 * 60 / 1_000_000;
            let last = s.source.to().0 * 60 / 1_000_000;
            // Half-second chunks, as export plans them.
            (first..last)
                .step_by(30)
                .map(move |f| main.chunk_hash(s, MediaId::default(), 60, f..(f + 30).min(last)))
        })
        .collect()
}

#[test]
fn the_examples_round_trip_and_are_in_the_contract() {
    for example in [BRAND, FORMAT, CUT] {
        assert!(
            CONTRACT.contains(example),
            "missing from the contract:\n{example}"
        );
    }
    let brand = Brand::parse(BRAND).unwrap();
    assert_eq!(brand.to_kdl(), BRAND);
    assert_eq!(FormatLayer::parse(FORMAT).unwrap().to_kdl(), FORMAT);
    assert_eq!(Cut::parse(CUT).unwrap().to_kdl(), CUT);

    let talk = brand.layout(LayoutName::new("talk-left").unwrap()).unwrap();
    assert_eq!(talk.slots[0].track, Track::Camera);
    assert_eq!(talk.slots[0].focus, [45, 50]);
    let layer = FormatLayer::parse(FORMAT).unwrap();
    assert_eq!(layer.layouts.unwrap()[2], custom("talk-left"));
    assert_eq!(layer.layout_switch.unwrap().sound, Some(SwitchSound::Brand));
}

#[test]
fn a_take_switches_on_every_edge_with_the_written_or_the_format_switch() {
    let timeline = evaluate(&project(CUT));
    let switches: Vec<(u64, Layout, Layout, SwitchKind, u64)> = segments(&timeline)
        .into_iter()
        .filter_map(|s| s.switch.as_ref())
        .map(|s| (s.at.0, s.from, s.to, s.switch.kind, s.window.duration().0))
        .collect();
    let side = custom("side-by-side");
    let talk = custom("talk-left");
    assert_eq!(
        switches,
        [
            (8_000_000, Layout::Face, side, SwitchKind::Dissolve, 400_000),
            (
                20_000_000,
                side,
                Layout::Face,
                SwitchKind::ZoomThrough,
                500_000
            ),
            (24_000_000, Layout::Face, talk, SwitchKind::Morph, 600_000),
            (30_000_000, talk, Layout::Face, SwitchKind::Morph, 600_000),
        ]
    );
}

#[test]
fn a_layout_from_the_first_kept_moment_starts_the_block_without_a_switch() {
    let cut = "chapter \"A\" {\n    take t01 {\n        remove from=0 to=1_000_000 by=manual\n        keep from=1_000_000 to=9_000_000\n        layout split from=0 to=4_000_000\n    }\n}\n";
    let timeline = evaluate(&project(cut));
    let at: Vec<u64> = segments(&timeline)
        .into_iter()
        .filter_map(|s| s.switch.as_ref().map(|s| s.at.0))
        .collect();
    assert_eq!(at, [4_000_000]);
}

#[test]
fn a_switch_keys_only_the_chunks_of_its_window() {
    let before = keys(&evaluate(&project(CUT)));
    let changed = CUT.replace("in dissolve", "in morph");
    let after = keys(&evaluate(&project(&changed)));
    assert_eq!(before.len(), after.len());
    let differ: Vec<usize> = (0..before.len())
        .filter(|&i| before[i] != after[i])
        .collect();
    // The dissolve's 400 ms window at 8 s is the first chunk of the side-by-side segment.
    assert_eq!(differ.len(), 1);

    // A cut draws nothing: its chunks key as if there were no switch.
    let cut = CUT.replace("in dissolve length=400_000", "in cut");
    let hard = Cut::parse(&cut).unwrap().set_switch(
        shot_core::TakeAt {
            chapter: 0,
            take: 0,
        },
        Micros(8_000_000),
        None,
    );
    assert!(hard.is_ok());
    let none = keys(&evaluate(&project(&cut)));
    let mut plain = project(CUT);
    plain.format.layout_switch = Some(LayoutSwitch {
        kind: SwitchKind::Cut,
        ..LayoutSwitch::DEFAULT
    });
    plain.cut.chapters[0].takes[0]
        .layouts
        .iter_mut()
        .for_each(|c| {
            c.enter = None;
            c.exit = None;
        });
    assert_eq!(none.len(), keys(&evaluate(&plain)).len());
    let first = segments(&evaluate(&project(&cut)))[1].clone();
    assert_eq!(first.switch.as_ref().unwrap().switch.kind, SwitchKind::Cut);
    assert!(
        !first
            .switch
            .as_ref()
            .unwrap()
            .draws_over(TimeRange::new(Micros(8_000_000), Micros(8_500_000)).unwrap())
    );
}

#[test]
fn layout_and_switch_mistakes_are_refused_with_their_line() {
    let refused = |text: &str| Cut::parse(text).unwrap_err().to_string();
    let cut = |inner: &str| {
        format!(
            "chapter \"A\" {{\n    take t01 {{\n        keep from=0 to=9_000_000\n{inner}    }}\n}}\n"
        )
    };
    let twice = cut(
        "        layout split from=1_000_000 to=2_000_000 {\n            in dissolve\n            in cut\n        }\n",
    );
    assert!(refused(&twice).contains(":6:"), "{}", refused(&twice));
    let unknown =
        cut("        layout split from=1_000_000 to=2_000_000 {\n            wipe\n        }\n");
    assert!(refused(&unknown).contains("wipe"));
    let kind =
        cut("        layout split from=1_000_000 to=2_000_000 {\n            in wipe\n        }\n");
    assert!(refused(&kind).contains("morph"));

    let brand_refused = |text: &str| Brand::parse(text).unwrap_err().to_string();
    assert!(
        brand_refused("layout split {\n    screen x=0 y=0 width=10 height=10\n}\n")
            .contains("not a layout name")
    );
    assert!(
        brand_refused("layout wide {\n    screen x=3000 y=0 width=1000 height=10\n}\n")
            .contains("inside the 3840x2160 frame")
    );
    assert!(
        brand_refused(
            "layout wide {\n    screen x=0 y=0 width=10 height=10\n    screen x=0 y=0 width=10 height=10\n}\n"
        )
        .contains("placed twice")
    );
    assert!(brand_refused("layout wide {\n}\n").contains("places the screen"));
}

#[test]
fn a_format_layer_adds_its_own_layouts_to_the_brand() {
    let channel = Brand::parse(BRAND).unwrap();
    let layer = FormatLayer::parse(
        "brand {\n    layout talk-left {\n        camera x=0 y=0 width=1920 height=2160\n    }\n    layout zoomed {\n        screen x=0 y=0 width=3840 height=2160\n    }\n}\n",
    )
    .unwrap();
    let merged = channel.merge(layer.brand.as_ref().unwrap());
    let names: Vec<&str> = merged.layouts.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["side-by-side", "talk-left", "zoomed"]);
    let talk = merged
        .layout(LayoutName::new("talk-left").unwrap())
        .unwrap();
    assert_eq!(talk.slots.len(), 1);
}
