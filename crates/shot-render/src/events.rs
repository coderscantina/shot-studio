//! A take's `events.jsonl` as the renderer reads it: the cursor path, clicks and key presses.
//! Pure, parsed from text.
//!
//! The cursor is where the latest event with a position put it. Smoothing averages that path over
//! a window centered on the frame, so the drawn cursor neither lags nor runs ahead.

use serde::Deserialize;
use shot_core::Micros;

/// A click: the press of a mouse button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Click {
    pub t: Micros,
    pub at: [f32; 2],
}

/// A key press with the modifiers held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPress {
    pub t: Micros,
    /// Lowercase, as the log writes it: `a`, `space`, `enter`, `f5`.
    pub key: String,
    pub mods: Vec<Modifier>,
}

/// Held modifiers in the log's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modifier {
    Cmd,
    Shift,
    Alt,
    Ctrl,
    Fn,
}

#[derive(Debug, thiserror::Error)]
#[error("events.jsonl:{line}: {message}")]
pub struct EventsError {
    pub line: usize,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventLog {
    /// Every event with a position, in time order.
    path: Vec<(Micros, [f32; 2])>,
    /// The integral of the path from its first event to each event, for smoothing.
    integral: Vec<[f64; 2]>,
    clicks: Vec<Click>,
    keys: Vec<KeyPress>,
}

#[derive(Deserialize)]
struct Line {
    t: u64,
    kind: String,
    x: Option<i32>,
    y: Option<i32>,
    key: Option<String>,
    #[serde(default)]
    mods: Vec<Modifier>,
}

impl EventLog {
    /// Parses the log. Unknown fields and kinds are skipped. A last line without its newline is
    /// what a crash leaves mid-write, so it is dropped when it does not parse.
    pub fn parse(text: &str) -> Result<Self, EventsError> {
        let mut log = Self::default();
        let lines: Vec<&str> = text.split('\n').collect();
        for (index, raw) in lines.iter().enumerate() {
            if raw.trim().is_empty() {
                continue;
            }
            let error = |message: String| EventsError {
                line: index + 1,
                message,
            };
            let line: Line = match serde_json::from_str(raw) {
                Ok(line) => line,
                Err(_) if index + 1 == lines.len() => break,
                Err(e) => return Err(error(e.to_string())),
            };
            let t = Micros(line.t);
            if log.path.last().is_some_and(|(last, _)| t < *last) {
                return Err(error("events are not sorted by time".into()));
            }
            let at = line.x.zip(line.y).map(|(x, y)| [x as f32, y as f32]);
            match (line.kind.as_str(), at) {
                ("key", _) => log.keys.push(KeyPress {
                    t,
                    key: line
                        .key
                        .ok_or_else(|| error("a key event needs `key`".into()))?,
                    mods: line.mods,
                }),
                ("move" | "down" | "up" | "scroll" | "marker", Some(at)) => {
                    if line.kind == "down" {
                        log.clicks.push(Click { t, at });
                    }
                    log.path.push((t, at));
                }
                ("move" | "down" | "up" | "scroll" | "marker", None) => {
                    return Err(error(format!("a {} event needs `x` and `y`", line.kind)));
                }
                _ => {}
            }
        }
        log.integral = integrate(&log.path);
        Ok(log)
    }

    /// Every event with a position, in time order.
    pub fn path(&self) -> &[(Micros, [f32; 2])] {
        &self.path
    }

    pub fn clicks(&self) -> &[Click] {
        &self.clicks
    }

    pub fn keys(&self) -> &[KeyPress] {
        &self.keys
    }

    /// The cursor at `t`, averaged over `smoothing` centered on it. `None` without any position.
    pub fn cursor(&self, t: Micros, smoothing: Micros) -> Option<[f32; 2]> {
        if smoothing.0 == 0 {
            return self.raw(t);
        }
        let half = smoothing.0 as f64 / 2.0;
        let t = t.0 as f64;
        let [x0, y0] = self.integral_at(t - half)?;
        let [x1, y1] = self.integral_at(t + half)?;
        let width = 2.0 * half;
        Some([((x1 - x0) / width) as f32, ((y1 - y0) / width) as f32])
    }

