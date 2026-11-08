//! Openverse, anonymous: search for sounds and images, fetch previews and files.
//!
//! Every search asks only for CC0, the Public Domain Mark and CC BY, the licenses that allow
//! commercial use (YouTube monetization) and edits without binding the video to a license, and
//! only for file types the app plays or draws: MP3, WAV and FLAC sounds, JPEG and PNG stills.
//! [`parse`] checks both again, so a result outside them never shows. Anonymous clients get 20
//! searches a minute and 200 a day; [`Error::Limited`] says when that ran out.

use std::io::Read;
use std::time::Duration;

use serde::Deserialize;
use shot_core::{Creator, Credit, License};

const API: &str = "https://api.openverse.org/v1";
/// The licenses asked for, as Openverse names them.
const LICENSES: &str = "cc0,pdm,by";
/// The most results an anonymous search returns at once.
pub const PAGE_SIZE: u32 = 20;
const USER_AGENT: &str = concat!(
    "ShotStudio/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/coderscantina/shot-studio)"
);

/// What a search looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Media {
    Audio,
    Image,
}

impl Media {
    fn endpoint(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Image => "images",
        }
    }

    /// The file types asked for, which the app plays or draws.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Audio => &["mp3", "wav", "flac"],
            Self::Image => &["jpg", "png"],
        }
    }

    /// The largest file a download accepts.
    pub fn limit(self) -> u64 {
        match self {
            Self::Audio => 40 << 20,
            Self::Image => 25 << 20,
        }
    }
}

/// One result: what it is, where it comes from and its license.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub id: String,
    pub media: Media,
    pub title: String,
    pub creator: Option<Creator>,
    pub license: License,
    pub version: Option<String>,
    pub license_url: Option<String>,
    /// The page the asset is on at its provider.
    pub source: String,
    pub provider: Option<String>,
    /// The file to download, also the preview of a sound.
    pub file: String,
    /// The file's type, `mp3`, `wav`, `flac`, `jpg` or `png`.
    pub extension: &'static str,
    pub size: Option<u64>,
    pub duration: Option<Duration>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// A small preview image.
    pub thumbnail: Option<String>,
    pub tags: Vec<String>,
    pub attribution: String,
}

impl Hit {
    /// The credit saved beside the download.
    pub fn credit(&self) -> Credit {
        Credit {
            title: self.title.clone(),
            creator: self.creator.clone(),
            license: self.license,
            version: self.version.clone(),
            license_url: self.license_url.clone(),
            source: self.source.clone(),
            provider: self.provider.clone(),
            file: Some(self.file.clone()),
            openverse: Some(self.id.clone()),
            tags: self.tags.clone(),
            attribution: self.attribution.clone(),
        }
    }
}

/// One page of results.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    pub hits: Vec<Hit>,
    /// 1-based.
    pub page: u32,
    pub pages: u32,
    pub total: u64,
}

