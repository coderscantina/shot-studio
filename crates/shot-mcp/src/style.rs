//! The semantic model for agents and the CLI: loads a video's style inputs from a channel repo,
//! builds its lock, explains instances and measures the scope of a change.
//!
//! The IO is behind [`Repo`], so the same code reads a working tree ([`Disk`]), a video's branch
//! tip ([`Git`]) or either with a pending change on top ([`Patched`]). The style engine itself
//! stays in `shot-core` and does no IO ([0019]).
//!
//! [0019]: ../../../docs/decisions/0019-semantic-video-elements-and-style.md

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use shot_board::Board;
use shot_core::{
    Applied, Brand, ContentHash, Cut, ElementKind, Elements, Format, FormatKind, FormatLayer, Id,
    Instance, Lock, LockTake, Micros, MigrateInput, MigrationReport, Override, Project, Provenance,
    Script, Selector, Status, StyleInput, TimeRange, Video, build,
};

/// A channel repo as the style loader reads it. Paths are relative to the repo root.
pub trait Repo {
    /// The bytes of `path` as video `slug` sees them, `None` if the file does not exist.
    fn read(&self, slug: &str, path: &str) -> Result<Option<Vec<u8>>, String>;
    /// Every video of the channel, sorted.
    fn slugs(&self) -> Result<Vec<String>, String>;
}

/// The working tree of a channel repo, or a video folder outside one: a channel of one, whose
/// `formats/<format>.kdl` is the shipped preset and whose brand is the default one.
pub struct Disk {
    root: PathBuf,
    /// The slug of the video when `root` is that video's folder outside a channel repo.
    alone: Option<String>,
}

impl Disk {
    /// The channel repo that holds `video`, a folder `videos/<slug>`, and the video's slug. Any
    /// other folder is a channel of one, its slug the folder's name.
    pub fn locate(video: &Path) -> Result<(Self, String), String> {
        let video = video
            .canonicalize()
            .map_err(|e| format!("{}: {e}", video.display()))?;
        let slug = video
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("{} is not a video folder", video.display()))?
            .to_owned();
        let videos = video.parent();
        match videos {
            Some(videos) if videos.file_name().is_some_and(|n| n == "videos") => {
                let root = videos.parent().unwrap_or(videos).to_path_buf();
                Ok((Self { root, alone: None }, slug))
            }
            _ => Ok((
                Self {
                    root: video,
                    alone: Some(slug.clone()),
                },
                slug,
            )),
        }
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            alone: None,
        }
    }

    /// The channel repo, or the video's folder for a channel of one.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether the video is in a channel repo rather than a channel of one.
    pub fn in_channel(&self) -> bool {
        self.alone.is_none()
    }

    /// Where repo path `path` is on disk. A channel of one has its video's files in its
    /// folder, and brand files relative to it as the renderer reads them.
    fn path(&self, path: &str) -> PathBuf {
        match &self.alone {
            Some(slug) => {
                let video = format!("videos/{slug}/");
                self.root.join(path.strip_prefix(&video).unwrap_or(path))
            }
            None => self.root.join(path),
        }
    }

    /// Replaces `path` with `text` through [`shot_fs::write`], so a reader never sees half a lock.
    pub fn write(&self, path: &str, text: &str) -> Result<(), String> {
        let target = self.path(path);
        shot_fs::write(&target, text).map_err(|e| format!("{}: {e}", target.display()))
    }
}

impl Repo for Disk {
    fn read(&self, _slug: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        if self.alone.is_some() {
            if let Some(kind) = path
                .strip_prefix("formats/")
                .and_then(|f| f.strip_suffix(".kdl"))
            {
                let kind = FormatKind::ALL.iter().find(|k| k.as_str() == kind);
                return Ok(kind.map(|k| k.preset_source().as_bytes().to_vec()));
            }
            if path.starts_with("brand/") || path.starts_with("styles/") {
                return Ok(None);
            }
        }
        let path = self.path(path);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    fn slugs(&self) -> Result<Vec<String>, String> {
        if let Some(slug) = &self.alone {
            return Ok(vec![slug.clone()]);
        }
        let dir = self.root.join("videos");
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut slugs: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        slugs.sort();
        Ok(slugs)
    }
}

/// The committed state of each video: its branch tip, else the default branch.
pub struct Git<'a>(pub &'a Board);

