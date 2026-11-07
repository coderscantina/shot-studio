//! Idle screen: stretches where the screen track does not change.
//!
//! The encoder already diffs every frame against the one before, so a frame's encoded size is
//! its frame diff: a predicted frame of an unchanged screen is a few hundred bytes of skipped
//! blocks (555 bytes at 4K from VideoToolbox), a visible change costs kilobytes. Reading the sizes from the fragment headers of
//! `screen.mp4` takes milliseconds for a 17 min take and decodes nothing, and works on a take
//! that is still being written.

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

use shot_core::{Micros, TimeRange};

/// An encoded frame no larger than one byte per this many pixels counts as unchanged. 2 KB at
/// 3840x2160, 0.5 KB at 1920x1080.
const PIXELS_PER_STILL_BYTE: u64 = 4_000;

/// One encoded video frame: its presentation time, size in bytes and whether it is a keyframe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedFrame {
    pub t: Micros,
    pub size: u32,
    pub key: bool,
}

/// The frames of the first video track of a fragmented MP4 and its size in pixels, read from
/// the `moov` and `moof` headers only.
pub fn encoded_frames(path: &Path) -> io::Result<(Vec<EncodedFrame>, u64)> {
    let mut file = BufReader::new(File::open(path)?);
    let length = file.get_ref().metadata()?.len();
    let bad = |why: &str| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {why}", path.display()),
        )
    };
    let mut track: Option<Track> = None;
    let mut frames = Vec::new();
    let mut decode_time = 0u64;
    let mut offset = 0;
    while offset + 8 <= length {
        let mut header = [0; 8];
        file.read_exact(&mut header)?;
        let size = u64::from(u32::from_be_bytes(header[..4].try_into().expect("4 bytes")));
        let (size, header_size) = match size {
            0 => (length - offset, 8),
            1 => {
                let mut large = [0; 8];
                file.read_exact(&mut large)?;
                (u64::from_be_bytes(large), 16)
            }
            size => (size, 8),
        };
        if size < header_size || offset + size > length {
            // A take still being written ends in a partial box.
            break;
        }
        let body_size = size - header_size;
        match &header[4..] {
            b"moov" | b"moof" => {
                let mut body = vec![0; body_size as usize];
                file.read_exact(&mut body)?;
                if &header[4..] == b"moov" {
                    track = Some(Track::of(&body).ok_or_else(|| bad("no readable video track"))?);
                } else if let Some(track) = &track {
                    track
                        .read_moof(&body, &mut decode_time, &mut frames)
                        .ok_or_else(|| bad("unreadable moof"))?;
                }
            }
            _ => {
                file.seek_relative(body_size as i64)?;
            }
        }
        offset += size;
    }
    let track = track.ok_or_else(|| bad("no moov"))?;
    frames.sort_by_key(|f| f.t);
    Ok((frames, track.pixels))
}

/// Stretches of at least `min` where the screen does not change, in whole keyframe intervals.
/// An interval is unchanged when its typical predicted frame is: the encoder spends a few
/// kilobytes refining the picture right after each keyframe, and a blinking caret is one frame,
/// so single frames do not decide.
pub fn still_ranges(frames: &[EncodedFrame], pixels: u64, min: Micros) -> Vec<TimeRange> {
    let limit = (pixels / PIXELS_PER_STILL_BYTE).max(64);
    let mut out = Vec::new();
    let mut push = |from: Micros, to: Micros| {
        if to.0.saturating_sub(from.0) >= min.0
            && let Some(range) = TimeRange::new(from, to)
        {
            out.push(range);
        }
    };
    let mut start: Option<Micros> = None;
    let mut sizes = Vec::new();
    for interval in frames.chunk_by(|_, next| !next.key) {
        sizes.clear();
        sizes.extend(interval.iter().filter(|f| !f.key).map(|f| f.size));
        sizes.sort_unstable();
        let still = sizes
            .get(sizes.len() / 2)
            .is_some_and(|&median| u64::from(median) <= limit);
        let at = interval[0].t;
        match (still, start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                push(from, at);
                start = None;
            }
            _ => {}
        }
    }
    if let (Some(from), Some(last)) = (start, frames.last()) {
        push(from, last.t);
    }
    out
}

struct Track {
    id: u32,
    timescale: u64,
    pixels: u64,
    default_duration: u32,
    default_size: u32,
    default_flags: u32,
}

const NON_SYNC: u32 = 0x0001_0000;

impl Track {
    fn of(moov: &[u8]) -> Option<Self> {
        let trak = boxes(moov)
            .filter(|(kind, _)| kind == b"trak")
            .find(|(_, trak)| {
                find(trak, &[b"mdia", b"hdlr"]).and_then(|hdlr| hdlr.get(8..12)) == Some(b"vide")
            })?
            .1;
        let tkhd = find(trak, &[b"tkhd"])?;
        let v1 = *tkhd.first()? == 1;
        let id = be32(tkhd, if v1 { 20 } else { 12 })?;
        let dims = if v1 { 88 } else { 76 };
        let pixels = u64::from(be32(tkhd, dims)? >> 16) * u64::from(be32(tkhd, dims + 4)? >> 16);
        let mdhd = find(trak, &[b"mdia", b"mdhd"])?;
        let timescale = be32(mdhd, if *mdhd.first()? == 1 { 20 } else { 12 })?;
        let trex = find(moov, &[b"mvex"]).and_then(|mvex| {
            boxes(mvex)
                .filter(|(kind, _)| kind == b"trex")
                .map(|(_, trex)| trex)
                .find(|trex| be32(trex, 4) == Some(id))
        });
        let default = |at| trex.and_then(|t| be32(t, at)).unwrap_or(0);
        (timescale > 0).then_some(Self {
            id,
            timescale: u64::from(timescale),
            pixels,
            default_duration: default(12),
            default_size: default(16),
            default_flags: default(20),
        })
    }

