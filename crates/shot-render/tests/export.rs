//! Export of the generated target project. Needs `SHOT_FIXTURES` and the hardware encoder.

#![cfg(target_os = "macos")]

use std::path::PathBuf;

use shot_audio::{CEILING_DBTP, Meter, TARGET_LUFS};
use shot_core::{Cut, Format, Project, Script, Video};
use shot_media::audio::PcmReader;
use shot_render::export::{ExportOptions, export};

fn project(cut: &str) -> (tempfile::TempDir, Project) {
    let fixtures = PathBuf::from(std::env::var("SHOT_FIXTURES").expect("SHOT_FIXTURES is set"))
        .join("target-project");
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(fixtures.join("media"), dir.path().join("media")).unwrap();
    let read = |name: &str| std::fs::read_to_string(fixtures.join(name)).unwrap();
    let project = Project {
        video: Video::parse(&read("video.kdl")).unwrap(),
        script: Script::parse(&read("script.md")).unwrap(),
        format: Format::parse(&read("format.kdl")).unwrap(),
        cut: Cut::parse(cut).unwrap(),
    };
    (dir, project)
}

#[test]
#[ignore = "needs the generated target project and the hardware HEVC encoder"]
fn re_export_renders_only_the_chunks_that_changed() {
    let cut = |end: &str| {
        format!(
            "chapter \"A\" {{\n    take t01 {{\n        keep from=800_000 to=1_800_000\n        layout split from=800_000 to=1_800_000\n    }}\n}}\n\nchapter \"B\" {{\n    take t01 {{\n        keep from=96_800_000 to={end}\n    }}\n}}\n"
        )
    };
    let (dir, first) = project(&cut("97_800_000"));
    let out = dir.path().join("out.mp4");
    let report = export(
        dir.path(),
        &first,
        "main",
        &out,
        ExportOptions::default(),
        &|_| {},
    )
    .unwrap();
    // 48..60, 60..90, 90..108 and 5808..5820, 5820..5850, 5850..5868.
    assert_eq!((report.chunks, report.rendered, report.frames), (6, 6, 120));

    let (_, second) = project(&cut("98_300_000"));
    let report = export(
        dir.path(),
        &second,
        "main",
        &out,
        ExportOptions::default(),
        &|_| {},
    )
    .unwrap();
    // The old last chunk 5850..5868 becomes 5850..5880, and 5880..5898 is new.
    assert_eq!((report.chunks, report.rendered, report.frames), (7, 2, 150));

    // The audio joined into its room after the video reads back finished.
    let mut reader = PcmReader::open(&out).unwrap();
    let mut samples = Vec::new();
    reader.read(0, 150 * 800, &mut samples).unwrap();
    let mut meter = Meter::new();
    meter.add(&samples);
    let loudness = meter.loudness().unwrap();
    assert!((loudness - TARGET_LUFS).abs() <= 0.5, "{loudness} LUFS");
    assert!(
        meter.true_peak() <= CEILING_DBTP,
        "{} dBTP",
        meter.true_peak()
    );
}
