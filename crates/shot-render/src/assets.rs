//! The files frames draw from besides the video tracks, read once per video folder: each take's
//! event log, the code-diff snapshots in `sources/`, the b-roll stills and diffs overlays name,
//! and the video's theme with the brand's fonts and Lottie templates.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shot_core::{Effects, Id, Overlay};
use vello::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};

use crate::code_diff::{CodeDiff, CodeDiffOverlay};
use crate::events::{EventLog, EventsError};
use crate::frame::Look;
use crate::overlay::broll::{self, Media};
use crate::theme::{Theme, ThemeError};

/// The event log of a take, by file name inside `media/<take>/`.
pub const EVENTS_FILE: &str = "events.jsonl";

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("{path}: {source}")]
    Events { path: PathBuf, source: EventsError },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: {source}")]
    Image {
        path: PathBuf,
        source: image::ImageError,
    },
    #[error(transparent)]
    Theme(#[from] ThemeError),
}

/// Stills larger than this are scaled down to it, the size of the canvas.
const MAX_STILL: (u32, u32) = (3840, 2160);

/// Loads and keeps what [`Look`]s need from one video folder.
#[derive(Debug)]
pub struct Assets {
    dir: PathBuf,
    /// Read from the folder on first use unless given.
    theme: Option<Arc<Theme>>,
    events: HashMap<Id, Option<Arc<EventLog>>>,
    diffs: HashMap<String, Arc<CodeDiff>>,
    images: HashMap<String, ImageData>,
}

impl Assets {
    /// The assets of a video folder, its theme read from its files on first use.
    pub fn new(video_dir: &Path) -> Self {
        Self {
            dir: video_dir.to_owned(),
            theme: None,
            events: HashMap::new(),
            diffs: HashMap::new(),
            images: HashMap::new(),
        }
    }

    /// The assets of a video folder with its theme already read.
    pub fn with_theme(video_dir: &Path, theme: Arc<Theme>) -> Self {
        Self {
            theme: Some(theme),
            ..Self::new(video_dir)
        }
    }

    /// The video's theme, read from its `video.kdl`, `cut.kdl` and `cut.lock` on first use.
    pub fn theme(&mut self) -> Result<Arc<Theme>, AssetError> {
        if let Some(theme) = &self.theme {
            return Ok(theme.clone());
        }
        let theme = Arc::new(Theme::open(&self.dir)?);
        self.theme = Some(theme.clone());
        Ok(theme)
    }

    /// What a segment of `take` with `overlays` draws with `effects`. A take without an event
    /// log draws no cursor or keystrokes.
    pub fn look(
        &mut self,
        take: &Id,
        overlays: &[Overlay],
        effects: Effects,
    ) -> Result<Look, AssetError> {
        let mut diffs = HashMap::new();
        let mut images = HashMap::new();
        for code in overlays.iter().filter_map(CodeDiffOverlay::of) {
            let diff = self.diff(&code.diff)?;
            diffs.insert(code.diff, diff);
        }
        for (file, media) in overlays.iter().filter_map(broll::file) {
            match media {
                Media::Diff => {
                    diffs.insert(file.to_owned(), self.diff(file)?);
                }
                Media::Still => {
                    images.insert(file.to_owned(), self.image(file)?);
                }
                Media::Clip => {}
            }
        }
        Ok(Look {
            effects,
            events: self.events(take)?,
            diffs,
            images,
            theme: self.theme()?,
            take: Some(take.clone()),
        })
    }

    pub fn events(&mut self, take: &Id) -> Result<Option<Arc<EventLog>>, AssetError> {
        if let Some(events) = self.events.get(take) {
            return Ok(events.clone());
        }
        let path = self.dir.join("media").join(take.as_str()).join(EVENTS_FILE);
        let events = match fs::read_to_string(&path) {
            Ok(text) => Some(Arc::new(EventLog::parse(&text).map_err(|source| {
                AssetError::Events {
                    path: path.clone(),
                    source,
                }
            })?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(source) => return Err(AssetError::Io { path, source }),
        };
        self.events.insert(take.clone(), events.clone());
        Ok(events)
    }

    /// A still, as straight-alpha RGBA no larger than the canvas.
    fn image(&mut self, file: &str) -> Result<ImageData, AssetError> {
        if let Some(image) = self.images.get(file) {
            return Ok(image.clone());
        }
        let path = self.dir.join(file);
        let mut decoded = image::open(&path)
            .map_err(|source| AssetError::Image {
                path: path.clone(),
                source,
            })?
            .into_rgba8();
        let (width, height) = decoded.dimensions();
        if width > MAX_STILL.0 || height > MAX_STILL.1 {
            let scale = f64::min(
                f64::from(MAX_STILL.0) / f64::from(width),
                f64::from(MAX_STILL.1) / f64::from(height),
            );
            decoded = image::imageops::resize(
                &decoded,
                (f64::from(width) * scale).round() as u32,
                (f64::from(height) * scale).round() as u32,
                image::imageops::FilterType::Triangle,
            );
        }
        let (width, height) = decoded.dimensions();
        let image = ImageData {
            data: Blob::new(Arc::new(decoded.into_raw())),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width,
            height,
        };
        self.images.insert(file.to_owned(), image.clone());
        Ok(image)
    }

    fn diff(&mut self, file: &str) -> Result<Arc<CodeDiff>, AssetError> {
        if let Some(diff) = self.diffs.get(file) {
            return Ok(diff.clone());
        }
        let path = self.dir.join(file);
        let text = fs::read_to_string(&path).map_err(|source| AssetError::Io {
            path: path.clone(),
            source,
        })?;
        let diff = Arc::new(CodeDiff::parse(&text));
        self.diffs.insert(file.to_owned(), diff.clone());
        Ok(diff)
    }
}
