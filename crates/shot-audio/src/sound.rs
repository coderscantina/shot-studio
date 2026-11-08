//! Sounds: decoded samples, the bundled CC0 set and the lookup across the video's `sounds/`,
//! the channel's `brand/sounds/` and the bundled set, in that order ([0021]).
//!
//! [0021]: ../../../docs/decisions/0021-sound-transitions-and-bookends.md

use std::fmt;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shot_core::Id;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::SAMPLE_RATE;

/// Mono 48 kHz samples of a sound, shared, with the sample it peaks at.
#[derive(Clone, PartialEq)]
pub struct Sound {
    samples: Arc<[f32]>,
    peak: usize,
}

/// The peak is the loudest stretch of this many samples, 10 ms.
const PEAK_WINDOW: usize = SAMPLE_RATE as usize / 100;

impl Sound {
    pub fn new(samples: Vec<f32>) -> Self {
        let mut sum = 0.0f64;
        let mut best = (0.0f64, 0);
        for (i, s) in samples.iter().enumerate() {
            sum += f64::from(s * s);
            if i >= PEAK_WINDOW {
                sum -= f64::from(samples[i - PEAK_WINDOW].powi(2));
            }
            if sum > best.0 {
                best = (sum, i);
            }
        }
        let window = best.1.saturating_sub(PEAK_WINDOW - 1)..(best.1 + 1).min(samples.len());
        let peak = window
            .clone()
            .zip(&samples[window])
            .fold(
                (0, 0.0f32),
                |a, (i, s)| if s.abs() > a.1 { (i, s.abs()) } else { a },
            )
            .0;
        Self {
            samples: samples.into(),
            peak,
        }
    }

    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The loudest sample of the loudest 10 ms, where a hit or a whoosh lands.
    pub fn peak(&self) -> usize {
        self.peak
    }
}

impl fmt::Debug for Sound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sound")
            .field("len", &self.samples.len())
            .field("peak", &self.peak)
            .finish()
    }
}

/// What a bundled sound is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sfx,
    Music,
}

/// A sound bundled with the app: CC0, 48 kHz mono FLAC. Sources are in `sources/manifest.json`.
pub struct Bundled {
    pub name: &'static str,
    pub kind: Kind,
    flac: &'static [u8],
}

impl Bundled {
    /// Decodes the sound, bit-exact: FLAC is lossless.
    pub fn decode(&self) -> Sound {
        let source = MediaSourceStream::new(Box::new(Cursor::new(self.flac)), Default::default());
        Sound::new(decode_mono(source, "flac").expect("bundled sounds are valid FLAC"))
    }
}

impl fmt::Debug for Bundled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bundled")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .finish()
    }
}

macro_rules! bundled {
    ($($name:literal $kind:ident),* $(,)?) => {
        /// The bundled set, sorted by name.
        pub const BUNDLED: &[Bundled] = &[$(Bundled {
            name: $name,
            kind: Kind::$kind,
            flac: include_bytes!(concat!("../assets/sounds/", $name, ".flac")),
        }),*];
    };
}

bundled! {
    "bed-calm" Music,
    "bed-lofi" Music,
    "bed-pulse" Music,
    "click" Sfx,
    "glitch" Sfx,
    "impact" Sfx,
    "pluck" Sfx,
    "pop" Sfx,
    "riser" Sfx,
    "swish" Sfx,
    "swoosh" Sfx,
    "thud" Sfx,
    "tick" Sfx,
    "whoosh" Sfx,
}

/// A bundled sound by name.
pub fn bundled(name: &str) -> Option<&'static Bundled> {
    BUNDLED
        .binary_search_by(|b| b.name.cmp(name))
        .ok()
        .map(|i| &BUNDLED[i])
}

/// File extensions a sound in a folder may have, tried in this order.
pub const EXTENSIONS: [&str; 7] = ["wav", "flac", "aif", "aiff", "caf", "m4a", "mp3"];

