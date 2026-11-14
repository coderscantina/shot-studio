//! Safe saves of project files, and the writer lock of a video folder.
//!
//! [`write`] replaces a file so that a crash or a full disk leaves the old file or the new one,
//! never an empty or partial one. [`WriteLock`] makes the app, `shot` and the MCP server wait
//! for each other while they read, change and write one video's files, or a channel's `brand/`. `shot-core` does no IO,
//! so this is the one place every writer shares.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Replaces `path` with `bytes`: writes a uniquely named file in the same folder, flushes it to
/// disk, renames it over `path` and flushes the folder. On an error `path` is untouched and the
/// temporary file is gone.
pub fn write(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let dir = folder(path);
    let tmp = temp_path(path)?;
    let replaced = (|| {
        let mut file = File::options().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes.as_ref())?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if replaced.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    replaced?;
    File::open(dir)?.sync_all()
}

/// The folder `path` is in, `.` for a bare file name.
fn folder(path: &Path) -> &Path {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    }
}

/// `.<name>.<pid>.<n>.tmp` beside `path`: unique across processes and threads, and hidden.
fn temp_path(path: &Path) -> io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} names no file", path.display()),
        )
    })?;
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    Ok(folder(path).join(format!(
        ".{}.{}.{n}.tmp",
        name.to_string_lossy(),
        std::process::id()
    )))
}

/// The advisory lock on a video folder's files, `<video>/.shot/write.lock`, or on a channel's
/// `brand/`, `<repo>/.shot/brand.lock`. Take it before reading the files a change starts from
/// and hold it until the last write; dropping it lets the next writer in. It is not reentrant:
/// taking it twice in one process waits forever. A writer that needs both takes the video's
/// first.
#[must_use = "the lock is released when this is dropped"]
pub struct WriteLock {
    _file: File,
}

impl WriteLock {
    /// Waits until no other writer holds the lock of `video`, then holds it.
    pub fn acquire(video: &Path) -> io::Result<Self> {
        Self::at(video, "write.lock")
    }

    /// Waits until no other writer holds the lock of the `brand/` folder of the channel repo
    /// at `repo`, then holds it.
    pub fn channel(repo: &Path) -> io::Result<Self> {
        Self::at(repo, "brand.lock")
    }

    fn at(root: &Path, name: &str) -> io::Result<Self> {
        let dir = root.join(".shot");
        fs::create_dir_all(&dir)?;
        let file = File::options()
            .create(true)
            .write(true)
            .truncate(false)
            .open(dir.join(name))?;
        file.lock()?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::thread;
    use std::time::Duration;

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn replaces_the_file_and_leaves_nothing_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut.kdl");
        fs::write(&path, "old\n").unwrap();
        write(&path, "new\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(entries(dir.path()), ["cut.kdl"]);
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_and_no_temporary_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut.kdl");
        fs::write(&path, "old\n").unwrap();

        // The rename fails: the target is a folder with files in it.
        let blocked = dir.path().join("blocked");
        fs::create_dir_all(blocked.join("in-the-way")).unwrap();
        assert!(write(&blocked, "new\n").is_err());
        assert_eq!(entries(dir.path()), ["blocked", "cut.kdl"]);

        // The temporary file cannot be created: the folder is read-only.
        let writable = fs::metadata(dir.path()).unwrap().permissions();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
        let failed = write(&path, "new\n");
        fs::set_permissions(dir.path(), writable).unwrap();
        assert!(failed.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "old\n");
        assert_eq!(entries(dir.path()), ["blocked", "cut.kdl"]);
    }

    #[test]
    fn writers_that_hold_the_lock_never_lose_an_edit() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().to_owned();
        let count = video.join("count");
        write(&count, "0").unwrap();
        let writers: Vec<_> = (0..2)
            .map(|_| {
                let (video, count) = (video.clone(), count.clone());
                thread::spawn(move || {
                    for _ in 0..20 {
                        // Each acquire opens its own handle, as a second process would.
                        let _lock = WriteLock::acquire(&video).unwrap();
                        let n: u32 = fs::read_to_string(&count).unwrap().parse().unwrap();
                        thread::sleep(Duration::from_millis(1));
                        write(&count, (n + 1).to_string()).unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(fs::read_to_string(&count).unwrap(), "40");
    }
}
