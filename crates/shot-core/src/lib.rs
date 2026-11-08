//! Project model, cut.kdl, style cascade and timeline evaluation. No IO: files are parsed
//! from `&str` and written to `String`.
//!
//! The grammar of each file is in `docs/contracts/project-files.md`.

mod brand;
mod cut;
mod edit;
mod element;
mod engine;
mod error;
mod format;
mod kdl_io;
mod lock;
mod migrate;
mod script;
mod style;
mod timeline;
mod transcript;
mod values;
mod video;

pub use brand::{
    Bookend, Brand, BundledFont, Camera, CameraShape, Colors, Font, Fonts, Import, LottieTemplate,
    Motion, Phase, Preset, PresetName, Slot, SlotType, SoundEffect, SoundKey, Stinger,
};
pub use cut::{
    CaptionFix, Chapter, Clip, Cut, DerivedCut, DerivedKind, LayoutChange, Music, Overlay,
    Override, OverrideEffect, ParamValue, Point, RangeEdit, RemovedBy, Review, TakeEdit, Zoom,
    ZoomBy,
};
pub use edit::{Decision, EditError, TakeAt, WordState, WordView};
pub use element::{Attr, AttrValue, DuplicateElement, Element, ElementKind, Elements};
pub use engine::{StyleError, StyleInput, build};
pub use error::ParseError;
pub use format::{
    AutoCut, AutoZoom, CaptionStyle, Color, CursorStyle, DeviceFrame, Effects, Format, FormatKind,
    FormatLayer, Keystrokes, Layout, Pacing, PrompterMode, RuleKind, Shadow, TimedRule,
    WindowStyle, ZoomMotion, ZoomTrigger,
};
pub use lock::{
    Applied, Asset, ElementRef, Instance, LOCK_VERSION, Lock, LockTake, Orphan, Provenance,
};
pub use migrate::{
    B_ROLL_IDLE, B_ROLL_MARKER, EMPHASIS_HOLD, EMPHASIS_ZOOM, LINK_CARD_FOR, MigrateError,
    MigrateInput, Migration, Report as MigrationReport, ZOOM_LEAD, has_rule_nodes, implied_rules,
    migrate, preset_rules,
};
pub use script::{Beat, Script, ScriptChapter};
pub use style::{
    ActiveRule, Effect, EffectKey, Filter, Layer, Rule, RuleBody, Selector, StyleSheet,
    TRANSITION_LENGTH, Target, TransitionKind,
};
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