/// Where a sound name resolved to.
#[derive(Debug, Clone)]
pub enum Source {
    /// A file in the video's or the channel's `sounds/`, decoded by the caller.
    File(PathBuf),
    Bundled(&'static Bundled),
}

/// Looks sounds up by name in the video's `sounds/`, then `brand/sounds/`, then the bundled set.
#[derive(Debug, Clone)]
pub struct Library {
    dirs: Vec<PathBuf>,
}

impl Library {
    /// `brand` is the channel's `brand/` folder, if the video has a channel.
    pub fn new(video: &Path, brand: Option<&Path>) -> Self {
        Self {
            dirs: std::iter::once(video)
                .chain(brand)
                .map(|dir| dir.join("sounds"))
                .collect(),
        }
    }

    /// The first `<name>.<extension>` in the folders, with [`EXTENSIONS`] in order, else the
    /// bundled sound of that name.
    pub fn find(&self, name: &Id) -> Option<Source> {
        self.dirs
            .iter()
            .flat_map(|dir| {
                EXTENSIONS
                    .iter()
                    .map(move |ext| dir.join(format!("{name}.{ext}")))
            })
            .find(|path| path.is_file())
            .map(Source::File)
            .or_else(|| bundled(name.as_str()).map(Source::Bundled))
    }
}

/// Mono samples of a stream symphonia reads, averaged across channels, at the stream's rate.
pub(crate) fn decode_mono(source: MediaSourceStream, extension: &str) -> Result<Vec<f32>, Error> {
    let mut hint = Hint::new();
    hint.with_extension(extension);
    let mut format = symphonia::default::get_probe().probe(
        &hint,
        source,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or(Error::Unsupported("no audio track"))?;
    let id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or(Error::Unsupported("no audio parameters"))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())?;
    let mut interleaved = Vec::new();
    let mut out = Vec::new();
    while let Some(packet) = format.next_packet()? {
        if packet.track_id != id {
            continue;
        }
        let buffer = decoder.decode(&packet)?;
        let channels = buffer.spec().channels().count().max(1);
        buffer.copy_to_vec_interleaved(&mut interleaved);
        out.extend(
            interleaved
                .chunks_exact(channels)
                .map(|frame: &[f32]| frame.iter().sum::<f32>() / channels as f32),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_sound_decodes_to_48_khz_and_is_sorted() {
        assert!(BUNDLED.windows(2).all(|w| w[0].name < w[1].name));
        for b in BUNDLED {
            let sound = b.decode();
            assert!(!sound.is_empty(), "{} is empty", b.name);
            let peak = sound.samples().iter().fold(0.0f32, |a, s| a.max(s.abs()));
            assert!(peak > 0.1 && peak < 1.0, "{} peaks at {peak}", b.name);
            assert_eq!(b.decode(), sound, "{} decodes the same twice", b.name);
        }
    }

    #[test]
    fn lookup_prefers_the_video_then_the_brand_then_the_bundled_set() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("video");
        let brand = dir.path().join("brand");
        for d in [&video, &brand] {
            std::fs::create_dir_all(d.join("sounds")).unwrap();
        }
        std::fs::write(brand.join("sounds/pop.mp3"), b"").unwrap();
        std::fs::write(brand.join("sounds/whoosh.wav"), b"").unwrap();
        std::fs::write(video.join("sounds/whoosh.m4a"), b"").unwrap();
        let library = Library::new(&video, Some(&brand));
        let find = |name: &str| library.find(&Id::new(name).unwrap());

        assert!(
            matches!(find("whoosh"), Some(Source::File(p)) if p == video.join("sounds/whoosh.m4a"))
        );
        assert!(matches!(find("pop"), Some(Source::File(p)) if p == brand.join("sounds/pop.mp3")));
        assert!(matches!(find("tick"), Some(Source::Bundled(b)) if b.name == "tick"));
        assert!(find("missing").is_none());
    }
}
