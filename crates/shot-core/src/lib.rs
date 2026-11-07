//! Project model, cut.kdl and timeline evaluation. No IO: files are parsed from `&str` and
//! written to `String`.
//!
//! The grammar of each file is in `docs/contracts/project-files.md`.

mod cut;
mod error;
mod format;
mod kdl_io;
mod script;
mod timeline;
mod values;
mod video;

pub use cut::{
    Chapter, Clip, Cut, DerivedCut, DerivedKind, LayoutChange, Overlay, ParamValue, RangeEdit,
    RemovedBy, TakeEdit, Zoom,
};
pub use error::ParseError;
pub use format::{
    AutoCut, CaptionStyle, Format, FormatKind, Layout, Pacing, PrompterMode, RuleKind, TimedRule,
};
pub use script::{Beat, Script, ScriptChapter};
pub use timeline::{
    ChapterSegments, ContentHash, Output, OutputKind, RENDERER_VERSION, Segment, Timeline,
};
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
