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
//!
//! The output window shows only while the Record screen does, never opens over the app's own
//! display unless asked, and Esc, its close control, ⌘⇧P and the Prompter menu all hide it.

mod native;
mod output;
mod pane;
mod settings;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{App, Menu, MenuItem, actions};
use objc2_core_foundation::CFRetained;
use objc2_core_video::CVPixelBuffer;
use shot_capture::events::{Hotkey, Mark, Modifier};
use shot_capture::{Marks, Preview};
use shot_core::{Layout, Project};
use shot_prompter::{Moved, Pace, Prompter};

pub use native::display_names;
pub use output::PrompterOutput;
pub use pane::PrompterPane;
pub use settings::Settings;

actions!(prompter, [TogglePrompter, HidePrompter]);

/// The key context of the output window.
pub(crate) const OUTPUT_CONTEXT: &str = "PrompterOutput";

/// Where no Record screen takes ⌘⇧P or Esc on the output, both close every prompter output, so
/// there is always a way out.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &TogglePrompter, cx| close_outputs(cx));
    cx.on_action(|_: &HidePrompter, cx| close_outputs(cx));
}

/// The Prompter menu of the menu bar.
pub fn menu() -> Menu {
    Menu::new("Prompter").items([
        MenuItem::action("Show or hide prompter", TogglePrompter),
        MenuItem::action("Hide prompter", HidePrompter),
    ])
}

fn close_outputs(cx: &mut App) {
    for window in cx.windows() {
        if let Some(output) = window.downcast::<output::PrompterOutput>() {
            let _ = output.update(cx, |_, window, _| window.remove_window());
        }
    }
}

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
    /// The scene the take shows: picked on the Record screen or with 1 to 4. `None` is the
    /// format's default layout. Shared with the monitor that rehearses it.
    pub scene: Scene,
    /// The Record screen's live inputs, for the camera behind the text.
    pub feed: Feed,
}

/// The scene picked for the take, shared between threads.
pub type Scene = Arc<Mutex<Option<Layout>>>;

/// The live inputs of the standby or the running take, while there are some.
pub type Feed = Arc<Mutex<Option<Preview>>>;

impl Live {
    pub fn new(project: &Project, pace: Pace) -> Arc<Self> {
        let live = Arc::new(Self {
            prompter: Mutex::new(Prompter::new(&project.script, project.format.prompter)),
            marks: Mutex::new(None),
            layouts: Mutex::new(Vec::new()),
            scene: Scene::default(),
            feed: Feed::default(),
        });
        live.reset(project);
        live.lock().set_pace(pace);
        live
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, Prompter> {
        self.prompter.lock().expect("prompter")
    }

    /// Starts over from the project's script and format, keeping the pace.
    pub fn reset(&self, project: &Project) {
        let mut prompter = self.lock();
        let pace = prompter.pace();
        *prompter = Prompter::new(&project.script, project.format.prompter);
        prompter.set_pace(pace);
        let format = &project.format;
        *self.layouts.lock().expect("layouts") = std::iter::once(format.default_layout)
            .chain(format.other_layouts.iter().copied())
            .collect();
    }

    /// Voice follow heard `words`: moves the prompter, and when that took it to another beat or
    /// started one over, writes the marker a hotkey would, so the take knows its beats. A beat
    /// reached a few words in is dated back by the time those words took.
    pub fn hear(&self, words: &[String]) {
        let (moved, words_per_minute) = {
            let mut prompter = self.lock();
            (prompter.hear(words), prompter.pace().words_per_minute)
        };
        let Some(moved) = moved else {
            return;
        };
        let Some(marks) = &*self.marks.lock().expect("marks") else {
            return;
        };
        match moved {
            Moved::Next { beat, words } => marks.write_ago(
                Mark::Next { beat },
                Duration::from_secs_f32(words as f32 * 60.0 / words_per_minute.max(1.0)),
            ),
            Moved::Again { beat } => marks.write(Mark::Again { beat }),
        }
    }

    /// The newest camera frame, while the inputs run.
    pub fn camera(&self) -> Option<CFRetained<CVPixelBuffer>> {
        let feed = self.feed.lock().expect("feed").clone()?;
        feed.camera().map(|frame| frame.buffer)
    }

    /// Records only `beats` of the script, by index from 0, from the next start on.
    pub fn select(&self, beats: std::ops::RangeInclusive<usize>) {
        self.lock().select(beats);
    }

    /// The take started: writes at its zero the beat it starts on when that is not the first,
    /// so its beats count from there, and the scene when it is not the default.
    pub fn mark_start(&self) {
        let Some(marks) = &*self.marks.lock().expect("marks") else {
            return;
        };
        // Long enough to reach back past the start: the marks land at zero.
        let start = Duration::from_secs(60);
        let beat = self.lock().beat();
        if beat > 1 {
            marks.write_ago(Mark::Next { beat }, start);
        }
        let default = self.layouts.lock().expect("layouts").first().copied();
        if let Some(layout) = *self.scene.lock().expect("scene")
            && Some(layout) != default
        {
            marks.write_ago(Mark::Layout { layout }, start);
        }
    }

    /// Picks the scene: before a take, the one it starts in; while recording, a switch.
    pub fn set_scene(&self, layout: Layout) {
        *self.scene.lock().expect("scene") = Some(layout);
        if let Some(marks) = &*self.marks.lock().expect("marks") {
            marks.write(Mark::Layout { layout });
        }
    }

    /// The scene the take shows now.
    pub fn scene(&self) -> Option<Layout> {
        *self.scene.lock().expect("scene")
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
                *self.scene.lock().expect("scene") = Some(layout);
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