impl Page {
    pub fn has_more(&self) -> bool {
        self.page < self.pages
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Openverse can't be reached. Check the connection and try again")]
    Offline,
    #[error("Openverse's limit for searches without an account is used up. Try again in a minute")]
    Limited,
    #[error("Openverse answered {0}")]
    Status(u16),
    #[error("Openverse sent something unexpected: {0}")]
    Invalid(String),
    #[error("the file is larger than {} MB", .0 >> 20)]
    TooLarge(u64),
    #[error("stopped")]
    Stopped,
}

impl From<ureq::Error> for Error {
    fn from(error: ureq::Error) -> Self {
        match error {
            ureq::Error::StatusCode(429) => Self::Limited,
            ureq::Error::StatusCode(code) => Self::Status(code),
            ureq::Error::BodyExceedsLimit(limit) => Self::TooLarge(limit),
            ureq::Error::HostNotFound
            | ureq::Error::ConnectionFailed
            | ureq::Error::Timeout(_)
            | ureq::Error::Io(_) => Self::Offline,
            other => Self::Invalid(other.to_string()),
        }
    }
}

#[derive(Deserialize)]
struct RawPage {
    #[serde(default)]
    result_count: u64,
    #[serde(default)]
    page_count: u32,
    #[serde(default)]
    page: u32,
    results: Vec<RawHit>,
}

#[derive(Deserialize)]
struct RawHit {
    id: String,
    title: Option<String>,
    foreign_landing_url: Option<String>,
    url: Option<String>,
    creator: Option<String>,
    creator_url: Option<String>,
    license: String,
    license_version: Option<String>,
    license_url: Option<String>,
    provider: Option<String>,
    filetype: Option<String>,
    filesize: Option<u64>,
    #[serde(default)]
    tags: Vec<RawTag>,
    attribution: Option<String>,
    #[serde(default)]
    mature: bool,
    /// Milliseconds, for audio.
    duration: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    thumbnail: Option<String>,
}

#[derive(Deserialize)]
struct RawTag {
    name: String,
    /// Set for machine-made tags, which are left out.
    accuracy: Option<f64>,
}

/// A page of results from Openverse's JSON. Results under another license, of another file
/// type, marked mature or without a source page or file are left out.
pub fn parse(media: Media, json: &str) -> Result<Page, Error> {
    let raw: RawPage = serde_json::from_str(json).map_err(|e| Error::Invalid(e.to_string()))?;
    let hits = raw
        .results
        .into_iter()
        .filter(|r| !r.mature)
        .filter_map(|r| hit(media, r))
        .collect();
    Ok(Page {
        hits,
        page: raw.page.max(1),
        pages: raw.page_count,
        total: raw.result_count,
    })
}

fn hit(media: Media, raw: RawHit) -> Option<Hit> {
    let license = match raw.license.as_str() {
        "cc0" => License::Cc0,
        "pdm" => License::Pdm,
        "by" => License::By,
        _ => return None,
    };
    let file = raw.url.filter(|u| u.starts_with("https://"))?;
    let extension = extension(media, raw.filetype.as_deref(), &file)?;
    let title = raw
        .title
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Untitled".to_owned());
    let creator = raw
        .creator
        .filter(|c| !c.trim().is_empty())
        .map(|name| Creator {
            name: name.trim().to_owned(),
            url: raw.creator_url,
        });
    let attribution = raw.attribution.unwrap_or_else(|| {
        let by = creator
            .as_ref()
            .map(|c| format!(" by {}", c.name))
            .unwrap_or_default();
        format!("\"{title}\"{by}, {}", license.label())
    });
    Some(Hit {
        id: raw.id,
        media,
        title,
        creator,
        license,
        version: raw.license_version,
        license_url: raw.license_url,
        source: raw.foreign_landing_url?,
        provider: raw.provider,
        file,
        extension,
        size: raw.filesize,
        duration: raw.duration.map(Duration::from_millis),
        width: raw.width,
        height: raw.height,
        thumbnail: raw.thumbnail,
        tags: raw
            .tags
            .into_iter()
            .filter(|t| t.accuracy.is_none())
            .map(|t| t.name)
            .collect(),
        attribution,
    })
}

/// The file type of a result, from its `filetype` or else its URL, if the app can use it.
fn extension(media: Media, filetype: Option<&str>, url: &str) -> Option<&'static str> {
    let from_url = || {
        let path = url.split(['?', '#']).next()?;
        let (_, ext) = path.rsplit_once('/')?.1.rsplit_once('.')?;
        Some(ext.to_ascii_lowercase())
    };
    let ext = filetype.map(str::to_ascii_lowercase).or_else(from_url)?;
    let ext = if ext == "jpeg" { "jpg".to_owned() } else { ext };
    media.extensions().iter().copied().find(|e| *e == ext)
}