impl Repo for Git<'_> {
    fn read(&self, slug: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        self.0.read_repo_file(slug, path).map_err(|e| e.to_string())
    }

    fn slugs(&self) -> Result<Vec<String>, String> {
        let cards = self.0.cards().map_err(|e| e.to_string())?;
        Ok(cards.into_iter().map(|c| c.slug).collect())
    }
}

/// A repo with some files replaced: what the channel would be if a change were accepted.
pub struct Patched<'a> {
    pub inner: &'a dyn Repo,
    pub files: &'a BTreeMap<String, String>,
}

impl Repo for Patched<'_> {
    fn read(&self, slug: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        match self.files.get(path) {
            Some(text) => Ok(Some(text.clone().into_bytes())),
            None => self.inner.read(slug, path),
        }
    }

    fn slugs(&self) -> Result<Vec<String>, String> {
        self.inner.slugs()
    }
}

/// A video with its lock built from the files.
pub struct Built {
    pub video: Video,
    pub cut: Cut,
    /// The video's `format.kdl`.
    pub layer: FormatLayer,
    /// The channel's `formats/<format>.kdl`.
    pub channel: FormatLayer,
    pub elements: Vec<Elements>,
    pub lock: Lock,
}

impl Built {
    /// Frozen at the release: rule changes do not reach it.
    pub fn shipped(&self) -> bool {
        self.video.status == Status::Shipped
    }
}

/// `videos/<slug>/<file>`.
pub fn video_path(slug: &str, file: &str) -> String {
    format!("videos/{slug}/{file}")
}

