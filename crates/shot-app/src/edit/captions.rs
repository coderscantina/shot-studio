//! The Captions panel: the caption lines in time, editable in place, and the caption style.

use shot_core::CaptionStyle;

/// A caption preset's name in the UI.
pub fn style_name(style: CaptionStyle) -> &'static str {
    match style {
        CaptionStyle::WordPop => "Word pop",
        CaptionStyle::Clean => "Clean",
        CaptionStyle::Karaoke => "Karaoke",
        CaptionStyle::Bold => "Bold",
        CaptionStyle::Off => "Off",
    }
}
