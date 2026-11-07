use shot_core::{
    CaptionStyle, Cut, DerivedKind, Format, FormatKind, Id, Layout, Micros, OutputKind, Project,
    RangeEdit, Script, Timeline, Video,
};

const VIDEO: &str = include_str!("../../../fixtures/examples/video.kdl");
const FORMAT: &str = include_str!("../../../fixtures/examples/format.kdl");
const CUT: &str = include_str!("../../../fixtures/examples/cut.kdl");
const SCRIPT: &str = include_str!("../../../fixtures/examples/script.md");
const CONTRACT: &str = include_str!("../../../docs/contracts/project-files.md");

fn project() -> Project {
    Project {
        video: Video::parse(VIDEO).unwrap(),
        script: Script::parse(SCRIPT).unwrap(),
        format: Format::parse(FORMAT).unwrap(),
        cut: Cut::parse(CUT).unwrap(),
    }
}

/// The contract shows the fixture examples verbatim, so the documented examples are the tested ones.
fn assert_in_contract(example: &str) {
    assert!(
        CONTRACT.contains(example),
        "the contract is missing this example:\n{example}"
    );
}

#[test]
fn video_round_trips() {
    assert_in_contract(VIDEO);
    assert_eq!(Video::parse(VIDEO).unwrap().to_kdl(), VIDEO);
}

#[test]
fn format_round_trips() {
    assert_in_contract(FORMAT);
    assert_eq!(Format::parse(FORMAT).unwrap().to_kdl(), FORMAT);
}

#[test]
fn cut_round_trips() {
    assert_in_contract(CUT);
    assert_eq!(Cut::parse(CUT).unwrap().to_kdl(), CUT);
}

#[test]
fn presets_parse_and_are_canonical() {
    for &kind in FormatKind::ALL {
        assert_eq!(
            kind.preset().to_kdl(),
            kind.preset_source(),
            "{kind} preset"
        );
    }
}

#[test]
fn errors_carry_line_and_column() {
    let unknown = CUT.replace(
        "        zoom 112 from=8_000_000",
        "        zoomm 112 from=8_000_000",
    );
    let err = Cut::parse(&unknown).unwrap_err();
    assert_eq!((err.file, err.line, err.column), ("cut.kdl", 8, 9), "{err}");

    let bad_value = CUT.replace("by=retake", "by=sneeze");
    let err = Cut::parse(&bad_value).unwrap_err();
    assert_eq!((err.line, err.column), (15, 46), "{err}");

    let syntax = VIDEO.replace("status editing", "status \"editing");
    assert_eq!(Video::parse(&syntax).unwrap_err().line, 2);

    let unknown_take = CUT.replace("clip t02", "clip t09");
    assert_eq!(Cut::parse(&unknown_take).unwrap_err().line, 40);
}

#[test]
fn changing_one_chapter_changes_only_its_hashes() {
    let mut project = project();
    let before = Timeline::evaluate(&project);

    let RangeEdit::Remove { range, .. } = project.cut.chapters[1].takes[0].ranges[1] else {
        panic!("the example removes a retake in chapter two");
    };
    project.cut.chapters[1].takes[0].ranges[1] = RangeEdit::Keep(range);
    let after = Timeline::evaluate(&project);

    let hashes = |t: &Timeline, chapter: usize| -> Vec<_> {
        t.main().chapters[chapter]
            .segments
            .iter()
            .map(|s| s.hash)
            .collect()
    };
    assert_eq!(hashes(&before, 0), hashes(&after, 0));
    assert_ne!(hashes(&before, 1), hashes(&after, 1));
    assert_eq!(hashes(&before, 2), hashes(&after, 2));
}

#[test]
fn derived_cut_selects_main_material() {
    let timeline = Timeline::evaluate(&project());
    let short = &timeline.outputs[1];
    assert_eq!(short.kind, OutputKind::Derived(DerivedKind::Short));
    assert_eq!(short.kind.aspect(), (9, 16));
    assert_eq!(short.captions, CaptionStyle::WordPop);

    // t02 16.2 s to 44 s is kept whole in main, so it is one segment.
    let picked: Vec<_> = short.chapters[0]
        .segments
        .iter()
        .map(|s| (s.take.clone(), s.source.from(), s.source.to()))
        .collect();
    let t = |id: &str| Id::new(id).unwrap();
    assert_eq!(
        picked,
        [
            (t("t01"), Micros(5_100_000), Micros(12_000_000)),
            (t("t02"), Micros(16_200_000), Micros(44_000_000)),
        ]
    );
    assert_eq!(short.chapters[0].segments[0].zooms.len(), 1);

    let main_hook = &timeline.main().chapters[0].segments[1];
    assert_eq!(main_hook.source, short.chapters[0].segments[0].source);
    assert_ne!(main_hook.hash, short.chapters[0].segments[0].hash);

    let x_clip = &timeline.outputs[2];
    assert_eq!(x_clip.captions, CaptionStyle::Clean);
    let layouts: Vec<_> = x_clip.chapters[0]
        .segments
        .iter()
        .map(|s| s.layout)
        .collect();
    assert_eq!(layouts, [Layout::Face, Layout::Split]);
}

#[test]
fn script_parses_chapters_beats_and_sources() {
    let script = Script::parse(SCRIPT).unwrap();
    assert_in_contract(SCRIPT);
    assert_eq!(script.title, "Why we deploy on Fridays");
    let titles: Vec<_> = script.chapters.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Hook", "The rule everyone repeats", "What we do instead"]
    );

    let hook = &script.chapters[0].beats[0];
    assert_eq!(hook.line, 5);
    assert_eq!(
        hook.text,
        "Everyone says never deploy on a Friday. We do it every week. the post"
    );
    assert_eq!(hook.sources, ["sources/never-friday.md"]);
    assert_eq!(
        hook.script,
        "Everyone says never deploy on a Friday. We ship every single week, on Friday afternoon.\nAnd nothing burns."
    );

    let rollback = &script.chapters[1].beats[1];
    assert_eq!(rollback.sources.len(), 2);
    assert!(rollback.script.is_empty());

    let err = Script::parse("# Title\n\n- a beat before any chapter\n").unwrap_err();
    assert_eq!((err.file, err.line), ("script.md", 3));
}
