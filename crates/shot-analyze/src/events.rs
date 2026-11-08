//! Reading what the rules and elements need from a take's `events.jsonl`: the `again` markers,
//! clicks and key presses.

use std::fs;
use std::io;
use std::path::Path;

use serde::Deserialize;
use shot_core::Micros;

#[derive(Deserialize)]
struct Line<'a> {
    t: u64,
    kind: &'a str,
    #[serde(default)]
    mark: Option<&'a str>,
}

/// Times of the `again` (A) markers, in order. A missing log has none, and a line that does not
/// parse is skipped: a crash may cut the last line short.
pub fn again_markers(events: &Path) -> io::Result<Vec<Micros>> {
    let text = match fs::read_to_string(events) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(text
        .lines()
        // Most lines are cursor moves; only marker lines are parsed.
        .filter(|line| line.contains("\"marker\""))
        .filter_map(|line| serde_json::from_str::<Line>(line).ok())
        .filter(|line| line.kind == "marker" && line.mark == Some("again"))
        .map(|line| Micros(line.t))
        .collect())
}

/// The clicks and key presses of an event log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Input {
    /// Each press of a mouse button, with the cursor position in screen pixels.
    pub clicks: Vec<(Micros, i32, i32)>,
    pub keys: Vec<Micros>,
}

#[derive(Deserialize)]
struct InputLine<'a> {
    t: u64,
    kind: &'a str,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
}

/// Reads [`Input`] like [`again_markers`] reads markers: a missing log has none, a torn line is
/// skipped.
pub(crate) fn input(events: &Path) -> io::Result<Input> {
    let text = match fs::read_to_string(events) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Input::default()),
        Err(e) => return Err(e),
    };
    let mut input = Input::default();
    for line in text.lines() {
        input.read(line);
    }
    Ok(input)
}

impl Input {
    /// Adds the click or key press of one event log line. Other lines, and one that does not
    /// parse, add nothing.
    pub(crate) fn read(&mut self, line: &str) {
        if !line.contains("\"down\"") && !line.contains("\"key\"") {
            return;
        }
        let Ok(line) = serde_json::from_str::<InputLine>(line) else {
            return;
        };
        match line.kind {
            "down" => self.clicks.push((Micros(line.t), line.x, line.y)),
            "key" => self.keys.push(Micros(line.t)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_again_markers_and_skips_a_torn_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"t\":0,\"kind\":\"move\",\"x\":1,\"y\":2}\n",
                "{\"t\":4210000,\"kind\":\"marker\",\"x\":2210,\"y\":640,\"mark\":\"again\",\"beat\":2}\n",
                "{\"t\":9800000,\"kind\":\"marker\",\"x\":2210,\"y\":640,\"mark\":\"next\",\"beat\":3}\n",
                "{\"t\":9900000,\"kind\":\"marker\",\"x\":2210,\"y\":6",
            ),
        )
        .unwrap();
        assert_eq!(again_markers(&path).unwrap(), [Micros(4_210_000)]);
        assert!(
            again_markers(&dir.path().join("none.jsonl"))
                .unwrap()
                .is_empty()
        );
    }
}
