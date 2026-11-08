//! What is on disk: the bundled sounds and beds, the channel's and the video's own files and
//! the downloads with their credit, and where a download goes ([0026]).
//!
//! | What | In a channel | In a video outside one |
//! | --- | --- | --- |
//! | a sound | `brand/sounds/` | the video's `sounds/` |
//! | an image | `library/` | the video's `sources/` |
//!
//! Each download gets `<name>.credit.kdl` beside it ([`shot_core::Credit`]).
//!
//! [0026]: ../../../docs/decisions/0026-library-search-on-openverse.md

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use shot_audio::sound::{BUNDLED, EXTENSIONS, Kind, Source};
use shot_core::{Applied, Credit, Cut, Effect, Id, Lock, OverrideEffect, ParamValue};

use crate::openverse::{Hit, Media};

/// The extension of a credit file, which replaces the asset's own.
const CREDIT: &str = "credit.kdl";
/// Still images a pop or a b-roll draws.
pub const IMAGE_EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Sounds,
    Music,
    Images,
}

/// Where an asset lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    /// The video's own folder.
    Video,
    /// The channel repo: `brand/sounds/` or `library/`.
    Channel,
    /// Built into the app.
    Bundled,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Self::Video => "This video",
            Self::Channel => "Channel",
            Self::Bundled => "Bundled",
        }
    }
}

/// One sound, music file or image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub section: Section,
    /// The name a sound is placed by, the file stem otherwise.
    pub name: String,
    pub origin: Origin,
    /// `None` for a bundled sound or bed.
    pub path: Option<PathBuf>,
    /// Set for a download.
    pub credit: Option<Credit>,
}

impl Asset {
    /// The words local search matches: the name, the title and the tags.
    pub fn matches(&self, words: &[String]) -> bool {
        let credit = self.credit.as_ref();
        let haystack = [
            Some(self.name.to_lowercase()),
            credit.map(|c| c.title.to_lowercase()),
            credit.map(|c| c.tags.join(" ").to_lowercase()),
            credit
                .and_then(|c| c.creator.as_ref())
                .map(|c| c.name.to_lowercase()),
        ];
        words.iter().all(|word| {
            haystack
                .iter()
                .flatten()
                .any(|text| text.contains(word.as_str()))
        })
    }
}

/// A download written to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub asset: Asset,
    /// The file relative to the video folder, as `cut.kdl` names it, when there is a video.
    pub relative: Option<String>,
}

/// The folders of a channel, a video, or a video in a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folders {
    /// The channel repo.
    pub channel: Option<PathBuf>,
    /// The video folder.
    pub video: Option<PathBuf>,
}

impl Folders {
    /// The folders of the video at `dir`, with its channel when it sits at
    /// `<repo>/videos/<slug>`.
    pub fn of_video(dir: &Path) -> Self {
        let channel = dir
            .parent()
            .filter(|p| p.file_name().is_some_and(|n| n == "videos"))
            .and_then(Path::parent)
            .filter(|repo| repo.join(".git").exists() || repo.join("brand").is_dir())
            .map(Path::to_owned);
        Self {
            channel,
            video: Some(dir.to_owned()),
        }
    }

    fn brand(&self) -> Option<PathBuf> {
        self.channel.as_ref().map(|c| c.join("brand"))
    }

    /// Where a downloaded asset of `media` goes.
    pub fn download_dir(&self, media: Media) -> Option<PathBuf> {
        match (media, &self.channel, &self.video) {
            (Media::Audio, Some(channel), _) => Some(channel.join("brand/sounds")),
            (Media::Image, Some(channel), _) => Some(channel.join("library")),
            (Media::Audio, None, Some(video)) => Some(video.join("sounds")),
            (Media::Image, None, Some(video)) => Some(video.join("sources")),
            (_, None, None) => None,
        }
    }

