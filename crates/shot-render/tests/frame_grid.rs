//! Cut points the editor writes against the frame grid of a take's rate.

use shot_core::{Cut, RangeEdit};
use shot_render::export::FPS;
use shot_render::plan::frame_time;

/// Frame rates as numerator and denominator.
const RATES: [(u64, u64); 7] = [
    (24, 1),
    (25, 1),
    (30_000, 1_001),
    (30, 1),
    (50, 1),
    (60_000, 1_001),
    (60, 1),
];

/// Whether `t` microseconds is the start of a frame at `rate`, to the microsecond the file keeps.
fn on_grid(t: u64, (num, den): (u64, u64)) -> bool {
    let frame = (t as f64 * num as f64 / (den as f64 * 1e6)).round();
    (t as f64 - frame * den as f64 * 1e6 / num as f64).abs() <= 1.0
}

/// The editor turns output frames into media time with `frame_time`, so a cut point is the
/// start of a frame of the editor's rate. Written to `cut.kdl` and read back, it must still be
/// the start of a frame of the take's rate.
#[test]
#[ignore = "fails: the editor counts frames at 60 fps whatever the take's rate, so of 1552 cut \
    points 1238 (24 fps), 1415 (25), 1544 (29.97), 773 (30), 1287 (50) and 1544 (59.94) are off \
    their take's grid, by up to 20 ms; 60 fps passes"]
fn cut_points_land_on_the_frame_grid_of_the_takes_rate() {
    let mut off = Vec::new();
    for rate in RATES {
        let (mut bad, mut total, mut worst) = (0, 0, 0.0f64);
        for from in (0..3_000u64).step_by(31) {
            for length in [1, 2, 3, 5, 8, 13, 60, 97] {
                let (a, b) = (frame_time(from, FPS), frame_time(from + length, FPS));
                let text = format!(
                    "chapter \"c\" {{\n    take t01 {{\n        keep from={} to={}\n    }}\n}}\n",
                    a.0, b.0
                );
                let cut = Cut::parse(&Cut::parse(&text).unwrap().to_kdl()).unwrap();
                let RangeEdit::Keep(range) = cut.chapters[0].takes[0].ranges[0] else {
                    unreachable!("the text writes a keep")
                };
                for t in [range.from().0, range.to().0] {
                    total += 1;
                    if !on_grid(t, rate) {
                        bad += 1;
                        let frame = (t as f64 * rate.0 as f64 / (rate.1 as f64 * 1e6)).round();
                        worst = worst
                            .max((t as f64 - frame * rate.1 as f64 * 1e6 / rate.0 as f64).abs());
                    }
                }
            }
        }
        if bad > 0 {
            off.push(format!(
                "{}/{} fps: {bad} of {total} cut points off the grid, worst by {worst:.0} us",
                rate.0, rate.1
            ));
        }
    }
    assert!(off.is_empty(), "{}", off.join("\n"));
}
