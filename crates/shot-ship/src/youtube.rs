//! The YouTube Data API v3 calls `shot ship` makes: a resumable `videos.insert` and
//! `thumbnails.set`.
//!
//! The upload follows Google's resumable protocol: one request opens a session with the
//! metadata, then the file goes up in chunks. After a network error or a 5xx the client asks the
//! session how much arrived (`Content-Range: bytes */<size>`) and continues from there, with
//! exponential backoff. The session URI is handed to the caller, so an interrupted ship resumes
//! the same upload. An expired access token is refreshed once per request, a quota error stops
//! right away.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use ureq::Agent;
use ureq::http::Response;

use crate::oauth::{self, AuthError, Client, Endpoints, TokenStore};

/// Chunks are a multiple of 256 KiB, as the protocol requires of every chunk but the last.
pub const CHUNK: usize = 32 * 256 * 1024;
/// YouTube's limits on the snippet.
pub const TITLE_MAX: usize = 100;
pub const DESCRIPTION_MAX: usize = 5000;
/// A custom thumbnail is at most 2 MB.
pub const THUMBNAIL_MAX: usize = 2 * 1024 * 1024;
/// Science & Technology.
pub const CATEGORY: &str = "28";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Privacy {
    #[default]
    Private,
    Unlisted,
    Public,
}

impl Privacy {
    pub const ALL: [Self; 3] = [Self::Private, Self::Unlisted, Self::Public];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Unlisted => "unlisted",
            Self::Public => "public",
        }
    }
}

/// What a video is published with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub title: String,
    pub description: String,
    pub privacy: Privacy,
    /// Always sent: YouTube requires the audience to be declared.
    pub made_for_kids: bool,
}

impl Metadata {
    /// Why YouTube would refuse this snippet, if it would.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let title = self.title.chars().count();
        if self.title.trim().is_empty() {
            out.push("the title is empty".to_owned());
        } else if title > TITLE_MAX {
            out.push(format!(
                "the title is {title} characters, YouTube allows {TITLE_MAX}"
            ));
        }
        if self.description.len() > DESCRIPTION_MAX {
            out.push(format!(
                "the description is {} bytes, YouTube allows {DESCRIPTION_MAX}",
                self.description.len()
            ));
        }
        for (field, text) in [("title", &self.title), ("description", &self.description)] {
            if text.contains(['<', '>']) {
                out.push(format!(
                    "the {field} contains < or >, which YouTube refuses"
                ));
            }
        }
        out
    }

    fn resource(&self) -> serde_json::Value {
        json!({
            "snippet": {
                "title": self.title,
                "description": self.description,
                "categoryId": CATEGORY,
            },
            "status": {
                "privacyStatus": self.privacy.as_str(),
                "selfDeclaredMadeForKids": self.made_for_kids,
            },
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum YouTubeError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(
        "the YouTube API quota is used up for today ({0}). It resets at midnight Pacific time; \
         run `shot ship` again then and the upload continues"
    )]
    Quota(String),
    #[error("YouTube refused the request: {status} {reason}: {message}")]
    Refused {
        status: u16,
        reason: String,
        message: String,
    },
    #[error("YouTube did not answer after {attempts} attempts: {last}")]
    Unavailable { attempts: u32, last: String },
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// How often and how patiently a failing request is tried again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    pub attempts: u32,
    /// The first wait, doubled after each failure.
    pub first: Duration,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            attempts: 6,
            first: Duration::from_secs(1),
        }
    }
}

/// An upload's progress, reported after every chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    pub bytes: u64,
    pub total: u64,
}

/// A signed-in YouTube client for one channel.
pub struct YouTube {
    agent: Agent,
    client: Client,
    endpoints: Endpoints,
    store: Box<dyn TokenStore>,
    access: Option<String>,
    pub retry: Retry,
    pub chunk: usize,
}

/// One answer of the API, read whole. Upload responses are small JSON documents.
struct Answer {
    status: u16,
    range_end: Option<u64>,
    location: Option<String>,
    body: String,
}

impl Answer {
    fn read(mut response: Response<ureq::Body>) -> Result<Self, ureq::Error> {
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let range_end = header("range").and_then(|r| {
            r.strip_prefix("bytes=")?
                .split_once('-')?
                .1
                .trim()
                .parse()
                .ok()
        });
        let location = header("location");
        Ok(Self {
            status: response.status().as_u16(),
            range_end,
            location,
            body: response.body_mut().read_to_string()?,
        })
    }

    fn retryable(&self) -> bool {
        matches!(self.status, 500 | 502 | 503 | 504)
    }

