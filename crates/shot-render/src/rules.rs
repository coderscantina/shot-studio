//! Runs the auto-zoom rule over a video folder: every take block of the cut gets the zooms its
//! take's event log calls for, sized to the take's screen track.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use shot_core::{Cut, Format, Id};
use shot_media::videotoolbox::{DecodeError, Decoder};

use crate::assets::{AssetError, Assets};
use crate::autozoom;
use crate::surface::size;

#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error("format.kdl has no auto-zoom rule")]
    NoRule,
    #[error(transparent)]
    Assets(#[from] AssetError),
    #[error("{path}: {source}")]
    Decode { path: PathBuf, source: DecodeError },
}

/// `cut` with the auto-zoom rule of `format` run on every take block of the main cut. A take
/// without an event log or screen track keeps its zooms.
pub fn auto_zoom(video_dir: &Path, cut: &Cut, format: &Format) -> Result<Cut, RuleError> {
    let rule = format.auto_zoom.as_ref().ok_or(RuleError::NoRule)?;
    let mut assets = Assets::new(video_dir);
    let mut screens: HashMap<Id, Option<(u32, u32)>> = HashMap::new();
    let mut cut = cut.clone();
    for chapter in &mut cut.chapters {
        for edit in &mut chapter.takes {
            let Some(events) = assets.events(&edit.take)? else {
                continue;
            };
            let screen = match screens.get(&edit.take) {
                Some(screen) => *screen,
                None => {
                    let path = video_dir
                        .join("media")
                        .join(edit.take.as_str())
                        .join("screen.mp4");
                    let screen = screen_size(&path)?;
                    screens.insert(edit.take.clone(), screen);
                    screen
                }
            };
            if let Some(screen) = screen {
                *edit = autozoom::apply(edit, rule, &events, screen);
            }
        }
    }
    Ok(cut)
}

/// The pixel size of a screen track's first frame, `None` without the file or a frame.
fn screen_size(path: &Path) -> Result<Option<(u32, u32)>, RuleError> {
    if !path.exists() {
        return Ok(None);
    }
    let error = |source| RuleError::Decode {
        path: path.to_owned(),
        source,
    };
    let mut decoder = Decoder::open_native(path).map_err(error)?;
    Ok(decoder
        .next_frame()
        .map_err(error)?
        .map(|f| size(&f.buffer)))
}
