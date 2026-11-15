//! `shot migrate`: moves videos written before the lock onto style rules, one or every video of
//! the channel, and the channel's brand onto theme files. Opening a video migrates it too ([`shot_mcp::style::open`]); this also runs
//! analysis for every take without an elements file first, so the rules find their elements.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;
use shot_mcp::style::{self, Disk};
use shot_ship::lock::LOCK_FILE;

#[derive(Args)]
pub struct MigrateArgs {
    /// One video folder. Without it, every video of the channel (`--repo`).
    #[arg(long)]
    pub(crate) video: Option<PathBuf>,
}

pub fn run(repo: PathBuf, args: MigrateArgs) -> Result<(), String> {
    if args.video.is_none() && shot_mcp::brand::migrate(&repo)? {
        println!("brand/brand.kdl: its look moved to brand/themes/");
    }
    let videos = match args.video {
        Some(video) => vec![video],
        None => style::Repo::slugs(&Disk::new(&repo))?
            .into_iter()
            .map(|slug| crate::video_folder(&repo, &repo.join("videos").join(slug)))
            .collect::<Result<_, _>>()?,
    };
    let mut failed = 0;
    for video in &videos {
        if let Err(e) = migrate(video) {
            println!("{}: {e}", video.display());
            failed += 1;
        }
    }
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} of {} videos did not migrate", videos.len())),
    }
}

fn migrate(dir: &Path) -> Result<(), String> {
    if dir.join(LOCK_FILE).exists() {
        println!("{}: on style rules already", dir.display());
        return Ok(());
    }
    let started = Instant::now();
    if dir.join("media").is_dir() {
        let _lock = crate::lock(dir)?;
        for take in crate::analyze::takes(dir)? {
            if !dir.join("elements").join(format!("{take}.jsonl")).exists() {
                crate::analyze::analyze_take(dir, &take, false)?;
            }
        }
    }
    let analyzed = started.elapsed();
    let (_, opened) = crate::open(dir)?;
    let instances: usize = opened.lock.takes.iter().map(|t| t.instances.len()).sum();
    match &opened.migrated {
        Some(_) => println!(
            "{}: the lock has {instances} instances",
            style::video_path(&opened.slug, LOCK_FILE)
        ),
        None => println!("{}: nothing to migrate, it has a lock now", dir.display()),
    }
    eprintln!(
        "elements in {:.2} s, migration in {:.2} s",
        analyzed.as_secs_f64(),
        (started.elapsed() - analyzed).as_secs_f64()
    );
    Ok(())
}
