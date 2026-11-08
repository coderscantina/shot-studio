//! `cargo xtask bench`: fixed workloads in release on the generated fixtures, one JSON line per
//! workload with wall time and peak memory, and a comparison against an earlier run.
//!
//! ```text
//! cargo xtask bench [--out <file>] [--compare <file>] [--fail-over <percent>]
//! ```
//!
//! Needs `SHOT_FIXTURES` (see `fixtures/README.md`). The workloads are the editor preview bench
//! (plain and with the glass reel) and an export of a 6 s cut. `shot-app --bench` is left out: it
//! opens a window, which a session without a display cannot run. Results go to `--out`, by default
//! `bench.jsonl` in the cargo target directory. `--compare` prints the change per workload against
//! an earlier results file, and `--fail-over` exits non-zero when a workload got slower by more
//! than that many percent.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{env, fs};

use serde::{Deserialize, Serialize};

/// One workload's result, a line of the results file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Record {
    workload: String,
    wall_ms: u64,
    peak_rss_bytes: u64,
    git_rev: String,
    macos: String,
}

/// A workload that got slower than the limit.
#[derive(Debug, PartialEq)]
struct Slowdown {
    workload: String,
    percent: f64,
}

/// The change per workload in words, and the workloads slower than `fail_over` percent.
fn compare(
    before: &[Record],
    now: &[Record],
    fail_over: Option<f64>,
) -> (Vec<String>, Vec<Slowdown>) {
    let (mut lines, mut slower) = (Vec::new(), Vec::new());
    for record in now {
        let Some(old) = before.iter().find(|b| b.workload == record.workload) else {
            lines.push(format!("{}: new, {} ms", record.workload, record.wall_ms));
            continue;
        };
        let change = |old: u64, new: u64| (new as f64 - old as f64) / old.max(1) as f64 * 100.0;
        let percent = change(old.wall_ms, record.wall_ms);
        lines.push(format!(
            "{}: {} ms -> {} ms ({percent:+.1}%), peak memory {} MB -> {} MB ({:+.1}%)",
            record.workload,
            old.wall_ms,
            record.wall_ms,
            old.peak_rss_bytes >> 20,
            record.peak_rss_bytes >> 20,
            change(old.peak_rss_bytes, record.peak_rss_bytes),
        ));
        if fail_over.is_some_and(|limit| percent > limit) {
            slower.push(Slowdown {
                workload: record.workload.clone(),
                percent,
            });
        }
    }
    (lines, slower)
}

struct Args {
    out: Option<PathBuf>,
    compare: Option<PathBuf>,
    fail_over: Option<f64>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    if args.next().as_deref() != Some("bench") {
        return Err(
            "usage: cargo xtask bench [--out <file>] [--compare <file>] [--fail-over <percent>]"
                .into(),
        );
    }
    let mut parsed = Args {
        out: None,
        compare: None,
        fail_over: None,
    };
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--out" => parsed.out = Some(value()?.into()),
            "--compare" => parsed.compare = Some(value()?.into()),
            "--fail-over" => {
                parsed.fail_over =
                    Some(value()?.parse().map_err(|_| "--fail-over needs a number")?);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(parsed)
}

fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR").map_or_else(|| "target".into(), PathBuf::from)
}

/// Runs `program` and returns its wall time and the peak memory of that process alone.
fn measure(workload: &str, program: &Path, args: &[&str]) -> Result<(u64, u64), String> {
    let started = std::time::Instant::now();
    let child = Command::new(program)
        .args(args)
        .spawn()
        .map_err(|e| format!("{workload}: {}: {e}", program.display()))?;
    let mut status = 0;
    // SAFETY: an all-zero rusage is valid, and wait4 fills it for the one child it reaps.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::wait4(child.id() as i32, &mut status, 0, &mut usage) } < 0 {
        return Err(format!("{workload}: {}", std::io::Error::last_os_error()));
    }
    let wall_ms = started.elapsed().as_millis() as u64;
    if !(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0) {
        return Err(format!("{workload} failed (wait status {status})"));
    }
    // macOS reports ru_maxrss in bytes.
    Ok((wall_ms, usage.ru_maxrss as u64))
}