    /// The sounds placed by name, in lookup order: the video's `sounds/`, the channel's
    /// `brand/sounds/`, then the bundled effects. A name found earlier hides the same name
    /// later, as the lookup does.
    pub fn sounds(&self) -> Vec<Asset> {
        let mut found = Vec::new();
        let dirs = [
            (self.video.as_ref().map(|v| v.join("sounds")), Origin::Video),
            (self.brand().map(|b| b.join("sounds")), Origin::Channel),
        ];
        for (dir, origin) in dirs {
            for (path, stem) in files(dir.as_deref(), &EXTENSIONS) {
                if Id::new(&stem).is_some() && !found.iter().any(|a: &Asset| a.name == stem) {
                    found.push(file_asset(Section::Sounds, stem, origin, path));
                }
            }
        }
        for bundled in BUNDLED.iter().filter(|b| b.kind == Kind::Sfx) {
            if !found.iter().any(|a| a.name == bundled.name) {
                found.push(bundled_asset(Section::Sounds, bundled.name));
            }
        }
        found
    }

    /// The music a video can put under its cut: audio in its `sources/`, then the bundled beds.
    pub fn music(&self) -> Vec<Asset> {
        let dir = self.video.as_ref().map(|v| v.join("sources"));
        let mut found: Vec<Asset> = files(dir.as_deref(), &EXTENSIONS)
            .into_iter()
            .map(|(path, stem)| file_asset(Section::Music, stem, Origin::Video, path))
            .collect();
        found.extend(
            BUNDLED
                .iter()
                .filter(|b| b.kind == Kind::Music)
                .map(|b| bundled_asset(Section::Music, b.name)),
        );
        found
    }

    /// Stills for pops and b-roll: the video's `sources/`, then the channel's `library/`.
    pub fn images(&self) -> Vec<Asset> {
        let dirs = [
            (
                self.video.as_ref().map(|v| v.join("sources")),
                Origin::Video,
            ),
            (
                self.channel.as_ref().map(|c| c.join("library")),
                Origin::Channel,
            ),
        ];
        dirs.into_iter()
            .flat_map(|(dir, origin)| {
                files(dir.as_deref(), &IMAGE_EXTENSIONS)
                    .into_iter()
                    .map(move |(path, stem)| file_asset(Section::Images, stem, origin, path))
            })
            .collect()
    }

    /// `path` as `cut.kdl` names it from the video folder: `sources/cat.jpg`, or
    /// `../../library/cat.jpg` for the channel's.
    pub fn in_video(&self, path: &Path) -> Option<String> {
        relative(self.video.as_ref()?, path)
    }

    /// `path` as a person finds it: from the channel repo, else from the video folder.
    pub fn shown(&self, path: &Path) -> String {
        let from = self.channel.as_ref().or(self.video.as_ref());
        from.and_then(|dir| path.strip_prefix(dir).ok())
            .map_or_else(|| path.display().to_string(), |p| p.display().to_string())
    }

    /// The asset downloaded before from the Openverse result `id`, if it is still there.
    pub fn downloaded(&self, media: Media, id: &str) -> Option<Asset> {
        let assets = match media {
            Media::Audio => self.sounds(),
            Media::Image => self.images(),
        };
        assets.into_iter().find(|a| {
            a.credit
                .as_ref()
                .is_some_and(|c| c.openverse.as_deref() == Some(id))
        })
    }

    /// Writes `bytes`, the file of `hit`, to its download folder under a free name from its
    /// title, with its credit beside it. Refuses bytes that are not the hit's kind of file.
    pub fn save(&self, hit: &Hit, bytes: &[u8]) -> Result<Saved, String> {
        if !sniff(hit.media, bytes) {
            return Err(match hit.media {
                Media::Audio => "The download is not an MP3, WAV or FLAC file".into(),
                Media::Image => "The download is not a PNG or JPEG image".into(),
            });
        }
        let dir = self
            .download_dir(hit.media)
            .ok_or("There is no channel or video to download into")?;
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let base = slug(&hit.title, hit.media);
        let name = (1..)
            .map(|n| match n {
                1 => base.clone(),
                n => format!("{base}-{n}"),
            })
            .find(|name| !taken(&dir, name))
            .expect("some number is free");
        let path = dir.join(format!("{name}.{}", hit.extension));
        let credit = hit.credit();
        write_atomic(&path.with_extension(CREDIT), credit.to_kdl().as_bytes())?;
        write_atomic(&path, bytes)?;
        let relative = self.in_video(&path);
        let section = match hit.media {
            Media::Audio => Section::Sounds,
            Media::Image => Section::Images,
        };
        let origin = if self.channel.is_some() {
            Origin::Channel
        } else {
            Origin::Video
        };
        Ok(Saved {
            asset: Asset {
                section,
                name,
                origin,
                path: Some(path),
                credit: Some(credit),
            },
            relative,
        })
    }

