//! Fragmented MP4 writing, reading and joining, with no IO beyond the files it is handed.
//!
//! A file is an init segment (`ftyp` and a `moov` without samples) followed by fragments, each a
//! `moof` with one track run and the `mdat` holding its samples. Sample offsets are relative to
//! the `moof` (`default-base-is-moof`), so a fragment can be copied to another file as is: joining
//! files only rewrites the sequence number and the decode time of each fragment.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// One sample in a track run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub size: u32,
    /// In the track's timescale.
    pub duration: u32,
    pub sync: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTrack {
    pub width: u32,
    pub height: u32,
    pub timescale: u32,
    /// The `hvcC` decoder configuration record from the encoder.
    pub hvcc: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrack {
    pub sample_rate: u32,
    pub channels: u16,
    pub bitrate: u32,
    /// The MPEG-4 AudioSpecificConfig.
    pub config: Vec<u8>,
    /// Encoder delay in samples, skipped by the edit list.
    pub priming: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Track {
    Video(VideoTrack),
    Audio(AudioTrack),
}

impl Track {
    fn timescale(&self) -> u32 {
        match self {
            Self::Video(video) => video.timescale,
            Self::Audio(audio) => audio.sample_rate,
        }
    }
}

/// A chapter marker, written as a Nero `chpl` box that ffmpeg and most players read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    /// Start in the movie timescale.
    pub start: u64,
    pub title: String,
}

/// What the init segment declares. Track ids are the positions in `tracks`, from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Movie {
    pub timescale: u32,
    /// Presentation length in the movie timescale.
    pub duration: u64,
    pub tracks: Vec<Track>,
    pub chapters: Vec<Chapter>,
}

#[derive(Debug, thiserror::Error)]
pub enum MuxError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("{0} is not a fragmented MP4 this muxer wrote: {1}")]
    Format(String, &'static str),
    #[error("the segments use different codec settings, {0} differs from the first")]
    CodecMismatch(String),
}

/// Writes `ftyp` and `moov`.
pub fn write_init(out: &mut impl Write, movie: &Movie) -> io::Result<()> {
    let mut buf = Vec::new();
    write_box(&mut buf, b"ftyp", |b| {
        b.extend_from_slice(b"isom");
        b.extend_from_slice(&0x200u32.to_be_bytes());
        for brand in [b"isom", b"iso6", b"mp41"] {
            b.extend_from_slice(brand);
        }
    });
    write_box(&mut buf, b"moov", |b| {
        full_box(b, b"mvhd", 1, 0, |b| {
            b.extend_from_slice(&[0; 16]);
            put32(b, movie.timescale);
            put64(b, movie.duration);
            put32(b, 0x0001_0000);
            put16(b, 0x0100);
            b.extend_from_slice(&[0; 10]);
            put_matrix(b);
            b.extend_from_slice(&[0; 24]);
            put32(b, movie.tracks.len() as u32 + 1);
        });
        for (index, track) in movie.tracks.iter().enumerate() {
            write_trak(b, movie, index as u32 + 1, track);
        }
        write_box(b, b"mvex", |b| {
            full_box(b, b"mehd", 1, 0, |b| put64(b, movie.duration));
            for id in 1..=movie.tracks.len() as u32 {
                full_box(b, b"trex", 0, 0, |b| {
                    put32(b, id);
                    put32(b, 1);
                    b.extend_from_slice(&[0; 12]);
                });
            }
        });
        if !movie.chapters.is_empty() {
            write_box(b, b"udta", |b| write_chpl(b, movie));
        }
    });
    out.write_all(&buf)
}

