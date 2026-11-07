//! Export of the generated target project. Needs `SHOT_FIXTURES` and the hardware encoder.

#![cfg(target_os = "macos")]

use std::path::PathBuf;

use shot_core::{Cut, Format, Project, Script, Video};
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
fn re_export_renders_only_the_segment_that_changed() {
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
    assert_eq!(
        (report.segments, report.rendered, report.frames),
        (2, 2, 120)
    );

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
    assert_eq!(
        (report.segments, report.rendered, report.frames),
        (2, 1, 150)
    );
    assert!(out.metadata().unwrap().len() > 0);
}
