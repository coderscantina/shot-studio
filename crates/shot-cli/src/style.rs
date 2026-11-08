//! `shot style`: build a video's `cut.lock`, explain what it applied at a time, and measure
//! how far a rule reaches. The logic is `shot_mcp::style`, shared with the MCP tools.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use shot_core::{Id, Micros, Selector};
use shot_mcp::style::{self, Disk};

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
        /// Every migrated, unshipped video of the channel (`--repo`).
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
        StyleAction::Build { video, .. } => build(&video),
        StyleAction::Explain { video, at, take } => explain(&video, at, take.as_deref()),
        StyleAction::Scope { rule } => {
            let selector = Selector::parse(&rule).map_err(|e| e.to_string())?;
            let reach = style::reach(&Disk::new(repo), &selector)?;
            print!("on {selector}\n{reach}");
            Ok(())
        }
    }
}

fn build(video: &std::path::Path) -> Result<(), String> {
    let (disk, slug) = Disk::locate(video)?;
    write(&disk, &slug)
}

/// Builds every video with a lock; a shipped or unmigrated one is listed and left alone.
fn build_all(repo: PathBuf) -> Result<(), String> {
    let disk = Disk::new(repo);
    for slug in style::Repo::slugs(&disk)? {
        let path = style::video_path(&slug, "cut.lock");
        if style::Repo::read(&disk, &slug, &path)?.is_none() {
            println!("videos/{slug}: no cut.lock, not migrated");
            continue;
        }
        if let Err(e) = write(&disk, &slug) {
            println!("videos/{slug}: {e}");
        }
    }
    Ok(())
}

fn write(disk: &Disk, slug: &str) -> Result<(), String> {
    let built = style::build_video(disk, slug)?;
    if built.shipped() {
        return Err(format!(
            "shipped, its lock is frozen at the release: `shot restyle {slug}` rebuilds it"
        ));
    }
    let path = style::video_path(slug, "cut.lock");
    let written = built.lock.to_kdl();
    let unchanged =
        style::Repo::read(disk, slug, &path)?.is_some_and(|old| old == written.as_bytes());
    let instances: usize = built.lock.takes.iter().map(|t| t.instances.len()).sum();
    let orphans: usize = built.lock.takes.iter().map(|t| t.orphans.len()).sum();
    if !unchanged {
        disk.write(&path, &written)?;
    }
    println!(
        "{path}: {instances} instances, {orphans} orphan overrides{}",
        if unchanged { ", unchanged" } else { "" }
    );
    Ok(())
}

fn explain(video: &std::path::Path, at: f64, take: Option<&str>) -> Result<(), String> {
    if !at.is_finite() || at < 0.0 {
        return Err("--at is a time in seconds".to_owned());
    }
    let take = take
        .map(|t| Id::new(t).ok_or_else(|| format!("`{t}` is not a take id")))
        .transpose()?;
    let (disk, slug) = Disk::locate(video)?;
    let built = style::build_video(&disk, &slug)?;
    print!(
        "{}",
        style::explain(&built, take.as_ref(), Micros((at * 1e6).round() as u64))
    );
    Ok(())
}