    /// The credits of every downloaded asset the cut of the video uses, each once, in the order
    /// first used: placed and automatic sounds, the music bed and cues, and the files of
    /// `b-roll` and `pop` overlays. `lock` is the video's `cut.lock`, when it has one.
    pub fn credits(&self, cut: &Cut, lock: Option<&Lock>) -> Vec<Credit> {
        let Some(video) = &self.video else {
            return Vec::new();
        };
        let mut sounds: Vec<Id> = Vec::new();
        let mut files: Vec<String> = cut.music.iter().map(|m| m.file.clone()).collect();
        let overlays = cut
            .chapters
            .iter()
            .flat_map(|c| &c.takes)
            .flat_map(|t| &t.overlays)
            .map(|o| (&o.template, &o.params));
        let instances = lock
            .into_iter()
            .flat_map(|l| &l.takes)
            .flat_map(|t| &t.instances);
        let mut params = Vec::new();
        for (template, list) in overlays {
            params.push((template, list));
        }
        for instance in instances {
            match &instance.applied {
                Applied::Sfx { sound, .. } => sounds.push(sound.clone()),
                Applied::Music { file, .. } => files.push(file.clone()),
                Applied::Overlay {
                    template,
                    params: p,
                    ..
                } => params.push((template, p)),
                _ => {}
            }
        }
        sounds.extend(
            cut.overrides
                .iter()
                .flat_map(|o| &o.effects)
                .filter_map(|e| match e {
                    OverrideEffect::Set(Effect::Sfx { sound, .. }) => Some(sound.clone()),
                    _ => None,
                }),
        );
        if let Some(lock) = lock {
            sounds.extend(lock.brand.sounds.iter().map(|(_, sound)| sound.clone()));
        }
        for (template, list) in params {
            let name = match template.as_str() {
                "b-roll" => "file",
                "pop" => "image",
                _ => continue,
            };
            files.extend(list.iter().find_map(|(n, v)| match v {
                ParamValue::Text(file) if n.as_str() == name => Some(file.clone()),
                _ => None,
            }));
        }
        let lookup = shot_audio::sound::Library::new(video, self.brand().as_deref());
        let mut paths: Vec<PathBuf> = sounds
            .iter()
            .filter_map(|name| match lookup.find(name)? {
                Source::File(path) => Some(path),
                Source::Bundled(_) => None,
            })
            .collect();
        paths.extend(files.iter().map(|f| video.join(f)));
        let mut seen = BTreeSet::new();
        paths
            .into_iter()
            .filter_map(|path| read_credit(&path))
            .filter(|c| seen.insert(c.source.clone()))
            .collect()
    }
}

/// The credits as a block for the video description, empty without credits.
pub fn description(credits: &[Credit]) -> String {
    if credits.is_empty() {
        return String::new();
    }
    let mut text = String::from("Credits\n");
    for credit in credits {
        text.push_str(&credit.line());
        text.push('\n');
    }
    text
}

/// The credit beside `asset`, if it has one that reads.
pub fn read_credit(asset: &Path) -> Option<Credit> {
    let text = fs::read_to_string(asset.with_extension(CREDIT)).ok()?;
    Credit::parse(&text).ok()
}

fn file_asset(section: Section, name: String, origin: Origin, path: PathBuf) -> Asset {
    Asset {
        section,
        credit: read_credit(&path),
        name,
        origin,
        path: Some(path),
    }
}