fn write_trak(b: &mut Vec<u8>, movie: &Movie, id: u32, track: &Track) {
    write_box(b, b"trak", |b| {
        let (flags, volume, width, height) = match track {
            Track::Video(video) => (3, 0, video.width, video.height),
            Track::Audio(_) => (3, 0x0100, 0, 0),
        };
        full_box(b, b"tkhd", 1, flags, |b| {
            b.extend_from_slice(&[0; 16]);
            put32(b, id);
            put32(b, 0);
            put64(b, movie.duration);
            b.extend_from_slice(&[0; 12]);
            put16(b, volume);
            put16(b, 0);
            put_matrix(b);
            put32(b, width << 16);
            put32(b, height << 16);
        });
        if let Track::Audio(audio) = track {
            write_box(b, b"edts", |b| {
                full_box(b, b"elst", 1, 0, |b| {
                    put32(b, 1);
                    put64(b, movie.duration);
                    put64(b, u64::from(audio.priming));
                    put32(b, 0x0001_0000);
                });
            });
        }
        write_box(b, b"mdia", |b| {
            full_box(b, b"mdhd", 1, 0, |b| {
                b.extend_from_slice(&[0; 16]);
                put32(b, track.timescale());
                put64(b, 0);
                // "und", packed as three 5-bit letters.
                put16(b, 0x55c4);
                put16(b, 0);
            });
            let (handler, name): (&[u8; 4], &[u8]) = match track {
                Track::Video(_) => (b"vide", b"Video\0"),
                Track::Audio(_) => (b"soun", b"Sound\0"),
            };
            full_box(b, b"hdlr", 0, 0, |b| {
                put32(b, 0);
                b.extend_from_slice(handler);
                b.extend_from_slice(&[0; 12]);
                b.extend_from_slice(name);
            });
            write_box(b, b"minf", |b| {
                match track {
                    Track::Video(_) => full_box(b, b"vmhd", 0, 1, |b| b.extend_from_slice(&[0; 8])),
                    Track::Audio(_) => full_box(b, b"smhd", 0, 0, |b| put32(b, 0)),
                }
                write_box(b, b"dinf", |b| {
                    full_box(b, b"dref", 0, 0, |b| {
                        put32(b, 1);
                        full_box(b, b"url ", 0, 1, |_| {});
                    });
                });
                write_box(b, b"stbl", |b| {
                    full_box(b, b"stsd", 0, 0, |b| {
                        put32(b, 1);
                        match track {
                            Track::Video(video) => write_hvc1(b, video),
                            Track::Audio(audio) => write_mp4a(b, audio, id),
                        }
                    });
                    full_box(b, b"stts", 0, 0, |b| put32(b, 0));
                    full_box(b, b"stsc", 0, 0, |b| put32(b, 0));
                    full_box(b, b"stsz", 0, 0, |b| put64(b, 0));
                    full_box(b, b"stco", 0, 0, |b| put32(b, 0));
                });
            });
        });
    });
}

fn write_hvc1(b: &mut Vec<u8>, video: &VideoTrack) {
    write_box(b, b"hvc1", |b| {
        b.extend_from_slice(&[0; 6]);
        put16(b, 1);
        b.extend_from_slice(&[0; 16]);
        put16(b, video.width as u16);
        put16(b, video.height as u16);
        put32(b, 0x0048_0000);
        put32(b, 0x0048_0000);
        put32(b, 0);
        put16(b, 1);
        let mut name = [0u8; 32];
        let label = b"HEVC";
        name[0] = label.len() as u8;
        name[1..=label.len()].copy_from_slice(label);
        b.extend_from_slice(&name);
        put16(b, 0x0018);
        put16(b, 0xffff);
        write_box(b, b"hvcC", |b| b.extend_from_slice(&video.hvcc));
        // BT.709 primaries, transfer and matrix, limited range.
        write_box(b, b"colr", |b| {
            b.extend_from_slice(b"nclx");
            put16(b, 1);
            put16(b, 1);
            put16(b, 1);
            b.push(0);
        });
        write_box(b, b"pasp", |b| {
            put32(b, 1);
            put32(b, 1);
        });
    });
}

