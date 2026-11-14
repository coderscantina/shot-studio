use shot_core::{
    CaptionFix, CaptionStyle, Color, ContentHash, CursorStyle, Cut, DerivedKind, DeviceFrame,
    Format, FormatKind, FormatLayer, Framing, Id, Layout, Lock, MediaId, Micros, Output,
    OutputKind, Point, Project, RangeEdit, Script, Segment, Shadow, TakeAt, TimeRange, Timeline,
    Transcript, Video, WindowStyle, Word,
};

const VIDEO: &str = include_str!("../../../fixtures/examples/video.kdl");
const FORMAT: &str = include_str!("../../../fixtures/examples/format.kdl");
const CUT: &str = include_str!("../../../fixtures/examples/cut.kdl");
const SCRIPT: &str = include_str!("../../../fixtures/examples/script.md");
const TOUR_FORMAT: &str = include_str!("../../../fixtures/examples/tour/format.kdl");
const TOUR_CUT: &str = include_str!("../../../fixtures/examples/tour/cut.kdl");
const CONTRACT: &str = include_str!("../../../docs/contracts/project-files.md");

/// The settings of `text` over the preset of `kind`.
fn settings(kind: FormatKind, text: &str) -> Format {
    kind.preset().resolve([&FormatLayer::parse(text).unwrap()])
}

/// The timeline of a video whose lock applies nothing.
fn evaluate(project: &Project) -> Timeline {
    Timeline::evaluate(project, &Lock::default())
}

fn project() -> Project {
    Project {
        video: Video::parse(VIDEO).unwrap(),
        script: Script::parse(SCRIPT).unwrap(),
        format: settings(FormatKind::Opinion, FORMAT),
        cut: Cut::parse(CUT).unwrap(),
    }
}

/// The product tour of the contract's second example.
fn tour() -> Project {
    Project {
        video: Video::parse("title \"Search filters\"\nstatus editing\nformat product-tour\n")
            .unwrap(),
        script: Script::parse("# Search filters\n").unwrap(),
        format: settings(FormatKind::ProductTour, TOUR_FORMAT),
        cut: Cut::parse(TOUR_CUT).unwrap(),
    }
}

/// The key of every main segment with its layout, in output order.
fn main_keys(timeline: &Timeline) -> Vec<(Layout, ContentHash)> {
    let main = timeline.main();
    main.chapters
        .iter()
        .flat_map(|c| &c.segments)
        .map(|s| (s.layout, key(main, s, MediaId::default())))
        .collect()
}

fn derived_keys(timeline: &Timeline, index: usize) -> Vec<ContentHash> {
    let output = &timeline.outputs[index];
    output.chapters[0]
        .segments
        .iter()
        .map(|s| key(output, s, MediaId::default()))
        .collect()
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
    assert_eq!(FormatLayer::parse(FORMAT).unwrap().to_kdl(), FORMAT);
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
            kind.preset_layer().to_kdl(),
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
    assert_eq!(
        (err.file, err.line, err.column),
        ("cut.kdl", 13, 9),
        "{err}"
    );

    let bad_value = CUT.replace("33_000_000 by=manual", "33_000_000 by=sneeze");
    let err = Cut::parse(&bad_value).unwrap_err();
    assert_eq!((err.line, err.column), (20, 46), "{err}");

    let syntax = VIDEO.replace("status editing", "status \"editing");
    assert_eq!(Video::parse(&syntax).unwrap_err().line, 2);

    let unknown_take = CUT.replace("clip t02", "clip t09");
    assert_eq!(Cut::parse(&unknown_take).unwrap_err().line, 43);
}

#[test]
fn a_file_from_a_newer_version_is_refused_with_both_versions() {
    fn check<T>(text: &str, parse: fn(&str) -> Result<T, shot_core::ParseError>) {
        // The new build's nodes after `version` must not be reported as unknown first.
        let err = parse(&format!("version 99\n{text}future-node 1\n"))
            .err()
            .expect("refused");
        assert_eq!((err.line, err.column), (1, 1), "{err}");
        assert!(
            err.message.contains("version 99") && err.message.contains("version 1"),
            "{err}"
        );
        assert!(parse(&format!("version 1\n{text}")).is_ok());
        assert!(parse(&format!("version 0\n{text}")).is_err());
    }
    check(VIDEO, Video::parse);
    check(FORMAT, FormatLayer::parse);
    check(CUT, Cut::parse);
    check("colors accent=\"#ff7a59\"\n", shot_core::Brand::parse);
    // Version 1 is the default, so the canonical form leaves it out.
    assert_eq!(
        Cut::parse(&format!("version 1\n{CUT}")).unwrap().to_kdl(),
        CUT
    );
}

