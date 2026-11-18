//! Project model, cut.kdl, style cascade and timeline evaluation. No IO: files are parsed
//! from `&str` and written to `String`.
//!
//! The grammar of each file is in `docs/contracts/project-files.md`.

mod brand;
mod captions;
mod credit;
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
mod takes;
mod themes;
pub mod thumbnail;
mod timeline;
mod transcript;
mod transition;
mod values;
mod video;

pub use brand::{
    Bookend, Brand, BundledFont, Camera, CameraShape, Colors, CustomLayout, Font, Fonts, Import,
    LayoutSlot, LottieTemplate, Motion, OverlayPreset, Phase, Preset, PresetName, Slot, SlotType,
    SoundEffect, SoundKey, Stinger, Track,
};
pub use captions::{
    CaptionBack, CaptionCase, CaptionDesign, CaptionFont, CaptionLook, CaptionPosition, Emphasis,
};
pub use credit::{Creator, Credit, License};
pub use cut::{
    CaptionBreak, CaptionFix, Chapter, Clip, Cut, DerivedCut, DerivedKind, Framing, LayoutChange,
    Music, Overlay, Override, OverrideEffect, ParamValue, Pick, Point, RangeEdit, TakeEdit, Zoom,
    ZoomShape,
};
pub use edit::{Decision, EditError, Look, Placed, RemovedBy, Review, TakeAt, WordState, WordView};
pub use element::{Attr, AttrValue, DuplicateElement, Element, ElementKind, Elements};
pub use engine::{StyleError, StyleInput, build};
pub use error::ParseError;
pub use format::Inputs;
pub use format::{
    CaptionStyle, Color, CursorStyle, DeviceFrame, Effects, Format, FormatKind, FormatLayer,
    Keystrokes, Layout, LayoutName, PrompterMode, Shadow, WindowStyle, ZoomMotion,
};
pub use lock::{
    Applied, Asset, ElementRef, Instance, LOCK_VERSION, Lock, LockTake, Orphan, Provenance,
};
pub use migrate::{
    BrandMigrateError, MigrateError, MigrateInput, Migration, Report as MigrationReport, migrate,
    migrate_brand, settings_diff, without_old_settings, written_before_lock,
};
pub use script::{Beat, Script, ScriptChapter};
pub use style::{
    ActiveRule, Effect, EffectKey, Filter, Layer, Rule, RuleBody, Selector, StyleSheet, Target,
};
pub use takes::{Attempt, Group, Lineup, Row, RowAt, TakeWords, word_diff, words_in};
pub use themes::{
    BRAND_FILE, DEFAULT_THEME, FileChange, ThemeError, Themes, picked_theme, theme_file,
};
pub use timeline::{
    ChapterSegments, ContentHash, MediaId, Output, OutputKind, RENDERER_VERSION, SceneSwitch,
    Segment, Timeline,
};
pub use transcript::{EDGE_PAD, Transcript, Word, cut_after, cut_before, word_cut};
pub use transition::{Transition, TransitionKind, TransitionSound};
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
