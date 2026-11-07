//! gpui UI, bundled as shot.app.
//!
//! For now the milestone 02 preview spike: `shot-app [--bench] [--zoom] [--wall-clock] [--seconds N] [--seeks N] <video>`.

mod bench;
mod metrics;
mod preview;
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
                "{message}\nusage: shot-app [--bench] [--zoom] [--wall-clock] [--seconds N] [--seeks N] <video>"
            );
            return ExitCode::FAILURE;
        }
    };
    gpui_platform::application().run(move |cx| preview::open(options, cx));
    ExitCode::SUCCESS
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut video = None;
    let mut zoom = 1.0;
    let mut display_lock = true;
    let mut bench = false;
    let mut seconds = 60;
    let mut seeks = 60;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let mut number = |name: &str| {
            args.next()
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| format!("{name} needs a number"))
        };
        match arg.as_str() {
            "--bench" => bench = true,
            "--zoom" => zoom = 2.0,
            "--wall-clock" => display_lock = false,
            "--seconds" => seconds = number("--seconds")?,
            "--seeks" => seeks = number("--seeks")? as usize,
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            path => video = Some(PathBuf::from(path)),
        }
    }
    Ok(Options {
        video: video.ok_or("missing video path")?,
        zoom,
        display_lock,
        bench: bench.then_some(BenchOptions { seconds, seeks }),
    })
}
