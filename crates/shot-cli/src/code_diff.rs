//! `shot code-diff`: the product video effect that writes `cut.kdl`.

use std::path::{Path, PathBuf};

use clap::Args;
use shot_core::{Cut, Id, Micros, TimeRange};
use shot_render::code_diff::{self, CodeDiffOverlay};

#[derive(Args)]
pub struct CodeDiffArgs {
    /// The video folder.
    #[arg(long)]
    video: PathBuf,
    /// The git repository the code comes from.
    #[arg(long)]
    code: PathBuf,
    /// The commit range, `from..to`.
    #[arg(long)]
    range: String,
    /// The snapshot's name: it is written to sources/<name>.diff.
    #[arg(long)]
    name: String,
    /// The take the overlay sits on.
    #[arg(long)]
    take: String,
    /// Media time in seconds where the overlay starts and ends.
    #[arg(long)]
    from: f64,
    #[arg(long)]
    to: f64,
    /// The line of the snapshot to highlight, from 1.
    #[arg(long)]
    line: Option<usize>,
    /// Highlight from when the voice says this phrase, found in the take's transcript.
    #[arg(long, requires = "line")]
    phrase: Option<String>,
}

pub fn run(args: CodeDiffArgs) -> Result<(), String> {
    let (_lock, opened) = crate::open(&args.video)?;
    let project = opened.project;
    let take =
        Id::new(args.take.as_str()).ok_or_else(|| format!("{} is not a take id", args.take))?;
    let micros = |seconds: f64| Micros((seconds * 1e6).round() as u64);
    let range =
        TimeRange::new(micros(args.from), micros(args.to)).ok_or("--from must be before --to")?;
    let snapshot = code_diff::snapshot(&args.code, &args.range)
        .map_err(|e| format!("{}: {e}", args.code.display()))?;
    let file = format!("sources/{}.diff", args.name);
    let highlight_at = match &args.phrase {
        Some(phrase) => {
            let transcript = shot_analyze::cache::read_cached(&args.video, &take)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("take {take} has no transcript yet"))?;
            Some(
                code_diff::phrase_at(transcript.words(), phrase)
                    .ok_or_else(|| format!("take {take} never says \"{phrase}\""))?,
            )
        }
        None => None,
    };
    let overlay = CodeDiffOverlay {
        range,
        diff: file.clone(),
        highlight_line: args.line,
        highlight_at,
    }
    .to_overlay();
    let mut cut = project.cut.clone();
    let block = cut
        .chapters
        .iter_mut()
        .flat_map(|c| &mut c.takes)
        .find(|t| t.take == take && t.ranges.iter().any(|r| r.range().overlaps(range)))
        .ok_or_else(|| format!("the cut shows no part of {take} in that range"))?;
    block.overlays.push(overlay);
    let path = args.video.join(&file);
    std::fs::create_dir_all(args.video.join("sources")).map_err(|e| e.to_string())?;
    shot_fs::write(&path, snapshot).map_err(|e| format!("{}: {e}", path.display()))?;
    write_cut(&args.video, &cut)?;
    println!("wrote {file} and a code-diff overlay on {take}");
    Ok(())
}

fn write_cut(dir: &Path, cut: &Cut) -> Result<(), String> {
    let path = dir.join("cut.kdl");
    shot_fs::write(&path, cut.to_kdl()).map_err(|e| format!("{}: {e}", path.display()))
}
