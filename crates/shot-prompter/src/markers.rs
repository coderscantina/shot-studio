//! Hotkey markers into `cut.kdl` nodes, once, when a take is appended (decision 0014).

use shot_capture::events::{Mark, Marker};
use shot_core::{Format, LayoutChange, Micros, Point, TakeEdit, TimeRange, Zoom, ZoomBy};

/// Zoom of a Z marker when the format has no auto zoom to take scale and hold from.
const ZOOM_SCALE: u16 = 150;
const ZOOM_HOLD: Micros = Micros(2_000_000);

/// Writes a take's layout markers as `layout` nodes and its Z markers as `zoom ... by=marker`
/// nodes, centered on the cursor at the press.
///
/// A layout lasts until the next layout marker or the end of the take, and the format's default
/// layout needs no node. A zoom has the format's auto zoom scale and hold, and ends early where
/// the next Z marker starts. Zooms already `by=marker` are replaced, every other zoom stays and
/// wins where they overlap. A, N, M and B markers are left to the retake rule, chapters and
/// b-roll.
pub fn apply_markers(edit: &mut TakeEdit, markers: &[Marker], format: &Format) {
    let Some(end) = edit.ranges.iter().map(|r| r.range().to()).max() else {
        return;
    };
    let mut markers: Vec<&Marker> = markers.iter().filter(|m| m.t < end).collect();
    markers.sort_by_key(|m| m.t);

    let switches: Vec<_> = markers
        .iter()
        .filter_map(|m| match m.mark {
            Mark::Layout { layout } => Some((m.t, layout)),
            _ => None,
        })
        .collect();
    if !switches.is_empty() {
        edit.layouts = switches
            .iter()
            .enumerate()
            .filter(|(i, (_, layout))| {
                *layout != format.default_layout && (*i == 0 || switches[i - 1].1 != *layout)
            })
            .filter_map(|(i, &(from, layout))| {
                let to = switches[i + 1..]
                    .iter()
                    .find(|(_, next)| *next != layout)
                    .map_or(end, |(t, _)| *t);
                Some(LayoutChange {
                    layout,
                    range: TimeRange::new(from, to)?,
                })
            })
            .collect();
    }

    let (scale, hold) = format
        .auto_zoom
        .as_ref()
        .map_or((ZOOM_SCALE, ZOOM_HOLD), |z| (z.scale, z.hold));
    let presses: Vec<&Marker> = markers
        .iter()
        .copied()
        .filter(|m| m.mark == Mark::Zoom)
        .collect();
    edit.zooms.retain(|z| z.by != Some(ZoomBy::Marker));
    for (i, press) in presses.iter().enumerate() {
        let mut to = Micros((press.t.0 + hold.0).min(end.0));
        if let Some(next) = presses.get(i + 1) {
            to = to.min(next.t);
        }
        let mut from = press.t;
        // Other zooms win: start after one that covers the press, end before the next one.
        for zoom in &edit.zooms {
            let (zf, zt) = (zoom.range.from(), zoom.range.to());
            if zf <= from && from < zt {
                from = zt;
            } else if from < zf && zf < to {
                to = zf;
            }
        }
        if let Some(range) = TimeRange::new(from, to) {
            edit.zooms.push(Zoom {
                scale,
                range,
                center: Some(Point {
                    x: press.x,
                    y: press.y,
                }),
                by: Some(ZoomBy::Marker),
            });
        }
    }
    edit.zooms.sort_by_key(|z| z.range.from());
}

#[cfg(test)]
mod tests {
    use shot_core::{Cut, FormatKind, Id, Layout};

    use super::*;

    fn marker(t: u64, mark: Mark) -> Marker {
        Marker {
            t: Micros(t),
            x: 1480,
            y: 910,
            mark,
        }
    }

    fn take() -> TakeEdit {
        let cut = Cut::default().append_take(
            Id::new("t01").unwrap(),
            "Take t01",
            TimeRange::new(Micros(0), Micros(20_000_000)).unwrap(),
        );
        cut.chapters[0].takes[0].clone()
    }

    #[test]
    fn layout_markers_become_layout_ranges_without_the_default() {
        let format = FormatKind::Opinion.preset();
        assert_eq!(format.default_layout, Layout::Face);
        let mut edit = take();
        let layout = |layout| Mark::Layout { layout };
        apply_markers(
            &mut edit,
            &[
                marker(2_000_000, layout(Layout::Split)),
                marker(4_000_000, layout(Layout::Split)),
                marker(6_000_000, layout(Layout::Face)),
                marker(9_000_000, Mark::Marker),
                marker(15_000_000, layout(Layout::Content)),
            ],
            &format,
        );
        let ranges: Vec<_> = edit
            .layouts
            .iter()
            .map(|l| (l.layout, l.range.from().0, l.range.to().0))
            .collect();
        assert_eq!(
            ranges,
            [
                (Layout::Split, 2_000_000, 6_000_000),
                (Layout::Content, 15_000_000, 20_000_000),
            ]
        );
    }

    #[test]
    fn zoom_markers_become_centered_marker_zooms_that_yield_to_others() {
        let format = FormatKind::Opinion.preset();
        let mut edit = take();
        let person = Zoom {
            scale: 112,
            range: TimeRange::new(Micros(10_500_000), Micros(12_000_000)).unwrap(),
            center: None,
            by: None,
        };
        edit.zooms.push(person);
        let markers = [
            marker(1_000_000, Mark::Zoom),
            marker(2_000_000, Mark::Zoom),
            marker(10_000_000, Mark::Zoom),
            marker(19_500_000, Mark::Zoom),
        ];
        apply_markers(&mut edit, &markers, &format);
        apply_markers(&mut edit, &markers, &format);
        let zooms: Vec<_> = edit
            .zooms
            .iter()
            .map(|z| (z.scale, z.range.from().0, z.range.to().0, z.by))
            .collect();
        let by = Some(ZoomBy::Marker);
        assert_eq!(
            zooms,
            [
                (150, 1_000_000, 2_000_000, by),
                (150, 2_000_000, 4_000_000, by),
                (150, 10_000_000, 10_500_000, by),
                (112, 10_500_000, 12_000_000, None),
                (150, 19_500_000, 20_000_000, by),
            ]
        );
        assert_eq!(edit.zooms[0].center, Some(Point { x: 1480, y: 910 }));
    }
}
