//! `shot analyze`: runs a take's analysis again and rewrites `elements/<take>.jsonl`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;
use shot_analyze::elements::{self, BY_CLAUDE, Depth};
use shot_analyze::{SpeechDetector, Transcriber, cache, decode_mono, models};
use shot_core::{Element, ElementKind, Id, Transcript};
use shot_mcp::bridge::{Claude, Session, new_log_dir};

#[derive(Args)]
pub struct AnalyzeArgs {
    /// The video folder.
    #[arg(long)]
    video: PathBuf,
    /// One take, as in `media/<take>/`. Without it, every take.
    #[arg(long, value_parser = parse_id)]
    take: Option<Id>,
    /// Ask Claude for claims, punchlines, asides, topic shifts, products and statements that
    /// need a source, replacing its earlier answer. Without it, Claude's elements are kept.
    #[arg(long)]
    claude: bool,
}

fn parse_id(text: &str) -> Result<Id, String> {
    Id::new(text).ok_or_else(|| format!("`{text}` is not a take id"))
}

pub fn run(args: AnalyzeArgs) -> Result<(), String> {
    let takes = match args.take {
        Some(take) => vec![take],
        None => takes(&args.video)?,
    };
    for take in takes {
        let (transcript, speech) = listened(&args.video, &take)?;
        let start = Instant::now();
        let mut found = elements::analyze(&args.video, &transcript, &speech, Depth::Full)
            .map_err(|e| e.to_string())?;
        let analyzed = start.elapsed();
        let claude: Vec<Element> = if args.claude {
            ask_claude(&args.video, &transcript)?
        } else {
            let old = elements::read(&args.video, &take).map_err(|e| e.to_string())?;
            old.iter()
                .flat_map(|old| old.iter())
                .filter(|e| e.by == BY_CLAUDE)
                .cloned()
                .collect()
        };
        elements::add(&mut found, claude);
        let path = elements::write(&args.video, &found).map_err(|e| e.to_string())?;
        for kind in ElementKind::ALL {
            let count = found.of_kind(*kind).count();
            if count > 0 {
                println!("{take} {:<13} {count:>6}", kind.as_str());
            }
        }
        eprintln!(
            "{} elements in {:.2} s, wrote {}",
            found.len(),
            analyzed.as_secs_f64(),
            path.display()
        );
    }
    Ok(())
}

/// The take folders in `media/`, in order.
fn takes(video: &Path) -> Result<Vec<Id>, String> {
    let media = video.join("media");
    let entries = std::fs::read_dir(&media).map_err(|e| format!("{}: {e}", media.display()))?;
    let mut takes: Vec<Id> = entries
        .filter_map(|e| Id::new(e.ok()?.file_name().into_string().ok()?))
        .collect();
    takes.sort_by_key(|id| {
        id.as_str()
            .trim_start_matches('t')
            .parse::<u64>()
            .unwrap_or(0)
    });
    Ok(takes)
}

/// The cached transcript and speech, or both from the mic track, cached for next time. A take
/// without a mic has neither.
fn listened(video: &Path, take: &Id) -> Result<(Transcript, shot_analyze::Speech), String> {
    let fail = |e: shot_analyze::Error| e.to_string();
    let transcript = cache::read_cached(video, take).map_err(fail)?;
    let speech = cache::read_speech(video, take).map_err(fail)?;
    if let (Some(transcript), Some(speech)) = (transcript, speech) {
        return Ok((transcript, speech));
    }
    let mic = video.join("media").join(take.as_str()).join("mic.mp4");
    if !mic.exists() {
        return Ok((
            Transcript::new(take.clone()),
            shot_analyze::Speech::default(),
        ));
    }
    let whisper = crate::models::ensure(&models::whisper_model())?;
    let vad = crate::models::ensure(&models::vad_model())?;
    let transcriber = Transcriber::new(&whisper, &vad, take.clone(), |_| {}).map_err(fail)?;
    let detector = SpeechDetector::new(&vad).map_err(fail)?;
    decode_mono(&mic, |t, samples, rate| {
        transcriber.feed(t, samples, rate);
        detector.feed(t, samples, rate);
    })
    .map_err(fail)?;
    let transcript = transcriber.finish().map_err(fail)?;
    let speech = detector.finish().map_err(fail)?;
    cache::write_cached(video, &transcript).map_err(fail)?;
    cache::write_speech(video, take, &speech).map_err(fail)?;
    Ok((transcript, speech))
}

/// Claude's reading of the transcript through the bridge, its session logged in the video's
/// `.shot/agent/`.
fn ask_claude(video: &Path, transcript: &Transcript) -> Result<Vec<Element>, String> {
    let claude = Claude::find().ok_or(
        "Claude Code is not installed: install it from https://claude.com/claude-code and sign in, then run this again",
    )?;
    let video = std::path::absolute(video).map_err(|e| e.to_string())?;
    let session = Session {
        log_dir: new_log_dir(&video),
        repo: video,
        shot: std::env::current_exe().map_err(|e| e.to_string())?,
        prompt: elements::claude::prompt(transcript),
        frame_task: false,
    };
    let answer = session.run(&claude, |_| {}).map_err(|e| e.to_string())?;
    Ok(elements::claude::read_answer(transcript, &answer))
}