fn text(repo: &dyn Repo, slug: &str, path: &str) -> Result<Option<String>, String> {
    repo.read(slug, path)?
        .map(|bytes| String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8")))
        .transpose()
}

fn parsed<T, E: std::fmt::Display>(
    path: &str,
    text: Option<String>,
    parse: impl FnOnce(&str) -> Result<T, E>,
) -> Result<Option<T>, String> {
    text.map(|t| parse(&t).map_err(|e| format!("{path}: {e}")))
        .transpose()
}

/// Everything a build of one video reads, parsed, with the hashes of the brand's assets.
pub struct Inputs {
    pub video: Video,
    pub cut: Cut,
    /// The video's `format.kdl`.
    pub video_layer: FormatLayer,
    /// The channel's `formats/<format>.kdl`.
    pub format: FormatLayer,
    pub brand: Brand,
    pub imported: Vec<Brand>,
    pub elements: Vec<Elements>,
    pub assets: BTreeMap<String, ContentHash>,
}

/// Reads the files of video `slug`, hashes the assets the brand names and builds the lock. A
/// missing `brand/brand.kdl`, channel format or `format.kdl` is an empty layer.
pub fn build_video(repo: &dyn Repo, slug: &str) -> Result<Built, String> {
    let inputs = inputs(repo, slug)?;
    let lock = build(&StyleInput {
        imported: &inputs.imported,
        brand: &inputs.brand,
        format: &inputs.format,
        video: &inputs.video_layer,
        cut: &inputs.cut,
        elements: &inputs.elements,
        assets: &inputs.assets,
    })
    .map_err(|e| format!("{slug}: {e}"))?;
    Ok(Built {
        video: inputs.video,
        cut: inputs.cut,
        layer: inputs.video_layer,
        channel: inputs.format,
        elements: inputs.elements,
        lock,
    })
}

/// Reads what a build of video `slug` reads. Imports are read from `styles/<id>.kdl` of the
/// repo, the checkout of the pinned commit.
pub fn inputs(repo: &dyn Repo, slug: &str) -> Result<Inputs, String> {
    let read = |path: String| text(repo, slug, &path).map(|t| (path, t));
    let (path, video_text) = read(video_path(slug, "video.kdl"))?;
    let video =
        parsed(&path, video_text, Video::parse)?.ok_or_else(|| format!("{path} is missing"))?;
    let (path, cut_text) = read(video_path(slug, "cut.kdl"))?;
    let cut = parsed(&path, cut_text, Cut::parse)?.unwrap_or_default();
    let (path, layer_text) = read(video_path(slug, "format.kdl"))?;
    let video_layer = parsed(&path, layer_text, FormatLayer::parse)?.unwrap_or_default();
    let (path, format_text) = read(format!("formats/{}.kdl", video.format))?;
    let format = parsed(&path, format_text, FormatLayer::parse)?.unwrap_or_default();
    let (brand, imported, assets) = brand_inputs(repo, slug, &format, &video_layer)?;
    let mut takes: Vec<&Id> = Vec::new();
    for take in cut.chapters.iter().flat_map(|c| &c.takes).map(|t| &t.take) {
        if !takes.contains(&take) {
            takes.push(take);
        }
    }
    let elements = read_elements(repo, slug, takes)?;
    Ok(Inputs {
        video,
        cut,
        video_layer,
        format,
        brand,
        imported,
        elements,
        assets,
    })
}

/// The channel's brand with the look of the theme the layers pick, its imports read from
/// `styles/<id>.kdl`, and the hashes of the files they and the brand blocks of the layers name.
type BrandInputs = (Brand, Vec<Brand>, BTreeMap<String, ContentHash>);

fn brand_inputs(
    repo: &dyn Repo,
    slug: &str,
    channel: &FormatLayer,
    video: &FormatLayer,
) -> Result<BrandInputs, String> {
    let read = |path: String| text(repo, slug, &path).map(|t| (path, t));
    let (path, brand_text) = read(shot_core::BRAND_FILE.to_owned())?;
    let mut brand = parsed(&path, brand_text, Brand::parse)?.unwrap_or_default();
    if let Some(theme) = shot_core::picked_theme(&brand, channel, video) {
        let (path, theme_text) = read(shot_core::theme_file(theme))?;
        let theme_text =
            theme_text.ok_or_else(|| format!("{path} is missing, theme `{theme}` is named"))?;
        let look = parsed(&path, Some(theme_text), Brand::parse_theme)?.unwrap_or_default();
        brand = brand.with_look(&look);
    }
    let mut imported = Vec::new();
    for import in &brand.imports {
        let (path, import_text) = read(format!("styles/{}.kdl", import.style))?;
        let import_text = import_text
            .ok_or_else(|| format!("{path} is missing, the brand imports {}", import.style))?;
        imported.push(parsed(&path, Some(import_text), Brand::parse)?.unwrap_or_default());
    }
    let patches = [channel, video]
        .into_iter()
        .filter_map(|l| l.brand.as_ref());
    let named = imported
        .iter()
        .chain([&brand])
        .chain(patches)
        .flat_map(Brand::assets);
    let mut assets = BTreeMap::new();
    for path in named.collect::<BTreeSet<_>>() {
        if let Some(bytes) = repo.read(slug, path)? {
            assets.insert(path.to_owned(), ContentHash::of(&bytes));
        }
    }
    Ok((brand, imported, assets))
}

/// The elements files of `takes` that exist.
fn read_elements<'a>(
    repo: &dyn Repo,
    slug: &str,
    takes: impl IntoIterator<Item = &'a Id>,
) -> Result<Vec<Elements>, String> {
    let mut elements = Vec::new();
    for take in takes {
        let path = video_path(slug, &format!("elements/{take}.jsonl"));
        let jsonl = text(repo, slug, &path)?;
        if let Some(found) = parsed(&path, jsonl, |t| Elements::parse(take.clone(), t))? {
            elements.push(found);
        }
    }
    Ok(elements)
}

/// A video opened from its folder: on style rules, with its lock as the files build it.
pub struct Opened {
    pub disk: Disk,
    pub slug: String,
    pub project: Project,
    /// The video's `format.kdl`.
    pub layer: FormatLayer,
    /// The settings under the video's layer: the preset with the channel's layer.
    pub base: Format,
    pub lock: Lock,
    /// What opening did to a video written before the lock, `None` for one on style rules.
    pub migrated: Option<MigrationReport>,
}