fn bundled_asset(section: Section, name: &str) -> Asset {
    Asset {
        section,
        name: name.to_owned(),
        origin: Origin::Bundled,
        path: None,
        credit: None,
    }
}

/// The files in `dir` with one of `extensions`, ignoring case, sorted by name, with their stem.
fn files(dir: Option<&Path>, extensions: &[&str]) -> Vec<(PathBuf, String)> {
    let Some(Ok(entries)) = dir.map(fs::read_dir) else {
        return Vec::new();
    };
    let mut found: Vec<(PathBuf, String)> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            let ext = path.extension()?.to_str()?.to_ascii_lowercase();
            let stem = path.file_stem()?.to_str()?.to_owned();
            extensions.contains(&ext.as_str()).then_some((path, stem))
        })
        .collect();
    found.sort();
    found.dedup_by(|a, b| a.1 == b.1);
    found
}

/// Whether a file or credit of stem `name` is in `dir`.
fn taken(dir: &Path, name: &str) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|e| {
        e.file_name()
            .to_str()
            .and_then(|f| f.split_once('.'))
            .is_some_and(|(stem, _)| stem == name)
    })
}

/// A name from a title that a sound can be placed by: lowercase letters, digits and dashes,
/// starting with a letter, at most 40 characters.
pub fn slug(title: &str, media: Media) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    let fallback = match media {
        Media::Audio => "sound",
        Media::Image => "image",
    };
    match slug.chars().next() {
        None => fallback.to_owned(),
        Some(c) if c.is_ascii_digit() => format!("{fallback}-{slug}"),
        Some(_) => slug.to_owned(),
    }
}

