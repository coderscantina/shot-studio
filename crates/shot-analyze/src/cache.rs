//! The transcript cache file, `.shot/cache/transcripts/<take>.json` under the video folder.
//!
//! Derived data, safe to delete: transcribing the take's mic track writes it again. The format
//! is one JSON object, times in microseconds of the take's media time, half-open ranges:
//!
//! ```json
//! {
//!   "version": 1,
//!   "take": "t01",
//!   "words": [
//!     { "text": "Ship", "from": 800000, "to": 1040000, "confidence": 97 }
//!   ]
//! }
//! ```
//!
//! A file with another `version` reads as absent, so a format change only costs a rerun.
//!
//! The speech of a take, `.shot/cache/speech/<take>.json`, is derived the same way: its speech
//! ranges and a loudness level in whole dBFS every 20 ms from `levels_from`:
//!
//! ```json
//! { "version": 1, "ranges": [[800000, 3520000]], "levels_from": 0, "levels": [-61, -24] }
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use shot_core::{Id, Micros, TimeRange, Transcript, Word};

use crate::{Error, Speech};

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    take: String,
    words: Vec<FileWord>,
}

#[derive(Serialize, Deserialize)]
struct FileWord {
    text: String,
    from: u64,
    to: u64,
    confidence: u8,
}

pub fn cache_path(video: &Path, take: &Id) -> PathBuf {
    video
        .join(".shot/cache/transcripts")
        .join(format!("{}.json", take.as_str()))
}

/// Writes the transcript atomically, replacing an older one.
pub fn write_cached(video: &Path, transcript: &Transcript) -> Result<PathBuf, Error> {
    let path = cache_path(video, transcript.take());
    let file = File {
        version: VERSION,
        take: transcript.take().as_str().to_owned(),
        words: transcript
            .words()
            .iter()
            .map(|w| FileWord {
                text: w.text.clone(),
                from: w.range.from().0,
                to: w.range.to().0,
                confidence: w.confidence,
            })
            .collect(),
    };
    let json = serde_json::to_vec_pretty(&file).map_err(|e| Error::Cache(e.to_string()))?;
    fs::create_dir_all(path.parent().expect("cache path has a parent"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Reads the cached transcript of `take`, or `None` when there is none in this format.
pub fn read_cached(video: &Path, take: &Id) -> Result<Option<Transcript>, Error> {
    let path = cache_path(video, take);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let invalid = |why: String| Error::Cache(format!("{}: {why}", path.display()));
    let file: File = serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
    if file.version != VERSION {
        return Ok(None);
    }
    if file.take != take.as_str() {
        return Err(invalid(format!("holds take {}", file.take)));
    }
    let mut transcript = Transcript::new(take.clone());
    for w in file.words {
        let range = TimeRange::new(Micros(w.from), Micros(w.to))
            .ok_or_else(|| invalid(format!("empty range for {:?}", w.text)))?;
        transcript
            .push(Word {
                text: w.text,
                range,
                confidence: w.confidence.min(100),
            })
            .map_err(|w| invalid(format!("{:?} overlaps the word before", w.text)))?;
    }
    Ok(Some(transcript))
}

#[derive(Serialize, Deserialize)]
struct SpeechFile {
    version: u32,
    ranges: Vec<(u64, u64)>,
    levels_from: u64,
    levels: Vec<i8>,
}

pub fn speech_path(video: &Path, take: &Id) -> PathBuf {
    video
        .join(".shot/cache/speech")
        .join(format!("{}.json", take.as_str()))
}

/// Writes a take's speech atomically, replacing an older one.
pub fn write_speech(video: &Path, take: &Id, speech: &Speech) -> Result<PathBuf, Error> {
    let path = speech_path(video, take);
    let file = SpeechFile {
        version: VERSION,
        ranges: speech
            .ranges
            .iter()
            .map(|r| (r.from().0, r.to().0))
            .collect(),
        levels_from: speech.levels_from.0,
        levels: speech.levels.clone(),
    };
    let json = serde_json::to_vec(&file).map_err(|e| Error::Cache(e.to_string()))?;
    fs::create_dir_all(path.parent().expect("cache path has a parent"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Reads the cached speech of `take`, or `None` when there is none in this format.
pub fn read_speech(video: &Path, take: &Id) -> Result<Option<Speech>, Error> {
    let path = speech_path(video, take);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let invalid = |why: String| Error::Cache(format!("{}: {why}", path.display()));
    let file: SpeechFile = serde_json::from_slice(&bytes).map_err(|e| invalid(e.to_string()))?;
    if file.version != VERSION {
        return Ok(None);
    }
    let ranges = file
        .ranges
        .into_iter()
        .map(|(from, to)| {
            TimeRange::new(Micros(from), Micros(to))
                .ok_or_else(|| invalid(format!("empty speech range at {from}")))
        })
        .collect::<Result<_, _>>()?;
    Ok(Some(Speech {
        ranges,
        levels: file.levels,
        levels_from: Micros(file.levels_from),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_round_trips_through_the_cache_file() {
        let dir = tempfile::tempdir().unwrap();
        let take = Id::new("t01").unwrap();
        assert!(read_speech(dir.path(), &take).unwrap().is_none());
        let speech = Speech {
            ranges: vec![TimeRange::new(Micros(800_000), Micros(3_520_000)).unwrap()],
            levels: vec![-61, -24],
            levels_from: Micros(0),
        };
        write_speech(dir.path(), &take, &speech).unwrap();
        assert_eq!(read_speech(dir.path(), &take).unwrap(), Some(speech));
    }

    #[test]
    fn round_trips_through_the_cache_file() {
        let dir = tempfile::tempdir().unwrap();
        let take = Id::new("t01").unwrap();
        assert!(read_cached(dir.path(), &take).unwrap().is_none());
        let mut transcript = Transcript::new(take.clone());
        for (text, from, to) in [("Ship", 800_000, 1_040_000), ("it.", 1_040_000, 1_300_000)] {
            let range = TimeRange::new(Micros(from), Micros(to)).unwrap();
            transcript
                .push(Word {
                    text: text.into(),
                    range,
                    confidence: 93,
                })
                .unwrap();
        }
        let path = write_cached(dir.path(), &transcript).unwrap();
        assert!(path.ends_with(".shot/cache/transcripts/t01.json"));
        assert_eq!(read_cached(dir.path(), &take).unwrap(), Some(transcript));
    }
}
