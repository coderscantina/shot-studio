//! The committed projects under `fixtures/projects`: the 17 minute target video and the demo
//! channel's videos. `scripts/fixtures.sh` copies them and adds generated media.

use std::fs;
use std::path::{Path, PathBuf};

use shot_core::{
    Cut, Format, Id, Layout, Micros, Project, RangeEdit, Script, TakeAt, TimeRange, Timeline,
    Transcript, Video, Word,
};

const CHAPTERS_TSV: &str = include_str!("../../../fixtures/spec/chapters.tsv");

fn projects_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects")
}

fn target_file(name: &str) -> String {
    fs::read_to_string(projects_dir().join("target").join(name)).unwrap()
}

/// `(title, start, end)` of each chapter in microseconds, from the generator's spec.
fn spec_chapters() -> Vec<(String, u64, u64)> {
    let mut start = 0;
    CHAPTERS_TSV
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|line| {
            let mut cols = line.split('\t');
            let seconds: u64 = cols.nth(1).unwrap().parse().unwrap();
            let title = cols.nth(1).unwrap().to_owned();
            let end = start + seconds * 1_000_000;
            let chapter = (title, start, end);
            start = end;
            chapter
        })
        .collect()
}

fn files_named(dir: &Path, name: &str, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files_named(&path, name, found);
        } else if path.file_name().is_some_and(|n| n == name) {
            found.push(path);
        }
    }
}

#[test]
fn target_project_is_a_timeline_of_the_spec_chapters() {
    let project = Project {
        video: Video::parse(&target_file("video.kdl")).unwrap(),
        script: Script::parse(&target_file("script.md")).unwrap(),
        format: Format::parse(&target_file("format.kdl")).unwrap(),
        cut: Cut::parse(&target_file("cut.kdl")).unwrap(),
    };
    let spec = spec_chapters();
    assert_eq!(spec.len(), 8);

    let timeline = Timeline::evaluate(&project);
    let main = timeline.main();
    assert_eq!(main.chapters.len(), spec.len());
    assert_eq!(project.script.chapters.len(), spec.len());
    for (i, (title, start, end)) in spec.iter().enumerate() {
        assert_eq!(&main.chapters[i].title, title);
        assert_eq!(&project.script.chapters[i].title, title);

        // Kept and removed ranges together tile the chapter exactly.
        let mut at = *start;
        for edit in &project.cut.chapters[i].takes[0].ranges {
            let range = edit.range();
            assert_eq!(range.from(), Micros(at), "gap or overlap in {title}");
            at = range.to().0;
        }
        assert_eq!(at, *end, "{title} does not end at its chapter end");

        let segments = &main.chapters[i].segments;
        assert!(!segments.is_empty());
        assert!(segments.iter().all(|s| {
            s.source.from().0 >= *start && s.source.to().0 <= *end && s.take.as_str() == "t01"
        }));
    }
    assert!(
        project
            .cut
            .chapters
            .iter()
            .flat_map(|c| &c.takes[0].ranges)
            .any(|r| matches!(r, RangeEdit::Remove { .. }))
    );
}

#[test]
fn committed_kdl_is_canonical() {
    type Canonical = fn(&str) -> Result<String, shot_core::ParseError>;
    let kinds: [(&str, Canonical); 3] = [
        ("video.kdl", |t| Video::parse(t).map(|v| v.to_kdl())),
        ("format.kdl", |t| Format::parse(t).map(|v| v.to_kdl())),
        ("cut.kdl", |t| Cut::parse(t).map(|v| v.to_kdl())),
    ];
    let mut checked = 0;
    for (name, canonical) in kinds {
        let mut paths = Vec::new();
        files_named(&projects_dir(), name, &mut paths);
        for path in paths {
            let text = fs::read_to_string(&path).unwrap();
            let written = canonical(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(written, text, "{} is not canonical", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 4, "found only {checked} files");
}

#[test]
fn committed_scripts_parse() {
    let mut paths = Vec::new();
    files_named(&projects_dir(), "script.md", &mut paths);
    assert!(!paths.is_empty());
    for path in paths {
        Script::parse(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
}

/// Half-second words from `from` to `to` microseconds of take `t01`.
fn words(from: u64, to: u64) -> Transcript {
    let mut transcript = Transcript::new(Id::new("t01").unwrap());
    for start in (from..to - 500_000).step_by(500_000) {
        let range = TimeRange::new(Micros(start), Micros(start + 400_000)).unwrap();
        let word = Word {
            text: "word".to_owned(),
            range,
            confidence: 90,
        };
        transcript.push(word).unwrap();
    }
    transcript
}

/// The line span of the `n`th take block in `cut.kdl` text.
fn take_block(kdl: &str, n: usize) -> std::ops::Range<usize> {
    let lines: Vec<&str> = kdl.lines().collect();
    let start = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("    take "))
        .nth(n)
        .unwrap()
        .0;
    let end = start + lines[start..].iter().position(|l| *l == "    }").unwrap();
    start..end + 1
}

#[test]
fn an_edit_changes_only_the_lines_of_its_take_block() {
    let cut = Cut::parse(&target_file("cut.kdl")).unwrap();
    let before = cut.to_kdl();
    let at = TakeAt {
        chapter: 1,
        take: 0,
    };
    let transcript = words(96_800_000, 136_200_000);
    let selection = TimeRange::new(Micros(110_000_000), Micros(112_000_000)).unwrap();

    let edits = [
        cut.delete_words(at, &transcript, selection).unwrap(),
        cut.restore(
            at,
            TimeRange::new(Micros(96_000_000), Micros(97_000_000)).unwrap(),
        )
        .unwrap(),
        cut.set_layout(at, selection, Layout::Face).unwrap(),
    ];
    for edited in edits {
        let after = edited.to_kdl();
        assert_ne!(before, after);
        let (before_lines, after_lines): (Vec<_>, Vec<_>) =
            (before.lines().collect(), after.lines().collect());
        let head = before_lines
            .iter()
            .zip(&after_lines)
            .take_while(|(a, b)| a == b)
            .count();
        let tail = before_lines
            .iter()
            .rev()
            .zip(after_lines.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let block = take_block(&before, 1);
        let changed = head..before_lines.len() - tail;
        assert!(
            block.start <= changed.start && changed.end <= block.end,
            "changed lines {changed:?} leave the take block {block:?}"
        );
    }
}
