//! `shot autocut`: the rough cut of one take, written into `cut.kdl`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Args;
use shot_analyze::autocut::{self, TakeFacts, TakeFiles};
use shot_analyze::{SpeechDetector, Transcriber, cache, decode_mono, models};
use shot_core::{Cut, Format, Id, Micros, TimeRange};

#[derive(Args)]
pub struct AutocutArgs {
    /// The video folder.
    #[arg(long)]
    video: PathBuf,
    /// The take id, as in `media/<take>/`. A take the cut does not use yet is appended.
    #[arg(long, value_parser = parse_id)]
    take: Id,
    /// Listen to the mic track at recording speed, as capture would, and report how long after
    /// the last sample the rough cut is written. Without it, cached analysis is reused.
    #[arg(long)]
    realtime: bool,
}

fn parse_id(text: &str) -> Result<Id, String> {
    Id::new(text).ok_or_else(|| format!("`{text}` is not a take id"))
}

pub fn run(args: AutocutArgs) -> Result<(), String> {
    crate::migrate::refuse_if_migrated(&args.video, "cuts and review items")?;
    let fail = |e: shot_analyze::Error| e.to_string();
    let format_path = args.video.join("format.kdl");
    let format = Format::parse(&read(&format_path)?).map_err(|e| format!("format.kdl:{e}"))?;
    let cut_path = args.video.join("cut.kdl");
    let cut = match fs::read_to_string(&cut_path) {
        Ok(text) => Cut::parse(&text).map_err(|e| format!("cut.kdl:{e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Cut::default(),
        Err(e) => return Err(format!("{}: {e}", cut_path.display())),
    };
    let take_dir = args.video.join("media").join(args.take.as_str());
    let mic = take_dir.join("mic.mp4");

    let transcript = match cache::read_cached(&args.video, &args.take).map_err(fail)? {
        Some(transcript) if !args.realtime => Some(transcript),
        _ => None,
    };
    let speech = match cache::read_speech(&args.video, &args.take).map_err(fail)? {
        Some(speech) if !args.realtime && transcript.is_some() => Some(speech),
        _ => None,
    };
    let (transcript, speech, duration, fed) = match (transcript, speech) {
        (Some(transcript), Some(speech)) => {
            let duration = speech_end(&speech);
            (transcript, speech, duration, Instant::now())
        }
        (transcript, _) => {
            let vad = crate::models::ensure(&models::vad_model())?;
            let transcriber = match transcript {
                Some(_) => None,
                None => {
                    let whisper = crate::models::ensure(&models::whisper_model())?;
                    Some(
                        Transcriber::new(&whisper, &vad, args.take.clone(), |_| {})
                            .map_err(fail)?,
                    )
                }
            };
            let detector = SpeechDetector::new(&vad).map_err(fail)?;
            let start = Instant::now();
            let duration = decode_mono(&mic, |t, samples, rate| {
                if args.realtime
                    && let Some(wait) = Duration::from_micros(t.0).checked_sub(start.elapsed())
                {
                    std::thread::sleep(wait);
                }
                if let Some(transcriber) = &transcriber {
                    transcriber.feed(t, samples, rate);
                }
                detector.feed(t, samples, rate);
            })
            .map_err(fail)?;
            let fed = Instant::now();
            let transcript = match (transcriber, transcript) {
                (Some(transcriber), _) => transcriber.finish().map_err(fail)?,
                (None, Some(transcript)) => transcript,
                (None, None) => unreachable!("a transcriber runs when no transcript is cached"),
            };
            let speech = detector.finish().map_err(fail)?;
            cache::write_cached(&args.video, &transcript).map_err(fail)?;
            cache::write_speech(&args.video, &args.take, &speech).map_err(fail)?;
            (transcript, speech, duration, fed)
        }
    };
    let analyzed = fed.elapsed();

    let files = TakeFiles::read(&take_dir, format.auto_cut.idle_screen.min).map_err(fail)?;
    let in_cut = cut
        .chapters
        .iter()
        .any(|c| c.takes.iter().any(|t| t.take == args.take));
    let cut = match TimeRange::new(Micros(0), duration) {
        Some(range) if !in_cut => {
            cut.append_take(args.take.clone(), format!("Take {}", args.take), range)
        }
        _ => cut,
    };
    let facts = TakeFacts {
        transcript: &transcript,
        speech: &speech,
        still: &files.still,
        again: &files.again,
        duration,
    };
    let rough = autocut::rough_cut(&cut, &format, &facts, &HashMap::new());
    let tmp = args.video.join(".cut.kdl.tmp");
    fs::write(&tmp, rough.to_kdl())
        .and_then(|()| fs::rename(&tmp, &cut_path))
        .map_err(|e| format!("{}: {e}", cut_path.display()))?;
    let ready = fed.elapsed();

    for count in autocut::rule_counts(&rough) {
        println!(
            "{:<12} {:>4} removed {:>8.2} s {:>4} to review",
            count.rule.as_str(),
            count.removed,
            count.removed_time.0 as f64 / 1e6,
            count.queued
        );
    }
    let zooms = rough
        .chapters
        .iter()
        .flat_map(|c| &c.takes)
        .flat_map(|t| &t.zooms)
        .filter(|z| z.by.is_some())
        .count();
    println!("{zooms} punch-in zooms");
    eprintln!(
        "{} words, {} speech ranges, {} still stretches, {} again markers; transcript and speech \
         ready {:.2} s and the rough cut {:.2} s after the last sample, wrote {}",
        transcript.words().len(),
        speech.ranges.len(),
        files.still.len(),
        files.again.len(),
        analyzed.as_secs_f64(),
        ready.as_secs_f64(),
        cut_path.display()
    );
    Ok(())
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The end of the take as the levels cover it.
fn speech_end(speech: &shot_analyze::Speech) -> Micros {
    Micros(speech.levels_from.0 + speech.levels.len() as u64 * shot_analyze::LEVEL_STEP.0)
}
