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