/// A blocking Openverse client. Every call goes to the network, so run it off the UI thread.
#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(8)))
            .timeout_recv_response(Some(Duration::from_secs(15)))
            .https_only(true)
            .build();
        Self {
            agent: config.into(),
        }
    }

    /// Page `page` (1-based) of the results for `query`.
    pub fn search(&self, media: Media, query: &str, page: u32) -> Result<Page, Error> {
        let mut response = self
            .agent
            .get(format!("{API}/{}/", media.endpoint()))
            .query("q", query.trim())
            .query("license", LICENSES)
            .query("extension", media.extensions().join(","))
            .query("page", page.max(1).to_string())
            .query("page_size", PAGE_SIZE.to_string())
            .call()?;
        let json = response
            .body_mut()
            .with_config()
            .limit(4 << 20)
            .read_to_string()?;
        parse(media, &json)
    }

    /// The bytes at `url`, at most `limit` of them. `progress` hears the bytes so far and the
    /// size when the server says it, and stops the download by returning false.
    pub fn fetch(
        &self,
        url: &str,
        limit: u64,
        mut progress: impl FnMut(u64, Option<u64>) -> bool,
    ) -> Result<Vec<u8>, Error> {
        let mut response = self.agent.get(url).call()?;
        let total = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
        if total.is_some_and(|t| t > limit) {
            return Err(Error::TooLarge(limit));
        }
        let mut reader = response.body_mut().with_config().limit(limit).reader();
        let mut bytes = Vec::with_capacity(total.unwrap_or(0) as usize);
        let mut buf = vec![0; 64 << 10];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) if bytes.len() as u64 >= limit => return Err(Error::TooLarge(limit)),
                Err(e) => return Err(Error::Invalid(e.to_string())),
            };
            bytes.extend_from_slice(&buf[..n]);
            if !progress(bytes.len() as u64, total) {
                return Err(Error::Stopped);
            }
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUDIO: &str = include_str!("../tests/data/audio-applause.json");
    const IMAGES: &str = include_str!("../tests/data/images-facepalm.json");

    #[test]
    fn a_recorded_sound_search_parses_with_license_source_and_length() {
        let page = parse(Media::Audio, AUDIO).unwrap();
        assert_eq!((page.page, page.pages, page.total), (1, 12, 240));
        assert!(page.has_more());
        let first = &page.hits[0];
        assert_eq!(first.title, "Applause 2.WAV");
        assert_eq!(first.license, License::Cc0);
        assert_eq!(first.extension, "mp3");
        assert_eq!(first.duration, Some(Duration::from_millis(6320)));
        assert!(first.source.starts_with("https://freesound.org/"));
        assert!(first.file.starts_with("https://cdn.freesound.org/"));
        let credit = first.credit();
        assert_eq!(credit.openverse.as_deref(), Some(first.id.as_str()));
        assert_eq!(Credit::parse(&credit.to_kdl()).unwrap(), credit);
    }

    #[test]
    fn a_recorded_image_search_takes_the_type_from_the_url_and_drops_machine_tags() {
        let page = parse(Media::Image, IMAGES).unwrap();
        assert_eq!(page.hits.len(), 3);
        let first = &page.hits[0];
        assert_eq!(first.extension, "jpg");
        assert_eq!(first.license, License::By);
        assert_eq!((first.width, first.height), (Some(1024), Some(576)));
        assert_eq!(first.tags, ["daylight", "facepalm", "hangover"]);
        assert!(first.thumbnail.as_deref().unwrap().ends_with("/thumb/"));
    }

    #[test]
    fn other_licenses_types_and_mature_results_never_show() {
        let json = |license: &str, filetype: &str, mature: bool| {
            format!(
                r#"{{"result_count":1,"page_count":1,"page":1,"results":[{{"id":"a","title":"T",
                "foreign_landing_url":"https://x.org/a","url":"https://x.org/a.{filetype}",
                "license":"{license}","filetype":"{filetype}","mature":{mature}}}]}}"#
            )
        };
        let count = |media, license, filetype, mature| {
            parse(media, &json(license, filetype, mature))
                .unwrap()
                .hits
                .len()
        };
        assert_eq!(count(Media::Image, "by", "png", false), 1);
        assert_eq!(count(Media::Image, "by-nc", "png", false), 0);
        assert_eq!(count(Media::Image, "by-sa", "png", false), 0);
        assert_eq!(count(Media::Image, "by", "gif", false), 0);
        assert_eq!(count(Media::Image, "by", "png", true), 0);
        assert_eq!(count(Media::Audio, "pdm", "ogg", false), 0);
        assert_eq!(count(Media::Audio, "pdm", "wav", false), 1);
        assert!(parse(Media::Audio, "<html>").is_err());
    }

    #[test]
    #[ignore = "searches Openverse online"]
    fn a_live_search_returns_usable_sounds_and_images() {
        let client = Client::new();
        let sounds = client.search(Media::Audio, "whoosh", 1).unwrap();
        assert!(!sounds.hits.is_empty());
        let preview = client
            .fetch(&sounds.hits[0].file, Media::Audio.limit(), |_, _| true)
            .unwrap();
        assert!(!preview.is_empty());
        let images = client.search(Media::Image, "facepalm", 1).unwrap();
        assert!(images.hits.iter().all(|h| h.extension != "gif"));
    }
}
