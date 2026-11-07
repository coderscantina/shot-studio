//! Model downloads pinned by `models/manifest.json` (decision 0008).
//!
//! A download streams into `<file>.part`, hashing as it goes, and is renamed to `<file>` only
//! when size and SHA-256 match. An interrupted download resumes from the `.part` with an HTTP
//! range request, or starts over when the server ignores the range. A file in place is trusted
//! by its size, since it was verified before the rename.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::Error;

/// One pinned model download.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelSpec {
    pub name: String,
    pub url: String,
    pub file: String,
    pub sha256: String,
    pub size: u64,
}

/// Bytes on disk out of the expected size, reported while downloading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// Whether a model is ready to load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelState {
    Present,
    /// An interrupted download with this many bytes.
    Partial(u64),
    Missing,
}

#[derive(Deserialize)]
struct Manifest {
    models: Vec<ModelSpec>,
}

/// The models of the manifest shipped with this build.
pub fn manifest() -> Vec<ModelSpec> {
    let manifest: Manifest = serde_json::from_str(include_str!("../../../models/manifest.json"))
        .expect("models/manifest.json is valid");
    manifest.models
}

/// The whisper model the transcriber loads.
pub fn whisper_model() -> ModelSpec {
    named("whisper-large-v3-turbo")
}

/// The small whisper model the prompter's voice follow loads.
pub fn follow_model() -> ModelSpec {
    named("whisper-tiny-en")
}

/// The voice activity model the transcriber loads.
pub fn vad_model() -> ModelSpec {
    named("silero-vad-ggml")
}

fn named(name: &str) -> ModelSpec {
    manifest()
        .into_iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("the manifest pins {name}"))
}

/// `$SHOT_MODELS`, or `~/Library/Application Support/Shot Studio/models`.
pub fn models_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SHOT_MODELS") {
        return dir.into();
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/Shot Studio/models")
}

impl ModelSpec {
    pub fn path(&self, dir: &Path) -> PathBuf {
        dir.join(&self.file)
    }

    fn part_path(&self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.part", self.file))
    }

    pub fn state(&self, dir: &Path) -> ModelState {
        if fs::metadata(self.path(dir)).is_ok_and(|m| m.len() == self.size) {
            return ModelState::Present;
        }
        match fs::metadata(self.part_path(dir)) {
            Ok(m) => ModelState::Partial(m.len()),
            Err(_) => ModelState::Missing,
        }
    }

    /// Returns the model's path, downloading it first when it is not present.
    pub fn ensure(&self, dir: &Path, progress: impl FnMut(Progress)) -> Result<PathBuf, Error> {
        let path = self.path(dir);
        if self.state(dir) != ModelState::Present {
            fs::create_dir_all(dir)?;
            self.download(dir, progress)?;
        }
        Ok(path)
    }

    fn download(&self, dir: &Path, mut progress: impl FnMut(Progress)) -> Result<(), Error> {
        let part = self.part_path(dir);
        let mut hasher = Sha256::new();
        let mut done = match File::open(&part) {
            Ok(file) if file.metadata()?.len() < self.size => hash_into(&mut hasher, file)?,
            _ => 0,
        };

        let mut request = ureq::get(&self.url);
        if done > 0 {
            request = request.header("Range", format!("bytes={done}-"));
        }
        let mut response = request.call().map_err(|e| Error::Http(e.to_string()))?;
        if done > 0 && response.status() != 206 {
            hasher = Sha256::new();
            done = 0;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(done == 0)
            .append(done > 0)
            .open(&part)?;

        let mut body = response.body_mut().as_reader();
        let mut buf = vec![0; 1 << 20];
        progress(Progress {
            done,
            total: self.size,
        });
        loop {
            let n = match body.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::Http(e.to_string())),
            };
            file.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            done += n as u64;
            if done > self.size {
                break;
            }
            progress(Progress {
                done,
                total: self.size,
            });
        }
        file.sync_all()?;
        drop(file);

        let actual = hex(&hasher.finalize());
        if done != self.size || actual != self.sha256 {
            fs::remove_file(&part)?;
            return Err(Error::Checksum {
                file: self.file.clone(),
                expected: format!("{} bytes, sha256 {}", self.size, self.sha256),
                actual: format!("{done} bytes, sha256 {actual}"),
            });
        }
        fs::rename(&part, self.path(dir))?;
        Ok(())
    }
}

fn hash_into(hasher: &mut Sha256, mut file: File) -> io::Result<u64> {
    let mut buf = vec![0; 1 << 20];
    let mut total = 0;
    loop {
        match file.read(&mut buf)? {
            0 => return Ok(total),
            n => {
                hasher.update(&buf[..n]);
                total += n as u64;
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// Serves `body` on localhost, honoring `Range: bytes=N-`, and records the ranges asked for.
    fn serve(body: Vec<u8>, requests: usize) -> (String, Arc<Mutex<Vec<Option<u64>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let seen = ranges.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(requests) {
                let mut stream = stream.unwrap();
                let mut range = None;
                for line in BufReader::new(&stream).lines() {
                    let line = line.unwrap();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(from) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = from.trim_end_matches('-').parse().ok();
                    }
                }
                seen.lock().unwrap().push(range);
                let from = range.unwrap_or(0) as usize;
                let status = if range.is_some() {
                    "206 Partial Content"
                } else {
                    "200 OK"
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len() - from
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(&body[from..]).unwrap();
            }
        });
        (url, ranges)
    }

    fn spec(url: String, body: &[u8]) -> ModelSpec {
        ModelSpec {
            name: "test".into(),
            url,
            file: "model.bin".into(),
            sha256: hex(&Sha256::digest(body)),
            size: body.len() as u64,
        }
    }

    fn body() -> Vec<u8> {
        (0..3_000_000u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn downloads_verifies_and_reports_progress() {
        let dir = tempfile::tempdir().unwrap();
        let body = body();
        let (url, _) = serve(body.clone(), 1);
        let model = spec(url, &body);
        let mut last = None;
        let path = model.ensure(dir.path(), |p| last = Some(p)).unwrap();
        assert_eq!(fs::read(path).unwrap(), body);
        assert_eq!(
            last,
            Some(Progress {
                done: 3_000_000,
                total: 3_000_000
            })
        );
        assert_eq!(model.state(dir.path()), ModelState::Present);
    }

    #[test]
    fn resumes_a_partial_download_with_a_range_request() {
        let dir = tempfile::tempdir().unwrap();
        let body = body();
        let (url, ranges) = serve(body.clone(), 1);
        let model = spec(url, &body);
        fs::write(dir.path().join("model.bin.part"), &body[..1_000_000]).unwrap();
        assert_eq!(model.state(dir.path()), ModelState::Partial(1_000_000));
        let path = model.ensure(dir.path(), |_| {}).unwrap();
        assert_eq!(fs::read(path).unwrap(), body);
        assert_eq!(*ranges.lock().unwrap(), [Some(1_000_000)]);
    }

    #[test]
    fn a_wrong_checksum_deletes_the_download_and_fails() {
        let dir = tempfile::tempdir().unwrap();
        let body = body();
        let (url, _) = serve(body.clone(), 1);
        let mut model = spec(url, &body);
        model.sha256 = "0".repeat(64);
        let error = model.ensure(dir.path(), |_| {}).unwrap_err();
        assert!(matches!(error, Error::Checksum { .. }));
        assert_eq!(model.state(dir.path()), ModelState::Missing);
    }
}
