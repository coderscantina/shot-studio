//! `shot overlays`: run the overlay rules, place bleeps and blurs from the transcript, fill a
//! b-roll slot, and check every overlay against its template.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use shot_core::{Cut, Id, Micros};
use shot_render::overlay;
use shot_render::overlay_rules::{self, Redaction};

#[derive(Args)]
pub struct OverlaysArgs {
    /// The video folder.
    #[arg(long, global = true, default_value = ".")]
    video: PathBuf,
    #[command(subcommand)]
    action: OverlaysAction,
}

#[derive(Subcommand)]
enum OverlaysAction {
    /// Add link cards for URLs said, b-roll slots over idle screen and B markers, and highlights
    /// where you read out text on screen.
    Find,
    /// Replace the voice with a tone where the take says a phrase.
    Bleep(Phrase),
    /// Blur where a phrase shows on screen, from when the take says it.
    Blur {
        #[command(flatten)]
        phrase: Phrase,
        /// Seconds the blur stays after the phrase is said.
        #[arg(long, default_value_t = 2.0)]
        hold: f64,
    },
    /// Pick the file of the b-roll slot at a moment of a take.
    BRoll {
        #[arg(long)]
        take: String,
        /// Media time in seconds inside the slot.
        #[arg(long)]
        at: f64,
        /// A still, clip or diff, relative to the video folder.
        #[arg(long)]
        file: String,
    },
    /// List every overlay and check it against its template.
    Check,
}

#[derive(Args)]
struct Phrase {
    #[arg(long)]
    take: String,
    /// The words as the transcript has them, matched ignoring case and punctuation.
    #[arg(long)]
    phrase: String,
}

pub fn run(args: OverlaysArgs) -> Result<(), String> {
    let project = crate::export::load(&args.video)?;
    let take = |id: &str| Id::new(id).ok_or_else(|| format!("{id} is not a take id"));
    let micros = |seconds: f64| Micros((seconds * 1e6).round() as u64);
    let fail = |e: overlay_rules::OverlayRuleError| e.to_string();
    let cut = match args.action {
        OverlaysAction::Find => {
            crate::migrate::refuse_if_migrated(
                &args.video,
                "link cards, highlights and b-roll slots",
            )?;
            let (cut, added) = overlay_rules::run(&args.video, &project.cut).map_err(fail)?;
            println!(
                "{} link cards, {} b-roll slots, {} highlights added",
                added.link_cards, added.b_roll, added.highlights
            );
            cut
        }
        OverlaysAction::Bleep(p) => overlay_rules::redact(
            &args.video,
            &project.cut,
            &take(&p.take)?,
            &p.phrase,
            Redaction::Bleep,
        )
        .map_err(fail)?,
        OverlaysAction::Blur { phrase: p, hold } => overlay_rules::redact(
            &args.video,
            &project.cut,
            &take(&p.take)?,
            &p.phrase,
            Redaction::Blur { hold: micros(hold) },
        )
        .map_err(fail)?,
        OverlaysAction::BRoll { take: id, at, file } => {
            if overlay::broll::Media::of(&file).is_none() {
                return Err(format!("{file} is not a still, clip or diff"));
            }
            if !args.video.join(&file).exists() {
                return Err(format!("{file} is not in the video folder"));
            }
            overlay_rules::fill_b_roll(&project.cut, &take(&id)?, micros(at), &file)
                .map_err(fail)?
        }
        OverlaysAction::Check => return check(&project.cut),
    };
    write_cut(&args.video, &cut)
}

fn check(cut: &Cut) -> Result<(), String> {
    let mut bad = 0;
    for edit in cut.chapters.iter().flat_map(|c| &c.takes) {
        for o in &edit.overlays {
            let at = |t: Micros| format!("{:.2}", t.0 as f64 / 1e6);
            let verdict = match overlay::check(o) {
                Ok(_) => "ok".to_owned(),
                Err(e) => {
                    bad += 1;
                    e.to_string()
                }
            };
            println!(
                "{} {}..{} {}: {verdict}",
                edit.take,
                at(o.range.from()),
                at(o.range.to()),
                o.template
            );
        }
    }
    match bad {
        0 => Ok(()),
        n => Err(format!("{n} overlays do not fit their template")),
    }
}

fn write_cut(dir: &std::path::Path, cut: &Cut) -> Result<(), String> {
    let path = dir.join("cut.kdl");
    let tmp = dir.join(".cut.kdl.tmp");
    std::fs::write(&tmp, cut.to_kdl())
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("{}: {e}", path.display()))
}