    /// Every position the cursor takes in `[from, to)`, including where it is at `from`.
    pub fn positions(&self, from: Micros, to: Micros) -> impl Iterator<Item = [f32; 2]> + '_ {
        let start = self.path.partition_point(|(t, _)| *t <= from);
        self.raw(from).into_iter().chain(
            self.path[start..]
                .iter()
                .take_while(move |(t, _)| *t < to)
                .map(|(_, at)| *at),
        )
    }

    /// The latest position at or before `t`, or the first one before the log starts.
    fn raw(&self, t: Micros) -> Option<[f32; 2]> {
        let index = self.path.partition_point(|(at, _)| *at <= t);
        self.path
            .get(index.saturating_sub(1))
            .map(|(_, position)| *position)
    }

    /// The integral of the path from its first event to `t`, which may lie outside the log.
    fn integral_at(&self, t: f64) -> Option<[f64; 2]> {
        let index = self
            .path
            .partition_point(|(at, _)| at.0 as f64 <= t)
            .saturating_sub(1);
        let (at, [x, y]) = *self.path.get(index)?;
        let [ix, iy] = self.integral[index];
        let dt = t - at.0 as f64;
        Some([ix + f64::from(x) * dt, iy + f64::from(y) * dt])
    }
}

fn integrate(path: &[(Micros, [f32; 2])]) -> Vec<[f64; 2]> {
    let mut sum = [0.0, 0.0];
    let mut out = Vec::with_capacity(path.len());
    for (index, (t, _)) in path.iter().enumerate() {
        if let Some((previous, [x, y])) = index.checked_sub(1).map(|i| path[i]) {
            let dt = (t.0 - previous.0) as f64;
            sum = [sum[0] + f64::from(x) * dt, sum[1] + f64::from(y) * dt];
        }
        out.push(sum);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = r#"{"t":0,"kind":"move","x":100,"y":100}
{"t":1000000,"kind":"move","x":300,"y":100,"pressure":3}
{"t":1500000,"kind":"down","x":300,"y":100,"button":"left"}
{"t":1600000,"kind":"up","x":300,"y":100,"button":"left"}
{"t":2000000,"kind":"key","key":"k","mods":["cmd"]}
{"t":2100000,"kind":"future","x":1,"y":1}
{"t":3000000,"kind":"marker","x":500,"y":100,"mark":"zoom"}
{"t":3100000,"kind":"move","x":5"#;

    #[test]
    fn reads_clicks_keys_and_the_path_and_drops_a_torn_last_line() {
        let log = EventLog::parse(LOG).unwrap();
        assert_eq!(
            log.clicks(),
            [Click {
                t: Micros(1_500_000),
                at: [300.0, 100.0]
            }]
        );
        assert_eq!(log.keys()[0].mods, [Modifier::Cmd]);
        assert_eq!(log.cursor(Micros(999_999), Micros(0)), Some([100.0, 100.0]));
        assert_eq!(
            log.cursor(Micros(3_500_000), Micros(0)),
            Some([500.0, 100.0])
        );
        let error = EventLog::parse("{\"t\":5,\"kind\":\"move\"}\n").unwrap_err();
        assert_eq!(error.line, 1);
    }

    #[test]
    fn smoothing_averages_the_path_over_a_centered_window() {
        let log = EventLog::parse(LOG).unwrap();
        // Half of the window at x 100, half at x 300.
        let [x, y] = log.cursor(Micros(1_000_000), Micros(200_000)).unwrap();
        assert!((x - 200.0).abs() < 1e-3 && (y - 100.0).abs() < 1e-3);
        // Before the log starts the first position holds.
        assert_eq!(log.cursor(Micros(0), Micros(200_000)), Some([100.0, 100.0]));
    }
}
