//! Migrating a video written before the lock, and building it from the lock afterwards.

use std::collections::BTreeMap;

use shot_core::{
    Brand, Cut, Elements, Format, FormatKind, FormatLayer, Id, MediaId, Micros, MigrateInput,
    Project, Script, StyleInput, Timeline, Video, build, has_rule_nodes, migrate,
};

const OLD_CUT: &str = r#"chapter "One" {
    take t01 {
        remove from=0 to=750_000 by=silence
        keep from=750_000 to=4_050_000
        remove from=4_050_000 to=5_950_000 by=silence
        keep from=5_950_000 to=20_000_000
        remove from=20_000_000 to=21_000_000 by=filler
        keep from=21_000_000 to=27_050_000
        remove from=27_050_000 to=30_000_000 by=silence
        ignore from=15_000_000 to=15_400_000 by=filler
        review from=17_000_000 to=17_300_000 by=filler
        zoom 112 from=5_950_000 to=12_000_000 by=jump-cut
        overlay lower-third from=1_000_000 to=3_000_000 {
            text "Coder's Cantina"
        }
        overlay link-card from=8_000_000 to=14_000_000 {
            url "https://github.com/coderscantina/shot-studio"
        }
    }
}
"#;

/// The migrated cut: the material and the person's cut and overlay, an override that places
/// the jump-cut zoom and the `ignore` as `keep`. The filler removal no element explains stays
/// as a person's cut.
const NEW_CUT: &str = r#"chapter "One" {
    take t01 {
        keep from=0 to=20_000_000
        remove from=20_000_000 to=21_000_000 by=manual
        keep from=21_000_000 to=30_000_000
        overlay lower-third from=1_000_000 to=3_000_000 {
            text "Coder's Cantina"
        }
    }
}

override t01 word at=6_200_000 {
    zoom 112 hold=5_800_000 lead=250_000
}
override t01 filler at=15_000_000 {
    keep
}
"#;

const ELEMENTS: &str = r#"{"kind":"silence","t":0,"to":800000,"confidence":95,"by":"vad@silero-6.2"}
{"kind":"word","t":800000,"to":1200000,"confidence":90,"by":"whisper@large-v3-turbo","text":"Hello"}
{"kind":"silence","t":4000000,"to":6000000,"confidence":95,"by":"vad@silero-6.2"}
{"kind":"word","t":6200000,"to":6600000,"confidence":90,"by":"whisper@large-v3-turbo","text":"Next"}
{"kind":"url","t":8000000,"to":9000000,"by":"transcript","url":"https://github.com/coderscantina/shot-studio","host":"github"}
{"kind":"filler","t":15000000,"to":15400000,"confidence":60,"by":"whisper@large-v3-turbo","text":"um"}
{"kind":"filler","t":17000000,"to":17300000,"confidence":60,"by":"whisper@large-v3-turbo","text":"uh"}
{"kind":"silence","t":27000000,"to":30000000,"confidence":95,"by":"vad@silero-6.2"}
"#;

fn take() -> Id {
    Id::new("t01").unwrap()
}

fn old_project() -> Project {
    Project {
        video: Video::parse("title \"Migrate\"\nstatus editing\nformat opinion\n").unwrap(),
        script: Script::parse("# Migrate\n").unwrap(),
        format: FormatKind::Opinion.preset(),
        cut: Cut::parse(OLD_CUT).unwrap(),
    }
}

fn elements(text: &str) -> Vec<Elements> {
    vec![Elements::parse(take(), text).unwrap()]
}

/// The channel's opinion format as copied before presets carried rules: settings only.
fn settings_only() -> FormatLayer {
    FormatLayer::from(&FormatKind::Opinion.preset())
}

fn styled(project: &Project, lock: &shot_core::Lock) -> Timeline {
    Timeline::styled(project, lock)
}

/// Every chunk key of the main output, a whole segment per chunk.
fn chunk_keys(timeline: &Timeline) -> Vec<String> {
    let main = timeline.main();
    main.chapters
        .iter()
        .flat_map(|c| &c.segments)
        .map(|s| {
            let fps = 60;
            let frames = s.source.from().0 * fps / 1_000_000..s.source.to().0 * fps / 1_000_000;
            main.chunk_hash(s, MediaId::of(b"t01"), fps as u32, frames)
                .to_string()
        })
        .collect()
}

#[test]
fn a_migrated_video_keeps_every_chunk() {
    let old = old_project();
    let (brand, channel, elements, assets) = (
        Brand::default(),
        settings_only(),
        elements(ELEMENTS),
        BTreeMap::new(),
    );
    let migration = migrate(&MigrateInput {
        project: &old,
        imported: &[],
        brand: &brand,
        channel: &channel,
        elements: &elements,
        assets: &assets,
    })
    .unwrap();

    assert_eq!(migration.cut.to_kdl(), NEW_CUT);
    assert!(!has_rule_nodes(&migration.cut) && has_rule_nodes(&old.cut));
    let channel = migration
        .channel
        .as_ref()
        .expect("the default rules were added");
    assert_eq!(channel.rules, FormatKind::Opinion.preset_layer().rules);
    assert_eq!(migration.video, FormatLayer::default());
    assert_eq!(
        (migration.report.manual, migration.report.overrides),
        (1, 2)
    );

    let new = Project {
        cut: migration.cut.clone(),
        format: migration.format.clone(),
        ..old.clone()
    };
    let before = chunk_keys(&Timeline::evaluate(&old));
    assert_eq!(before.len(), 3);
    assert_eq!(chunk_keys(&styled(&new, &migration.lock)), before);
}

