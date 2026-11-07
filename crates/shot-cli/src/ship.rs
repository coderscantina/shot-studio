//! `shot check`, `shot thumbnails` and `shot describe`: a cut before it ships.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Args;
use shot_core::{Id, Project, Transcript};

use crate::export::load;

#[derive(Args)]
pub struct CheckArgs {
    /// The video folder.
    #[arg(long)]
    video: PathBuf,
    /// The cut to check.
    #[arg(long, default_value = "main")]
    cut: String,
    /// The exported file. Defaults to .shot/export/<cut>.mp4 in the video folder, where
    /// `shot export` writes it.
    #[arg(long)]
    out: Option<PathBuf>,
}

/// Prints only the problems; a cut that passes prints nothing and exits 0.
pub fn check(args: CheckArgs) -> Result<(), String> {
    let project = load(&args.video)?;
    let export = args
        .out
        .unwrap_or_else(|| export_path(&args.video, &args.cut));
    let problems = shot_ship::media::check_cut(&args.video, &project, &args.cut, &export)
        .map_err(|e| e.to_string())?;
    for problem in &problems {
        println!("{problem}");
    }
    match problems.len() {
        0 => Ok(()),
        1 => Err(format!("1 problem in {}", args.cut)),
        n => Err(format!("{n} problems in {}", args.cut)),
    }
}

#[derive(Args)]
pub struct ThumbnailsArgs {
    /// The video folder.
    #[arg(long)]
    video: PathBuf,
    /// How many candidates to write.
    #[arg(long, default_value_t = 6)]
    count: usize,
    /// Use this still as the main cut's thumbnail instead of finding candidates.
    #[arg(long)]
    pick: Option<PathBuf>,
}

pub fn thumbnails(args: ThumbnailsArgs) -> Result<(), String> {
    if let Some(still) = args.pick {
        let to = export_path(&args.video, "main").with_extension("png");
        fs::create_dir_all(to.parent().expect("has a parent"))
            .and_then(|()| fs::copy(&still, &to).map(drop))
            .map_err(|e| format!("{}: {e}", to.display()))?;
        println!("thumbnail is {}", to.display());
        return Ok(());
    }
    let project = load(&args.video)?;
    let found = shot_ship::thumbnails::find(&args.video, &project, args.count)
        .map_err(|e| e.to_string())?;
    if found.is_empty() {
        return Err("no face found in the camera frames the main cut shows".to_owned());
    }
    for (index, candidate) in found.iter().enumerate() {
        println!(
            "{}. {} at {} (score {:.2})  {}",
            index + 1,
            candidate.take,
            shot_ship::clock(candidate.at),
            candidate.score,
            candidate
                .path
                .as_deref()
                .map(Path::display)
                .map(|p| p.to_string())
                .unwrap_or_default()
        );
    }
    Ok(())
}

#[derive(Args)]
pub struct DescribeArgs {
    /// The video folder.
    #[arg(long)]
    video: PathBuf,
}

pub fn describe(args: DescribeArgs) -> Result<(), String> {
    let project = load(&args.video)?;
    let transcripts = transcripts(&args.video, &project)?;
    let description = shot_ship::describe::describe(&project, &transcripts);
    println!("{}\n\n{}", description.title, description.text.trim_end());
    Ok(())
}

fn export_path(video: &Path, cut: &str) -> PathBuf {
    video.join(".shot/export").join(format!("{cut}.mp4"))
}

fn transcripts(video: &Path, project: &Project) -> Result<HashMap<Id, Transcript>, String> {
    let mut found = HashMap::new();
    for take in project
        .cut
        .chapters
        .iter()
        .flat_map(|c| &c.takes)
        .map(|t| &t.take)
    {
        if !found.contains_key(take)
            && let Some(transcript) =
                shot_analyze::cache::read_cached(video, take).map_err(|e| e.to_string())?
        {
            found.insert(take.clone(), transcript);
        }
    }
    Ok(found)
}
