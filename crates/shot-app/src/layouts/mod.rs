//! Channel layouts in the app: their names on screen, the slots a new one starts from, and the
//! layout builder window ([`builder`]).

pub mod builder;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use shot_core::{CustomLayout, Effects, Layout, LayoutName, LayoutSlot, Track};
use shot_render::layout::{LayoutSpec, Source};

/// A channel layout's name as the screens show it, in sentence case: `talk-left` reads
/// "Talk left". Kept for the life of the app, there are a handful per channel.
pub fn display_name(name: LayoutName) -> &'static str {
    static NAMES: OnceLock<Mutex<HashMap<LayoutName, &'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Mutex::default)
        .lock()
        .expect("layout names");
    names.entry(name).or_insert_with(|| {
        let words = name.as_str().replace('-', " ");
        let mut chars = words.chars();
        let text = chars
            .next()
            .map(|first| first.to_uppercase().chain(chars).collect::<String>())
            .unwrap_or_default();
        Box::leak(text.into_boxed_str())
    })
}

/// The name a person types, as a layout name: lowercase, words joined by dashes, other
/// characters left out.
pub fn name_from(text: &str) -> Option<LayoutName> {
    let mut name = String::new();
    for c in text.trim().chars() {
        match c {
            'a'..='z' | '0'..='9' => name.push(c),
            'A'..='Z' => name.push(c.to_ascii_lowercase()),
            ' ' | '-' | '_' if !name.is_empty() && !name.ends_with('-') => name.push('-'),
            _ => {}
        }
    }
    LayoutName::new(name.trim_end_matches('-'))
}

/// The slots of `layout` as drawn with the default window style, to start a channel layout
/// from: a built-in's, or the channel's own.
pub fn slots_of(layout: Layout, custom: &[CustomLayout]) -> Vec<LayoutSlot> {
    let Ok(spec) = LayoutSpec::resolve(layout, &Effects::default().window, custom) else {
        return Vec::new();
    };
    let (w, h) = CustomLayout::FRAME;
    spec.slots
        .iter()
        .map(|slot| {
            let f = slot.frame;
            let left = f.left.clamp(0.0, f32::from(w) - 1.0);
            let top = f.top.clamp(0.0, f32::from(h) - 1.0);
            LayoutSlot {
                track: match slot.source {
                    Source::Screen => Track::Screen,
                    Source::Camera => Track::Camera,
                },
                x: left.round() as u16,
                y: top.round() as u16,
                width: (f.right.min(f32::from(w)) - left).round().max(1.0) as u16,
                height: (f.bottom.min(f32::from(h)) - top).round().max(1.0) as u16,
                corners: slot.radius.round() as u16,
                focus: slot.focus.map(|f| (f * 100.0).round() as u8),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_in_sentence_case_and_typed_names_become_ids() {
        let name = LayoutName::new("talk-left").unwrap();
        assert_eq!(display_name(name), "Talk left");
        assert_eq!(name_from("  Talk  Left!"), Some(name));
        assert_eq!(name_from("Split"), None, "a built-in's name");
        assert_eq!(name_from("2 up"), None, "starts with a digit");
    }

    #[test]
    fn a_built_in_starts_a_channel_layout_with_its_slots() {
        let slots = slots_of(Layout::FaceContent, &[]);
        assert_eq!(slots.len(), 2);
        assert_eq!(
            (slots[0].track, slots[0].width, slots[0].height),
            (Track::Screen, 3840, 2160)
        );
        assert_eq!((slots[1].x, slots[1].y, slots[1].corners), (2904, 1602, 36));
        let custom = CustomLayout {
            name: LayoutName::new("mine").unwrap(),
            slots,
        };
        let name = Layout::Custom(custom.name);
        assert_eq!(slots_of(name, std::slice::from_ref(&custom)), custom.slots);
    }
}