    fn read_moof(
        &self,
        moof: &[u8],
        decode_time: &mut u64,
        out: &mut Vec<EncodedFrame>,
    ) -> Option<()> {
        for (_, traf) in boxes(moof).filter(|(kind, _)| kind == b"traf") {
            let tfhd = find(traf, &[b"tfhd"])?;
            let flags = be32(tfhd, 0)? & 0xff_ffff;
            if be32(tfhd, 4)? != self.id {
                continue;
            }
            let mut at = 8;
            let mut field = |bit: u32, bytes: usize| {
                if flags & bit == 0 {
                    return None;
                }
                let value = be32(tfhd, at + bytes - 4);
                at += bytes;
                value
            };
            field(0x01, 8);
            field(0x02, 4);
            let default_duration = field(0x08, 4).unwrap_or(self.default_duration);
            let default_size = field(0x10, 4).unwrap_or(self.default_size);
            let default_flags = field(0x20, 4).unwrap_or(self.default_flags);
            if let Some(tfdt) = find(traf, &[b"tfdt"]) {
                *decode_time = if *tfdt.first()? == 1 {
                    u64::from(be32(tfdt, 4)?) << 32 | u64::from(be32(tfdt, 8)?)
                } else {
                    u64::from(be32(tfdt, 4)?)
                };
            }
            for (_, trun) in boxes(traf).filter(|(kind, _)| kind == b"trun") {
                let flags = be32(trun, 0)? & 0xff_ffff;
                let count = be32(trun, 4)?;
                let mut at = 8 + if flags & 0x01 != 0 { 4 } else { 0 };
                let first_flags = if flags & 0x04 != 0 {
                    at += 4;
                    Some(be32(trun, at - 4)?)
                } else {
                    None
                };
                for i in 0..count {
                    let mut next = |bit: u32| {
                        if flags & bit == 0 {
                            return Ok(None);
                        }
                        at += 4;
                        be32(trun, at - 4).map(Some).ok_or(())
                    };
                    let duration = next(0x100).ok()?.unwrap_or(default_duration);
                    let size = next(0x200).ok()?.unwrap_or(default_size);
                    let sample_flags = next(0x400)
                        .ok()?
                        .or(first_flags.filter(|_| i == 0))
                        .unwrap_or(default_flags);
                    let offset = next(0x800).ok()?.map_or(0, |o| i64::from(o as i32));
                    let pts = (*decode_time as i64 + offset).max(0) as u64;
                    out.push(EncodedFrame {
                        t: Micros(pts * 1_000_000 / self.timescale),
                        size,
                        key: sample_flags & NON_SYNC == 0,
                    });
                    *decode_time += u64::from(duration);
                }
            }
        }
        Some(())
    }
}

/// The child boxes of a box body as `(kind, body)`. Stops at the first malformed box.
fn boxes(body: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let size = be32(body, at)? as usize;
        let kind: [u8; 4] = body.get(at + 4..at + 8)?.try_into().ok()?;
        let child = body
            .get(at + 8..at.checked_add(size)?)
            .filter(|_| size >= 8)?;
        at += size;
        Some((kind, child))
    })
}

/// The body of the first box along `path`. Full boxes (`tkhd`, `mdhd`, ...) keep their version
/// and flags in front.
fn find<'a>(body: &'a [u8], path: &[&[u8; 4]]) -> Option<&'a [u8]> {
    path.iter().try_fold(body, |body, kind| {
        boxes(body)
            .find(|(k, _)| k == *kind)
            .map(|(_, child)| child)
    })
}

fn be32(body: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(body.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 60 fps frames from `sizes`, a keyframe every 30.
    fn frames(sizes: &[u32]) -> Vec<EncodedFrame> {
        sizes
            .iter()
            .enumerate()
            .map(|(i, &size)| EncodedFrame {
                t: Micros(i as u64 * 1_000_000 / 60),
                size,
                key: i % 30 == 0,
            })
            .collect()
    }

    #[test]
    fn keyframe_intervals_of_small_frames_make_still_ranges() {
        let mut sizes = vec![17_000; 60];
        sizes.extend([400; 255]);
        sizes.extend([17_000; 60]);
        let pixels = 3840 * 2160;
        let still = still_ranges(&frames(&sizes), pixels, Micros(3_000_000));
        assert_eq!(
            still,
            [TimeRange::new(Micros(1_000_000), Micros(5_000_000)).unwrap()]
        );
        assert!(still_ranges(&frames(&sizes), pixels, Micros(4_500_000)).is_empty());
    }

    #[test]
    #[ignore = "needs $SHOT_FIXTURES"]
    fn reads_every_frame_of_the_screen_fixture() {
        let path = std::path::PathBuf::from(std::env::var_os("SHOT_FIXTURES").unwrap())
            .join("screen-30s.mp4");
        let (frames, pixels) = encoded_frames(&path).unwrap();
        assert_eq!(pixels, 3840 * 2160);
        assert_eq!(frames.len(), 1800);
        assert_eq!(frames.iter().filter(|f| f.key).count(), 60);
        assert_eq!((frames[0].size, frames[1].size), (35_422, 17_110));
        assert!(still_ranges(&frames, pixels, Micros(1_000_000)).is_empty());
    }
}
