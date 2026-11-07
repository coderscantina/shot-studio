//! The auto-zoom rule: centered `zoom` nodes written from a take's event log, per
//! `auto-zoom` in `format.kdl` and decision 0014. Pure.
//!
//! - **click**: a zoom on each click, starting [`LEAD`] before it.
//! - **typing**: a zoom on each run of at least [`TYPING_KEYS`] key presses less than
//!   [`TYPING_GAP`] apart, centered where the cursor was when typing started, until [`TAIL`]
//!   after the last key.
//! - **cursor**: a zoom wherever the cursor rests within [`DWELL_RADIUS`] for at least `hold`.
//!
//! Each zoom starts `hold` long. Overlapping zooms merge while the cursor's path through both
//! fits the zoomed view; otherwise the earlier one ends where the later one starts. Each zoom
//! then centers as close to its trigger as it can while the cursor's path stays inside the view,
//! [`MARGIN`] clear of its edges. Where the path does not fit, the zoom starts later, up to its
//! trigger, and ends where the cursor leaves. So a zoom can end up shorter than `hold`, but it
//! never cuts off the cursor. Running the rule again replaces the zooms with
//! `by=click`, `typing` or `cursor` and leaves every other zoom alone; a new zoom that would
//! overlap one of those is not written.

use shot_core::{AutoZoom, Micros, Point, TakeEdit, TimeRange, Zoom, ZoomBy, ZoomTrigger};

use crate::events::EventLog;

/// How long before its trigger a zoom starts: a smooth zoom has eased in by the click.
pub const LEAD: Micros = Micros(500_000);
/// How long a typing zoom stays after the last key.
pub const TAIL: Micros = Micros(700_000);
pub const TYPING_GAP: Micros = Micros(1_000_000);
pub const TYPING_KEYS: usize = 3;
/// Screen pixels the cursor may drift while resting.
pub const DWELL_RADIUS: f32 = 48.0;
/// The share of the zoomed view kept clear around the cursor on each side.
pub const MARGIN: f32 = 0.08;

#[derive(Debug, Clone, Copy)]
struct Candidate {
    from: Micros,
    to: Micros,
    /// The moment that triggered it, which the zoom always covers.
    at: Micros,
    center: [f32; 2],
    by: ZoomBy,
}

/// `take` with its auto-zoom zooms replaced by the ones `rule` writes from `events`, on a screen
/// track of size `screen`.
pub fn apply(take: &TakeEdit, rule: &AutoZoom, events: &EventLog, screen: (u32, u32)) -> TakeEdit {
    let others: Vec<Zoom> = take
        .zooms
        .iter()
        .filter(|z| !is_auto_zoom(z.by))
        .copied()
        .collect();
    let written = zooms(rule, events, screen)
        .into_iter()
        .filter(|z| !others.iter().any(|o| o.range.overlaps(z.range)));
    let mut all: Vec<Zoom> = others.iter().copied().chain(written).collect();
    all.sort_by_key(|z| z.range.from());
    TakeEdit {
        zooms: all,
        ..take.clone()
    }
}

/// Whether the auto-zoom rule wrote a zoom with this provenance.
pub fn is_auto_zoom(by: Option<ZoomBy>) -> bool {
    matches!(by, Some(ZoomBy::Click | ZoomBy::Typing | ZoomBy::Cursor))
}