fn write_mp4a(b: &mut Vec<u8>, audio: &AudioTrack, id: u32) {
    write_box(b, b"mp4a", |b| {
        b.extend_from_slice(&[0; 6]);
        put16(b, 1);
        b.extend_from_slice(&[0; 8]);
        put16(b, audio.channels);
        put16(b, 16);
        put32(b, 0);
        put32(b, audio.sample_rate << 16);
        full_box(b, b"esds", 0, 0, |b| {
            let mut specific = Vec::new();
            descriptor(&mut specific, 0x05, &audio.config);
            let mut config = vec![0x40, 0x15];
            config.extend_from_slice(&[0, 0x18, 0]);
            put32(&mut config, audio.bitrate);
            put32(&mut config, audio.bitrate);
            config.extend_from_slice(&specific);
            let mut es = Vec::new();
            put16(&mut es, id as u16);
            es.push(0);
            descriptor(&mut es, 0x04, &config);
            descriptor(&mut es, 0x06, &[0x02]);
            descriptor(b, 0x03, &es);
        });
    });
}

fn write_chpl(b: &mut Vec<u8>, movie: &Movie) {
    full_box(b, b"chpl", 1, 0, |b| {
        put32(b, 0);
        let chapters = &movie.chapters[..movie.chapters.len().min(255)];
        b.push(chapters.len() as u8);
        for chapter in chapters {
            // chpl counts in 100 ns units.
            let start = u128::from(chapter.start) * 10_000_000 / u128::from(movie.timescale);
            put64(b, start as u64);
            let mut end = chapter.title.len().min(255);
            while !chapter.title.is_char_boundary(end) {
                end -= 1;
            }
            b.push(end as u8);
            b.extend_from_slice(&chapter.title.as_bytes()[..end]);
        }
    });
}

/// Writes one fragment: a `moof` with one track run, then an `mdat` with `data`.
/// `data` holds the samples back to back, in order.
pub fn write_fragment(
    out: &mut impl Write,
    sequence: u32,
    track_id: u32,
    base_time: u64,
    samples: &[Sample],
    data: &[u8],
) -> io::Result<()> {
    debug_assert_eq!(
        samples.iter().map(|s| s.size as usize).sum::<usize>(),
        data.len()
    );
    let mut moof = Vec::new();
    let mut data_offset_at = 0;
    write_box(&mut moof, b"moof", |b| {
        full_box(b, b"mfhd", 0, 0, |b| put32(b, sequence));
        write_box(b, b"traf", |b| {
            full_box(b, b"tfhd", 0, 0x02_0000, |b| put32(b, track_id));
            full_box(b, b"tfdt", 1, 0, |b| put64(b, base_time));
            // data offset, sample duration, size and flags present.
            full_box(b, b"trun", 0, 0x0701, |b| {
                put32(b, samples.len() as u32);
                data_offset_at = b.len();
                put32(b, 0);
                for sample in samples {
                    put32(b, sample.duration);
                    put32(b, sample.size);
                    put32(
                        b,
                        if sample.sync {
                            0x0200_0000
                        } else {
                            0x0101_0000
                        },
                    );
                }
            });
        });
    });
    let data_offset = moof.len() as u32 + 8;
    moof[data_offset_at..data_offset_at + 4].copy_from_slice(&data_offset.to_be_bytes());
    out.write_all(&moof)?;
    out.write_all(&(data.len() as u32 + 8).to_be_bytes())?;
    out.write_all(b"mdat")?;
    out.write_all(data)
}

/// A fragment inside a file this muxer wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentSpan {
    /// Byte offset of the `moof`.
    pub offset: u64,
    pub moof_size: u32,
    pub mdat_size: u32,
    pub base_time: u64,
    /// Sum of the sample durations.
    pub duration: u64,
}

/// A single-track fragmented file as the joiner needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentedFile {
    pub track: Track,
    pub fragments: Vec<FragmentSpan>,
}

impl FragmentedFile {
    pub fn duration(&self) -> u64 {
        self.fragments.iter().map(|f| f.duration).sum()
    }

