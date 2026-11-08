//! `shot restyle`: rebuilds a shipped video's lock on purpose. Shipping freezes the lock at the
//! release tag, so `shot style build`, export and check leave a shipped video's lock alone, and
//! a rule change reaches unshipped videos only ([0019]).
//!
//! [0019]: ../../../docs/decisions/0019-semantic-video-elements-and-style.md

use std::path::{Path, PathBuf};

use clap::Args;
use shot_mcp::style::{self, Disk, Repo};

use crate::migrate::LOCK;

#[derive(Args)]
pub struct RestyleArgs {
    /// The shipped video's slug in the channel repo (`--repo`).
    slug: String,
}

pub fn run(repo: PathBuf, args: RestyleArgs) -> Result<(), String> {
    let disk = Disk::new(repo);
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

/// Rebuilds a migrated video's lock from its files before it is rendered or checked, unless it
/// shipped. A folder outside a channel repo keeps the lock it has.
pub(crate) fn refresh(dir: &Path) -> Result<(), String> {
    if !dir.join(LOCK).exists() {
        return Ok(());
    }
    let Ok((disk, slug)) = Disk::locate(dir) else {
        return Ok(());
    };
    let built = style::build_video(&disk, &slug)?;
    if built.shipped() {
        return Ok(());
    }
    let path = style::video_path(&slug, LOCK);
    let text = built.lock.to_kdl();
    if disk
        .read(&slug, &path)?
        .is_none_or(|old| old != text.as_bytes())
    {
        disk.write(&path, &text)?;
        eprintln!("{path}: rebuilt from the files");
    }
    Ok(())
}
