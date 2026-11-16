//! Prompter settings of this machine, in `~/Library/Application Support/Shot Studio/prompter.json`.
//! They belong to the desk, not to a video, so they stay out of the project files.

use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shot_prompter::{Pace, Scroll};

/// The display picked when no display was chosen by hand.
pub const PROMPTER_NAME: &str = "Elgato Prom";

pub const TEXT_SIZE: RangeInclusive<f32> = 24.0..=140.0;
pub const LINE_HEIGHT: RangeInclusive<f32> = 1.0..=2.0;
pub const MARGIN: RangeInclusive<f32> = 0.0..=200.0;
pub const TEXT_WIDTH: RangeInclusive<f32> = 40.0..=100.0;
pub const READING_LINE: RangeInclusive<f32> = 10.0..=80.0;
pub const SPEED: RangeInclusive<f32> = 80.0..=240.0;
/// How much of the camera shows behind the text, in percent.
pub const CAMERA_OPACITY: RangeInclusive<f32> = 0.0..=100.0;
/// Countdown choices in seconds, 0 for none.
pub const COUNTDOWNS: [u8; 4] = [0, 3, 5, 10];

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
    pub text_size: f32,
    /// Line height as a multiple of the text size.
    pub line_height: f32,
    /// Space left and right of the text, in points of the output.
    pub margin: f32,
    /// The text column, in percent of the width between the margins.
    pub text_width: f32,
    /// Where the current line starts, in percent of the height from the top. The camera sits
    /// behind the glass, so reading near it keeps eye contact.
    pub reading_line: f32,
    /// Speaking rate of the steady scroll and of time left, in words per minute.
    pub speed: f32,
    /// Seconds the prompter counts down at the start of a take, 0 for none.
    pub countdown: u8,
    pub colors: Colors,
    /// The live camera, dimmed, behind the text: framing and expression in view while reading.
    pub camera: bool,
    /// How much of the camera shows while the countdown runs, in percent: brighter, to check
    /// the framing before the first line.
    pub camera_countdown: f32,
    /// How much of the camera shows the rest of the time, in percent: dim, so the text reads.
    pub camera_prompter: f32,
    /// The key the Stream Deck pedal sends.
    pub advance: AdvanceKey,
    /// Hotkeys as plain letters instead of ⌃⌥ plus the letter. Plain letters are taken from
    /// every app while recording, so typing on screen would drop markers.
    pub plain_keys: bool,
    /// The pane's "More settings" are open. Closed by default, so the daily controls stand alone.
    pub more: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            display: None,
            mirror: false,
            scroll: ScrollMode::Voice,
            text_size: 56.0,
            line_height: 1.2,
            margin: 44.0,
            text_width: 100.0,
            reading_line: 35.0,
            speed: 150.0,
            countdown: 3,
            colors: Colors::Soft,
            camera: false,
            camera_countdown: 80.0,
            camera_prompter: 40.0,
            advance: AdvanceKey::F18,
            plain_keys: false,
            more: false,
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
            .map_or_else(Self::default, |text| Self::parse(&text))
    }

    /// Settings from `prompter.json`, missing fields at their defaults and every value in range.
    fn parse(text: &str) -> Self {
        let mut settings: Self = serde_json::from_str(text).unwrap_or_default();
        let clamp = |value: f32, range: RangeInclusive<f32>, default: f32| {
            if value.is_finite() {
                value.clamp(*range.start(), *range.end())
            } else {
                default
            }
        };
        let base = Self::default();
        settings.text_size = clamp(settings.text_size, TEXT_SIZE, base.text_size);
        settings.line_height = clamp(settings.line_height, LINE_HEIGHT, base.line_height);
        settings.margin = clamp(settings.margin, MARGIN, base.margin);
        settings.text_width = clamp(settings.text_width, TEXT_WIDTH, base.text_width);
        settings.reading_line = clamp(settings.reading_line, READING_LINE, base.reading_line);
        settings.speed = clamp(settings.speed, SPEED, base.speed);
        settings.camera_countdown = clamp(
            settings.camera_countdown,
            CAMERA_OPACITY,
            base.camera_countdown,
        );
        settings.camera_prompter = clamp(
            settings.camera_prompter,
            CAMERA_OPACITY,
            base.camera_prompter,
        );
        if !COUNTDOWNS.contains(&settings.countdown) {
            settings.countdown = base.countdown;
        }
        // An empty name meant "off" before hiding the output became its own control.
        settings.display = settings.display.filter(|name| !name.is_empty());
        settings
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

    /// Every setting back at its default, keeping what fits the hardware (the display and
    /// mirroring) and the open "More settings".
    pub fn reset(&mut self) {
        *self = Self {
            display: self.display.take(),
            mirror: self.mirror,
            more: self.more,
            ..Self::default()
        };
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

    pub fn pace(&self) -> Pace {
        Pace {
            scroll: self.scroll.into(),
            words_per_minute: self.speed,
            countdown: Duration::from_secs(u64::from(self.countdown)),
        }
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

/// Text colors on the prompter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Colors {
    /// Gray text that turns white as it is said, the rest dimmer.
    Soft,
    /// Near-white text and brighter context lines, for a bright room or a dense beam splitter.
    High,
    /// Warm yellow, easier on the eyes in a dark studio.
    Amber,
}

/// The colors of one [`Colors`] choice.
pub struct Palette {
    pub said: u32,
    pub ahead: u32,
    pub next: u32,
    pub previous: u32,
}

impl Colors {
    pub const ALL: [Self; 3] = [Self::Soft, Self::High, Self::Amber];

    pub fn label(self) -> &'static str {
        match self {
            Self::Soft => "Soft",
            Self::High => "High contrast",
            Self::Amber => "Amber",
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Self::Soft => Palette {
                said: 0xffffff,
                ahead: 0x9aa2ae,
                next: 0x7b838f,
                previous: 0x4f5560,
            },
            Self::High => Palette {
                said: 0xffffff,
                ahead: 0xd2d7de,
                next: 0xb4bbc6,
                previous: 0x7d8490,
            },
            Self::Amber => Palette {
                said: 0xffd66e,
                ahead: 0xb99b55,
                next: 0x9c844b,
                previous: 0x5e4f2e,
            },
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

    #[test]
    fn an_older_file_loads_with_new_settings_at_their_defaults_and_values_in_range() {
        let old = Settings::parse(
            r#"{"display": "", "mirror": true, "scroll": "steady", "text_size": 300}"#,
        );
        assert_eq!(
            old,
            Settings {
                mirror: true,
                scroll: ScrollMode::Steady,
                text_size: 140.0,
                ..Settings::default()
            }
        );
        let odd = Settings::parse(
            r#"{"line_height": 0.2, "countdown": 7, "speed": 1000, "camera_prompter": 140}"#,
        );
        assert_eq!(
            (
                odd.line_height,
                odd.countdown,
                odd.speed,
                odd.camera_prompter
            ),
            (1.0, 3, 240.0, 100.0)
        );
        assert_eq!(Settings::parse("not json"), Settings::default());
    }

    #[test]
    fn a_reset_keeps_the_hardware_and_the_open_disclosure() {
        let mut settings = Settings {
            display: Some("Elgato Prom.".into()),
            more: true,
            mirror: true,
            text_size: 90.0,
            plain_keys: true,
            ..Settings::default()
        };
        settings.reset();
        assert_eq!(
            settings,
            Settings {
                display: Some("Elgato Prom.".into()),
                mirror: true,
                more: true,
                ..Settings::default()
            }
        );
        assert!(!Settings::parse("{}").more, "closed by default");
    }
}
