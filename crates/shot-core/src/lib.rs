//! Project model, cut.kdl and timeline evaluation. No IO: files are parsed from `&str` and
//! written to `String`.
//!
//! The grammar of each file is in `docs/contracts/project-files.md`.

mod cut;
mod edit;
mod error;
mod format;
mod kdl_io;
mod script;
mod timeline;
mod transcript;
mod values;
mod video;

pub use cut::{
    CaptionFix, Chapter, Clip, Cut, DerivedCut, DerivedKind, LayoutChange, Music, Overlay,
    ParamValue, Point, RangeEdit, RemovedBy, Review, TakeEdit, Zoom, ZoomBy,
};
pub use edit::{Decision, EditError, TakeAt, WordState, WordView};
pub use error::ParseError;
pub use format::{
    AutoCut, AutoZoom, CaptionStyle, Color, CursorStyle, DeviceFrame, Effects, Format, FormatKind,
    Keystrokes, Layout, Pacing, PrompterMode, RuleKind, Shadow, TimedRule, WindowStyle, ZoomMotion,
    ZoomTrigger,
};
pub use script::{Beat, Script, ScriptChapter};
pub use timeline::{
    ChapterSegments, ContentHash, MediaId, Output, OutputKind, RENDERER_VERSION, Segment, Timeline,
};
pub use transcript::{Transcript, Word};
pub use values::{Id, Micros, TimeRange};
pub use video::{Date, Status, Video};

/// The four files of one video folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub video: Video,
    pub script: Script,
    pub format: Format,
    pub cut: Cut,
}
