//! Moving files to the Trash, where Finder can put them back.

use std::path::Path;

#[cfg(not(test))]
use objc2_foundation::{NSFileManager, NSString, NSURL};

/// Moves the file or folder at `path` to the user's Trash.
#[cfg(not(test))]
pub fn trash(path: &Path) -> Result<(), String> {
    let text = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath(&text);
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|e| format!("{}: {}", path.display(), e.localizedDescription()))
}

/// In a test the file or folder goes for good, so a test run leaves nothing in the Trash.
#[cfg(test)]
pub fn trash(path: &Path) -> Result<(), String> {
    let gone = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    gone.map_err(|e| format!("{}: {e}", path.display()))
}
