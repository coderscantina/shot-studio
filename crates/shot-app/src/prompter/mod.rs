//! The Elgato Prompter: its output window, its settings pane on the Record screen, and the
//! record hotkeys.
//!
//! One [`Live`] holds the prompter's state for everything that moves it: the voice-follow
//! listener on its own thread, the steady scroll on the recorder thread, and the hotkeys on the
//! main thread. A hotkey moves the prompter and writes its marker into the take in one step, so
//! the marker carries the beat the prompter shows after the press.
//!
//! Hotkeys are Carbon system hotkeys, registered from the start of a recording to its end, so
//! they work while another app is in front. Only the exact combinations are taken from other
//! apps: ⌃⌥ plus the letter by default, the plain letters when Mike switches that on, and the
//! pedal's key. No permission is needed. The event tap that records keys for the keystroke
//! overlay leaves these presses out, they are markers.

mod native;
mod output;
mod pane;
mod settings;

use std::sync::{Arc, Mutex};

use shot_capture::Marks;
use shot_capture::events::{Hotkey, Mark, Modifier};
use shot_core::{Layout, Project};
use shot_prompter::{Prompter, Scroll};

pub use pane::PrompterPane;
pub use settings::Settings;

/// A record hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKey {
    Again,
    Next,
    Marker,
    Zoom,
    BRoll,
    /// 1 to 4, as 0 to 3: the format's layouts in order.
    Layout(u8),
    /// The pedal's key, a next beat.
    Advance,
}

/// The letter and digit hotkeys in the order the Record screen lists them: the key, its macOS
/// virtual key code and the key name the event log uses.
const LETTERS: [(RecordKey, u16, &str); 9] = [
    (RecordKey::Again, 0, "a"),
    (RecordKey::Next, 45, "n"),
    (RecordKey::Marker, 46, "m"),
    (RecordKey::Zoom, 6, "z"),
    (RecordKey::BRoll, 11, "b"),
    (RecordKey::Layout(0), 18, "1"),
    (RecordKey::Layout(1), 19, "2"),
    (RecordKey::Layout(2), 20, "3"),
    (RecordKey::Layout(3), 21, "4"),
];

/// The prompter while it is on screen, shared by every thread that moves it.
pub struct Live {
    pub prompter: Mutex<Prompter>,
    marks: Mutex<Option<Marks>>,
    /// The format's layouts, for 1 to 4.
    layouts: Mutex<Vec<Layout>>,
}

impl Live {
    pub fn new(project: &Project, scroll: Scroll) -> Arc<Self> {
        let live = Arc::new(Self {
            prompter: Mutex::new(Prompter::new(&project.script, project.format.prompter)),
            marks: Mutex::new(None),
            layouts: Mutex::new(Vec::new()),
        });
        live.reset(project);
        live.lock().set_scroll(scroll);
        live
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, Prompter> {
        self.prompter.lock().expect("prompter")
    }

    /// Starts over from the project's script and format, keeping the scroll mode.
    pub fn reset(&self, project: &Project) {
        let mut prompter = self.lock();
        let scroll = prompter.scroll();
        *prompter = Prompter::new(&project.script, project.format.prompter);
        prompter.set_scroll(scroll);
        let format = &project.format;
        *self.layouts.lock().expect("layouts") = std::iter::once(format.default_layout)
            .chain(format.other_layouts.iter().copied())
            .collect();
    }

    /// Where markers go while a take records.
    pub fn set_marks(&self, marks: Option<Marks>) {
        *self.marks.lock().expect("marks") = marks;
    }

    /// Moves the prompter for a hotkey and writes its marker into the recording take.
    pub fn press(&self, key: RecordKey) {
        let mark = match key {
            RecordKey::Again => Mark::Again {
                beat: self.lock().again(),
            },
            RecordKey::Next | RecordKey::Advance => Mark::Next {
                beat: self.lock().next_beat(),
            },
            RecordKey::Marker => Mark::Marker,
            RecordKey::Zoom => Mark::Zoom,
            RecordKey::BRoll => Mark::BRoll,
            RecordKey::Layout(n) => {
                let Some(&layout) = self.layouts.lock().expect("layouts").get(usize::from(n))
                else {
                    return;
                };
                Mark::Layout { layout }
            }
        };
        if let Some(marks) = &*self.marks.lock().expect("marks") {
            marks.write(mark);
        }
    }
}

/// The hotkeys for the current settings: (key, virtual key code, Carbon modifiers).
fn bindings(settings: &Settings) -> Vec<(RecordKey, u16, u32)> {
    let modifiers = if settings.plain_keys {
        0
    } else {
        native::CONTROL | native::OPTION
    };
    let mut keys: Vec<_> = LETTERS
        .iter()
        .map(|&(key, code, _)| (key, code, modifiers))
        .collect();
    if settings.scroll == settings::ScrollMode::Pedal {
        keys.push((RecordKey::Advance, settings.advance.code(), 0));
    }
    keys
}

/// The presses the event log leaves out of its `key` events while these hotkeys are on.
pub fn log_filter(settings: &Settings) -> Vec<Hotkey> {
    let mods = if settings.plain_keys {
        Vec::new()
    } else {
        vec![Modifier::Alt, Modifier::Ctrl]
    };
    let mut keys: Vec<Hotkey> = LETTERS
        .iter()
        .map(|&(_, _, name)| Hotkey {
            key: name.to_owned(),
            mods: mods.clone(),
        })
        .collect();
    if settings.scroll == settings::ScrollMode::Pedal {
        keys.push(Hotkey {
            key: settings.advance.name().to_owned(),
            mods: Vec::new(),
        });
    }
    keys
}
