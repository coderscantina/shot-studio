//! `shot build`: rebuild a video's lock and the cached chunks of every cut, then check them,
//! once or on every change. With `--proposal` it builds a proposal's preview instead, in the
//! proposal's own cache space, leaving the branch's `cut.lock` and cache as they were ([0004]).
//!
//! [0004]: ../../../docs/decisions/0004-proposals-as-branches.md

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime};

use clap::Args;
use notify::{RecursiveMode, Watcher};
use shot_board::Board;
use shot_core::{Project, Script};
use shot_mcp::style::{self, Disk, Patched, Repo};
use shot_render::export::{self, ExportOptions, ExportReport};

use crate::export::load;
use crate::migrate::LOCK;

#[derive(Args)]
pub struct BuildArgs {
    /// The video folder, holding cut.kdl and media/.
    #[arg(long)]
    video: PathBuf,
    /// Build again on every change to cut.kdl, a style layer, an element file or a take.
    #[arg(long, conflicts_with = "proposal")]
    watch: bool,
    /// Build this proposal's preview: its lock and the chunks it changes.
    #[arg(long)]
    proposal: Option<String>,
    /// Encoder sessions running at the same time.
    #[arg(long, default_value_t = ExportOptions::default().jobs)]
    jobs: usize,
}

/// How long the files stay unchanged before a build starts, so the events of one save build
/// once.
const QUIET: Duration = Duration::from_millis(150);

pub fn run(args: BuildArgs) -> Result<(), String> {
    let options = ExportOptions { jobs: args.jobs };
    if let Some(name) = &args.proposal {
        return proposal(&args.video, name, options);
    }
    if !args.watch {
        let problems = build(&args.video, options, &|| false)?;
        return match problems {
            0 => Ok(()),
            1 => Err("1 problem".to_owned()),
            n => Err(format!("{n} problems")),
        };
    }
    watch(&args.video, options)
}

/// Rebuilds the lock, exports every cut and checks each one. Prints one line for the build,
/// then only the problems, and returns how many there are. The check stops once `changed`
/// returns true, since the next build replaces what it measures.
fn build(
    video: &Path,
    options: ExportOptions,
    changed: &(dyn Fn() -> bool + Sync),
) -> Result<usize, String> {
    let started = Instant::now();
    crate::restyle::refresh(video)?;
    let project = load(video)?;
    let cuts = cut_names(&project);
    let mut reports = Vec::with_capacity(cuts.len());
    for cut in &cuts {
        let out = shot_ship::ship::export_path(video, cut);
        let report = export::export(video, &project, cut, &out, options, &|_| {})
            .map_err(|e| format!("{cut}: {e}"))?;
        reports.push((cut.as_str(), report));
    }
    println!(
        "built {} in {:.1} s",
        summary(&reports),
        started.elapsed().as_secs_f64()
    );
    let mut problems = 0;
    for cut in &cuts {
        let out = shot_ship::ship::export_path(video, cut);
        let found = match shot_ship::media::check_cut_until(video, &project, cut, &out, changed) {
            Err(shot_ship::media::MediaError::Cancelled) => {
                println!("check stopped: the files changed");
                return Ok(problems);
            }
            found => found.map_err(|e| format!("{cut}: {e}"))?,
        };
        for problem in found {
            println!("{cut}: {problem}");
            problems += 1;
        }
    }
    Ok(problems)
}

/// The main cut, then every derived cut.
fn cut_names(project: &Project) -> Vec<String> {
    std::iter::once("main".to_owned())
        .chain(project.cut.derived.iter().map(|d| d.name.to_string()))
        .collect()
}

