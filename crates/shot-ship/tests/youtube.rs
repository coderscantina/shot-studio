//! The upload flow against a local stand-in for Google's OAuth and YouTube upload endpoints. It
//! speaks the resumable protocol: a session from the metadata, chunks with `Content-Range`, 308
//! with the received `Range`, a status query with `bytes */<size>`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use shot_core::{Date, DerivedKind, Micros, OutputKind};
use shot_ship::oauth::{AuthError, Client, Endpoints, TokenStore};
use shot_ship::record::Record;
use shot_ship::ship::{Channel, Destination, PlannedCut, publish};
use shot_ship::youtube::{Metadata, Privacy, Retry, Sent, YouTube, YouTubeError};

struct Request {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}

fn reply(status: u16, body: &str) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

/// What the fake Google knows and what it was asked.
#[derive(Default)]
struct Google {
    base: String,
    access: String,
    refresh: String,
    issued: usize,
    challenge: Option<String>,
    sessions: usize,
    received: Vec<u8>,
    total: usize,
    puts: usize,
    /// The nth chunk PUT answers this status instead of storing the bytes.
    fail_put: Option<(usize, u16)>,
    /// After the nth chunk PUT the access token expires.
    expire_after_put: Option<usize>,
    quota: bool,
    refuse_thumbnail: bool,
    metadata: Vec<serde_json::Value>,
    thumbnails: Vec<(String, usize)>,
    log: Vec<String>,
}

impl Google {
    fn handle(&mut self, request: &Request) -> Reply {
        let (path, query) = request
            .target
            .split_once('?')
            .unwrap_or((&request.target, ""));
        let query: HashMap<String, String> = form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        if path == "/token" {
            let form: HashMap<String, String> =
                form_urlencoded::parse(&request.body).into_owned().collect();
            self.log.push(format!("token {}", form["grant_type"]));
            let issue = |google: &mut Self| {
                google.issued += 1;
                google.access = format!("a{}", google.issued);
            };
            return match form["grant_type"].as_str() {
                "refresh_token" if form["refresh_token"] == self.refresh => {
                    issue(self);
                    reply(
                        200,
                        &format!(r#"{{"access_token":"{}","expires_in":3599}}"#, self.access),
                    )
                }
                "authorization_code"
                    if form["code"] == "code-1"
                        && self.challenge.as_deref()
                            == Some(
                                &URL_SAFE_NO_PAD
                                    .encode(Sha256::digest(form["code_verifier"].as_bytes())),
                            ) =>
                {
                    issue(self);
                    self.refresh = "r-new".to_owned();
                    reply(
                        200,
                        &format!(
                            r#"{{"access_token":"{}","refresh_token":"r-new"}}"#,
                            self.access
                        ),
                    )
                }
                _ => reply(400, r#"{"error":"invalid_grant"}"#),
            };
        }
        if request.headers.get("authorization") != Some(&format!("Bearer {}", self.access)) {
            self.log.push(format!("{} {path} 401", request.method));
            return reply(
                401,
                r#"{"error":{"code":401,"message":"Invalid Credentials"}}"#,
            );
        }
        match (request.method.as_str(), path) {
            ("POST", "/upload/youtube/v3/videos") => {
                self.log.push("open session".to_owned());
                assert_eq!(query["uploadType"], "resumable");
                assert_eq!(query["part"], "snippet,status");
                if self.quota {
                    return reply(
                        403,
                        r#"{"error":{"code":403,"message":"The request cannot be completed because you have exceeded your quota.","errors":[{"domain":"youtube.quota","reason":"quotaExceeded"}]}}"#,
                    );
                }
                assert_eq!(request.headers["x-upload-content-type"], "video/mp4");
                self.total = request.headers["x-upload-content-length"].parse().unwrap();
                self.metadata
                    .push(serde_json::from_slice(&request.body).unwrap());
                self.sessions += 1;
                self.received.clear();
                Reply {
                    status: 200,
                    headers: vec![(
                        "Location",
                        format!("{}/session/{}", self.base, self.sessions),
                    )],
                    body: String::new(),
                }
            }
            ("PUT", session) if session.starts_with("/session/") => {
                let range = request.headers["content-range"].clone();
                self.log.push(format!("put {range}"));
                if !range.starts_with("bytes */") {
                    self.puts += 1;
                    if self.fail_put.is_some_and(|(n, _)| n == self.puts) {
                        return reply(self.fail_put.unwrap().1, "backend error");
                    }
                    let from: usize = range[6..range.find('-').unwrap()].parse().unwrap();
                    assert_eq!(from, self.received.len(), "chunks continue where data ends");
                    self.received.extend_from_slice(&request.body);
                    if self.expire_after_put == Some(self.puts) {
                        self.access = "expired".to_owned();
                    }
                }
                if self.received.len() == self.total {
                    return reply(200, &format!(r#"{{"id":"vid-{}"}}"#, self.sessions));
                }
                Reply {
                    status: 308,
                    headers: if self.received.is_empty() {
                        Vec::new()
                    } else {
                        vec![("Range", format!("bytes=0-{}", self.received.len() - 1))]
                    },
                    body: String::new(),
                }
            }
            ("POST", "/upload/youtube/v3/thumbnails/set") if self.refuse_thumbnail => reply(
                403,
                r#"{"error":{"code":403,"message":"The authenticated user doesn't have permissions to upload and set custom video thumbnails.","errors":[{"reason":"forbidden"}]}}"#,
            ),
            ("POST", "/upload/youtube/v3/thumbnails/set") => {
                self.thumbnails
                    .push((query["videoId"].clone(), request.body.len()));
                reply(200, "{}")
            }
            _ => reply(404, "not found"),
        }
    }
}

/// Starts the fake Google on a free port.
fn serve() -> Arc<Mutex<Google>> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let google = Arc::new(Mutex::new(Google {
        base: format!("http://{}", listener.local_addr().unwrap()),
        refresh: "r1".to_owned(),
        ..Google::default()
    }));
    let shared = google.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let request = read_request(&stream);
            let answer = shared.lock().unwrap().handle(&request);
            write_reply(&mut stream, answer);
        }
    });
    google
}

