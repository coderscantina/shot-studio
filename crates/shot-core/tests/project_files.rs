use shot_core::{
    CaptionStyle, ContentHash, Cut, DerivedKind, Format, FormatKind, Id, Layout, MediaId, Micros,
    Output, OutputKind, Project, RangeEdit, Script, Segment, TakeAt, TimeRange, Timeline,
    Transcript, Video, Word,
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

/// The cache key of a whole segment at 60 fps.
fn key(output: &Output, segment: &Segment, media: MediaId) -> ContentHash {
    let frame = |t: Micros| t.0 * 60 / 1_000_000;
    let frames = frame(segment.source.from())..frame(segment.source.to());
    output.chunk_hash(segment, media, 60, frames)
}

fn chapter_keys(timeline: &Timeline, chapter: usize) -> Vec<ContentHash> {
    let main = timeline.main();
    main.chapters[chapter]
        .segments
        .iter()
        .map(|s| key(main, s, MediaId::default()))
        .collect()
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

    assert_eq!(chapter_keys(&before, 0), chapter_keys(&after, 0));
    assert_ne!(chapter_keys(&before, 1), chapter_keys(&after, 1));
    assert_eq!(chapter_keys(&before, 2), chapter_keys(&after, 2));
}

#[test]
fn a_take_recorded_again_under_its_old_id_gets_new_keys() {
    let timeline = Timeline::evaluate(&project());
    let main = timeline.main();
    let segment = &main.chapters[0].segments[0];
    let old = MediaId::of(b"screen.mp4 1000 bytes, modified at 1");
    let new = MediaId::of(b"screen.mp4 1200 bytes, modified at 2");
    assert_eq!(key(main, segment, old), key(main, segment, old));
    assert_ne!(key(main, segment, old), key(main, segment, new));
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
    let media = MediaId::default();
    assert_ne!(
        key(timeline.main(), main_hook, media),
        key(short, &short.chapters[0].segments[0], media)
    );

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

#[test]
fn deleting_words_changes_only_the_hashes_of_its_chapter() {
    let mut project = project();
    let before = Timeline::evaluate(&project);

    let mut transcript = Transcript::new(Id::new("t01").unwrap());
    for start in (12_000_000..13_000_000).step_by(500_000) {
        let range = TimeRange::new(Micros(start), Micros(start + 400_000)).unwrap();
        let word = Word {
            text: "word".to_owned(),
            range,
            confidence: 90,
        };
        transcript.push(word).unwrap();
    }
    let at = TakeAt {
        chapter: 1,
        take: 0,
    };
    let selection = TimeRange::new(Micros(12_000_000), Micros(12_100_000)).unwrap();
    project.cut = project
        .cut
        .delete_words(at, &transcript, selection)
        .unwrap();
    let after = Timeline::evaluate(&project);

    assert_eq!(chapter_keys(&before, 0), chapter_keys(&after, 0));
    assert_ne!(chapter_keys(&before, 1), chapter_keys(&after, 1));
    assert_eq!(chapter_keys(&before, 2), chapter_keys(&after, 2));
}
