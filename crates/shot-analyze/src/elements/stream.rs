//! A take's elements while it records: words as the transcriber settles them, clicks, typing and
//! markers as capture logs them, and the URLs said, so live style rules see them as they happen.
//! They are the elements the quick pass writes for the same facts.
//!
//! The file holds the settled elements and is replaced on every change, so a crash keeps them.
//! [`Stream::live`] adds the URLs of the transcriber's tentative words, which the next decode
//! may still change, for the monitor. The quick pass at stop replaces the file.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use shot_capture::events::{Marker, marker};
use shot_core::{Elements, Id, Micros, Word};

use super::{add, input_elements, url_elements, word_element, write};
use crate::Error;
use crate::events::Input;

/// How far back [`Stream::recent_events`] reaches. A frame's cursor, ripples and key labels
/// look back less.
const RECENT: Micros = Micros(3_000_000);

/// See the module docs. Feed it with [`Stream::word`], [`Stream::tentative`] and
/// [`Stream::poll`], then [`Stream::update`].
pub struct Stream {
    video: PathBuf,
    take: Id,
    events: Tail,
    words: Vec<Word>,
    tentative: Vec<Word>,
    input: Input,
    markers: Vec<Marker>,
    /// Event log lines of the last [`RECENT`], with their times.
    recent: VecDeque<(Micros, String)>,
    settled: Elements,
    live: Elements,
    /// Whether a fact arrived since the last update.
    changed: bool,
}

impl Stream {
    /// Starts an empty elements file for `take`, replacing one left by an earlier recording,
    /// and follows capture's `media/<take>/events.jsonl` as it grows.
    pub fn create(video: &Path, take: &Id) -> Result<Self, Error> {
        let empty = Elements::new(take.clone());
        write(video, &empty)?;
        Ok(Self {
            video: video.to_owned(),
            take: take.clone(),
            events: Tail::new(video.join("media").join(take.as_str()).join("events.jsonl")),
            words: Vec::new(),
            tentative: Vec::new(),
            input: Input::default(),
            markers: Vec::new(),
            recent: VecDeque::new(),
            settled: empty.clone(),
            live: empty,
            changed: false,
        })
    }

    /// Adds a word the transcriber settled. Words arrive in order. The tentative words are
    /// dropped: the decode that settled this word reports its own next.
    pub fn word(&mut self, word: Word) {
        self.words.push(word);
        self.tentative.clear();
        self.changed = true;
    }

    /// Replaces the tentative words, those after the settled ones in the latest decode.
    pub fn tentative(&mut self, words: Vec<Word>) {
        if words != self.tentative {
            self.tentative = words;
            self.changed = true;
        }
    }

    /// Reads what capture appended to the event log since the last call and returns how many
    /// lines that was. Before capture created the log there is nothing to read.
    pub fn poll(&mut self) -> io::Result<usize> {
        let lines = self.events.lines()?;
        for line in &lines {
            let Ok(Timed { t }) = serde_json::from_str::<Timed>(line) else {
                continue;
            };
            let before = (self.input.clicks.len(), self.input.keys.len());
            self.input.read(line);
            let input = before != (self.input.clicks.len(), self.input.keys.len());
            let marked = marker(line).map(|m| self.markers.push(m)).is_some();
            self.changed |= input || marked;
            self.recent.push_back((Micros(t), line.clone()));
        }
        if let Some(&(newest, _)) = self.recent.back() {
            let from = Micros(newest.0.saturating_sub(RECENT.0));
            while self.recent.front().is_some_and(|(t, _)| *t < from) {
                self.recent.pop_front();
            }
        }
        Ok(lines.len())
    }

    /// Brings the elements up to date with what arrived and writes the settled ones when they
    /// changed. Returns whether [`Stream::live`] changed.
    pub fn update(&mut self) -> Result<bool, Error> {
        if !std::mem::take(&mut self.changed) {
            return Ok(false);
        }
        let settled = self.elements(&self.words);
        if settled != self.settled {
            write(&self.video, &settled)?;
            self.settled = settled;
        }
        let live = if self.tentative.is_empty() {
            self.settled.clone()
        } else {
            self.elements(&[self.words.as_slice(), &self.tentative].concat())
        };
        let changed = live != self.live;
        self.live = live;
        Ok(changed)
    }

    /// The settled elements with the URLs of the tentative words.
    pub fn live(&self) -> &Elements {
        &self.live
    }

    /// The settled and then the tentative words.
    pub fn heard(&self) -> impl Iterator<Item = &Word> {
        self.words.iter().chain(&self.tentative)
    }

    /// The record hotkey markers so far, in time order.
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    /// The event log lines of the last 3 s, as one text.
    pub fn recent_events(&self) -> String {
        let mut text = String::new();
        for (_, line) in &self.recent {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    fn elements(&self, words: &[Word]) -> Elements {
        let mut out = Elements::new(self.take.clone());
        add(&mut out, words.iter().map(word_element));
        add(&mut out, url_elements(words));
        add(&mut out, input_elements(&self.input, &self.markers));
        out
    }
}

#[derive(Deserialize)]
struct Timed {
    t: u64,
}

/// The complete lines appended to a file since the last read.
struct Tail {
    path: PathBuf,
    file: Option<File>,
    /// A line read without its newline yet.
    partial: Vec<u8>,
}

impl Tail {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: None,
            partial: Vec::new(),
        }
    }

    fn lines(&mut self) -> io::Result<Vec<String>> {
        let file = match &mut self.file {
            Some(file) => file,
            None => match File::open(&self.path) {
                Ok(file) => self.file.insert(file),
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(e),
            },
        };
        let mut bytes = std::mem::take(&mut self.partial);
        file.read_to_end(&mut bytes)?;
        let whole = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        self.partial = bytes.split_off(whole);
        Ok(String::from_utf8_lossy(&bytes)
            .lines()
            .map(str::to_owned)
            .collect())
    }
}
