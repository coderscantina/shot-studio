//! `events.jsonl` lines, as the cursor and key event log in `docs/contracts/project-files.md`
//! defines them.

use std::io::BufRead;
use std::path::Path;

use serde::{Deserialize, Serialize};
use shot_core::{Layout, Micros};

/// One line of the event log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    pub t: u64,
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    pub fn new(t: Micros, kind: EventKind) -> Self {
        Self { t: t.0, kind }
    }

    /// The JSON line without the trailing newline.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("events serialize to JSON")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EventKind {
    Move {
        x: i32,
        y: i32,
    },
    Down {
        x: i32,
        y: i32,
        button: Button,
    },
    Up {
        x: i32,
        y: i32,
        button: Button,
    },
    Scroll {
        x: i32,
        y: i32,
        dx: i32,
        dy: i32,
    },
    Key {
        key: String,
        mods: Vec<Modifier>,
    },
    /// A record hotkey, with the cursor position at the press.
    Marker {
        x: i32,
        y: i32,
        #[serde(flatten)]
        mark: Mark,
    },
}

/// The record hotkey a marker stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mark", rename_all = "kebab-case")]
pub enum Mark {
    /// A, with the beat the prompter shows after the press, numbered from 1 across the script.
    Again { beat: u32 },
    /// N, with the beat the prompter shows after the press.
    Next { beat: u32 },
    /// M
    Marker,
    /// Z, zoom on the cursor.
    Zoom,
    /// B
    BRoll,
    /// 1 to 4, the layout chosen.
    Layout {
        #[serde(with = "layout_name")]
        layout: Layout,
    },
}

mod layout_name {
    use serde::{Deserialize, Deserializer, Serializer};
    use shot_core::Layout;

    pub fn serialize<S: Serializer>(layout: &Layout, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(layout.as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Layout, D::Error> {
        let name = String::deserialize(d)?;
        Layout::ALL
            .iter()
            .copied()
            .find(|l| l.as_str() == name)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown layout `{name}`")))
    }
}

/// A marker read back from the event log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    pub t: Micros,
    /// Cursor position in screen track pixels.
    pub x: i32,
    pub y: i32,
    pub mark: Mark,
}

#[derive(Deserialize)]
struct MarkerLine {
    t: u64,
    x: i32,
    y: i32,
    #[serde(flatten)]
    mark: Mark,
}

/// The markers of a take's `events.jsonl`, in time order. A take without an event log has none.
/// A torn last line, as a crash leaves it, is skipped.
pub fn read_markers(path: &Path) -> std::io::Result<Vec<Marker>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut markers = Vec::new();
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        if !line.contains(r#""kind":"marker""#) {
            continue;
        }
        if let Ok(m) = serde_json::from_str::<MarkerLine>(&line) {
            markers.push(Marker {
                t: Micros(m.t),
                x: m.x,
                y: m.y,
                mark: m.mark,
            });
        }
    }
    Ok(markers)
}

/// A key press the event log keeps out of `key` events, because it is a record hotkey and
/// becomes a marker instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    /// The key name as `key` events write it.
    pub key: String,
    pub mods: Vec<Modifier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    Left,
    Right,
    Other,
}

/// Held modifiers, declared in the order the log lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Modifier {
    Cmd,
    Shift,
    Alt,
    Ctrl,
    Fn,
}

/// CGEventFlags masks.
const FLAG_SHIFT: u64 = 0x0002_0000;
const FLAG_CTRL: u64 = 0x0004_0000;
const FLAG_ALT: u64 = 0x0008_0000;
const FLAG_CMD: u64 = 0x0010_0000;
const FLAG_FN: u64 = 0x0080_0000;