fn summary(reports: &[(&str, ExportReport)]) -> String {
    reports
        .iter()
        .map(|(cut, r)| format!("{cut} ({} of {} chunks rendered)", r.rendered, r.chunks))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Builds once, then again whenever what a build reads changed, until interrupted.
fn watch(video: &Path, options: ExportOptions) -> Result<(), String> {
    let video = video
        .canonicalize()
        .map_err(|e| format!("{}: {e}", video.display()))?;
    let inputs = Inputs::of(&video);
    let (events, incoming) = mpsc::channel();
    // Set by a change that matters, so a running check stops for the build after it.
    let changed = Arc::new(AtomicBool::new(false));
    let mut watcher = notify::recommended_watcher({
        let (inputs, changed) = (inputs.clone(), changed.clone());
        move |event: notify::Result<notify::Event>| {
            if event
                .as_ref()
                .is_ok_and(|e| e.paths.iter().any(|p| inputs.matters(p)))
            {
                changed.store(true, Ordering::Relaxed);
            }
            let _ = events.send(event);
        }
    })
    .map_err(|e| format!("watching files: {e}"))?;
    for root in inputs.roots() {
        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|e| format!("watching {}: {e}", root.display()))?;
    }
    println!("watching {}", video.display());
    let stop = || changed.load(Ordering::Relaxed);
    let mut built = inputs.fingerprint();
    report(build(&video, options, &stop));
    loop {
        // Wait for a change, then for the files to settle.
        loop {
            let event = incoming.recv().map_err(|_| "the file watcher stopped")?;
            if event.is_ok_and(|e| e.paths.iter().any(|p| inputs.matters(p))) {
                break;
            }
        }
        loop {
            match incoming.recv_timeout(QUIET) {
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("the file watcher stopped".into());
                }
            }
        }
        changed.store(false, Ordering::Relaxed);
        let now = inputs.fingerprint();
        if now == built {
            continue;
        }
        built = now;
        report(build(&video, options, &stop));
    }
}

fn report(result: Result<usize, String>) {
    if let Err(e) = result {
        println!("build failed: {e}");
    }
}

/// What a build of one video reads: its folder, the takes in `media/` (often a link to where
/// the recordings live) and the channel's style layers.
#[derive(Clone)]
struct Inputs {
    video: PathBuf,
    media: Option<PathBuf>,
    channel: Vec<PathBuf>,
}

impl Inputs {
    fn of(video: &Path) -> Self {
        let media = video.join("media").canonicalize().ok();
        let channel = match video.parent().filter(|p| p.ends_with("videos")) {
            Some(videos) => ["brand", "formats", "styles"]
                .iter()
                .filter_map(|dir| videos.with_file_name(dir).canonicalize().ok())
                .collect(),
            None => Vec::new(),
        };
        Self {
            video: video.to_owned(),
            media,
            channel,
        }
    }

    /// The folders to watch. The media folder is watched on its own when it lies elsewhere.
    fn roots(&self) -> Vec<PathBuf> {
        let media = self.media.iter().filter(|m| !m.starts_with(&self.video));
        std::iter::once(&self.video)
            .chain(media)
            .chain(&self.channel)
            .cloned()
            .collect()
    }

    /// Whether a change at `path` can change what a build makes. Hidden files, temporary
    /// files, the cache and exports under `.shot/` and the lock the build itself writes do not.
    fn matters(&self, path: &Path) -> bool {
        let within = |root: &PathBuf| path.strip_prefix(root).ok();
        let relative = within(&self.video)
            .filter(|p| *p != Path::new(LOCK) && p.extension().is_none_or(|e| e != "tmp"))
            .or_else(|| self.media.iter().chain(&self.channel).find_map(within));
        relative.is_some_and(|p| {
            !p.components().any(|c| {
                let name = c.as_os_str().to_string_lossy();
                name.starts_with('.') || name.ends_with('~')
            })
        })
    }

    /// The size and modification time of every file that [`Inputs::matters`], sorted, so two
    /// equal fingerprints mean nothing changed since.
    fn fingerprint(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut files = Vec::new();
        for root in self.roots() {
            walk(&root, &mut |path, meta| {
                if self.matters(path) {
                    let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    files.push((path.to_owned(), meta.len(), modified));
                }
            });
        }
        files.sort();
        files.dedup();
        files
    }
}

/// Calls `visit` for every file under `dir`, following links, skipping hidden folders.
fn walk(dir: &Path, visit: &mut dyn FnMut(&Path, &std::fs::Metadata)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            walk(&path, visit);
        } else {
            visit(&path, &meta);
        }
    }
}

