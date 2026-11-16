//! Moving files to the Trash, where Finder can put them back.

use std::path::Path;

use objc2_foundation::{NSFileManager, NSString, NSURL};

/// Moves the file or folder at `path` to the user's Trash.
pub fn trash(path: &Path) -> Result<(), String> {
    let text = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath(&text);
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|e| format!("{}: {}", path.display(), e.localizedDescription()))
}