#[test]
fn tour_examples_round_trip() {
    assert_in_contract(TOUR_FORMAT);
    assert_in_contract(TOUR_CUT);
    assert_eq!(
        FormatLayer::parse(TOUR_FORMAT).unwrap().to_kdl(),
        TOUR_FORMAT
    );
    assert_eq!(Cut::parse(TOUR_CUT).unwrap().to_kdl(), TOUR_CUT);
}

#[test]
fn old_fix_zoom_and_effect_nodes_report_their_line() {
    let rule_cut = CUT.replace("33_000_000 by=manual", "33_000_000 by=retake");
    let err = Cut::parse(&rule_cut).unwrap_err();
    assert_eq!((err.line, err.column), (20, 9), "{err}");
    assert!(
        err.message
            .contains("`remove by=retake` is from before style rules"),
        "{err}"
    );
    let review = CUT.replace(
        "        zoom 112 from=8_000_000",
        "        review from=9_800_000 to=10_400_000 by=filler\n        zoom 112 from=8_000_000",
    );
    assert_eq!(Cut::parse(&review).unwrap_err().line, 13);
    let rule_zoom = TOUR_CUT.replace("y=640", "y=640 by=click");
    assert!(
        Cut::parse(&rule_zoom)
            .unwrap_err()
            .message
            .contains("`zoom by=click`")
    );

    let fix_twice = CUT.replace("fix \"jeep UI\" \"gpui\"", "fix \"LARA VEL\" \"gpui\"");
    assert_eq!(Cut::parse(&fix_twice).unwrap_err().line, 5);

    let half_center = TOUR_CUT.replace(" y=640", "");
    assert_eq!(Cut::parse(&half_center).unwrap_err().line, 4);

    let bad_color = TOUR_FORMAT.replace("\"#141821\"", "\"#14182\"");
    let err = FormatLayer::parse(&bad_color).unwrap_err();
    assert_eq!((err.line, err.column), (7, 42), "{err}");
}

#[test]
fn caption_fixes_change_only_burned_in_chunks() {
    let mut project = project();
    let before = evaluate(&project);
    project.cut.caption_fixes.push(CaptionFix {
        heard: "jee pee you eye".to_owned(),
        said: "gpui".to_owned(),
    });
    let after = evaluate(&project);

    assert_eq!(main_keys(&before), main_keys(&after));
    assert_ne!(derived_keys(&before, 1), derived_keys(&after, 1));
    assert_ne!(derived_keys(&before, 2), derived_keys(&after, 2));

    // With captions off nothing is burned in, so the fixes do not count.
    let mut off = project.clone();
    off.cut.derived[1].captions = Some(CaptionStyle::Off);
    let off_before = evaluate(&off);
    off.cut.caption_fixes.clear();
    assert_eq!(
        derived_keys(&off_before, 2),
        derived_keys(&evaluate(&off), 2)
    );
}

#[test]
fn cursor_settings_reach_only_chunks_that_show_the_screen() {
    let mut project = project();
    let before = main_keys(&evaluate(&project));
    project.format.cursor = Some(CursorStyle {
        size: 140,
        smoothing: Micros(120_000),
        ripple: true,
    });
    let after = main_keys(&evaluate(&project));

    let layouts: Vec<Layout> = before.iter().map(|(l, _)| *l).collect();
    assert!(layouts.contains(&Layout::Face) && layouts.contains(&Layout::Split));
    for ((layout, old), (_, new)) in before.iter().zip(&after) {
        assert_eq!(old == new, !layout.shows_screen(), "{layout}");
    }
}

#[test]
fn window_style_reaches_only_the_window_layout() {
    let style = WindowStyle {
        padding: 96,
        corners: 0,
        shadow: Shadow::Hard,
        background: Color([0, 0, 0]),
        frame: DeviceFrame::Macbook,
    };
    let mut opinion = project();
    let before = main_keys(&evaluate(&opinion));
    opinion.format.window = Some(style);
    assert_eq!(before, main_keys(&evaluate(&opinion)));

    let mut tour = tour();
    let before = main_keys(&evaluate(&tour));
    assert_eq!(before[0].0, Layout::Window);
    tour.format.window = Some(style);
    assert_ne!(before, main_keys(&evaluate(&tour)));
}

#[test]
fn zoom_center_shape_and_motion_are_hashed() {
    let tour = tour();
    let before = main_keys(&evaluate(&tour));

    let mut moved = tour.clone();
    moved.cut.chapters[0].takes[0].zooms[0].center = Some(Point { x: 10, y: 10 });
    assert_ne!(before, main_keys(&evaluate(&moved)));

    let mut instant = tour;
    instant.format.zoom_motion = None;
    let instant_keys = main_keys(&evaluate(&instant));
    assert_ne!(before, instant_keys);

    // Framing and ramps are hashed where set; a default shape leaves every key as it was.
    let mut framed = instant.clone();
    framed.cut.chapters[0].takes[0].zooms[0].shape.framing = Framing::TopLeft;
    assert_ne!(instant_keys, main_keys(&evaluate(&framed)));
    let mut ramped = instant;
    ramped.cut.chapters[0].takes[0].zooms[2].shape.ramp_in = Some(Micros(300_000));
    let ramped_keys = main_keys(&evaluate(&ramped));
    assert_ne!(instant_keys, ramped_keys);
}