    /// Reads the init segment and the fragment table of a file written by
    /// [`write_init`] and [`write_fragment`] with one video track.
    pub fn read(path: &Path) -> Result<Self, MuxError> {
        let bad = |why| MuxError::Format(path.display().to_string(), why);
        let mut file = BufReader::new(File::open(path)?);
        let length = file.get_ref().metadata()?.len();
        let mut track = None;
        let mut fragments = Vec::new();
        let mut offset = 0;
        while offset < length {
            let (kind, size) = read_header(&mut file).map_err(|_| bad("truncated box"))?;
            if size < 8 || offset + size > length {
                return Err(bad("box size out of range"));
            }
            match &kind {
                b"moov" | b"moof" => {
                    let mut body = vec![0; size as usize - 8];
                    file.read_exact(&mut body)?;
                    if &kind == b"moov" {
                        track = Some(parse_video_track(&body).ok_or_else(|| bad("no hvc1 track"))?);
                    } else {
                        let (mdat_kind, mdat_size) =
                            read_header(&mut file).map_err(|_| bad("moof without mdat"))?;
                        if &mdat_kind != b"mdat" || offset + size + mdat_size > length {
                            return Err(bad("moof without mdat"));
                        }
                        let (base_time, duration) =
                            parse_moof(&body).ok_or_else(|| bad("unreadable moof"))?;
                        fragments.push(FragmentSpan {
                            offset,
                            moof_size: size as u32,
                            mdat_size: mdat_size as u32,
                            base_time,
                            duration,
                        });
                        file.seek_relative(mdat_size as i64 - 8)?;
                        offset += mdat_size;
                    }
                }
                _ => file.seek_relative(size as i64 - 8)?,
            }
            offset += size;
        }
        Ok(Self {
            track: track.ok_or_else(|| bad("no moov"))?,
            fragments,
        })
    }
}

/// Copies fragments from fragmented files into one output, renumbering them and shifting their
/// decode times, so the output has one init segment and continuous timestamps.
pub struct Joiner<W: Write> {
    out: W,
    sequence: u32,
}

impl<W: Write> Joiner<W> {
    /// Writes the init segment. Fragments follow with [`Self::copy`] and [`Self::write`].
    pub fn new(mut out: W, movie: &Movie) -> io::Result<Self> {
        write_init(&mut out, movie)?;
        Ok(Self { out, sequence: 1 })
    }

    /// Copies one fragment of `source` as track `track_id`, starting at `base_time`.
    pub fn copy(
        &mut self,
        source: &mut File,
        fragment: &FragmentSpan,
        track_id: u32,
        base_time: u64,
    ) -> Result<(), MuxError> {
        let mut moof = vec![0; fragment.moof_size as usize];
        source.seek(SeekFrom::Start(fragment.offset))?;
        source.read_exact(&mut moof)?;
        patch_moof(&mut moof, self.sequence, track_id, base_time)
            .ok_or_else(|| MuxError::Format("segment fragment".into(), "unexpected moof layout"))?;
        self.out.write_all(&moof)?;
        let copied = io::copy(
            &mut source.take(u64::from(fragment.mdat_size)),
            &mut self.out,
        )?;
        if copied != u64::from(fragment.mdat_size) {
            return Err(MuxError::Format(
                "segment fragment".into(),
                "truncated mdat",
            ));
        }
        self.sequence += 1;
        Ok(())
    }

    /// Writes a new fragment.
    pub fn write(
        &mut self,
        track_id: u32,
        base_time: u64,
        samples: &[Sample],
        data: &[u8],
    ) -> io::Result<()> {
        write_fragment(
            &mut self.out,
            self.sequence,
            track_id,
            base_time,
            samples,
            data,
        )?;
        self.sequence += 1;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.out.flush()?;
        Ok(self.out)
    }
}

fn write_box(out: &mut Vec<u8>, kind: &[u8; 4], body: impl FnOnce(&mut Vec<u8>)) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(kind);
    body(out);
    let size = (out.len() - start) as u32;
    out[start..start + 4].copy_from_slice(&size.to_be_bytes());
}

fn full_box(
    out: &mut Vec<u8>,
    kind: &[u8; 4],
    version: u8,
    flags: u32,
    body: impl FnOnce(&mut Vec<u8>),
) {
    write_box(out, kind, |b| {
        put32(b, (u32::from(version) << 24) | flags);
        body(b);
    });
}