/// Opens the video in `dir` the way the app, `shot` and the MCP server all open one, holding
/// its write lock is the caller's part:
///
/// - A video written before the lock (no `cut.lock`, and `pacing`, `auto-cut`, `auto-zoom` or
///   the rules' nodes in its files) is migrated with the elements it has, and refused with the
///   reason when that would change its frames. Its files are written, the channel's format too
///   when migration gives it rules.
/// - A video with a lock loses the old settings its format files still hold.
/// - The channel's brand files written before themes were files are migrated first, holding
///   the channel's lock, see [`crate::brand::migrate`].
/// - The lock is built from the files and written when it changed, unless the video shipped.
///
/// Anything that should follow a migration, such as a commit on the video's branch, goes here.
pub fn open(dir: &Path) -> Result<Opened, String> {
    let (disk, slug) = Disk::locate(dir)?;
    if disk.in_channel() {
        crate::brand::migrate(&disk.root)?;
    }
    let read = |path: &str| text(&disk, &slug, path);
    let lock_path = video_path(&slug, "cut.lock");
    let format_path = video_path(&slug, "format.kdl");
    let format = read(&format_path)?.unwrap_or_default();
    let cut = read(&video_path(&slug, "cut.kdl"))?.unwrap_or_default();
    let mut migrated = None;
    if read(&lock_path)?.is_none() && shot_core::written_before_lock(&format, &cut) {
        migrated = Some(migrate(&disk, &slug, &format, &cut)?);
    } else {
        let video_file = video_path(&slug, "video.kdl");
        let video = parsed(&video_file, read(&video_file)?, Video::parse)?
            .ok_or_else(|| format!("{video_file} is missing"))?;
        let channel_path = format!("formats/{}.kdl", video.format);
        for path in [format_path, channel_path] {
            if path.starts_with("formats/") && !disk.in_channel() {
                continue;
            }
            let Some(text) = read(&path)? else { continue };
            let (layer, dropped) =
                shot_core::without_old_settings(&text).map_err(|e| format!("{path}: {e}"))?;
            if dropped {
                disk.write(&path, &layer.to_kdl())?;
            }
        }
    }
    let built = build_video(&disk, &slug)?;
    let written = read(&lock_path)?;
    // A shipped video keeps the lock it shipped with.
    let lock = match written {
        Some(text) if built.shipped() => {
            Lock::parse(&text).map_err(|e| format!("{lock_path}: {e}"))?
        }
        written => {
            if written.as_deref() != Some(&built.lock.to_kdl()) {
                disk.write(&lock_path, &built.lock.to_kdl())?;
            }
            built.lock
        }
    };
    let script_path = video_path(&slug, "script.md");
    let script = parsed(&script_path, read(&script_path)?, Script::parse)?
        .ok_or_else(|| format!("{script_path} is missing"))?;
    let base = built.video.format.preset().resolve([&built.channel]);
    let project = Project {
        format: built.layer.apply(&base),
        video: built.video,
        script,
        cut: built.cut,
    };
    Ok(Opened {
        disk,
        slug,
        project,
        layer: built.layer,
        base,
        lock,
        migrated,
    })
}

/// Migrates video `slug`, written before the lock, with the elements it has, and writes the
/// migrated files and the lock.
fn migrate(disk: &Disk, slug: &str, format: &str, cut: &str) -> Result<MigrationReport, String> {
    let read = |path: &str| text(disk, slug, path);
    let path = |file: &str| video_path(slug, file);
    let video = parsed(&path("video.kdl"), read(&path("video.kdl"))?, Video::parse)?
        .ok_or_else(|| format!("{} is missing", path("video.kdl")))?;
    let script = parsed(&path("script.md"), read(&path("script.md"))?, Script::parse)?
        .ok_or_else(|| format!("{} is missing", path("script.md")))?;
    let channel_path = format!("formats/{}.kdl", video.format);
    let channel = read(&channel_path)?.unwrap_or_default();
    let (channel_layer, _) =
        shot_core::without_old_settings(&channel).map_err(|e| format!("{channel_path}: {e}"))?;
    let (brand, imported, assets) =
        brand_inputs(disk, slug, &channel_layer, &FormatLayer::default())?;
    let elements_dir = disk.path(&path("elements"));
    let takes: Vec<Id> = match std::fs::read_dir(&elements_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|name| Id::new(name.strip_suffix(".jsonl")?))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("{}: {e}", elements_dir.display())),
    };
    let elements = read_elements(disk, slug, &takes)?;
    let migration = shot_core::migrate(&MigrateInput {
        video: &video,
        script: &script,
        format,
        cut,
        imported: &imported,
        brand: &brand,
        channel: &channel,
        elements: &elements,
        assets: &assets,
    })
    .map_err(|e| format!("videos/{slug} does not open: {e}"))?;
    if let Some(channel) = &migration.channel {
        disk.write(&channel_path, &channel.to_kdl())?;
    }
    disk.write(&path("cut.kdl"), &migration.cut.to_kdl())?;
    disk.write(&path("format.kdl"), &migration.video.to_kdl())?;
    disk.write(&path("cut.lock"), &migration.lock.to_kdl())?;
    let rebuilt = build_video(disk, slug)?;
    if rebuilt.lock != migration.lock {
        return Err(format!(
            "{}: a build of the migrated files gives another lock",
            path("cut.lock")
        ));
    }
    Ok(migration.report)
}