/// The modifiers of a key press from its CGEventFlags. macOS sets the fn flag on arrows,
/// function and navigation keys by itself, so those keys only report the other modifiers.
pub fn modifiers(flags: u64, keycode: u16) -> Vec<Modifier> {
    [
        (FLAG_CMD, Modifier::Cmd),
        (FLAG_SHIFT, Modifier::Shift),
        (FLAG_ALT, Modifier::Alt),
        (FLAG_CTRL, Modifier::Ctrl),
        (FLAG_FN, Modifier::Fn),
    ]
    .into_iter()
    .filter(|&(mask, m)| flags & mask != 0 && !(m == Modifier::Fn && sets_fn(keycode)))
    .map(|(_, m)| m)
    .collect()
}

/// macOS virtual key codes of keys without a printable character.
const NAMED_KEYS: &[(u16, &str)] = &[
    (36, "enter"),
    (76, "enter"),
    (48, "tab"),
    (49, "space"),
    (51, "backspace"),
    (117, "delete"),
    (53, "escape"),
    (123, "left"),
    (124, "right"),
    (125, "down"),
    (126, "up"),
    (115, "home"),
    (119, "end"),
    (116, "pageup"),
    (121, "pagedown"),
    (114, "help"),
    (122, "f1"),
    (120, "f2"),
    (99, "f3"),
    (118, "f4"),
    (96, "f5"),
    (97, "f6"),
    (98, "f7"),
    (100, "f8"),
    (101, "f9"),
    (109, "f10"),
    (103, "f11"),
    (111, "f12"),
    (105, "f13"),
    (107, "f14"),
    (113, "f15"),
    (106, "f16"),
    (64, "f17"),
    (79, "f18"),
    (80, "f19"),
];

/// Arrows, function and navigation keys: the named keys after escape in the table.
fn sets_fn(keycode: u16) -> bool {
    NAMED_KEYS[7..].iter().any(|(code, _)| *code == keycode)
}

/// The lowercase key name of a key press: a named key by its code, otherwise the character the
/// layout typed. Control turns letters into control characters, which map back to the letter.
/// `None` when the press typed nothing nameable, like a dead key.
pub fn key_name(keycode: u16, typed: &str) -> Option<String> {
    if let Some(&(_, name)) = NAMED_KEYS.iter().find(|(code, _)| *code == keycode) {
        return Some(name.to_owned());
    }
    let mut chars = typed.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return Some(typed.to_lowercase());
    }
    match c {
        '\u{1}'..='\u{1a}' => Some(char::from(b'a' + c as u8 - 1).to_string()),
        c if c.is_control() => None,
        c => Some(c.to_lowercase().collect()),
    }
}

/// Where a display sits in global coordinates, to turn cursor points into screen track pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayGeometry {
    /// Top left corner in global points.
    pub origin: (f64, f64),
    /// Pixels of the screen track per point.
    pub scale: f64,
}

impl DisplayGeometry {
    /// Pixels from the top left of the screen track. A cursor on another display lies outside
    /// the frame.
    pub fn to_pixels(&self, x: f64, y: f64) -> (i32, i32) {
        (
            ((x - self.origin.0) * self.scale).round() as i32,
            ((y - self.origin.1) * self.scale).round() as i32,
        )
    }