/// An MPEG-4 descriptor with a one-byte length. Every descriptor here is under 128 bytes.
fn descriptor(out: &mut Vec<u8>, tag: u8, body: &[u8]) {
    debug_assert!(body.len() < 128);
    out.push(tag);
    out.push(body.len() as u8);
    out.extend_from_slice(body);
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_matrix(out: &mut Vec<u8>) {
    for value in [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000u32] {
        put32(out, value);
    }
}

fn read_header(read: &mut impl Read) -> io::Result<([u8; 4], u64)> {
    let mut header = [0; 8];
    read.read_exact(&mut header)?;
    let size = u32::from_be_bytes(header[..4].try_into().expect("4 bytes"));
    Ok((header[4..].try_into().expect("4 bytes"), u64::from(size)))
}

/// Child boxes of a box body as (kind, body start, body end) offsets into `data`.
fn children(data: &[u8], start: usize, end: usize) -> Option<Vec<([u8; 4], usize, usize)>> {
    let mut boxes = Vec::new();
    let mut at = start;
    while at < end {
        let size = u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize;
        if size < 8 || at + size > end {
            return None;
        }
        boxes.push((data[at + 4..at + 8].try_into().ok()?, at + 8, at + size));
        at += size;
    }
    Some(boxes)
}

fn child(data: &[u8], start: usize, end: usize, kind: &[u8; 4]) -> Option<(usize, usize)> {
    children(data, start, end)?
        .into_iter()
        .find(|(k, _, _)| k == kind)
        .map(|(_, s, e)| (s, e))
}

fn path(data: &[u8], kinds: &[&[u8; 4]]) -> Option<(usize, usize)> {
    kinds
        .iter()
        .try_fold((0, data.len()), |(start, end), kind| {
            child(data, start, end, kind)
        })
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn be64(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// The video track of a `moov` body.
fn parse_video_track(moov: &[u8]) -> Option<Track> {
    let (mdhd, _) = path(moov, &[b"trak", b"mdia", b"mdhd"])?;
    // Version 1: four bytes of version and flags, two 64-bit times, then the timescale.
    let timescale = be32(moov, mdhd + 4 + 16)?;
    let (stsd, stsd_end) = path(moov, &[b"trak", b"mdia", b"minf", b"stbl", b"stsd"])?;
    let (hvc1, hvc1_end) = child(moov, stsd + 8, stsd_end, b"hvc1")?;
    let width = u32::from(u16::from_be_bytes(
        moov.get(hvc1 + 24..hvc1 + 26)?.try_into().ok()?,
    ));
    let height = u32::from(u16::from_be_bytes(
        moov.get(hvc1 + 26..hvc1 + 28)?.try_into().ok()?,
    ));
    // The visual sample entry has 78 bytes of fields before its child boxes.
    let (hvcc, hvcc_end) = child(moov, hvc1 + 78, hvc1_end, b"hvcC")?;
    Some(Track::Video(VideoTrack {
        width,
        height,
        timescale,
        hvcc: moov[hvcc..hvcc_end].to_vec(),
    }))
}

/// Decode time and total sample duration of a `moof` body with one track run.
fn parse_moof(moof: &[u8]) -> Option<(u64, u64)> {
    let (tfdt, _) = path(moof, &[b"traf", b"tfdt"])?;
    let base_time = be64(moof, tfdt + 4)?;
    let (trun, _) = path(moof, &[b"traf", b"trun"])?;
    if be32(moof, trun)? & 0xff_ffff != 0x0701 {
        return None;
    }
    let count = be32(moof, trun + 4)? as usize;
    let duration = (0..count)
        .map(|i| be32(moof, trun + 12 + i * 12).map(u64::from))
        .sum::<Option<u64>>()?;
    Some((base_time, duration))
}

/// Rewrites the sequence number, track id and decode time of a whole `moof` box in place.
fn patch_moof(moof: &mut [u8], sequence: u32, track_id: u32, base_time: u64) -> Option<()> {
    let (body, end) = (8, moof.len());
    let (mfhd, _) = child(moof, body, end, b"mfhd")?;
    let (traf, traf_end) = child(moof, body, end, b"traf")?;
    let (tfhd, _) = child(moof, traf, traf_end, b"tfhd")?;
    let (tfdt, _) = child(moof, traf, traf_end, b"tfdt")?;
    if moof[tfdt] != 1 {
        return None;
    }
    moof.get_mut(mfhd + 4..mfhd + 8)?
        .copy_from_slice(&sequence.to_be_bytes());
    moof.get_mut(tfhd + 4..tfhd + 8)?
        .copy_from_slice(&track_id.to_be_bytes());
    moof.get_mut(tfdt + 4..tfdt + 12)?
        .copy_from_slice(&base_time.to_be_bytes());
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video() -> VideoTrack {
        VideoTrack {
            width: 3840,
            height: 2160,
            timescale: 60_000,
            hvcc: vec![1, 2, 3, 4, 5],
        }
    }

    fn segment(path: &Path, fragments: &[(u64, &[u8])]) {
        let mut out = File::create(path).unwrap();
        let movie = Movie {
            timescale: 60_000,
            duration: 0,
            tracks: vec![Track::Video(video())],
            chapters: Vec::new(),
        };
        write_init(&mut out, &movie).unwrap();
        for (sequence, (base, data)) in fragments.iter().enumerate() {
            let samples: Vec<Sample> = data
                .iter()
                .enumerate()
                .map(|(i, _)| Sample {
                    size: 1,
                    duration: 1000,
                    sync: i == 0,
                })
                .collect();
            write_fragment(&mut out, sequence as u32 + 1, 1, *base, &samples, data).unwrap();
        }
    }

    #[test]
    fn a_written_segment_reads_back_with_its_track_and_fragment_times() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.mp4");
        segment(&path, &[(0, b"abc"), (3000, b"de")]);
        let file = FragmentedFile::read(&path).unwrap();
        assert_eq!(file.track, Track::Video(video()));
        let times: Vec<_> = file
            .fragments
            .iter()
            .map(|f| (f.base_time, f.duration))
            .collect();
        assert_eq!(times, [(0, 3000), (3000, 2000)]);
        assert_eq!(file.duration(), 5000);
    }

    #[test]
    fn joining_renumbers_fragments_and_shifts_decode_times() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"));
        segment(&a, &[(0, b"abc")]);
        segment(&b, &[(0, b"de"), (2000, b"f")]);
        let movie = Movie {
            timescale: 60_000,
            duration: 6000,
            tracks: vec![Track::Video(video())],
            chapters: vec![Chapter {
                start: 3000,
                title: "Two".into(),
            }],
        };
        let joined = dir.path().join("joined.mp4");
        let mut joiner = Joiner::new(File::create(&joined).unwrap(), &movie).unwrap();
        let mut base = 0;
        for path in [&a, &b] {
            let file = FragmentedFile::read(path).unwrap();
            let mut source = File::open(path).unwrap();
            for fragment in &file.fragments {
                joiner
                    .copy(&mut source, fragment, 1, base + fragment.base_time)
                    .unwrap();
            }
            base += file.duration();
        }
        joiner.finish().unwrap();

        let file = FragmentedFile::read(&joined).unwrap();
        let times: Vec<_> = file
            .fragments
            .iter()
            .map(|f| (f.base_time, f.duration))
            .collect();
        assert_eq!(times, [(0, 3000), (3000, 2000), (5000, 1000)]);
        let bytes = std::fs::read(&joined).unwrap();
        let sequences: Vec<u32> = file
            .fragments
            .iter()
            .map(|f| be32(&bytes, f.offset as usize + 20).unwrap())
            .collect();
        assert_eq!(sequences, [1, 2, 3]);
        let mdat: Vec<u8> = file
            .fragments
            .iter()
            .flat_map(|f| {
                let start = (f.offset + u64::from(f.moof_size) + 8) as usize;
                bytes[start..start + f.mdat_size as usize - 8].to_vec()
            })
            .collect();
        assert_eq!(mdat, b"abcdef");
    }
}
