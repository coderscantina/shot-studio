//! One video track's frames in time order, for preview and export alike: the frame on screen at
//! a requested time, decoding forward while the times move forward and seeking otherwise.

use std::time::Duration;

use shot_media::videotoolbox::{DecodeError, Decoder, Frame};

/// Frame times come from integer division on both sides; this absorbs the rounding.
const PTS_TOLERANCE: Duration = Duration::from_millis(1);

/// Forward jumps up to this long decode on instead of seeking. With the recordings' 0.5 s
/// keyframe interval, a seek decodes up to 29 frames anyway.
const DECODE_ON: Duration = Duration::from_secs(1);

pub struct FrameCursor {
    decoder: Decoder,
    current: Option<Frame>,
    next: Option<Frame>,
    ended: bool,
}

impl FrameCursor {
    pub fn new(decoder: Decoder) -> Self {
        Self {
            decoder,
            current: None,
            next: None,
            ended: false,
        }
    }

    /// Moves to the frame on screen at `time` and returns it. Past the last frame, the last one
    /// stays. `None` only for a track without frames.
    pub fn goto(&mut self, time: Duration) -> Result<Option<&Frame>, DecodeError> {
        let shown = self.current.as_ref().map(|f| f.pts);
        let decodes_on =
            shown.is_some_and(|pts| pts <= time + PTS_TOLERANCE && time < pts + DECODE_ON);
        if !decodes_on {
            self.next = None;
            self.ended = false;
            self.current = self.decoder.seek(time + PTS_TOLERANCE, || false)?;
            return Ok(self.current.as_ref());
        }
        loop {
            if self.next.is_none() && !self.ended {
                self.next = self.decoder.next_frame()?;
                self.ended = self.next.is_none();
            }
            match &self.next {
                Some(next) if next.pts <= time + PTS_TOLERANCE => self.current = self.next.take(),
                _ => return Ok(self.current.as_ref()),
            }
        }
    }

    /// The frame the last [`goto`](Self::goto) returned.
    pub fn current(&self) -> Option<&Frame> {
        self.current.as_ref()
    }
}