    fn error(&self) -> YouTubeError {
        #[derive(Deserialize)]
        struct Envelope {
            error: Detail,
        }
        #[derive(Deserialize)]
        struct Detail {
            #[serde(default)]
            message: String,
            #[serde(default)]
            errors: Vec<Reason>,
        }
        #[derive(Deserialize)]
        struct Reason {
            #[serde(default)]
            reason: String,
        }
        let (reason, message) = match serde_json::from_str::<Envelope>(&self.body) {
            Ok(e) => (
                e.error
                    .errors
                    .into_iter()
                    .next()
                    .map(|r| r.reason)
                    .unwrap_or_default(),
                e.error.message,
            ),
            Err(_) => (String::new(), self.body.trim().to_owned()),
        };
        if matches!(reason.as_str(), "quotaExceeded" | "dailyLimitExceeded") {
            return YouTubeError::Quota(reason);
        }
        YouTubeError::Refused {
            status: self.status,
            reason,
            message,
        }
    }
}

/// Where an upload session stands.
enum Session {
    /// This many bytes arrived.
    At(u64),
    /// The upload finished, this is the video id.
    Done(String),
    /// The session URI no longer exists, it expires after about a week.
    Gone,
}

impl YouTube {
    pub fn new(client: Client, store: Box<dyn TokenStore>, endpoints: Endpoints) -> Self {
        let agent = Agent::new_with_config(
            Agent::config_builder()
                .http_status_as_error(false)
                // A resumable session answers 308 Resume Incomplete, which is not a redirect.
                .max_redirects(0)
                .timeout_connect(Some(Duration::from_secs(30)))
                .timeout_recv_response(Some(Duration::from_secs(300)))
                .build(),
        );
        Self {
            agent,
            client,
            endpoints,
            store,
            access: None,
            retry: Retry::default(),
            chunk: CHUNK,
        }
    }

    /// Whether a refresh token is stored.
    pub fn signed_in(&self) -> Result<bool, YouTubeError> {
        Ok(self.store.load()?.is_some())
    }

    /// Signs in through the browser and stores the refresh token. `open` shows the consent page.
    pub fn sign_in(
        &mut self,
        open: &mut dyn FnMut(&str) -> std::io::Result<()>,
    ) -> Result<(), YouTubeError> {
        let tokens = oauth::sign_in(
            &self.agent,
            &self.client,
            &self.endpoints,
            open,
            oauth::SIGN_IN_TIMEOUT,
        )?;
        self.store
            .save(tokens.refresh_token.as_deref().expect("sign_in checks it"))?;
        self.access = Some(tokens.access_token);
        Ok(())
    }

    /// Uploads `file` and returns the new video's id. `resume` is a session URI an earlier,
    /// interrupted call handed to `session`; it continues where that upload stopped.
    pub fn upload(
        &mut self,
        file: &Path,
        metadata: &Metadata,
        resume: Option<&str>,
        session: &mut dyn FnMut(&str),
        progress: &mut dyn FnMut(Sent),
    ) -> Result<String, YouTubeError> {
        let mut source = File::open(file)?;
        let total = source.metadata()?.len();
        let mut uri = match resume {
            Some(uri) => uri.to_owned(),
            None => self.open_session(metadata, total)?,
        };
        session(&uri);
        let mut at = 0;
        if resume.is_some() {
            match self.status(&uri, total)? {
                Session::At(bytes) => at = bytes,
                Session::Done(id) => return Ok(id),
                Session::Gone => {
                    uri = self.open_session(metadata, total)?;
                    session(&uri);
                }
            }
        }
        let mut chunk = vec![0; self.chunk];
        let mut failures = 0;
        loop {
            let len = (total - at).min(self.chunk as u64) as usize;
            source.seek(SeekFrom::Start(at))?;
            source.read_exact(&mut chunk[..len])?;
            let range = if len == 0 {
                format!("bytes */{total}")
            } else {
                format!("bytes {at}-{}/{total}", at + len as u64 - 1)
            };
            let token = self.access_token()?;
            let sent = self
                .agent
                .put(&uri)
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Range", range)
                .send(&chunk[..len])
                .and_then(Answer::read);
            let failure = match sent {
                Ok(answer) => match answer.status {
                    200 | 201 => return video_id(&answer.body),
                    308 => {
                        at = answer.range_end.map_or(0, |end| end + 1);
                        failures = 0;
                        progress(Sent { bytes: at, total });
                        continue;
                    }
                    401 => {
                        self.access = None;
                        format!("{} {}", answer.status, answer.body.trim())
                    }
                    404 | 410 => {
                        uri = self.open_session(metadata, total)?;
                        session(&uri);
                        at = 0;
                        continue;
                    }
                    _ if answer.retryable() => format!("{} {}", answer.status, answer.body.trim()),
                    _ => return Err(answer.error()),
                },
                Err(e) => e.to_string(),
            };
            failures += 1;
            self.back_off(failures, failure)?;
            match self.status(&uri, total)? {
                Session::At(bytes) => at = bytes,
                Session::Done(id) => return Ok(id),
                Session::Gone => {
                    uri = self.open_session(metadata, total)?;
                    session(&uri);
                    at = 0;
                }
            }
        }
    }

