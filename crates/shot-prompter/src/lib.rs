//! Elgato Prompter content and voice-follow scrolling.
//!
//! - [`Prompter`]: what the prompter shows, from `script.md` in the format's prompter mode, and
//!   how it moves: by voice ([`Prompter::hear`]), steady ([`Prompter::tick`]) or by beat
//!   ([`Prompter::next_beat`], [`Prompter::again`]).
//! - [`Listener`]: a light streaming recognizer for voice follow. It decodes the last few
//!   seconds of mic audio with a small whisper model every 100 ms, so the matcher in
//!   [`Prompter::hear`] sees words long before the take's transcriber calls them stable.
//! - [`apply_markers`]: turns the layout and zoom hotkey markers of a take into `cut.kdl` nodes
//!   when the take is appended.

mod listen;
mod markers;
mod matcher;
mod prompter;

pub use listen::{Listener, follow_model};
pub use markers::apply_markers;
pub use prompter::{Line, Prompter, Retake, Scroll, View};