    pub fn scale_distance(&self, points: f64) -> i32 {
        (points * self.scale).round() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_like_the_contract_example() {
        let lines = [
            Event::new(Micros(0), EventKind::Move { x: 1920, y: 1080 }),
            Event::new(
                Micros(512_000),
                EventKind::Down {
                    x: 2210,
                    y: 640,
                    button: Button::Left,
                },
            ),
            Event::new(
                Micros(1_200_000),
                EventKind::Scroll {
                    x: 2210,
                    y: 900,
                    dx: 0,
                    dy: -240,
                },
            ),
            Event::new(
                Micros(2_104_000),
                EventKind::Key {
                    key: "p".into(),
                    mods: vec![Modifier::Cmd, Modifier::Shift],
                },
            ),
            Event::new(
                Micros(2_350_000),
                EventKind::Key {
                    key: "enter".into(),
                    mods: vec![],
                },
            ),
        ]
        .map(|e| e.to_line());
        assert_eq!(
            lines,
            [
                r#"{"t":0,"kind":"move","x":1920,"y":1080}"#,
                r#"{"t":512000,"kind":"down","x":2210,"y":640,"button":"left"}"#,
                r#"{"t":1200000,"kind":"scroll","x":2210,"y":900,"dx":0,"dy":-240}"#,
                r#"{"t":2104000,"kind":"key","key":"p","mods":["cmd","shift"]}"#,
                r#"{"t":2350000,"kind":"key","key":"enter","mods":[]}"#,
            ]
        );
    }

    #[test]
    fn markers_round_trip_like_the_contract_example() {
        let lines = [
            r#"{"t":4210000,"kind":"marker","x":2210,"y":640,"mark":"again","beat":2}"#,
            r#"{"t":9800000,"kind":"marker","x":2210,"y":640,"mark":"next","beat":3}"#,
            r#"{"t":12000000,"kind":"marker","x":1480,"y":910,"mark":"zoom"}"#,
            r#"{"t":15500000,"kind":"marker","x":1480,"y":910,"mark":"layout","layout":"split"}"#,
        ];
        let marks = [
            Mark::Again { beat: 2 },
            Mark::Next { beat: 3 },
            Mark::Zoom,
            Mark::Layout {
                layout: Layout::Split,
            },
        ];
        let written: Vec<String> = lines
            .iter()
            .zip(marks)
            .map(|(line, mark)| {
                let m: MarkerLine = serde_json::from_str(line).unwrap();
                Event::new(
                    Micros(m.t),
                    EventKind::Marker {
                        x: m.x,
                        y: m.y,
                        mark,
                    },
                )
                .to_line()
            })
            .collect();
        assert_eq!(written, lines);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let log = format!(
            "{}\n{}\n{}\n{{\"t\":16000000,\"kind\":\"mar",
            r#"{"t":0,"kind":"move","x":1920,"y":1080}"#,
            lines[2],
            r#"{"t":12500000,"kind":"marker","x":1,"y":2,"mark":"b-roll"}"#,
        );
        std::fs::write(&path, log).unwrap();
        let markers = read_markers(&path).unwrap();
        assert_eq!(
            markers.iter().map(|m| (m.t, m.mark)).collect::<Vec<_>>(),
            [
                (Micros(12_000_000), Mark::Zoom),
                (Micros(12_500_000), Mark::BRoll)
            ]
        );
        assert_eq!((markers[0].x, markers[0].y), (1480, 910));
        assert!(
            read_markers(&dir.path().join("missing.jsonl"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn key_names_are_lowercase_and_survive_control() {
        assert_eq!(key_name(36, "\r").as_deref(), Some("enter"));
        assert_eq!(key_name(96, "").as_deref(), Some("f5"));
        assert_eq!(key_name(35, "P").as_deref(), Some("p"));
        assert_eq!(key_name(35, "\u{10}").as_deref(), Some("p"));
        assert_eq!(key_name(16, "z").as_deref(), Some("z"));
        assert_eq!(key_name(33, ""), None);
    }

    #[test]
    fn modifiers_keep_the_contract_order_and_drop_implicit_fn() {
        let flags = FLAG_SHIFT | FLAG_CMD | FLAG_FN;
        assert_eq!(
            modifiers(flags, 35),
            [Modifier::Cmd, Modifier::Shift, Modifier::Fn]
        );
        assert_eq!(modifiers(FLAG_FN | FLAG_ALT, 123), [Modifier::Alt]);
        assert!(modifiers(0, 36).is_empty());
    }

    #[test]
    fn cursor_points_map_to_screen_track_pixels() {
        let geometry = DisplayGeometry {
            origin: (-1512.0, 0.0),
            scale: 2.0,
        };
        assert_eq!(geometry.to_pixels(-1512.0, 0.0), (0, 0));
        assert_eq!(geometry.to_pixels(-756.25, 491.0), (1512, 982));
        assert_eq!(geometry.scale_distance(-120.0), -240);
    }
}
