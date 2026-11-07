//! `shot transcribe`: a take's mic track through the streaming transcriber into the cache.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Args;
use shot_analyze::{Transcriber, cache, decode_mono, models};
use shot_core::{Id, Micros};

#[derive(Args)]
pub struct TranscribeArgs {
    /// The video folder.
    #[arg(long)]
    video: PathBuf,
    /// The take id, as in `media/<take>/`.
    #[arg(long, value_parser = parse_id)]
    take: Id,
    /// Feed the audio at recording speed, as capture would, instead of as fast as possible.
    #[arg(long)]
    realtime: bool,
}

fn parse_id(text: &str) -> Result<Id, String> {
    Id::new(text).ok_or_else(|| format!("`{text}` is not a take id"))
}

pub fn run(args: TranscribeArgs) -> Result<(), String> {
    let mic = args
        .video
        .join("media")
        .join(args.take.as_str())
        .join("mic.mp4");
    let whisper = crate::models::ensure(&models::whisper_model())?;
    let vad = crate::models::ensure(&models::vad_model())?;
    let fail = |e: shot_analyze::Error| e.to_string();

    let transcriber = Transcriber::new(&whisper, &vad, args.take.clone(), |_| {}).map_err(fail)?;
    let start = Instant::now();
    let duration = decode_mono(&mic, |t, samples, rate| {
        if args.realtime
            && let Some(wait) = Duration::from_micros(t.0).checked_sub(start.elapsed())
        {
            std::thread::sleep(wait);
        }
        transcriber.feed(t, samples, rate);
    })
    .map_err(fail)?;
    let fed = Instant::now();
    let transcript = transcriber.finish().map_err(fail)?;
    let flush = fed.elapsed();
    let total = start.elapsed();

    for word in transcript.words() {
        println!(
            "{} {} {:>3}% {}",
            seconds(word.range.from()),
            seconds(word.range.to()),
            word.confidence,
            word.text
        );
    }
    let path = cache::write_cached(&args.video, &transcript).map_err(fail)?;
    let audio = Duration::from_micros(duration.0);
    eprintln!(
        "{} words from {:.1} s of audio in {:.1} s (real-time factor {:.3}), {:.2} s after the last sample, wrote {}",
        transcript.words().len(),
        audio.as_secs_f64(),
        total.as_secs_f64(),
        total.as_secs_f64() / audio.as_secs_f64().max(1e-9),
        flush.as_secs_f64(),
        path.display()
    );
    Ok(())
}

fn seconds(t: Micros) -> String {
    format!("{:>9.3}", t.0 as f64 / 1e6)
}