/// Builds proposal `name`'s lock from its files into its cache space and renders the chunks of
/// every cut that no cache holds yet.
fn proposal(video: &Path, name: &str, options: ExportOptions) -> Result<(), String> {
    let started = Instant::now();
    let (project, lock) = proposal_version(video, name)?;
    let dir = export::proposal_dir(video, name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(LOCK);
    std::fs::write(&path, lock.to_kdl()).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reports = Vec::new();
    let cuts = cut_names(&project);
    for cut in &cuts {
        let report = export::render_proposal(video, &project, &lock, name, cut, options, &|_| {})
            .map_err(|e| format!("{cut}: {e}"))?;
        reports.push((cut.as_str(), report));
    }
    println!(
        "built proposal {name}: {} in {:.1} s, in {}",
        summary(&reports),
        started.elapsed().as_secs_f64(),
        dir.display()
    );
    Ok(())
}

/// The project and lock of the video in `video` as proposal `name` has it: the proposal's
/// files on top of the video's, built like the video's own lock.
fn proposal_version(video: &Path, name: &str) -> Result<(Project, shot_core::Lock), String> {
    if !video.join(LOCK).exists() {
        return Err(format!(
            "{} is not migrated: proposal previews build a lock, run `shot migrate` first",
            video.display()
        ));
    }
    let (disk, slug) = Disk::locate(video)?;
    let root = video
        .canonicalize()
        .ok()
        .and_then(|v| v.parent()?.parent().map(Path::to_owned))
        .ok_or_else(|| format!("{} is not in a channel repo", video.display()))?;
    let files = Board::open(&root)
        .and_then(|board| board.proposal_files(&slug, name))
        .map_err(|e| e.to_string())?;
    let repo = Patched {
        inner: &disk,
        files: &files,
    };
    let built = style::build_video(&repo, &slug)?;
    let inputs = style::inputs(&repo, &slug)?;
    let script_path = style::video_path(&slug, "script.md");
    let script = repo
        .read(&slug, &script_path)?
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let project = Project {
        script: Script::parse(&script).map_err(|e| format!("{script_path}: {e}"))?,
        format: built
            .video
            .format
            .preset()
            .resolve([&inputs.format, &inputs.video_layer]),
        cut: built.cut,
        video: built.video,
    };
    Ok((project, built.lock))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changes_a_build_reads_count() {
        let repo = tempfile::tempdir().unwrap();
        let video = repo.path().join("videos/alpha");
        for dir in [
            video.join("elements"),
            video.join(".shot/cache"),
            repo.path().join("brand"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let inputs = Inputs::of(&video.canonicalize().unwrap());
        let at = |path: &str| inputs.video.join(path);
        assert!(inputs.matters(&at("cut.kdl")));
        assert!(inputs.matters(&at("format.kdl")));
        assert!(inputs.matters(&at("elements/t01.jsonl")));
        assert!(inputs.matters(&inputs.channel[0].join("brand.kdl")));
        assert!(!inputs.matters(&at("cut.lock")), "the build writes it");
        assert!(!inputs.matters(&at("cut.tmp")));
        assert!(!inputs.matters(&at(".cut.kdl.tmp")));
        assert!(!inputs.matters(&at(".shot/export/main.mp4")));
        assert!(!inputs.matters(&at("cut.kdl~")));
        assert!(!inputs.matters(Path::new("/elsewhere/cut.kdl")));
    }

    #[test]
    fn the_fingerprint_moves_with_a_save_and_not_with_the_build() {
        let repo = tempfile::tempdir().unwrap();
        let video = repo.path().join("videos/alpha");
        std::fs::create_dir_all(video.join(".shot/export")).unwrap();
        std::fs::write(video.join("cut.kdl"), "a").unwrap();
        let inputs = Inputs::of(&video.canonicalize().unwrap());
        let before = inputs.fingerprint();
        std::fs::write(video.join("cut.lock"), "lock 1").unwrap();
        std::fs::write(video.join(".shot/export/main.mp4"), "x").unwrap();
        assert_eq!(inputs.fingerprint(), before);
        std::fs::write(video.join("cut.kdl"), "ab").unwrap();
        assert_ne!(inputs.fingerprint(), before);
    }
}