/// The zooms `rule` writes from `events`, in order and not overlapping.
pub fn zooms(rule: &AutoZoom, events: &EventLog, screen: (u32, u32)) -> Vec<Zoom> {
    let view = View::new(rule.scale, screen);
    let mut candidates = Vec::new();
    for trigger in &rule.triggers {
        match trigger {
            ZoomTrigger::Click => candidates.extend(events.clicks().iter().map(|c| Candidate {
                from: before(c.t, LEAD),
                to: Micros(before(c.t, LEAD).0 + rule.hold.0),
                at: c.t,
                center: c.at,
                by: ZoomBy::Click,
            })),
            ZoomTrigger::Typing => candidates.extend(typing(events, rule.hold)),
            ZoomTrigger::Cursor => candidates.extend(dwells(events, rule.hold)),
        }
    }
    candidates.sort_by_key(|c| c.from);

    let mut merged: Vec<Candidate> = Vec::new();
    for next in candidates {
        if let Some(last) = merged.last_mut()
            && next.from < last.to
        {
            let to = last.to.max(next.to);
            let mut bounds = path_bounds(events, last.from, to);
            bounds.add(last.center);
            bounds.add(next.center);
            if view.fits(&bounds) {
                last.to = to;
                continue;
            }
            last.to = next.from;
        }
        merged.push(next);
    }

    let mut out: Vec<Zoom> = Vec::new();
    for candidate in merged {
        let from = out
            .last()
            .map_or(candidate.from, |z| candidate.from.max(z.range.to()));
        let at = candidate.at.max(from);
        let (range, center) = view.place(events, from, candidate.to, at, candidate.center);
        let Some(range) = range else {
            continue;
        };
        out.push(Zoom {
            scale: rule.scale,
            range,
            center: Some(Point {
                x: center[0].round() as i32,
                y: center[1].round() as i32,
            }),
            by: Some(candidate.by),
        });
    }
    out
}

fn before(t: Micros, by: Micros) -> Micros {
    Micros(t.0.saturating_sub(by.0))
}

fn typing(events: &EventLog, hold: Micros) -> Vec<Candidate> {
    let keys = events.keys();
    let mut out = Vec::new();
    let mut start = 0;
    for end in 1..=keys.len() {
        let breaks = end == keys.len() || keys[end].t.0 - keys[end - 1].t.0 >= TYPING_GAP.0;
        if !breaks {
            continue;
        }
        let run = &keys[start..end];
        start = end;
        if run.len() < TYPING_KEYS {
            continue;
        }
        let Some(center) = events.cursor(run[0].t, Micros(0)) else {
            continue;
        };
        let from = before(run[0].t, LEAD);
        let to = Micros((run[run.len() - 1].t.0 + TAIL.0).max(from.0 + hold.0));
        out.push(Candidate {
            from,
            to,
            at: run[0].t,
            center,
            by: ZoomBy::Typing,
        });
    }
    out
}

/// Where the cursor rests for at least `hold`.
fn dwells(events: &EventLog, hold: Micros) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut anchor: Option<(Micros, [f32; 2])> = None;
    let mut last = Micros(0);
    let close = |anchor: Option<(Micros, [f32; 2])>, until: Micros, out: &mut Vec<_>| {
        if let Some((from, center)) = anchor
            && until.0 - from.0 >= hold.0
        {
            out.push(Candidate {
                from,
                to: until,
                at: from,
                center,
                by: ZoomBy::Cursor,
            });
        }
    };
    for (t, at) in events.path() {
        last = *t;
        match anchor {
            Some((_, a)) if distance(a, *at) <= DWELL_RADIUS => {}
            _ => {
                close(anchor, *t, &mut out);
                anchor = Some((*t, *at));
            }
        }
    }
    close(anchor, last, &mut out);
    out
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// A bounding box of screen points.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    min: [f32; 2],
    max: [f32; 2],
}

impl Bounds {
    const EMPTY: Self = Self {
        min: [f32::MAX; 2],
        max: [f32::MIN; 2],
    };

    fn add(&mut self, p: [f32; 2]) {
        for (axis, value) in p.into_iter().enumerate() {
            self.min[axis] = self.min[axis].min(value);
            self.max[axis] = self.max[axis].max(value);
        }
    }

    fn is_empty(&self) -> bool {
        self.min[0] > self.max[0]
    }
}

fn path_bounds(events: &EventLog, from: Micros, to: Micros) -> Bounds {
    let mut bounds = Bounds::EMPTY;
    for p in events.positions(from, to) {
        bounds.add(p);
    }
    bounds
}

/// The zoomed view of one scale on one screen.
struct View {
    size: [f32; 2],
    screen: [f32; 2],
}