    /// Sets a video's custom thumbnail from a PNG or JPEG of at most [`THUMBNAIL_MAX`] bytes.
    pub fn set_thumbnail(
        &mut self,
        video_id: &str,
        image: &[u8],
        mime: &str,
    ) -> Result<(), YouTubeError> {
        let url = format!(
            "{}/thumbnails/set?videoId={video_id}&uploadType=media",
            self.endpoints.upload
        );
        let answer = self.request(|agent, token| {
            agent
                .post(&url)
                .header("Authorization", format!("Bearer {token}"))
                .content_type(mime)
                .send(image)
        })?;
        match answer.status {
            200 => Ok(()),
            _ => Err(answer.error()),
        }
    }

    fn open_session(&mut self, metadata: &Metadata, total: u64) -> Result<String, YouTubeError> {
        let url = format!(
            "{}/videos?uploadType=resumable&part=snippet,status",
            self.endpoints.upload
        );
        let body = metadata.resource().to_string();
        let answer = self.request(|agent, token| {
            agent
                .post(&url)
                .header("Authorization", format!("Bearer {token}"))
                .header("X-Upload-Content-Length", total.to_string())
                .header("X-Upload-Content-Type", "video/mp4")
                .content_type("application/json; charset=UTF-8")
                .send(&body)
        })?;
        match (answer.status, &answer.location) {
            (200, Some(location)) => Ok(location.clone()),
            (200, None) => Err(YouTubeError::Refused {
                status: 200,
                reason: "noSession".to_owned(),
                message: "the upload session came back without a Location".to_owned(),
            }),
            _ => Err(answer.error()),
        }
    }

    /// Asks an upload session how much of the file arrived.
    fn status(&mut self, uri: &str, total: u64) -> Result<Session, YouTubeError> {
        let answer = self.request(|agent, token| {
            agent
                .put(uri)
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Range", format!("bytes */{total}"))
                .send_empty()
        })?;
        match answer.status {
            200 | 201 => Ok(Session::Done(video_id(&answer.body)?)),
            308 => Ok(Session::At(answer.range_end.map_or(0, |end| end + 1))),
            404 | 410 => Ok(Session::Gone),
            _ => Err(answer.error()),
        }
    }

    /// Sends one request, refreshing the access token once on a 401 and backing off on network
    /// errors and 5xx answers.
    fn request(
        &mut self,
        send: impl Fn(&Agent, &str) -> Result<Response<ureq::Body>, ureq::Error>,
    ) -> Result<Answer, YouTubeError> {
        let mut failures = 0;
        let mut refreshed = false;
        loop {
            let token = self.access_token()?;
            let failure = match send(&self.agent, &token).and_then(Answer::read) {
                Ok(answer) if answer.status == 401 && !refreshed => {
                    self.access = None;
                    refreshed = true;
                    continue;
                }
                Ok(answer) if answer.retryable() => {
                    format!("{} {}", answer.status, answer.body.trim())
                }
                Ok(answer) => return Ok(answer),
                Err(e) => e.to_string(),
            };
            failures += 1;
            self.back_off(failures, failure)?;
        }
    }

    fn back_off(&self, failures: u32, last: String) -> Result<(), YouTubeError> {
        if failures >= self.retry.attempts {
            return Err(YouTubeError::Unavailable {
                attempts: failures,
                last,
            });
        }
        sleep(self.retry.first * 2u32.pow(failures - 1));
        Ok(())
    }

    fn access_token(&mut self) -> Result<String, YouTubeError> {
        if let Some(token) = &self.access {
            return Ok(token.clone());
        }
        let refresh_token = self.store.load()?.ok_or(AuthError::Expired)?;
        let token = match oauth::refresh(&self.agent, &self.client, &self.endpoints, &refresh_token)
        {
            Err(AuthError::Expired) => {
                self.store.clear()?;
                return Err(AuthError::Expired.into());
            }
            result => result?,
        };
        self.access = Some(token.clone());
        Ok(token)
    }
}

fn video_id(body: &str) -> Result<String, YouTubeError> {
    #[derive(Deserialize)]
    struct Video {
        id: String,
    }
    serde_json::from_str::<Video>(body)
        .map(|v| v.id)
        .map_err(|_| YouTubeError::Refused {
            status: 200,
            reason: "noVideoId".to_owned(),
            message: format!("the finished upload returned no video id: {}", body.trim()),
        })
}

/// The watch page of a video.
pub fn watch_url(video_id: &str) -> String {
    format!("https://youtu.be/{video_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_problems_name_youtube_limits() {
        let mut metadata = Metadata {
            title: "x".repeat(101),
            description: "a <b>".to_owned(),
            privacy: Privacy::Private,
            made_for_kids: false,
        };
        assert_eq!(
            metadata.problems(),
            [
                "the title is 101 characters, YouTube allows 100",
                "the description contains < or >, which YouTube refuses"
            ]
        );
        metadata.title = "Why we deploy on Fridays".to_owned();
        metadata.description = "Chapters\n0:00 Hook\n".to_owned();
        assert!(metadata.problems().is_empty());
        let resource = metadata.resource();
        assert_eq!(resource["status"]["privacyStatus"], "private");
        assert_eq!(resource["status"]["selfDeclaredMadeForKids"], false);
    }
}
