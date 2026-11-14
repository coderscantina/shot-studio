//! `shot restyle`: rebuilds a shipped video's lock on purpose. Shipping freezes the lock at the
//! release tag, so `shot style build`, export and check leave a shipped video's lock alone, and
//! a rule change reaches unshipped videos only ([0019]).
//!
//! [0019]: ../../../docs/decisions/0019-semantic-video-elements-and-style.md

use std::path::{Path, PathBuf};

use clap::Args;
use shot_mcp::style::{self, Disk, Repo};

use shot_ship::lock::LOCK_FILE as LOCK;

#[derive(Args)]
pub struct RestyleArgs {
    /// The shipped video's slug in the channel repo (`--repo`).
    slug: String,
}

pub fn run(repo: PathBuf, args: RestyleArgs) -> Result<(), String> {
    let folder = crate::video_folder(&repo, Path::new(&args.slug))?;
    let _lock = crate::lock(&folder)?;
    let (disk, _) = Disk::locate(&folder)?;
    let built = style::build_video(&disk, &args.slug)?;
    if !built.shipped() {
        return Err(format!(
            "{} is not shipped: `shot style build` keeps its lock current",
            args.slug
        ));
    }
    let path = style::video_path(&args.slug, LOCK);
    let old = disk.read(&args.slug, &path)?.unwrap_or_default();
    let new = built.lock.to_kdl();
    if old == new.as_bytes() {
        println!("{path}: unchanged, the rules give what shipped");
        return Ok(());
    }
    let old = String::from_utf8_lossy(&old);
    let lines = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
    let (before, after) = (lines(&old), lines(&new));
    let added = after.iter().filter(|l| !before.contains(l)).count();
    let removed = before.iter().filter(|l| !after.contains(l)).count();
    disk.write(&path, &new)?;
    println!(
        "{path}: rebuilt, {added} lines added, {removed} removed. Commit it and ship again for a new release"
    );
    Ok(())
}