/// Seconds with one decimal, as the other tools print times.
pub fn secs(t: Micros) -> String {
    format!("{:.1} s", t.0 as f64 / 1e6)
}

/// The start and end of what an instance covers.
fn span(applied: &Applied) -> (Micros, Micros) {
    match applied {
        Applied::Cut(range) | Applied::Review(range) => (range.from(), range.to()),
        Applied::Layout { range, .. }
        | Applied::Zoom { range, .. }
        | Applied::Overlay { range, .. }
        | Applied::Captions { range, .. } => (range.from(), range.to()),
        Applied::Sfx { at, .. } | Applied::Transition { at, .. } | Applied::Music { at, .. } => {
            (*at, *at)
        }
    }
}

/// An instance as `cut.lock` writes it, provenance included.
pub fn instance_text(take: &Id, instance: &Instance) -> String {
    let lock = Lock {
        takes: vec![LockTake {
            take: take.clone(),
            instances: vec![instance.clone()],
            orphans: Vec::new(),
        }],
        ..Lock::default()
    };
    let written = lock.to_kdl();
    let lines: Vec<&str> = written.lines().collect();
    lines[3..lines.len() - 1]
        .iter()
        .map(|l| l.strip_prefix("    ").unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n")
}

fn provenance(p: &Provenance) -> String {
    match (&p.rule, p.override_at) {
        (Some(rule), _) => format!("rule \"{rule}\" of the {} layer", p.layer),
        (None, Some(at)) => format!("the override at {}", secs(at)),
        (None, None) => format!("the {} layer", p.layer),
    }
}

fn indented(text: &str, by: &str) -> String {
    text.lines().map(|l| format!("{by}{l}\n")).collect()
}

/// The lock's instances with their provenance, optionally of one take and overlapping a window
/// of its media time. At most `limit` instances are written.
pub fn lock_text(
    lock: &Lock,
    take: Option<&Id>,
    window: (Option<Micros>, Option<Micros>),
    limit: usize,
) -> String {
    let (from, to) = window;
    let mut out = String::new();
    let mut shown = 0;
    let mut hidden = 0;
    for t in lock
        .takes
        .iter()
        .filter(|t| take.is_none_or(|id| *id == t.take))
    {
        let _ = writeln!(out, "take {} {{", t.take);
        for instance in &t.instances {
            let (start, end) = span(&instance.applied);
            if from.is_some_and(|f| end < f) || to.is_some_and(|t| start > t) {
                continue;
            }
            if shown == limit {
                hidden += 1;
                continue;
            }
            shown += 1;
            out.push_str(&indented(&instance_text(&t.take, instance), "    "));
        }
        for orphan in &t.orphans {
            let _ = writeln!(out, "    orphan {} at={}", orphan.kind, orphan.at.0);
        }
        out.push_str("}\n");
    }
    if hidden > 0 {
        let _ = writeln!(out, "({hidden} more instances, narrow the window)");
    }
    out
}

/// How near a moment must be to count as "at" a time.
const POINT_WINDOW: Micros = Micros(250_000);

/// Everything the lock applied at `at` in the media time of a take, and where each instance
/// came from, plus the overrides that sit there (an `off` leaves no instance behind).
pub fn explain(built: &Built, take: Option<&Id>, at: Micros) -> String {
    let mut out = String::new();
    for t in built
        .lock
        .takes
        .iter()
        .filter(|t| take.is_none_or(|id| *id == t.take))
    {
        let _ = writeln!(out, "{} at {}", t.take, secs(at));
        let mut any = false;
        for instance in &t.instances {
            let (start, end) = span(&instance.applied);
            let covers = if start == end {
                start.0.abs_diff(at.0) <= POINT_WINDOW.0
            } else {
                start <= at && at <= end
            };
            if !covers {
                continue;
            }
            any = true;
            let element = instance.element;
            out.push_str(&indented(&instance_text(&t.take, instance), "  "));
            let _ = writeln!(
                out,
                "    from {}, on the {} at {}",
                provenance(&instance.provenance),
                element.target,
                secs(element.t)
            );
        }
        for o in built
            .cut
            .overrides
            .iter()
            .filter(|o| o.take == t.take && o.at.0.abs_diff(at.0) <= Override::TOLERANCE.0)
        {
            any = true;
            let only = Cut {
                overrides: vec![o.clone()],
                ..Cut::default()
            };
            out.push_str("  in cut.kdl\n");
            out.push_str(&indented(only.to_kdl().trim_end(), "    "));
        }
        if !any {
            out.push_str("  nothing is applied here\n");
        }
    }
    if out.is_empty() {
        out.push_str("the main cut has no such take\n");
    }
    out
}

/// The elements of a take in a window, one canonical JSON line each, at most `limit`.
pub fn elements_text(
    elements: &Elements,
    kinds: &[ElementKind],
    window: (Option<Micros>, Option<Micros>),
    limit: usize,
) -> String {
    let (from, to) = window;
    let mut found = Elements::new(elements.take().clone());
    let mut hidden = 0;
    let matching = elements.iter().filter(|e| {
        let end = e.to.unwrap_or(e.t);
        (kinds.is_empty() || kinds.contains(&e.kind))
            && from.is_none_or(|f| end >= f)
            && to.is_none_or(|t| e.t <= t)
    });
    for element in matching {
        if found.len() == limit {
            hidden += 1;
        } else {
            let _ = found.push(element.clone());
        }
    }
    let mut out = found.to_jsonl();
    if hidden > 0 {
        let _ = writeln!(
            out,
            "({hidden} more elements, narrow the window or the kinds)"
        );
    }
    out
}

/// What a selector reaches in one video.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoReach {
    pub slug: String,
    pub elements: usize,
    pub instances: usize,
}

