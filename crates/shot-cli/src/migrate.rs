//! `shot migrate`: a video written before the lock becomes content, overrides and the rules of
//! its layers, once. The mapping is `shot_core::migrate`, which refuses a result that would
//! show other frames; this reads and writes the files.

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::Args;
use shot_core::{Cut, Format, FormatLayer, MigrateInput, Project, Script, Video, migrate};
use shot_mcp::style::{self, Disk, Repo};

/// The lock's file in the video folder. A video with one is migrated.
pub(crate) const LOCK: &str = "cut.lock";

#[derive(Args)]
pub struct MigrateArgs {
    /// The video folder, `videos/<slug>` of the channel repo.
    #[arg(long)]
    video: PathBuf,
}

pub fn run(args: MigrateArgs) -> Result<(), String> {
    let dir = &args.video;
    if dir.join(LOCK).exists() {
        return Err(format!(
            "{} has a cut.lock, it is migrated already",
            dir.display()
        ));
    }
    let started = Instant::now();
    let (disk, slug) = Disk::locate(dir)?;
    let read = |name: &str| {
        std::fs::read_to_string(dir.join(name))
            .map_err(|e| format!("{}: {e}", dir.join(name).display()))
    };
    let cut = Cut::parse(&read("cut.kdl")?).map_err(|e| e.to_string())?;
    let mut seen = Vec::new();
    for take in cut.chapters.iter().flat_map(|c| &c.takes).map(|b| &b.take) {
        let file = dir.join("elements").join(format!("{take}.jsonl"));
        if !seen.contains(&take) && !file.exists() {
            crate::analyze::analyze_take(dir, take, false)?;
        }
        seen.push(take);
    }
    let analyzed = started.elapsed();

    let inputs = style::inputs(&disk, &slug)?;
    let format = Format::parse(&read("format.kdl")?)
        .map_err(|e| format!("{e}: a video before the lock has a full format.kdl"))?;
    let project = Project {
        video: inputs.video.clone(),
        script: Script::parse(&read("script.md")?).map_err(|e| e.to_string())?,
        format,
        cut: inputs.cut.clone(),
    };
    let migration = migrate(&MigrateInput {
        project: &project,
        imported: &inputs.imported,
        brand: &inputs.brand,
        channel: &inputs.format,
        elements: &inputs.elements,
        assets: &inputs.assets,
    })
    .map_err(|e| e.to_string())?;

    let path = |file: &str| style::video_path(&slug, file);
    if let Some(channel) = &migration.channel {
        let file = format!("formats/{}.kdl", project.video.format);
        disk.write(&file, &channel.to_kdl())?;
        println!("{file}: added the rules of its settings");
    }
    disk.write(&path("cut.kdl"), &migration.cut.to_kdl())?;
    disk.write(&path("format.kdl"), &migration.video.to_kdl())?;
    disk.write(&path(LOCK), &migration.lock.to_kdl())?;
    let rebuilt = style::build_video(&disk, &slug)?;
    if rebuilt.lock != migration.lock {
        return Err(format!(
            "{}: a build of the migrated files gives another lock",
            path(LOCK)
        ));
    }
    let instances: usize = migration.lock.takes.iter().map(|t| t.instances.len()).sum();
    println!(
        "migrated videos/{slug}: {}; {} has {instances} instances",
        migration.report,
        path(LOCK)
    );
    eprintln!(
        "elements in {:.2} s, migration in {:.2} s",
        analyzed.as_secs_f64(),
        (started.elapsed() - analyzed).as_secs_f64()
    );
    Ok(())
}

/// The settings of a video folder: a migrated video's `format.kdl` is a layer over its
/// channel's format, an unmigrated one a full format.
pub(crate) fn video_format(dir: &Path, video: &Video, text: &str) -> Result<Format, String> {
    if !dir.join(LOCK).exists() {
        return Format::parse(text).map_err(|e| e.to_string());
    }
    let layer = FormatLayer::parse(text).map_err(|e| e.to_string())?;
    let channel = match Disk::locate(dir) {
        Ok((disk, slug)) => disk
            .read(&slug, &format!("formats/{}.kdl", video.format))?
            .map(|bytes| {
                let text = String::from_utf8(bytes).map_err(|_| "formats/ is not UTF-8")?;
                FormatLayer::parse(&text).map_err(|e| e.to_string())
            })
            .transpose()?,
        Err(_) => None,
    };
    Ok(video
        .format
        .preset()
        .resolve(channel.iter().chain([&layer])))
}

/// Whether the old node-writing rule commands may still write into this video's `cut.kdl`.
pub(crate) fn refuse_if_migrated(dir: &Path, command: &str) -> Result<(), String> {
    if dir.join(LOCK).exists() {
        return Err(format!(
            "{} is migrated: its {command} come from the style rules of its layers, see `shot style explain`",
            dir.display()
        ));
    }
    Ok(())
}