#[test]
fn the_music_bed_changes_no_hashes() {
    let mut project = project();
    let before = evaluate(&project);
    project.cut.music = None;
    let after = evaluate(&project);
    assert_eq!(main_keys(&before), main_keys(&after));
    assert_eq!(derived_keys(&before, 1), derived_keys(&after, 1));
}

#[test]
fn changing_one_chapter_changes_only_its_hashes() {
    let mut project = project();
    let before = evaluate(&project);

    let RangeEdit::Remove(range) = project.cut.chapters[1].takes[0].ranges[1] else {
        panic!("the example removes a retake in chapter two");
    };
    project.cut.chapters[1].takes[0].ranges[1] = RangeEdit::Keep(range);
    let after = evaluate(&project);

    assert_eq!(chapter_keys(&before, 0), chapter_keys(&after, 0));
    assert_ne!(chapter_keys(&before, 1), chapter_keys(&after, 1));
    assert_eq!(chapter_keys(&before, 2), chapter_keys(&after, 2));
}

#[test]
fn a_take_recorded_again_under_its_old_id_gets_new_keys() {
    let timeline = evaluate(&project());
    let main = timeline.main();
    let segment = &main.chapters[0].segments[0];
    let old = MediaId::of(b"screen.mp4 1000 bytes, modified at 1");
    let new = MediaId::of(b"screen.mp4 1200 bytes, modified at 2");
    assert_eq!(key(main, segment, old), key(main, segment, old));
    assert_ne!(key(main, segment, old), key(main, segment, new));
}