/// What a rule produces across a channel's unshipped videos.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reach {
    pub videos: Vec<VideoReach>,
    /// Shipped videos, which a rule change leaves alone.
    pub frozen: usize,
    pub unreadable: Vec<(String, String)>,
    /// Unshipped videos that were checked.
    pub checked: usize,
}

impl Reach {
    pub fn elements(&self) -> usize {
        self.videos.iter().map(|v| v.elements).sum()
    }

    pub fn instances(&self) -> usize {
        self.videos.iter().map(|v| v.instances).sum()
    }
}

impl std::fmt::Display for Reach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "reaches {} instances on {} elements in {} of {} videos",
            self.instances(),
            self.elements(),
            self.videos.len(),
            self.checked
        )?;
        for v in &self.videos {
            writeln!(
                f,
                "  {}  {} instances on {} elements",
                v.slug, v.instances, v.elements
            )?;
        }
        frozen_and_unreadable(f, self.frozen, &self.unreadable)
    }
}

fn frozen_and_unreadable(
    f: &mut std::fmt::Formatter<'_>,
    frozen: usize,
    unreadable: &[(String, String)],
) -> std::fmt::Result {
    if frozen > 0 {
        writeln!(f, "shipped videos keep their lock: {frozen}")?;
    }
    for (slug, error) in unreadable {
        writeln!(f, "{slug} cannot be read: {error}")?;
    }
    Ok(())
}

/// The instances `selector` produces now, as the winning rule of any layer, in every unshipped
/// video of the channel.
pub fn reach(repo: &dyn Repo, selector: &Selector) -> Result<Reach, String> {
    let mut result = Reach::default();
    for slug in repo.slugs()? {
        let built = match build_video(repo, &slug) {
            Ok(built) => built,
            Err(error) => {
                result.unreadable.push((slug, error));
                continue;
            }
        };
        if built.shipped() {
            result.frozen += 1;
            continue;
        }
        result.checked += 1;
        let mut elements = BTreeSet::new();
        let mut instances = 0;
        for t in &built.lock.takes {
            for i in t
                .instances
                .iter()
                .filter(|i| i.provenance.rule.as_ref() == Some(selector))
            {
                instances += 1;
                elements.insert((&t.take, i.element.target, i.element.t));
            }
        }
        if instances > 0 {
            result.videos.push(VideoReach {
                slug,
                elements: elements.len(),
                instances,
            });
        }
    }
    Ok(result)
}