fn read_request(stream: &TcpStream) -> Request {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap().to_owned();
    let target = parts.next().unwrap().to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
    }
    let length = headers
        .get("content-length")
        .map_or(0, |l| l.parse().unwrap());
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    Request {
        method,
        target,
        headers,
        body,
    }
}

fn write_reply(stream: &mut TcpStream, reply: Reply) {
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (name, value) in reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(reply.body.as_bytes());
}

#[derive(Clone, Default)]
struct Memory(Arc<Mutex<Option<String>>>);

impl TokenStore for Memory {
    fn load(&self) -> Result<Option<String>, AuthError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, refresh_token: &str) -> Result<(), AuthError> {
        *self.0.lock().unwrap() = Some(refresh_token.to_owned());
        Ok(())
    }
    fn clear(&self) -> Result<(), AuthError> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

fn youtube(google: &Arc<Mutex<Google>>, refresh: Option<&str>) -> (YouTube, Memory) {
    let base = google.lock().unwrap().base.clone();
    let store = Memory(Arc::new(Mutex::new(refresh.map(str::to_owned))));
    let mut client = YouTube::new(
        Client {
            client_id: "id".to_owned(),
            client_secret: "secret".to_owned(),
        },
        Box::new(store.clone()),
        Endpoints {
            auth: format!("{base}/auth"),
            token: format!("{base}/token"),
            upload: format!("{base}/upload/youtube/v3"),
        },
    );
    client.chunk = 8;
    client.retry = Retry {
        attempts: 3,
        first: std::time::Duration::from_millis(1),
    };
    (client, store)
}

fn metadata() -> Metadata {
    Metadata {
        title: "Why we deploy on Fridays".to_owned(),
        description: "Chapters\n0:00 Hook".to_owned(),
        privacy: Privacy::Private,
        made_for_kids: false,
    }
}

const FILE: &[u8] = b"twenty bytes of mp4!";

fn file(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("main.mp4");
    std::fs::write(&path, FILE).unwrap();
    path
}

#[test]
fn uploads_in_chunks_with_private_metadata_and_returns_the_video_id() {
    let google = serve();
    let (mut client, _) = youtube(&google, Some("r1"));
    let dir = tempfile::tempdir().unwrap();
    let mut sessions = Vec::new();
    let mut sent = Vec::new();
    let id = client
        .upload(
            &file(dir.path()),
            &metadata(),
            None,
            &mut |uri| sessions.push(uri.to_owned()),
            &mut |s| sent.push(s),
        )
        .unwrap();
    assert_eq!(id, "vid-1");
    let google = google.lock().unwrap();
    assert_eq!(google.received, FILE);
    assert_eq!(sessions, [format!("{}/session/1", google.base)]);
    assert_eq!(
        sent,
        [
            Sent {
                bytes: 8,
                total: 20
            },
            Sent {
                bytes: 16,
                total: 20
            }
        ]
    );
    let resource = &google.metadata[0];
    assert_eq!(resource["snippet"]["title"], "Why we deploy on Fridays");
    assert_eq!(resource["status"]["privacyStatus"], "private");
    assert_eq!(resource["status"]["selfDeclaredMadeForKids"], false);
}

