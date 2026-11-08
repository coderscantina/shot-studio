//! The golden-frame rule, shared by every test that compares two frames drawn by the renderer.
//!
//! Two frames match when no channel differs by more than [`MAX_STEP`] code values and at most
//! [`MAX_PIXELS`] pixels differ at all.

/// The most a channel may differ by, in 8-bit code values.
pub const MAX_STEP: u8 = 1;
/// The most pixels that may differ at all.
pub const MAX_PIXELS: usize = 16;

/// Panics with `label` and the measured difference unless `a` and `b`, frames `width` pixels
/// wide, match. A failure also writes both frames' difference, times 8, and `b` as
/// `golden-failures/<label>-diff.png` and `-actual.png` under the cargo target directory
/// (`CARGO_TARGET_DIR`, else `target`), and names the paths in the panic message. `a` is the
/// expected frame, `b` the one under test.
pub fn assert_match(label: &str, width: u32, a: &[[u8; 3]], b: &[[u8; 3]]) {
    assert_eq!(a.len(), b.len(), "{label}: frames differ in size");
    let (mut pixels, mut step) = (0usize, 0u8);
    for (a, b) in a.iter().zip(b) {
        let worst = (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0);
        pixels += usize::from(worst > 0);
        step = step.max(worst);
    }
    if step <= MAX_STEP && pixels <= MAX_PIXELS {
        return;
    }
    let diff: Vec<[u8; 3]> = a
        .iter()
        .zip(b)
        .map(|(a, b)| std::array::from_fn(|i| a[i].abs_diff(b[i]).saturating_mul(8)))
        .collect();
    let saved = [("actual", b), ("diff", &diff)].map(|(kind, frame)| {
        write_failure(label, kind, width, frame).unwrap_or_else(|e| format!("no {kind} image: {e}"))
    });
    panic!(
        "{label}: {pixels} pixels differ, by up to {step} (at most {MAX_PIXELS} pixels by {MAX_STEP})\n  actual: {}\n  diff (x8): {}",
        saved[0], saved[1]
    );
}

fn write_failure(label: &str, kind: &str, width: u32, frame: &[[u8; 3]]) -> Result<String, String> {
    let dir = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| "target".into(), std::path::PathBuf::from)
        .join("golden-failures");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let path = dir.join(format!("{name}-{kind}.png"));
    let height = (frame.len() / width as usize) as u32;
    image::RgbImage::from_raw(width, height, frame.as_flattened().to_vec())
        .ok_or("the frame is not a whole number of rows")?
        .save(&path)
        .map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// The frames of a played pass, for tests that seek to frames afterwards: a hash of every frame,
/// and the pixels of the ones that will be sought.
pub struct Played {
    width: u32,
    hashes: Vec<blake3::Hash>,
    kept: std::collections::HashMap<u64, Vec<[u8; 3]>>,
}

impl Played {
    /// Keeps the pixels of the `sought` frames out of `frames`, each `width` pixels wide.
    pub fn new(width: u32, frames: u64, sought: impl IntoIterator<Item = u64>) -> Self {
        Self {
            width,
            hashes: Vec::with_capacity(frames as usize),
            kept: sought.into_iter().map(|f| (f, Vec::new())).collect(),
        }
    }

    /// Records the next frame of the pass, in order.
    pub fn push(&mut self, pixels: &[[u8; 3]]) {
        let frame = self.hashes.len() as u64;
        self.hashes.push(blake3::hash(pixels.as_flattened()));
        if let Some(kept) = self.kept.get_mut(&frame) {
            *kept = pixels.to_vec();
        }
    }

    /// Passes at once on equal pixels, otherwise holds `pixels` to [`assert_match`].
    pub fn assert_same(&self, label: &str, frame: u64, pixels: &[[u8; 3]]) {
        if blake3::hash(pixels.as_flattened()) == self.hashes[frame as usize] {
            return;
        }
        let played = self.kept.get(&frame).expect("a sought frame was kept");
        assert_match(
            &format!("{label} frame {frame}"),
            self.width,
            played,
            pixels,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_step_on_few_pixels_passes_and_anything_more_fails() {
        let base = vec![[10u8; 3]; 100];
        let nudge = |count: usize, by: u8| {
            let mut frame = base.clone();
            frame[..count].iter_mut().for_each(|p| p[1] += by);
            frame
        };
        assert_match("same", 10, &base, &base);
        assert_match("edge", 10, &base, &nudge(MAX_PIXELS, 1));
        for bad in [nudge(MAX_PIXELS + 1, 1), nudge(1, 2)] {
            let caught = std::panic::catch_unwind(|| assert_match("bad", 10, &base, &bad));
            assert!(caught.is_err());
        }
    }

    #[test]
    fn a_failure_leaves_the_actual_frame_and_the_x8_difference() {
        let base = vec![[10u8; 3]; 100];
        let mut other = base.clone();
        other[3] = [14, 10, 10];
        let caught =
            std::panic::catch_unwind(|| assert_match("golden unit/failure", 10, &base, &other))
                .unwrap_err();
        let message = caught.downcast_ref::<String>().unwrap();
        let path = |kind: &str| {
            let line = message
                .lines()
                .find(|l| l.trim_start().starts_with(kind))
                .unwrap();
            std::path::PathBuf::from(line.rsplit_once(": ").unwrap().1)
        };
        let diff = image::open(path("diff")).unwrap().to_rgb8();
        assert_eq!(diff.get_pixel(3, 0).0, [32, 0, 0]);
        let actual = image::open(path("actual")).unwrap().to_rgb8();
        assert_eq!(actual.get_pixel(3, 0).0, [14, 10, 10]);
    }
}