/// How a change moves one video's lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoImpact {
    pub slug: String,
    pub added: usize,
    pub removed: usize,
    pub brand_changed: bool,
}

impl VideoImpact {
    fn is_empty(&self) -> bool {
        self.added == 0 && self.removed == 0 && !self.brand_changed
    }
}

/// The lock changes a set of files would cause across the videos they reach.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Impact {
    pub videos: Vec<VideoImpact>,
    pub frozen: usize,
    pub unreadable: Vec<(String, String)>,
    /// Videos the change reaches, whether or not their lock moves.
    pub checked: usize,
}

impl Impact {
    pub fn added(&self) -> usize {
        self.videos.iter().map(|v| v.added).sum()
    }

    pub fn removed(&self) -> usize {
        self.videos.iter().map(|v| v.removed).sum()
    }

    fn changed_videos(&self) -> impl Iterator<Item = &VideoImpact> {
        self.videos.iter().filter(|v| !v.is_empty())
    }
}

impl std::fmt::Display for Impact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let changed = self.changed_videos().count();
        writeln!(
            f,
            "Changes the lock of {changed} of {} videos: {} instances added, {} removed.",
            self.checked,
            self.added(),
            self.removed()
        )?;
        for v in self.changed_videos() {
            let brand = if v.brand_changed {
                ", brand differs"
            } else {
                ""
            };
            writeln!(f, "  {}  +{} -{}{brand}", v.slug, v.added, v.removed)?;
        }
        frozen_and_unreadable(f, self.frozen, &self.unreadable)
    }
}

/// Every instance and orphan of a lock as text, with how many times it appears.
fn lines(lock: &Lock) -> BTreeMap<(String, String), usize> {
    let mut counts = BTreeMap::new();
    for t in &lock.takes {
        let each = t.instances.iter().map(|i| instance_text(&t.take, i)).chain(
            t.orphans
                .iter()
                .map(|o| format!("orphan {} at={}", o.kind, o.at.0)),
        );
        for line in each {
            *counts.entry((t.take.to_string(), line)).or_insert(0) += 1;
        }
    }
    counts
}

fn difference(old: &Lock, new: &Lock) -> (usize, usize) {
    let (old, new) = (lines(old), lines(new));
    let count = |from: &BTreeMap<_, usize>, other: &BTreeMap<_, usize>| {
        from.iter()
            .map(|(k, n)| n.saturating_sub(other.get(k).copied().unwrap_or(0)))
            .sum()
    };
    (count(&new, &old), count(&old, &new))
}

/// The videos a change to `files` reaches: only `slug` when every file is in its folder, else
/// every video of the channel.
fn reached(
    repo: &dyn Repo,
    slug: &str,
    files: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let own = video_path(slug, "");
    if files.keys().all(|p| p.starts_with(&own)) {
        Ok(vec![slug.to_owned()])
    } else {
        repo.slugs()
    }
}

/// Builds the videos `files` reach with and without them and counts how their locks differ.
/// Shipped videos are frozen and skipped, unless the change is in their own folder.
pub fn impact(
    repo: &dyn Repo,
    slug: &str,
    files: &BTreeMap<String, String>,
) -> Result<Impact, String> {
    let patched = Patched { inner: repo, files };
    let mut result = Impact::default();
    for reached_slug in reached(repo, slug, files)? {
        let own =
            reached_slug == slug && files.keys().all(|p| p.starts_with(&video_path(slug, "")));
        let old = match build_video(repo, &reached_slug) {
            Ok(old) => old,
            Err(error) => {
                result.unreadable.push((reached_slug, error));
                continue;
            }
        };
        if old.shipped() && !own {
            result.frozen += 1;
            continue;
        }
        let new = build_video(&patched, &reached_slug)
            .map_err(|e| format!("the change breaks {reached_slug}: {e}"))?;
        result.checked += 1;
        let (added, removed) = difference(&old.lock, &new.lock);
        result.videos.push(VideoImpact {
            slug: reached_slug,
            added,
            removed,
            brand_changed: old.lock.brand != new.lock.brand,
        });
    }
    Ok(result)
}

/// Where a rule change is written, lowest layer last in the cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleLayer {
    /// The video's own `format.kdl`.
    Video,
    /// The channel's `formats/<format>.kdl` of the video's format.
    Format,
    /// `brand/brand.kdl`.
    Brand,
}

