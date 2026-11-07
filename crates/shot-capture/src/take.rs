//! Take ids and take folders.

use std::io;
use std::path::{Path, PathBuf};

use shot_core::Id;

/// The id after the highest `tNN` among `existing`, so a gap left by a deleted take is never
/// filled. Names that are not take ids are ignored.
pub fn next_take_id<'a>(existing: impl IntoIterator<Item = &'a str>) -> Id {
    let highest = existing
        .into_iter()
        .filter_map(|name| name.strip_prefix('t'))
        .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|digits| digits.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    Id::new(format!("t{:02}", highest + 1)).expect("tNN is a valid id")
}

/// Creates `media/<take>/` in `video_dir` for a new take and returns its id and path.
pub fn create_take_dir(video_dir: &Path) -> io::Result<(Id, PathBuf)> {
    let media = video_dir.join("media");
    std::fs::create_dir_all(&media)?;
    loop {
        let names: Vec<String> = std::fs::read_dir(&media)?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        let id = next_take_id(names.iter().map(String::as_str));
        let dir = media.join(id.as_str());
        // create_dir fails on an existing folder, so a concurrent session cannot share the id.
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_take_id_follows_the_highest_and_never_fills_gaps() {
        assert_eq!(next_take_id([]).as_str(), "t01");
        assert_eq!(next_take_id(["t01", "t03", "broll", "t2x"]).as_str(), "t04");
        assert_eq!(next_take_id(["t99"]).as_str(), "t100");
        assert_eq!(next_take_id(["t", ".DS_Store"]).as_str(), "t01");
    }

    #[test]
    fn create_take_dir_makes_media_and_a_fresh_folder() {
        let video = tempfile::tempdir().unwrap();
        let (first, dir) = create_take_dir(video.path()).unwrap();
        assert_eq!(first.as_str(), "t01");
        assert!(dir.is_dir());
        assert_eq!(create_take_dir(video.path()).unwrap().0.as_str(), "t02");
    }
}
