//! `shot derived`: add, list and remove Shorts and X clips in `cut.kdl`. `shot export --cut
//! <name>` renders one.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use shot_core::{CaptionStyle, Clip, DerivedCut, DerivedKind, Id, Micros, TimeRange};

#[derive(Args)]
pub struct DerivedArgs {
    /// The video folder.
    #[arg(long, global = true, default_value = ".")]
    video: PathBuf,
    #[command(subcommand)]
    action: DerivedAction,
}

#[derive(Subcommand)]
enum DerivedAction {
    /// Add a clip to a derived cut, creating the cut if it is new.
    Add {
        /// The cut's name, an id like `friday-deploys`.
        name: String,
        /// `short` (9:16, reframed on the face) or `x-clip` (1:1).
        #[arg(long, default_value = "short", value_parser = kind)]
        kind: DerivedKind,
        #[arg(long)]
        take: String,
        /// Media time in seconds where the clip starts and ends.
        #[arg(long)]
        from: f64,
        #[arg(long)]
        to: f64,
        /// Overrides the main cut's caption style: `word-pop`, `clean` or `off`.
        #[arg(long, value_parser = style)]
        captions: Option<CaptionStyle>,
    },
    /// List the derived cuts.
    List,
    /// Remove a derived cut.
    Remove { name: String },
}

fn kind(text: &str) -> Result<DerivedKind, String> {
    DerivedKind::ALL
        .iter()
        .copied()
        .find(|k| k.as_str() == text)
        .ok_or_else(|| "expected short or x-clip".to_owned())
}

fn style(text: &str) -> Result<CaptionStyle, String> {
    CaptionStyle::ALL
        .iter()
        .copied()
        .find(|s| s.as_str() == text)
        .ok_or_else(|| "expected word-pop, clean or off".to_owned())
}

pub fn run(args: DerivedArgs) -> Result<(), String> {
    let project = crate::export::load(&args.video)?;
    let mut cut = project.cut;
    let id = |text: &str| Id::new(text).ok_or_else(|| format!("{text} is not an id"));
    match args.action {
        DerivedAction::Add {
            name,
            kind,
            take,
            from,
            to,
            captions,
        } => {
            let micros = |seconds: f64| Micros((seconds * 1e6).round() as u64);
            let clip = Clip {
                take: id(&take)?,
                range: TimeRange::new(micros(from), micros(to))
                    .ok_or("--from must be before --to")?,
            };
            let name = id(&name)?;
            match cut.derived.iter_mut().find(|d| d.name == name) {
                Some(existing) if existing.kind != kind => {
                    return Err(format!("{name} is a {}", existing.kind));
                }
                Some(existing) => {
                    existing.clips.push(clip);
                    if captions.is_some() {
                        existing.captions = captions;
                    }
                }
                None => cut.derived.push(DerivedCut {
                    kind,
                    name,
                    captions,
                    clips: vec![clip],
                }),
            }
        }
        DerivedAction::List => {
            for d in &cut.derived {
                let seconds: u64 = d.clips.iter().map(|c| c.range.duration().0).sum();
                println!(
                    "{} {} {} clips, {:.1} s of source{}",
                    d.kind,
                    d.name,
                    d.clips.len(),
                    seconds as f64 / 1e6,
                    d.captions
                        .map_or(String::new(), |c| format!(", captions {c}"))
                );
            }
            return Ok(());
        }
        DerivedAction::Remove { name } => {
            let before = cut.derived.len();
            cut.derived.retain(|d| d.name.as_str() != name);
            if cut.derived.len() == before {
                return Err(format!("there is no derived cut named {name}"));
            }
        }
    }
    // Parsing the written text checks what the contract asks, such as the take being in the
    // main cut, before the file changes.
    let text = cut.to_kdl();
    shot_core::Cut::parse(&text).map_err(|e| format!("cut.kdl:{e}"))?;
    let path = args.video.join("cut.kdl");
    let tmp = args.video.join(".cut.kdl.tmp");
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("{}: {e}", path.display()))
}
