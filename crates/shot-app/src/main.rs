//! gpui UI, bundled as shot.app.
//!
//! `shot-app <video-folder>` opens a video folder on its Edit screen, with Record one click away.
//!
//! `shot-app [flags] <video.mp4>` opens the preview spike window on one file:
//!
//! - `--bench`: play, then seek at random, for several runs, print the measurements and a PASS or
//!   FAIL line, exit 0 on pass and 1 on fail. A run passes when no frame is dropped, late or
//!   missing, every seek shows its exact frame and seek p95 stays under 100 ms.
//! - `--bench-edit <video-folder>`: the same for the Edit screen's engine, headless, on the
//!   folder's edited timeline through the render graph. `--glass` adds a glass and motion blur
//!   reel on every frame.
//! - `--runs N`: bench runs, default 5.
//! - `--seconds N`: playback per run, default 60.
//! - `--seeks N`: random seeks per run, default 60.
//! - `--zoom`: start with the 2x punch-in, a wgpu pass on every frame.
//! - `--wall-clock`: pace by wall clock alone instead of locking to the display, for comparison.

mod agent;
mod bench;
mod board;
mod cut;
mod edit;
mod edit_bench;
mod engine;
mod folder;
mod live;
mod metrics;
mod model;
mod peaks;
mod place;
mod preview;
mod prompter;
mod record;
mod score;
mod script;
mod ship;
mod sound;
mod studio;
mod style;
mod theme;
mod ui;
mod zoom;

use std::path::PathBuf;
use std::process::ExitCode;

use bench::BenchOptions;
use preview::Options;

fn main() -> ExitCode {
    let options = match parse_args(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!(
                "{message}\nusage: shot-app <video-folder>\n       shot-app [--bench] [--bench-edit] [--glass] [--runs N] [--seconds N] [--seeks N] [--zoom] [--wall-clock] <video.mp4>"
            );
            return ExitCode::FAILURE;
        }
    };
    if let Some(bench) = options.bench.as_ref().filter(|bench| bench.edit) {
        return match edit_bench::run(&options.video, bench) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(error) => {
                eprintln!("{}: {error}", options.video.display());
                ExitCode::FAILURE
            }
        };
    }
    if options.video.is_dir() && options.bench.is_none() {
        let dir = options.video;
        gpui_platform::application()
            .with_assets(theme::Assets)
            .run(move |cx| {
                if let Err(error) = studio::open(&dir, cx) {
                    eprintln!("{}: {error}", dir.display());
                    cx.quit();
                }
            });
        return ExitCode::SUCCESS;
    }
    gpui_platform::application().run(move |cx| preview::open(options, cx));
    ExitCode::SUCCESS
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut video = None;
    let mut zoom = 1.0;
    let mut display_lock = true;
    let mut bench = false;
    let mut edit = false;
    let mut glass = false;
    let mut seconds = 60;
    let mut seeks = 60;
    let mut runs = 5;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let mut number = |name: &str| {
            args.next()
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| format!("{name} needs a number"))
        };
        match arg.as_str() {
            "--bench" => bench = true,
            "--bench-edit" => {
                bench = true;
                edit = true;
            }
            "--glass" => glass = true,
            "--zoom" => zoom = 2.0,
            "--wall-clock" => display_lock = false,
            "--seconds" => seconds = number("--seconds")?,
            "--seeks" => seeks = number("--seeks")? as usize,
            "--runs" => runs = number("--runs")?.max(1) as usize,
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            path => video = Some(PathBuf::from(path)),
        }
    }
    Ok(Options {
        video: video.ok_or("missing video path")?,
        zoom,
        display_lock,
        bench: bench.then_some(BenchOptions {
            seconds,
            seeks,
            runs,
            edit,
            glass,
        }),
    })
}