impl RuleLayer {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "video" => Ok(Self::Video),
            "format" => Ok(Self::Format),
            "brand" => Ok(Self::Brand),
            other => Err(format!("`{other}` is not video, format or brand")),
        }
    }
}

/// `format.kdl`, the channel format or `brand.kdl` with `rule` (one `on` node) written into
/// its layer: replacing the rule with the same selector, else added after the others.
pub fn rule_change(
    repo: &dyn Repo,
    slug: &str,
    layer: RuleLayer,
    rule: &str,
) -> Result<BTreeMap<String, String>, String> {
    let parsed = FormatLayer::parse(rule).map_err(|e| format!("the rule does not parse: {e}"))?;
    let [new] = parsed.rules.rules.as_slice() else {
        return Err("give exactly one `on` rule".to_owned());
    };
    if parsed
        != (FormatLayer {
            rules: parsed.rules.clone(),
            ..FormatLayer::default()
        })
    {
        return Err("give exactly one `on` rule and nothing else".to_owned());
    }
    let video = build_video(repo, slug)?.video;
    let path = match layer {
        RuleLayer::Video => video_path(slug, "format.kdl"),
        RuleLayer::Format => format!("formats/{}.kdl", video.format),
        RuleLayer::Brand => "brand/brand.kdl".to_owned(),
    };
    let current = text(repo, slug, &path)?;
    let put = |sheet: &mut shot_core::StyleSheet| match sheet
        .rules
        .iter_mut()
        .find(|r| r.selector == new.selector)
    {
        Some(existing) => *existing = new.clone(),
        None => sheet.rules.push(new.clone()),
    };
    let written = match layer {
        RuleLayer::Brand => {
            let mut brand = parsed_or_default(&path, current, Brand::parse)?;
            put(&mut brand.rules);
            brand.to_kdl()
        }
        RuleLayer::Video | RuleLayer::Format => {
            let mut file = parsed_or_default(&path, current, FormatLayer::parse)?;
            put(&mut file.rules);
            file.to_kdl()
        }
    };
    Ok(BTreeMap::from([(path, written)]))
}

fn parsed_or_default<T: Default, E: std::fmt::Display>(
    path: &str,
    text: Option<String>,
    parse: impl FnOnce(&str) -> Result<T, E>,
) -> Result<T, String> {
    Ok(parsed(path, text, parse)?.unwrap_or_default())
}

/// `cut.kdl` with an override of the element `kind` at `at` in `take`, replacing one with the
/// same identity. `effects` are the override's children: `keep`, `cut`, `zoom off`, an effect.
/// Refused when no element of that kind is within the override's tolerance of `at`.
pub fn override_change(
    repo: &dyn Repo,
    slug: &str,
    take: &Id,
    kind: ElementKind,
    at: Micros,
    effects: &str,
) -> Result<BTreeMap<String, String>, String> {
    let path = video_path(slug, "cut.kdl");
    let mut cut = parsed_or_default(&path, text(repo, slug, &path)?, Cut::parse)?;
    cut.overrides
        .retain(|o| !(o.take == *take && o.kind == kind && o.at == at));
    let node = format!("override {take} {kind} at={} {{\n{effects}\n}}\n", at.0);
    let combined = format!("{}\n{node}", cut.to_kdl());
    let cut = Cut::parse(&combined).map_err(|e| format!("the override does not parse: {e}"))?;
    let files = BTreeMap::from([(path, cut.to_kdl())]);
    let built = build_video(
        &Patched {
            inner: repo,
            files: &files,
        },
        slug,
    )?;
    let bound = built
        .lock
        .take(take)
        .is_some_and(|t| !t.orphans.iter().any(|o| o.at == at && o.kind == kind));
    if !bound {
        return Err(format!(
            "no {kind} element within {} of {} in take {take}; read the elements and use one's t",
            secs(Override::TOLERANCE),
            secs(at)
        ));
    }
    Ok(files)
}

/// The time a window argument names, as a range check helper for tools.
pub fn window(
    from: Option<u64>,
    to: Option<u64>,
) -> Result<(Option<Micros>, Option<Micros>), String> {
    if let (Some(f), Some(t)) = (from, to) {
        TimeRange::new(Micros(f), Micros(t)).ok_or("`from_us` must be before `to_us`")?;
    }
    Ok((from.map(Micros), to.map(Micros)))
}
