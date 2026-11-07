//! Prompter settings of this machine, in `~/Library/Application Support/Shot Studio/prompter.json`.
//! They belong to the desk, not to a video, so they stay out of the project files.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use shot_prompter::Scroll;

/// The display picked when no display was chosen by hand.
pub const PROMPTER_NAME: &str = "Elgato Prom";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The prompter display by name, `None` for the first whose name starts with
    /// [`PROMPTER_NAME`].
    pub display: Option<String>,
    /// Flip the output horizontally for a beam splitter that does not.
    pub mirror: bool,
    pub scroll: ScrollMode,
    /// Size of the current line in points.
    pub text_size: u16,
    /// The key the Stream Deck pedal sends.
    pub advance: AdvanceKey,
    /// Hotkeys as plain letters instead of ⌃⌥ plus the letter. Plain letters are taken from
    /// every app while recording, so typing on screen would drop markers.
    pub plain_keys: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            display: None,
            mirror: false,
            scroll: ScrollMode::Voice,
            text_size: 56,
            advance: AdvanceKey::F18,
            plain_keys: false,
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home).join("Library/Application Support/Shot Studio/prompter.json")
    }

    /// The saved settings, or the defaults when there are none or they do not parse.
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(
            path,
            serde_json::to_string_pretty(self).expect("settings serialize"),
        );
    }

    /// The prompter display among `displays` (id, name): the chosen one, or the Elgato.
    pub fn pick_display(&self, displays: &[(u32, String)]) -> Option<u32> {
        displays
            .iter()
            .find(|(_, name)| match &self.display {
                Some(chosen) => name == chosen,
                None => name.starts_with(PROMPTER_NAME),
            })
            .map(|(id, _)| *id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScrollMode {
    Voice,
    Steady,
    Pedal,
}

impl From<ScrollMode> for Scroll {
    fn from(mode: ScrollMode) -> Self {
        match mode {
            ScrollMode::Voice => Scroll::Voice,
            ScrollMode::Steady => Scroll::Steady,
            ScrollMode::Pedal => Scroll::Pedal,
        }
    }
}

/// Keys a Stream Deck hotkey action can send and no app uses by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdvanceKey {
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    PageDown,
}

impl AdvanceKey {
    pub const ALL: [Self; 8] = [
        Self::F13,
        Self::F14,
        Self::F15,
        Self::F16,
        Self::F17,
        Self::F18,
        Self::F19,
        Self::PageDown,
    ];

    /// macOS virtual key code.
    pub fn code(self) -> u16 {
        match self {
            Self::F13 => 105,
            Self::F14 => 107,
            Self::F15 => 113,
            Self::F16 => 106,
            Self::F17 => 64,
            Self::F18 => 79,
            Self::F19 => 80,
            Self::PageDown => 121,
        }
    }

    /// The key name the event log uses.
    pub fn name(self) -> &'static str {
        match self {
            Self::F13 => "f13",
            Self::F14 => "f14",
            Self::F15 => "f15",
            Self::F16 => "f16",
            Self::F17 => "f17",
            Self::F18 => "f18",
            Self::F19 => "f19",
            Self::PageDown => "pagedown",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::F13 => "F13",
            Self::F14 => "F14",
            Self::F15 => "F15",
            Self::F16 => "F16",
            Self::F17 => "F17",
            Self::F18 => "F18",
            Self::F19 => "F19",
            Self::PageDown => "Page Down",
        }
    }

    pub fn following(self) -> Self {
        let index = Self::ALL.iter().position(|&k| k == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_elgato_is_picked_by_name_unless_another_display_was_chosen() {
        let displays = [
            (1, "U2515H Left".to_owned()),
            (2, "U2515H Right".to_owned()),
            (3, "Elgato Prom.".to_owned()),
        ];
        let mut settings = Settings::default();
        assert_eq!(settings.pick_display(&displays), Some(3));
        settings.display = Some("U2515H Right".into());
        assert_eq!(settings.pick_display(&displays), Some(2));
        assert_eq!(settings.pick_display(&displays[..1]), None);
    }
}