/// The migrated files, built again with `change` applied to the video's layer and elements.
fn rebuilt(
    migration: &shot_core::Migration,
    cut: &Cut,
    video: &FormatLayer,
    elements: &[Elements],
) -> shot_core::Lock {
    let brand = Brand::default();
    let assets = BTreeMap::new();
    build(&StyleInput {
        imported: &[],
        brand: &brand,
        format: migration.channel.as_ref().unwrap(),
        video,
        cut,
        elements,
        assets: &assets,
    })
    .unwrap()
}

fn migrated() -> shot_core::Migration {
    let (brand, channel, elements, assets) = (
        Brand::default(),
        settings_only(),
        elements(ELEMENTS),
        BTreeMap::new(),
    );
    migrate(&MigrateInput {
        project: &old_project(),
        imported: &[],
        brand: &brand,
        channel: &channel,
        elements: &elements,
        assets: &assets,
    })
    .unwrap()
}

#[test]
fn changing_the_pacing_recuts_the_video() {
    let migration = migrated();
    let project = Project {
        cut: migration.cut.clone(),
        format: migration.format.clone(),
        ..old_project()
    };
    let natural =
        FormatLayer::parse("on silence min=600_000 {\n    cut breath=250_000 confidence=85\n}\n")
            .unwrap();
    let lock = rebuilt(&migration, &migration.cut, &natural, &elements(ELEMENTS));
    let cuts: Vec<_> = lock.take(&take()).unwrap().cuts().collect();
    assert_eq!(
        cuts.iter()
            .map(|r| (r.from().0, r.to().0))
            .collect::<Vec<_>>(),
        [
            (0, 550_000),
            (4_250_000, 5_750_000),
            (27_250_000, 30_000_000)
        ]
    );
    assert_ne!(
        styled(&project, &lock).main().chapters,
        styled(&project, &migration.lock).main().chapters
    );
}

#[test]
fn an_override_survives_a_recut_and_an_orphan_is_listed() {
    let migration = migrated();
    let video = FormatLayer::default();
    // A person cuts elsewhere: the kept filler stays kept, its override still bound.
    let mut recut = migration.cut.clone();
    let text = recut
        .to_kdl()
        .replace("keep from=21_000_000 to=30_000_000", "keep from=21_000_000 to=23_000_000\n        remove from=23_000_000 to=24_000_000 by=manual\n        keep from=24_000_000 to=30_000_000");
    recut = Cut::parse(&text).unwrap();
    let lock = rebuilt(&migration, &recut, &video, &elements(ELEMENTS));
    let t01 = lock.take(&take()).unwrap();
    assert!(t01.orphans.is_empty());
    assert!(
        !t01.instances
            .iter()
            .any(|i| i.element.t == Micros(15_000_000))
    );

    // Re-analysis shifts the filler a little: the override follows it.
    let shifted = ELEMENTS.replace(
        r#""t":15000000,"to":15400000"#,
        r#""t":15200000,"to":15500000"#,
    );
    let lock = rebuilt(&migration, &recut, &video, &elements(&shifted));
    assert!(lock.take(&take()).unwrap().orphans.is_empty());

    // Re-analysis no longer finds it: the override is an orphan.
    let gone: String = ELEMENTS
        .lines()
        .filter(|l| !l.contains(r#""text":"um""#))
        .map(|l| format!("{l}\n"))
        .collect();
    let lock = rebuilt(&migration, &recut, &video, &elements(&gone));
    let orphans = &lock.take(&take()).unwrap().orphans;
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].at, Micros(15_000_000));
}

#[test]
fn a_video_that_differs_from_its_channel_keeps_its_rules() {
    let mut old = old_project();
    old.format = Format::parse(
        &FormatKind::Opinion
            .preset()
            .to_kdl()
            .replace("pacing tight", "pacing natural")
            .replace(
                "silence on=#true min=600_000",
                "silence on=#true min=900_000",
            ),
    )
    .unwrap();
    old.cut = Cut::parse(
        r#"chapter "One" {
    take t01 {
        keep from=0 to=30_000_000
    }
}
"#,
    )
    .unwrap();
    let (brand, channel, elements, assets) = (
        Brand::default(),
        FormatKind::Opinion.preset_layer(),
        elements(ELEMENTS),
        BTreeMap::new(),
    );
    let migration = migrate(&MigrateInput {
        project: &old,
        imported: &[],
        brand: &brand,
        channel: &channel,
        elements: &elements,
        assets: &assets,
    })
    .unwrap();
    assert!(migration.channel.is_none());
    let video = migration.video.to_kdl();
    assert!(
        video.starts_with("pacing natural\nauto-cut confidence=85 {"),
        "{video}"
    );
    assert!(
        video.contains("on silence min=900_000 {\n    cut breath=250_000 confidence=85\n}\n"),
        "{video}"
    );
    assert!(video.contains("on silence min=600_000 off\n"), "{video}");
}

#[test]
fn a_chapter_starts_where_the_pacing_lets_it_play() {
    let migration = migrated();
    let swoosh = FormatLayer::parse("on chapter-start {\n    sfx swoosh\n}\n").unwrap();
    let lock = rebuilt(&migration, &migration.cut, &swoosh, &elements(ELEMENTS));
    let starts: Vec<Micros> = lock
        .take(&take())
        .unwrap()
        .instances
        .iter()
        .filter(|i| i.element.target == shot_core::Target::ChapterStart)
        .map(|i| i.applied.start())
        .collect();
    // The take opens on a silence the pacing cuts to 0.75 s.
    assert_eq!(starts, [Micros(750_000)]);
}
