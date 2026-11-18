//! `shot ingest`: a clip from outside, a phone clip first of all, made the next take of a video
//! ([0039]): appended to `cut.kdl` as its own chapter like a recording, analysed, the cut
//! composed from the takes ([0040]) and the lock rebuilt.
//!
//! [0039]: ../../../docs/decisions/0039-ingested-clips-are-takes.md
//! [0040]: ../../../docs/decisions/0040-the-cut-proposes-itself.md

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::Args;
use shot_capture::{Progress, ingest};
use shot_core::{Micros, TimeRange};

#[derive(Args)]
pub struct IngestArgs {
    /// The clip: any file AVFoundation plays, with a video track.
    file: PathBuf,
    /// The video folder the take joins.
    #[arg(long)]
    pub(crate) video: PathBuf,
    /// Ask Claude for the take's claims, punchlines and other elements, as `shot analyze --claude`.
    #[arg(long)]
    claude: bool,
}

pub fn run(args: IngestArgs) -> Result<(), String> {
    // Refuses a folder that is no video before the transcode.
    drop(crate::open(&args.video)?);
    let started = Instant::now();
    let progress = Progress::default();
    let finished = AtomicBool::new(false);
    let ingested = std::thread::scope(|scope| {
        if std::io::stderr().is_terminal() {
            scope.spawn(|| {
                while !finished.load(Ordering::Relaxed) {
                    let total = progress.total().as_secs_f64().max(f64::EPSILON);
                    let share = 100.0 * progress.done().as_secs_f64() / total;
                    eprint!("\rtranscoding {share:.0} %");
                    let _ = std::io::stderr().flush();
                    std::thread::sleep(Duration::from_millis(200));
                }
                eprint!("\r");
            });
        }
        let ingested = ingest(&args.file, &args.video, &progress);
        finished.store(true, Ordering::Relaxed);
        ingested
    })
    .map_err(|e| format!("{}: {e}", args.file.display()))?;
    let take = &ingested.take;
    let transcoded = started.elapsed();
    println!(
        "{take}: {:.1} s from {}{}",
        ingested.duration.0 as f64 / 1e6,
        args.file.display(),
        if ingested.mic { "" } else { ", no sound" }
    );
    append(&args.video, take, ingested.duration)?;

    let analysis = Instant::now();
    {
        let _lock = crate::lock(&args.video)?;
        crate::analyze::analyze_take(&args.video, take, args.claude)?;
    }
    let analysed = analysis.elapsed();
    let build = Instant::now();
    compose(&args.video, take)?;
    crate::style::write(&args.video)?;
    eprintln!(
        "transcoded in {:.2} s, analysed in {:.2} s, lock built in {:.2} s",
        transcoded.as_secs_f64(),
        analysed.as_secs_f64(),
        build.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Composes the cut now that the new take has words and elements, as the app does: the best
/// scored attempt plays each beat unless a person chose or edited there.
fn compose(video: &Path, take: &shot_core::Id) -> Result<(), String> {
    let (_lock, opened) = crate::open(video)?;
    let cut = match shot_mcp::takes::compose(video, &opened.project, Some(take)) {
        Ok(cut) => cut,
        Err(error) => {
            eprintln!("the cut was not composed, {take} stays a chapter of its own: {error}");
            return Ok(());
        }
    };
    if cut == opened.project.cut {
        return Ok(());
    }
    let path = video.join("cut.kdl");
    shot_fs::write(&path, cut.to_kdl()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Appends the take to `cut.kdl` as a chapter of its own, as the app appends a recording.
fn append(video: &Path, take: &shot_core::Id, duration: Micros) -> Result<(), String> {
    let range = TimeRange::new(Micros(0), duration).ok_or(format!("take {take} is empty"))?;
    let (_lock, opened) = crate::open(video)?;
    let events = video.join("media").join(take.as_str()).join("events.jsonl");
    let markers = shot_capture::events::read_markers(&events)
        .map_err(|e| format!("{}: {e}", events.display()))?;
    let project = &opened.project;
    let cut = shot_prompter::appended(&project.cut, take, range, &markers, &project.format);
    let path = video.join("cut.kdl");
    shot_fs::write(&path, cut.to_kdl()).map_err(|e| format!("{}: {e}", path.display()))
}
