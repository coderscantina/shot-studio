//! The spell checker behind a small seam: the system's on macOS, none elsewhere. What it checks and
//! how the editor shows it is in `spelling.rs`.

#[cfg(target_os = "macos")]
mod macos;

use std::ops::Range;
use std::rc::Rc;

/// Called with the misspelled words' byte ranges in the text, on any thread.
pub type Done = Box<dyn FnOnce(Vec<Range<usize>>) + Send>;

/// A platform's spell checker.
pub trait Checker {
    /// Checks `text` off the calling thread and calls `done` when it knows.
    fn check(&self, text: String, done: Done);

    /// What the word might have been, best first. Asked for on the main thread.
    fn suggestions(&self, word: &str) -> Vec<String>;

    /// Adds the word to the person's dictionary, so it is never misspelled again.
    fn learn(&self, word: &str);
}

/// No spell checker: nothing is ever misspelled.
#[cfg(not(target_os = "macos"))]
struct NoChecker;

#[cfg(not(target_os = "macos"))]
impl Checker for NoChecker {
    fn check(&self, _: String, done: Done) {
        done(Vec::new());
    }

    fn suggestions(&self, _: &str) -> Vec<String> {
        Vec::new()
    }

    fn learn(&self, _: &str) {}
}

/// The spell checker of this platform.
pub fn system() -> Rc<dyn Checker> {
    #[cfg(target_os = "macos")]
    {
        Rc::new(macos::SystemChecker::default())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Rc::new(NoChecker)
    }
}