/// Whether `bytes` start like a file of the kind the app plays or draws.
fn sniff(media: Media, bytes: &[u8]) -> bool {
    match media {
        Media::Image => bytes.starts_with(b"\x89PNG") || bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        Media::Audio => {
            bytes.starts_with(b"ID3")
                || bytes.starts_with(b"fLaC")
                || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE"))
                || (bytes.len() > 1 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0)
        }
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let part = path.with_extension("part");
    fs::write(&part, bytes)
        .and_then(|()| fs::rename(&part, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `path` relative to the video folder `video`, with `/` between folders and `..` up to the
/// channel, as `cut.kdl` names files.
fn relative(video: &Path, path: &Path) -> Option<String> {
    let video = video.canonicalize().ok()?;
    let path = path.canonicalize().ok()?;
    let common = video
        .components()
        .zip(path.components())
        .take_while(|(a, b)| a == b)
        .count();
    let ups = video.components().count() - common;
    let parts: Option<Vec<&str>> = path
        .components()
        .skip(common)
        .map(|c| c.as_os_str().to_str())
        .collect();
    let mut out: Vec<&str> = vec![".."; ups];
    out.extend(parts?);
    Some(out.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shot_core::License;

    fn hit(media: Media, title: &str, extension: &'static str) -> Hit {
        Hit {
            id: format!("id-{title}"),
            media,
            title: title.into(),
            creator: None,
            license: License::By,
            version: Some("2.0".into()),
            license_url: None,
            source: format!("https://example.org/{title}"),
            provider: Some("flickr".into()),
            file: "https://example.org/file".into(),
            extension,
            size: None,
            duration: None,
            width: None,
            height: None,
            thumbnail: None,
            tags: vec!["meme".into()],
            attribution: format!("{title} by someone, CC BY 2.0"),
        }
    }

    fn channel() -> (tempfile::TempDir, Folders) {
        let root = tempfile::tempdir().unwrap();
        let video = root.path().join("videos/fridays");
        fs::create_dir_all(&video).unwrap();
        fs::create_dir_all(root.path().join("brand")).unwrap();
        let folders = Folders::of_video(&video);
        assert_eq!(folders.channel.as_deref(), Some(root.path()));
        (root, folders)
    }

    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0];
    const MP3: &[u8] = b"ID3\x04\0\0\0\0\0\0";

    #[test]
    fn downloads_land_in_the_channel_with_a_credit_and_a_free_name() {
        let (root, folders) = channel();
        let sound = folders
            .save(&hit(Media::Audio, "Applause 2.WAV", "mp3"), MP3)
            .unwrap();
        assert_eq!(sound.asset.name, "applause-2-wav");
        assert_eq!(
            sound.asset.path.as_deref(),
            Some(
                root.path()
                    .join("brand/sounds/applause-2-wav.mp3")
                    .as_path()
            )
        );
        assert!(
            root.path()
                .join("brand/sounds/applause-2-wav.credit.kdl")
                .exists()
        );
        let again = folders
            .save(&hit(Media::Audio, "Applause 2.WAV", "mp3"), MP3)
            .unwrap();
        assert_eq!(again.asset.name, "applause-2-wav-2");

        let image = folders
            .save(&hit(Media::Image, "1 Facepalm!", "jpg"), JPEG)
            .unwrap();
        assert_eq!(
            image.relative.as_deref(),
            Some("../../library/image-1-facepalm.jpg")
        );
        assert!(
            folders
                .save(&hit(Media::Image, "x", "jpg"), b"GIF89a")
                .is_err()
        );

        let sounds = folders.sounds();
        let downloaded = sounds.iter().find(|a| a.name == "applause-2-wav").unwrap();
        assert_eq!(downloaded.origin, Origin::Channel);
        assert!(downloaded.matches(&["meme".into(), "appl".into()]));
        assert!(
            sounds
                .iter()
                .any(|a| a.name == "whoosh" && a.origin == Origin::Bundled)
        );
        assert_eq!(folders.images().len(), 1);
        assert_eq!(
            folders
                .downloaded(Media::Image, "id-1 Facepalm!")
                .map(|a| a.name),
            Some("image-1-facepalm".into())
        );
    }

    #[test]
    fn a_video_outside_a_channel_downloads_into_its_own_folder() {
        let root = tempfile::tempdir().unwrap();
        let folders = Folders::of_video(root.path());
        assert_eq!(folders.channel, None);
        let saved = folders
            .save(&hit(Media::Image, "Cat", "jpg"), JPEG)
            .unwrap();
        assert_eq!(saved.relative.as_deref(), Some("sources/cat.jpg"));
        assert_eq!(
            folders.download_dir(Media::Audio),
            Some(root.path().join("sounds"))
        );
    }

    #[test]
    fn the_credits_of_a_cut_list_each_downloaded_asset_it_uses_once() {
        let (_root, folders) = channel();
        folders
            .save(&hit(Media::Audio, "Boing", "mp3"), MP3)
            .unwrap();
        folders
            .save(&hit(Media::Audio, "Unused", "mp3"), MP3)
            .unwrap();
        folders
            .save(&hit(Media::Image, "Facepalm", "jpg"), JPEG)
            .unwrap();
        let cut = Cut::parse(
            "chapter \"A\" {\n    take t01 {\n        keep from=0 to=9_000_000\n        \
             overlay pop from=1_000_000 to=2_000_000 {\n            image \
             \"../../library/facepalm.jpg\"\n        }\n        overlay b-roll from=3_000_000 \
             to=4_000_000 {\n            file \"../../library/facepalm.jpg\"\n        }\n    \
             }\n}\n\noverride t01 word at=1_000_000 {\n    sfx boing\n}\n\noverride t01 word \
             at=2_000_000 {\n    sfx whoosh\n}\n",
        )
        .unwrap();
        let credits = folders.credits(&cut, None);
        let titles: Vec<&str> = credits.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["Boing", "Facepalm"]);
        let text = description(&credits);
        assert!(text.starts_with(
            "Credits\nBoing by someone, CC BY 2.0 Source: https://example.org/Boing\n"
        ));
        assert_eq!(description(&[]), "");
    }

    #[test]
    fn slugs_are_sound_names() {
        assert_eq!(slug("Deep Whoosh #1", Media::Audio), "deep-whoosh-1");
        assert_eq!(slug("¡¡¡", Media::Audio), "sound");
        assert!(Id::new(slug("42 — Ünïcode stuff", Media::Audio)).is_some());
    }
}