fn output(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// A 6 s cut of the fixture take in a folder of its own, so no workload writes to the fixture.
fn short_video(fixtures: &Path, dir: &Path) -> Result<(), String> {
    let source = fixtures.join("target-project");
    std::os::unix::fs::symlink(source.join("media"), dir.join("media"))
        .map_err(|e| e.to_string())?;
    for file in ["video.kdl", "format.kdl"] {
        fs::copy(source.join(file), dir.join(file)).map_err(|e| e.to_string())?;
    }
    fs::write(dir.join("script.md"), "# Bench\n").map_err(|e| e.to_string())?;
    fs::write(
        dir.join("cut.kdl"),
        "chapter \"Bench\" {\n    take t01 {\n        keep from=10_000_000 to=16_000_000\n    }\n}\n",
    )
    .map_err(|e| e.to_string())
}

fn bench(args: Args) -> Result<bool, String> {
    let fixtures = PathBuf::from(
        env::var_os("SHOT_FIXTURES").ok_or("set SHOT_FIXTURES, see fixtures/README.md")?,
    );
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "-p", "shot-app", "-p", "shot-cli"])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("the release build failed".into());
    }
    let release = target_dir().join("release");
    let video = tempfile::tempdir().map_err(|e| e.to_string())?;
    short_video(&fixtures, video.path())?;
    let video_dir = video.path().to_str().ok_or("temp path")?;
    let export = video.path().join("bench.mp4");
    let export = export.to_str().ok_or("temp path")?;
    let edit = |extra: &[&'static str]| {
        let mut args = vec![
            "--bench-edit",
            "--runs",
            "1",
            "--seconds",
            "5",
            "--seeks",
            "30",
        ];
        args.extend(extra);
        args.push(video_dir);
        args
    };
    let workloads: [(&str, &str, Vec<&str>); 3] = [
        ("edit-preview", "shot-app", edit(&[])),
        ("edit-preview-glass", "shot-app", edit(&["--glass"])),
        (
            "export",
            "shot",
            vec!["export", "--video", video_dir, "--out", export],
        ),
    ];
    let git_rev = output("git", &["rev-parse", "--short", "HEAD"]);
    let macos = output("sw_vers", &["-productVersion"]);
    let mut records = Vec::new();
    for (workload, binary, args) in &workloads {
        eprintln!("running {workload}");
        let (wall_ms, peak_rss_bytes) = measure(workload, &release.join(binary), args)?;
        records.push(Record {
            workload: (*workload).to_owned(),
            wall_ms,
            peak_rss_bytes,
            git_rev: git_rev.clone(),
            macos: macos.clone(),
        });
    }
    let lines: Vec<String> = records
        .iter()
        .map(|r| serde_json::to_string(r).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let out = args.out.unwrap_or_else(|| target_dir().join("bench.jsonl"));
    fs::write(&out, lines.join("\n") + "\n").map_err(|e| format!("{}: {e}", out.display()))?;
    println!("wrote {}", out.display());
    for line in &lines {
        println!("{line}");
    }
    let Some(base) = args.compare else {
        return Ok(true);
    };
    let before = fs::read_to_string(&base)
        .map_err(|e| format!("{}: {e}", base.display()))?
        .lines()
        .map(|l| serde_json::from_str(l).map_err(|e| format!("{}: {e}", base.display())))
        .collect::<Result<Vec<Record>, _>>()?;
    let (changes, slower) = compare(&before, &records, args.fail_over);
    changes.iter().for_each(|line| println!("{line}"));
    for Slowdown { workload, percent } in &slower {
        eprintln!("{workload} is {percent:.1}% slower, over the limit");
    }
    Ok(slower.is_empty())
}

fn main() -> ExitCode {
    match parse_args(env::args().skip(1)).and_then(bench) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(workload: &str, wall_ms: u64) -> Record {
        Record {
            workload: workload.into(),
            wall_ms,
            peak_rss_bytes: 100 << 20,
            git_rev: "abc1234".into(),
            macos: "27.0".into(),
        }
    }

    #[test]
    fn a_slowdown_over_the_limit_is_named_and_a_speedup_or_new_workload_is_not() {
        let before = [record("preview", 1000), record("export", 4000)];
        let now = [
            record("preview", 1200),
            record("export", 3000),
            record("edit", 500),
        ];
        let (lines, slower) = compare(&before, &now, Some(10.0));
        assert_eq!(
            slower,
            [Slowdown {
                workload: "preview".into(),
                percent: 20.0
            }]
        );
        assert!(
            lines[0].starts_with("preview: 1000 ms -> 1200 ms (+20.0%)"),
            "{lines:?}"
        );
        assert!(lines[1].contains("(-25.0%)"), "{lines:?}");
        assert_eq!(lines[2], "edit: new, 500 ms");
        assert!(compare(&before, &now, None).1.is_empty());
        assert!(compare(&before, &now, Some(25.0)).1.is_empty());
    }
}
