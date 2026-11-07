//! `shot export`: one video folder's cut to an MP4, rendering only chunks that changed.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;
use shot_core::{Cut, Format, Project, Script, Video};
use shot_render::export::{self, ExportOptions, Progress};

#[derive(Args)]
pub struct ExportArgs {
    /// The video folder, holding cut.kdl and media/.
    #[arg(long)]
    video: PathBuf,
    /// The output file. Defaults to .shot/export/<cut>.mp4 in the video folder.
    #[arg(long)]
    out: Option<PathBuf>,
    /// The cut to export.
    #[arg(long, default_value = "main")]
    cut: String,
    /// Encoder sessions running at the same time.
    #[arg(long, default_value_t = ExportOptions::default().jobs)]
    jobs: usize,
}

pub fn run(args: ExportArgs) -> Result<(), String> {
    let project = load(&args.video)?;
    let out = args.out.unwrap_or_else(|| {
        args.video
            .join(".shot/export")
            .join(format!("{}.mp4", args.cut))
    });
    let started = Instant::now();
    let interactive = std::io::stderr().is_terminal();
    let progress = |p: Progress| {
        if interactive {
            eprint!("\rrendered {}/{} chunks", p.rendered, p.to_render);
            let _ = std::io::stderr().flush();
        }
    };
    let report = export::export(
        &args.video,
        &project,
        &args.cut,
        &out,
        ExportOptions { jobs: args.jobs },
        &progress,
    )
    .map_err(|e| e.to_string())?;
    if interactive && report.rendered > 0 {
        eprintln!();
    }
    let render = report.render_time.as_secs_f64();
    println!(
        "exported {} ({:.1} s) to {}",
        args.cut,
        report.duration().as_secs_f64(),
        report.output.display()
    );
    println!(
        "{} of {} chunks rendered, {} frames in {:.1} s ({:.0} fps), joined in {:.1} s, {:.1} s total",
        report.rendered,
        report.chunks,
        report.rendered_frames,
        render,
        report.rendered_frames as f64 / render.max(1e-9),
        report.join_time.as_secs_f64(),
        started.elapsed().as_secs_f64()
    );
    if report.base_chunks > 0 {
        println!(
            "drawn from {} main chunks: {} reused from the cache, {} rendered",
            report.base_chunks,
            report.base_chunks - report.base_rendered,
            report.base_rendered
        );
    }
    Ok(())
}

pub(crate) fn load(dir: &Path) -> Result<Project, String> {
    let read = |name: &str| {
        std::fs::read_to_string(dir.join(name))
            .map_err(|e| format!("{}: {e}", dir.join(name).display()))
    };
    let parse_error = |e: shot_core::ParseError| e.to_string();
    Ok(Project {
        video: Video::parse(&read("video.kdl")?).map_err(parse_error)?,
        script: Script::parse(&read("script.md")?).map_err(parse_error)?,
        format: Format::parse(&read("format.kdl")?).map_err(parse_error)?,
        cut: Cut::parse(&read("cut.kdl")?).map_err(parse_error)?,
    })
}
