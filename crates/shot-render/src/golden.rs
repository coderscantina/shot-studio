//! The golden-frame rule, shared by every test that compares two frames drawn by the renderer.
//!
//! Two frames match when no channel differs by more than [`MAX_STEP`] code values and at most
//! [`MAX_PIXELS`] pixels differ at all.

/// The most a channel may differ by, in 8-bit code values.
pub const MAX_STEP: u8 = 1;
/// The most pixels that may differ at all.
pub const MAX_PIXELS: usize = 16;

/// Panics with `label` and the measured difference unless `a` and `b` match.
pub fn assert_match(label: &str, a: &[[u8; 3]], b: &[[u8; 3]]) {
    assert_eq!(a.len(), b.len(), "{label}: frames differ in size");
    let (mut pixels, mut step) = (0usize, 0u8);
    for (a, b) in a.iter().zip(b) {
        let worst = (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0);
        pixels += usize::from(worst > 0);
        step = step.max(worst);
    }
    assert!(
        step <= MAX_STEP && pixels <= MAX_PIXELS,
        "{label}: {pixels} pixels differ, by up to {step} (at most {MAX_PIXELS} pixels by {MAX_STEP})"
    );
}

/// The frames of a played pass, for tests that seek to frames afterwards: a hash of every frame,
/// and the pixels of the ones that will be sought.
pub struct Played {
    hashes: Vec<blake3::Hash>,
    kept: std::collections::HashMap<u64, Vec<[u8; 3]>>,
}

impl Played {
    /// Keeps the pixels of the `sought` frames out of `frames`.
    pub fn new(frames: u64, sought: impl IntoIterator<Item = u64>) -> Self {
        Self {
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
        assert_match(&format!("{label} frame {frame}"), played, pixels);
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
        assert_match("same", &base, &base);
        assert_match("edge", &base, &nudge(MAX_PIXELS, 1));
        for bad in [nudge(MAX_PIXELS + 1, 1), nudge(1, 2)] {
            let caught = std::panic::catch_unwind(|| assert_match("bad", &base, &bad));
            assert!(caught.is_err());
        }
    }
}
