//! `shot style`: build a video's `cut.lock`, explain what it applied at a time, and measure
//! how far a rule reaches. The logic is `shot_mcp::style`, shared with the MCP tools.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use shot_core::{Id, Micros, Selector, Status};
use shot_mcp::style::{self, Disk};
use shot_ship::lock::LOCK_FILE;

#[derive(Args)]
pub struct StyleArgs {
    #[command(subcommand)]
    action: StyleAction,
}

#[derive(Subcommand)]
enum StyleAction {
    /// Build the video's lock from its files and write cut.lock. A shipped video's lock is
    /// frozen at its release; `shot restyle` rebuilds it.
    Build {
        /// The video folder, `videos/<slug>` of the channel repo.
        #[arg(long, default_value = ".", conflicts_with = "all")]
        video: PathBuf,
        /// Every unshipped video of the channel (`--repo`).
        #[arg(long)]
        all: bool,
    },
    /// List the instances at a time with the layer and rule or override behind each.
    Explain {
        #[arg(long, default_value = ".")]
        video: PathBuf,
        /// Media time of the take in seconds.
        #[arg(long)]
        at: f64,
        /// Only this take. Default: every take of the main cut.
        #[arg(long)]
        take: Option<String>,
    },
    /// How many instances and videos a rule reaches across the channel (`--repo`).
    Scope {
        /// The rule's selector as the lock writes it, for example `silence min=500_000`.
        #[arg(long)]
        rule: String,
    },
}

pub fn run(repo: PathBuf, args: StyleArgs) -> Result<(), String> {
    match args.action {
        StyleAction::Build { all: true, .. } => build_all(repo),
        StyleAction::Build { video, .. } => write(&video),
        StyleAction::Explain { video, at, take } => explain(&video, at, take.as_deref()),
        StyleAction::Scope { rule } => {
            let selector = Selector::parse(&rule).map_err(|e| e.to_string())?;
            let reach = style::reach(&Disk::new(repo), &selector)?;
            print!("on {selector}\n{reach}");
            Ok(())
        }
    }
}

/// Builds every video of the channel; one that fails is listed and left alone.
fn build_all(repo: PathBuf) -> Result<(), String> {
    for slug in style::Repo::slugs(&Disk::new(&repo))? {
        if let Err(e) = write(&repo.join("videos").join(&slug)) {
            println!("videos/{slug}: {e}");
        }
    }
    Ok(())
}

/// Opens the video in `video`, which builds its lock and writes it when it changed.
fn write(video: &Path) -> Result<(), String> {
    let before = std::fs::read_to_string(video.join(LOCK_FILE)).ok();
    let (_, opened) = crate::open(video)?;
    let slug = &opened.slug;
    if opened.project.video.status == Status::Shipped {
        return Err(format!(
            "shipped, its lock is frozen at the release: `shot restyle {slug}` rebuilds it"
        ));
    }
    let lock = &opened.lock;
    let instances: usize = lock.takes.iter().map(|t| t.instances.len()).sum();
    let orphans: usize = lock.takes.iter().map(|t| t.orphans.len()).sum();
    let unchanged = before.as_deref() == Some(lock.to_kdl().as_str());
    println!(
        "{}: {instances} instances, {orphans} orphan overrides{}",
        style::video_path(slug, LOCK_FILE),
        if unchanged { ", unchanged" } else { "" }
    );
    Ok(())
}

fn explain(video: &Path, at: f64, take: Option<&str>) -> Result<(), String> {
    if !at.is_finite() || at < 0.0 {
        return Err("--at is a time in seconds".to_owned());
    }
    let take = take
        .map(|t| Id::new(t).ok_or_else(|| format!("`{t}` is not a take id")))
        .transpose()?;
    let (_, opened) = crate::open(video)?;
    let built = style::build_video(&opened.disk, &opened.slug)?;
    print!(
        "{}",
        style::explain(&built, take.as_ref(), Micros((at * 1e6).round() as u64))
    );
    Ok(())
}