#[test]
fn derived_cut_selects_main_material() {
    let timeline = evaluate(&project());
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

    // The same frames key apart in the main cut and the Short.
    let clip = &short.chapters[0].segments[0];
    let main_hook = Segment {
        source: clip.source,
        ..timeline.main().chapters[0].segments[0].clone()
    };
    let media = MediaId::default();
    assert_ne!(
        key(timeline.main(), &main_hook, media),
        key(short, clip, media)
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
fn text_between_a_chapter_heading_and_its_first_beat_is_the_chapter_note() {
    let script = Script::parse(
        "# T\n\n## One\nWhy this matters\nto us.\n\nSecond paragraph.\n\n- a beat\nits script\n",
    )
    .unwrap();
    let chapter = &script.chapters[0];
    assert_eq!(chapter.note, "Why this matters to us.\nSecond paragraph.");
    assert_eq!(chapter.beats[0].script, "its script");
    assert!(Script::parse("# T\nloose text\n## One\n").is_err());
}

#[test]
fn deleting_words_changes_only_the_hashes_of_its_chapter() {
    let mut project = project();
    let before = evaluate(&project);

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
    let after = evaluate(&project);

    assert_eq!(chapter_keys(&before, 0), chapter_keys(&after, 0));
    assert_ne!(chapter_keys(&before, 1), chapter_keys(&after, 1));
    assert_eq!(chapter_keys(&before, 2), chapter_keys(&after, 2));
}

/// The caption style examples of the contract: a brand's look, a video's look over it, and a
/// person's line breaks.
const BRAND_CAPTIONS: &str = "captions font=body size=110 weight=800 color=\"#ffffff\" highlight=\"#ffd84d\" back=outline position=middle nudge=-5 words=4 case=upper\n";
const FORMAT_CAPTIONS: &str = "captions karaoke size=120 position=top\n";
const CAPTION_BREAKS: &str = "caption-fixes {\n    fix \"lara vel\" \"Laravel\"\n    split t01 at=12_400_000\n    join t01 at=15_100_000\n}\n";

#[test]
fn caption_looks_and_line_breaks_round_trip() {
    for example in [BRAND_CAPTIONS, FORMAT_CAPTIONS, CAPTION_BREAKS] {
        assert_in_contract(example);
    }
    let brand = shot_core::Brand::parse(BRAND_CAPTIONS).unwrap();
    assert_eq!(brand.to_kdl(), BRAND_CAPTIONS);
    assert_eq!(brand.captions.words, Some(4));
    let format = FORMAT.replace("captions word-pop\n", FORMAT_CAPTIONS);
    assert_eq!(FormatLayer::parse(&format).unwrap().to_kdl(), format);
    let parsed = settings(FormatKind::Opinion, &format);
    assert_eq!(parsed.captions, CaptionStyle::Karaoke);
    let design = brand
        .captions
        .merge(&parsed.caption_look)
        .design(parsed.captions);
    assert_eq!(
        (design.size, design.weight),
        (120, 800),
        "the video's size over the brand's"
    );
    let cut = Cut::parse(CAPTION_BREAKS).unwrap();
    assert_eq!(cut.caption_breaks.len(), 2);
    assert!(cut.caption_breaks[0].split && !cut.caption_breaks[1].split);
    assert_eq!(cut.to_kdl(), CAPTION_BREAKS);
    let twice = CAPTION_BREAKS.replace("join t01 at=15_100_000", "join t01 at=12_400_000");
    assert!(Cut::parse(&twice).is_err(), "one break per word");
}

#[test]
fn a_caption_look_or_line_break_changes_only_burned_in_chunks() {
    let mut project = project();
    let before = evaluate(&project);
    project.format.caption_look.size = Some(130);
    let after = evaluate(&project);
    assert_eq!(main_keys(&before), main_keys(&after));
    assert_ne!(derived_keys(&before, 1), derived_keys(&after, 1));
    project.cut.caption_breaks.push(shot_core::CaptionBreak {
        take: Id::new("t01").unwrap(),
        at: Micros(1_000_000),
        split: true,
    });
    let split = evaluate(&project);
    assert_eq!(main_keys(&after), main_keys(&split));
    assert_ne!(derived_keys(&after, 1), derived_keys(&split, 1));
}

/// A talking-head video that records no screen and no system audio.
const RECORD_LAYER: &str = "record screen=#false camera=#true mic=#true system-audio=#false\n";

#[test]
fn the_record_node_round_trips_and_switches_tracks_per_video() {
    assert_in_contract(RECORD_LAYER);
    let layer = shot_core::FormatLayer::parse(RECORD_LAYER).unwrap();
    assert_eq!(layer.to_kdl(), RECORD_LAYER);
    let channel = settings(FormatKind::Opinion, FORMAT);
    assert_eq!(channel.inputs(), shot_core::Inputs::default());
    let video = channel.resolve([&layer]);
    let inputs = video.inputs();
    assert!(!inputs.screen && inputs.camera && inputs.mic && !inputs.system_audio);
    let full = video.to_kdl();
    assert!(
        full.ends_with(RECORD_LAYER),
        "written after the settings:\n{full}"
    );
    assert_eq!(settings(FormatKind::Opinion, &full), video);
    assert!(shot_core::FormatLayer::parse("record screen=#false\n").is_err());
}

#[test]
fn a_take_without_a_track_falls_back_to_the_layout_it_can_draw() {
    assert_eq!(Layout::FaceContent.for_tracks(true, false), Layout::Content);
    assert_eq!(Layout::Split.for_tracks(true, false), Layout::Content);
    assert_eq!(Layout::Window.for_tracks(true, false), Layout::Window);
    assert_eq!(Layout::Split.for_tracks(false, true), Layout::Face);
    assert_eq!(Layout::Window.for_tracks(false, true), Layout::Face);
    assert_eq!(Layout::Split.for_tracks(true, true), Layout::Split);
    assert_eq!(Layout::Split.for_tracks(false, false), Layout::Split);
}

const THUMBNAIL: &str = include_str!("../../../fixtures/examples/thumbnail.kdl");

#[test]
fn the_thumbnail_design_round_trips_and_names_its_upload() {
    use shot_core::thumbnail::{FaceStyle, StickerShape, ThumbnailDesign, ThumbnailPreset};
    assert_in_contract(THUMBNAIL);
    let design = ThumbnailDesign::parse(THUMBNAIL).unwrap();
    assert_eq!(design.to_kdl(), THUMBNAIL);
    let upload = design.uploaded().unwrap();
    assert_eq!(upload.preset, ThumbnailPreset::BigFace);
    assert_eq!(upload.face, FaceStyle::Off);
    assert_eq!(upload.stickers[0].shape, StickerShape::Emoji);
    assert_eq!(upload.stickers[0].emoji.as_deref(), Some("🔥"));
    assert_eq!(design.free_name().as_str(), "c");
    assert_eq!(
        ThumbnailDesign::parse("").unwrap(),
        ThumbnailDesign::default()
    );
    for broken in [
        "upload z\nvariant a preset=screen {\n}\n",
        "variant a preset=screen {\n}\n",
        "upload a\nvariant a preset=screen {\n    headline \"1\" \"2\" \"3\" color=text style=outline\n}\n",
        "upload a\nvariant a preset=screen {\n    sticker emoji x=1 y=1 size=50 turn=0\n}\n",
        "upload a\nvariant a preset=screen {\n    background screen dim=95\n}\n",
    ] {
        assert!(ThumbnailDesign::parse(broken).is_err(), "{broken}");
    }
}
