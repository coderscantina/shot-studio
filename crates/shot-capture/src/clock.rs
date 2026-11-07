//! The take's media clock.
//!
//! Every source stamps its samples on a clock of its own (ScreenCaptureKit and AVCaptureSession
//! each report a synchronization clock, the event tap reads the host clock). A session picks one
//! origin on the host clock and expresses it in each source's clock with `CMSyncConvertTime`, so
//! media time is `pts - origin` in every track and in the event log. Samples are rewritten to
//! media time before they reach the writer. Screen frames are the exception: a pacer writes the
//! newest frame at `k / 60` s after the origin, read from the host clock.

use shot_core::Micros;

/// Converts source timestamps to media time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaClock {
    origin_nanos: i64,
}

impl MediaClock {
    /// A clock whose zero is `origin_nanos` on the source's clock.
    pub fn new(origin_nanos: i64) -> Self {
        Self { origin_nanos }
    }

    /// Media time of a source timestamp, or `None` for one before the origin.
    pub fn media_time(self, nanos: i64) -> Option<Micros> {
        let since = nanos.checked_sub(self.origin_nanos)?;
        u64::try_from(since).ok().map(|n| Micros(n / 1_000))
    }
}

/// Nanoseconds of a `value / timescale` timestamp, exact up to the nanosecond.
pub fn nanos(value: i64, timescale: i32) -> i64 {
    if timescale <= 0 {
        return 0;
    }
    (i128::from(value) * 1_000_000_000 / i128::from(timescale)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_time_counts_from_the_origin_and_drops_earlier_samples() {
        let clock = MediaClock::new(5_000_000_000);
        assert_eq!(clock.media_time(5_000_000_000), Some(Micros(0)));
        assert_eq!(clock.media_time(5_016_667_999), Some(Micros(16_667)));
        assert_eq!(clock.media_time(4_999_999_999), None);
    }

    #[test]
    fn nanos_converts_any_timescale() {
        assert_eq!(nanos(48_000, 48_000), 1_000_000_000);
        assert_eq!(nanos(1, 60), 16_666_666);
        assert_eq!(nanos(123_456_789, 1_000_000_000), 123_456_789);
        assert_eq!(nanos(3_600 * 48_000 * 24, 48_000), 86_400_000_000_000);
    }
}