impl View {
    fn new(scale: u16, (width, height): (u32, u32)) -> Self {
        let factor = f32::from(scale.max(100)) / 100.0;
        Self {
            size: [width as f32 / factor, height as f32 / factor],
            screen: [width as f32, height as f32],
        }
    }

    /// Half the view less its margin: how far from the center the cursor may go.
    fn reach(&self, axis: usize) -> f32 {
        self.size[axis] * (0.5 - MARGIN)
    }

    fn fits(&self, bounds: &Bounds) -> bool {
        bounds.is_empty()
            || (0..2).all(|axis| bounds.max[axis] - bounds.min[axis] <= 2.0 * self.reach(axis))
    }

    /// The range and center of a zoom over `[from, to)` triggered at `at` and aimed at `aim`.
    /// Where the cursor path does not fit the view, the zoom starts later, up to `at`, and ends
    /// where the path leaves the view. It centers as close to `aim` as the path allows.
    fn place(
        &self,
        events: &EventLog,
        from: Micros,
        to: Micros,
        at: Micros,
        aim: [f32; 2],
    ) -> (Option<TimeRange>, [f32; 2]) {
        let path = events.path();
        let mut bounds = Bounds::EMPTY;
        events.positions(at, at).for_each(|p| bounds.add(p));
        // Back from the trigger: each event's position holds from its time on.
        let mut start = from;
        let first_after = path.partition_point(|(t, _)| *t <= at);
        for k in (0..first_after).rev() {
            let (t, _) = path[k];
            if t <= from {
                break;
            }
            let mut grown = bounds;
            if let Some((_, before)) = k.checked_sub(1).map(|i| path[i]) {
                grown.add(before);
            }
            if !self.fits(&grown) {
                start = t;
                break;
            }
            bounds = grown;
        }
        // Forward from the trigger: end where the next position does not fit.
        let mut end = to;
        for (t, p) in &path[first_after..] {
            if *t >= to {
                break;
            }
            let mut grown = bounds;
            grown.add(*p);
            if !self.fits(&grown) {
                end = *t;
                break;
            }
            bounds = grown;
        }
        let center = [0, 1].map(|axis| {
            let ideal = if bounds.is_empty() {
                aim[axis]
            } else {
                aim[axis].clamp(
                    bounds.max[axis] - self.reach(axis),
                    bounds.min[axis] + self.reach(axis),
                )
            };
            let half = self.size[axis] / 2.0;
            ideal.clamp(half, (self.screen[axis] - half).max(half))
        });
        (TimeRange::new(start, end), center)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zoom::punch_at;
    use shot_core::ZoomMotion;

    const SCREEN: (u32, u32) = (3840, 2160);

    fn rule(triggers: Vec<ZoomTrigger>) -> AutoZoom {
        AutoZoom {
            triggers,
            scale: 160,
            hold: Micros(1_500_000),
        }
    }

    /// Every frame of every zoom shows the cursor inside the visible part of the screen.
    pub(crate) fn cursor_always_in_view(zooms: &[Zoom], events: &EventLog, end: Micros) -> bool {
        let spec = crate::layout::LayoutSpec::of(shot_core::Layout::Content).unwrap();
        let sizes = crate::layout::Sources {
            screen: SCREEN,
            camera: (1920, 1080),
        };
        (0..end.0).step_by(16_667).all(|t| {
            let t = Micros(t);
            let punch = punch_at(zooms, t, ZoomMotion::Smooth, SCREEN);
            let crop = spec.scene(crate::layout::Canvas::UHD, sizes, punch).layers[0].crop;
            let Some([x, y]) = events.cursor(t, Micros(0)) else {
                return true;
            };
            crop.contains([x / SCREEN.0 as f32, y / SCREEN.1 as f32])
        })
    }

    #[test]
    fn clicks_far_apart_get_one_zoom_each_centered_on_them() {
        let log = EventLog::parse(
            "{\"t\":0,\"kind\":\"move\",\"x\":800,\"y\":500}
{\"t\":1000000,\"kind\":\"down\",\"x\":800,\"y\":500,\"button\":\"left\"}
{\"t\":4000000,\"kind\":\"move\",\"x\":3000,\"y\":1800}
{\"t\":5000000,\"kind\":\"down\",\"x\":3000,\"y\":1800,\"button\":\"left\"}
",
        )
        .unwrap();
        let zooms = zooms(&rule(vec![ZoomTrigger::Click]), &log, SCREEN);
        let summary: Vec<_> = zooms
            .iter()
            .map(|z| (z.range.from().0, z.range.to().0, z.center, z.by))
            .collect();
        // The view is 2400x1350 at 160 %, so the centers clamp to the screen.
        assert_eq!(
            summary,
            [
                (
                    500_000,
                    2_000_000,
                    Some(Point { x: 1200, y: 675 }),
                    Some(ZoomBy::Click)
                ),
                (
                    4_500_000,
                    6_000_000,
                    Some(Point { x: 2640, y: 1485 }),
                    Some(ZoomBy::Click)
                ),
            ]
        );
        assert!(cursor_always_in_view(&zooms, &log, Micros(7_000_000)));
    }

    #[test]
    fn a_far_second_click_splits_the_zoom_instead_of_cutting_off_the_cursor() {
        let log = EventLog::parse(
            "{\"t\":0,\"kind\":\"move\",\"x\":400,\"y\":400}
{\"t\":1000000,\"kind\":\"down\",\"x\":400,\"y\":400,\"button\":\"left\"}
{\"t\":1600000,\"kind\":\"move\",\"x\":3400,\"y\":1700}
{\"t\":2000000,\"kind\":\"down\",\"x\":3400,\"y\":1700,\"button\":\"left\"}
",
        )
        .unwrap();
        let zooms = zooms(&rule(vec![ZoomTrigger::Click]), &log, SCREEN);
        assert_eq!(zooms.len(), 2);
        // The two do not fit one view: the first ends where the second would start, and the
        // second waits until the cursor has arrived.
        assert_eq!(zooms[0].range.to(), Micros(1_500_000));
        assert_eq!(zooms[1].range.from(), Micros(1_600_000));
        assert!(cursor_always_in_view(&zooms, &log, Micros(4_000_000)));
    }

    #[test]
    fn typing_zooms_where_it_starts_and_reruns_keep_a_persons_zoom() {
        let mut lines = vec!["{\"t\":0,\"kind\":\"move\",\"x\":1400,\"y\":900}".to_owned()];
        for i in 0..5u64 {
            lines.push(format!(
                "{{\"t\":{},\"kind\":\"key\",\"key\":\"a\",\"mods\":[]}}",
                3_000_000 + i * 120_000
            ));
        }
        let log = EventLog::parse(&(lines.join("\n") + "\n")).unwrap();
        let person = Zoom {
            scale: 120,
            range: TimeRange::new(Micros(10_000_000), Micros(11_000_000)).unwrap(),
            center: None,
            by: None,
        };
        let stale = Zoom {
            by: Some(ZoomBy::Click),
            range: TimeRange::new(Micros(20_000_000), Micros(21_000_000)).unwrap(),
            ..person
        };
        let take = TakeEdit {
            take: shot_core::Id::new("t01").unwrap(),
            ranges: Vec::new(),
            layouts: Vec::new(),
            reviews: Vec::new(),
            zooms: vec![person, stale],
            overlays: Vec::new(),
        };
        let applied = apply(&take, &rule(vec![ZoomTrigger::Typing]), &log, SCREEN);
        let by: Vec<_> = applied
            .zooms
            .iter()
            .map(|z| (z.range.from().0, z.by))
            .collect();
        assert_eq!(by, [(2_500_000, Some(ZoomBy::Typing)), (10_000_000, None)]);
        assert_eq!(applied.zooms[0].range.to(), Micros(4_180_000));
        assert_eq!(
            apply(&applied, &rule(vec![ZoomTrigger::Typing]), &log, SCREEN),
            applied
        );
    }
}