#[test]
fn a_server_error_mid_upload_resumes_from_what_arrived() {
    let google = serve();
    google.lock().unwrap().fail_put = Some((2, 503));
    let (mut client, _) = youtube(&google, Some("r1"));
    let dir = tempfile::tempdir().unwrap();
    let id = client
        .upload(
            &file(dir.path()),
            &metadata(),
            None,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(id, "vid-1");
    let google = google.lock().unwrap();
    assert_eq!(google.received, FILE);
    assert_eq!(
        google.log[2..],
        [
            "put bytes 0-7/20",
            "put bytes 8-15/20",
            "put bytes */20",
            "put bytes 8-15/20",
            "put bytes 16-19/20"
        ]
    );
}

#[test]
fn a_saved_session_continues_without_a_new_one() {
    let google = serve();
    let (mut client, _) = youtube(&google, Some("r1"));
    let dir = tempfile::tempdir().unwrap();
    let uri = {
        let mut google = google.lock().unwrap();
        google.sessions = 1;
        google.total = FILE.len();
        google.received = FILE[..8].to_vec();
        format!("{}/session/1", google.base)
    };
    let id = client
        .upload(
            &file(dir.path()),
            &metadata(),
            Some(&uri),
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(id, "vid-1");
    let google = google.lock().unwrap();
    assert_eq!(google.received, FILE);
    assert!(google.metadata.is_empty(), "no second session");
    assert_eq!(google.log[1], "put bytes */20");
}

#[test]
fn an_expired_access_token_is_refreshed_and_the_upload_goes_on() {
    let google = serve();
    google.lock().unwrap().expire_after_put = Some(1);
    let (mut client, _) = youtube(&google, Some("r1"));
    let dir = tempfile::tempdir().unwrap();
    let id = client
        .upload(
            &file(dir.path()),
            &metadata(),
            None,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();
    assert_eq!(id, "vid-1");
    let google = google.lock().unwrap();
    assert_eq!(google.received, FILE);
    let refreshes = google
        .log
        .iter()
        .filter(|l| *l == "token refresh_token")
        .count();
    assert_eq!(refreshes, 2);
}

#[test]
fn a_quota_error_stops_without_retrying() {
    let google = serve();
    google.lock().unwrap().quota = true;
    let (mut client, _) = youtube(&google, Some("r1"));
    let dir = tempfile::tempdir().unwrap();
    let error = client
        .upload(
            &file(dir.path()),
            &metadata(),
            None,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap_err();
    assert!(matches!(error, YouTubeError::Quota(ref r) if r == "quotaExceeded"));
    assert!(error.to_string().contains("resets at midnight Pacific"));
    let google = google.lock().unwrap();
    assert_eq!(google.log, ["token refresh_token", "open session"]);
}

#[test]
fn a_revoked_refresh_token_is_forgotten_and_asks_for_a_new_sign_in() {
    let google = serve();
    let (mut client, store) = youtube(&google, Some("revoked"));
    let dir = tempfile::tempdir().unwrap();
    let error = client
        .upload(
            &file(dir.path()),
            &metadata(),
            None,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap_err();
    assert!(matches!(error, YouTubeError::Auth(AuthError::Expired)));
    assert_eq!(store.load().unwrap(), None);
}

#[test]
fn sign_in_redirects_to_loopback_and_exchanges_the_code_with_pkce() {
    let google = serve();
    let (mut client, store) = youtube(&google, None);
    let shared = google.clone();
    client
        .sign_in(&mut |url| {
            let query: HashMap<String, String> =
                form_urlencoded::parse(url.split_once('?').unwrap().1.as_bytes())
                    .into_owned()
                    .collect();
            assert_eq!(query["code_challenge_method"], "S256");
            assert_eq!(
                query["scope"],
                "https://www.googleapis.com/auth/youtube.upload"
            );
            assert!(query["redirect_uri"].starts_with("http://127.0.0.1:"));
            shared.lock().unwrap().challenge = Some(query["code_challenge"].clone());
            let redirect = format!(
                "{}/?state={}&code=code-1&scope=x",
                query["redirect_uri"], query["state"]
            );
            std::thread::spawn(move || {
                let address = redirect
                    .trim_start_matches("http://")
                    .split('/')
                    .next()
                    .unwrap();
                let mut stream = TcpStream::connect(address).unwrap();
                let path = &redirect[redirect.find("/?").unwrap()..];
                write!(stream, "GET {path} HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
                let mut page = String::new();
                stream.read_to_string(&mut page).unwrap();
                assert!(page.contains("signed in"));
            });
            Ok(())
        })
        .unwrap();
    assert_eq!(store.load().unwrap().as_deref(), Some("r-new"));
}

/// A channel repo with one video on `main`, and its video folder.
fn channel(dir: &Path) -> (Channel, std::path::PathBuf) {
    let repo = git2::Repository::init(dir).unwrap();
    let video = dir.join("videos/demo");
    std::fs::create_dir_all(&video).unwrap();
    std::fs::write(
        video.join("video.kdl"),
        "title \"Demo\"\nstatus editing\nformat opinion\n",
    )
    .unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("videos/demo/video.kdl")).unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@localhost").unwrap();
    repo.commit(Some("refs/heads/main"), &sig, &sig, "add demo", &tree, &[])
        .unwrap();
    (Channel::of(&video).unwrap(), video)
}

fn planned(video: &Path, name: &str, kind: OutputKind, destination: Destination) -> PlannedCut {
    let export = video.join(format!(".shot/export/{name}.mp4"));
    std::fs::create_dir_all(export.parent().unwrap()).unwrap();
    std::fs::write(&export, FILE).unwrap();
    PlannedCut {
        name: name.to_owned(),
        kind,
        length: Micros(60_000_000),
        export: export.clone(),
        destination,
        metadata: (destination == Destination::YouTube).then(|| Metadata {
            description: if kind == OutputKind::Main {
                "Chapters\n0:00 Hook".to_owned()
            } else {
                "Never on a Friday?".to_owned()
            },
            ..metadata()
        }),
        thumbnail: (kind == OutputKind::Main).then(|| {
            let png = export.with_extension("png");
            std::fs::write(&png, b"png bytes").unwrap();
            png
        }),
    }
}

#[test]
fn publish_uploads_main_then_shorts_tags_the_release_and_runs_only_once() {
    let google = serve();
    let (mut client, _) = youtube(&google, Some("r1"));
    let dir = tempfile::tempdir().unwrap();
    let (channel, video) = channel(dir.path());
    let cuts = [
        planned(&video, "main", OutputKind::Main, Destination::YouTube),
        planned(
            &video,
            "friday",
            OutputKind::Derived(DerivedKind::Short),
            Destination::YouTube,
        ),
        planned(
            &video,
            "the-rule",
            OutputKind::Derived(DerivedKind::XClip),
            Destination::File,
        ),
    ];
    let today = Date::new(2026, 11, 7).unwrap();
    let shipped = publish(&video, &channel, &cuts, &mut client, today, &mut |_| {}).unwrap();
    assert_eq!(
        shipped.videos,
        [
            ("main".to_owned(), "vid-1".to_owned()),
            ("friday".to_owned(), "vid-2".to_owned())
        ]
    );
    assert_eq!(shipped.files, [cuts[2].export.clone()]);
    assert_eq!(shipped.tag, "v26.11.7");
    {
        let google = google.lock().unwrap();
        assert_eq!(google.thumbnails, [("vid-1".to_owned(), 9)]);
        assert_eq!(
            google.metadata[1]["snippet"]["description"],
            "Never on a Friday?\n\nFull video: https://youtu.be/vid-1"
        );
    }
    let record = Record::load(&video).unwrap();
    assert_eq!(record.video_id("main"), Some("vid-1"));
    assert_eq!(record.release.as_deref(), Some("v26.11.7"));
    let board = shot_board::Board::open(dir.path()).unwrap();
    let card = board.cards().unwrap().remove(0);
    assert_eq!(card.video.unwrap().status, shot_core::Status::Shipped);
    assert_eq!(board.releases().unwrap()[0].slug.as_deref(), Some("demo"));

    let requests = google.lock().unwrap().log.len();
    let again = publish(&video, &channel, &cuts, &mut client, today, &mut |_| {}).unwrap();
    assert_eq!(again, shipped);
    assert_eq!(
        google.lock().unwrap().log.len(),
        requests,
        "nothing sent twice"
    );
    assert_eq!(board.releases().unwrap().len(), 1);
}

#[test]
fn a_refused_thumbnail_does_not_stop_the_release() {
    let google = serve();
    google.lock().unwrap().refuse_thumbnail = true;
    let (mut client, _) = youtube(&google, Some("r1"));
    let dir = tempfile::tempdir().unwrap();
    let (channel, video) = channel(dir.path());
    let cuts = [planned(
        &video,
        "main",
        OutputKind::Main,
        Destination::YouTube,
    )];
    let mut steps = Vec::new();
    let shipped = publish(
        &video,
        &channel,
        &cuts,
        &mut client,
        Date::new(2026, 11, 7).unwrap(),
        &mut |s| steps.push(s),
    )
    .unwrap();
    assert_eq!(shipped.tag, "v26.11.7");
    assert!(
        steps
            .iter()
            .any(|s| matches!(s, shot_ship::ship::Step::ThumbnailRefused { .. }))
    );
    assert!(!Record::load(&video).unwrap().uploads["main"].thumbnail);
}
