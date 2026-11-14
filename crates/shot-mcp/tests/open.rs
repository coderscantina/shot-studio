//! Opening a video as the app, `shot` and the MCP server do: a video written before the lock is
//! migrated with the frames it had, one whose migration would differ does not open, and a folder
//! outside a channel repo is a channel of one.

use std::path::Path;

use shot_core::{Brand, Cut, FormatKind, FormatLayer, Layer, Lock, MediaId, Project, Timeline};
use shot_mcp::style;

const TARGET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/projects/target"
);

/// Every chunk key of the main output, a whole segment per chunk.
fn chunk_keys(project: &Project, lock: &Lock) -> Vec<String> {
    let timeline = Timeline::evaluate(project, lock);
    let main = timeline.main();
    main.chapters
        .iter()
        .flat_map(|c| &c.segments)
        .map(|s| {
            let frames = s.source.from().0 * 60 / 1_000_000..s.source.to().0 * 60 / 1_000_000;
            main.chunk_hash(s, MediaId::of(b"t01"), 60, frames)
                .to_string()
        })
        .collect()
}

fn write(dir: &Path, file: &str, text: &str) {
    std::fs::write(dir.join(file), text).unwrap();
}

#[test]
fn opening_an_old_video_migrates_it_with_the_frames_it_had() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path().join("target");
    std::fs::create_dir_all(&video).unwrap();
    for file in ["video.kdl", "script.md", "format.kdl", "cut.kdl"] {
        std::fs::copy(Path::new(TARGET).join(file), video.join(file)).unwrap();
    }
    // The old cut as it played: the rules' removals as a person's.
    let mut played = std::fs::read_to_string(video.join("cut.kdl")).unwrap();
    for rule in ["silence", "pause", "filler", "retake", "misspoken"] {
        played = played.replace(&format!(" by={rule}\n"), " by=manual\n");
    }

    let opened = style::open(&video).unwrap();
    assert!(opened.migrated.is_some());
    let before = Project {
        cut: Cut::parse(&played).unwrap(),
        format: FormatKind::Opinion.preset(),
        ..opened.project.clone()
    };
    let keys = chunk_keys(&before, &Lock::default());
    assert!(keys.len() > 8);
    assert_eq!(chunk_keys(&opened.project, &opened.lock), keys);

    let read = |file: &str| std::fs::read_to_string(video.join(file)).unwrap();
    assert!(Cut::parse(&read("cut.kdl")).is_ok());
    assert!(!read("format.kdl").contains("pacing"));
    assert_eq!(Lock::parse(&read("cut.lock")).unwrap(), opened.lock);
    let again = style::open(&video).unwrap();
    assert!(again.migrated.is_none(), "a video migrates once");
    assert_eq!(again.lock, opened.lock);
}

#[test]
fn a_video_whose_migration_would_differ_does_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path();
    write(
        video,
        "video.kdl",
        "title \"A\"\nstatus editing\nformat opinion\n",
    );
    write(video, "script.md", "# A\n");
    write(video, "format.kdl", "pacing tight\n");
    // Two kept ranges that meet play as two segments; migration would join them.
    let cut = "chapter \"A\" {\n    take t01 {\n        keep from=0 to=10_000_000\n        keep from=10_000_000 to=20_000_000\n    }\n}\n";
    write(video, "cut.kdl", cut);
    let error = style::open(video).err().unwrap();
    assert!(
        error.contains("does not open: the migrated video would differ"),
        "{error}"
    );
    assert!(!video.join("cut.lock").exists());
    assert_eq!(std::fs::read_to_string(video.join("cut.kdl")).unwrap(), cut);
}

#[test]
fn a_folder_outside_a_channel_gets_a_lock_from_the_preset_and_the_default_brand() {
    let dir = tempfile::tempdir().unwrap();
    let video = dir.path();
    write(
        video,
        "video.kdl",
        "title \"A\"\nstatus editing\nformat opinion\n",
    );
    write(video, "script.md", "# A\n");
    write(video, "format.kdl", "");
    let cut = |to: &str| {
        format!("chapter \"A\" {{\n    take t01 {{\n        keep from=0 to={to}\n    }}\n}}\n")
    };
    write(video, "cut.kdl", &cut("10_000_000"));
    std::fs::create_dir_all(video.join("elements")).unwrap();
    write(
        video,
        "elements/t01.jsonl",
        "{\"kind\":\"silence\",\"t\":2000000,\"to\":4000000,\"confidence\":95,\"by\":\"test\"}\n",
    );

    let opened = style::open(video).unwrap();
    assert_eq!(opened.lock.brand, Brand::default());
    assert_eq!(opened.layer, FormatLayer::default());
    let cuts: Vec<_> = opened.lock.takes[0].cuts().collect();
    assert_eq!(cuts.len(), 1, "the preset's silence rule cuts");
    assert_eq!(
        opened.lock.takes[0].instances[0].provenance.layer,
        Layer::Format
    );
    let written = std::fs::read_to_string(video.join("cut.lock")).unwrap();
    assert_eq!(Lock::parse(&written).unwrap(), opened.lock);

    // An edit that ends the take before the silence rebuilds the lock without its cut.
    write(video, "cut.kdl", &cut("2_000_000"));
    let edited = style::open(video).unwrap();
    assert_eq!(edited.lock.takes[0].cuts().count(), 0);
}
