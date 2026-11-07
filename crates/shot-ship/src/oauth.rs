//! Google OAuth 2.0 for a desktop app: the loopback redirect with PKCE (RFC 8252, RFC 7636) and
//! the refresh token in a [`TokenStore`], the macOS Keychain in the app and the CLI.
//!
//! The OAuth client (id and secret of a "Desktop app" client) comes from a file the person
//! downloads from the Google Cloud console, see [`client_path`]. It is never in the repo. For a
//! desktop client the secret is not a secret in Google's sense, PKCE is what protects the code.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Upload videos and set their thumbnails, nothing else on the channel.
pub const SCOPE: &str = "https://www.googleapis.com/auth/youtube.upload";
/// How long the sign-in waits for the browser to come back.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// Where Google's OAuth and upload endpoints are. Tests point these at a local server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub auth: String,
    pub token: String,
    /// The upload root of the Data API, `.../upload/youtube/v3`.
    pub upload: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            auth: "https://accounts.google.com/o/oauth2/v2/auth".to_owned(),
            token: "https://oauth2.googleapis.com/token".to_owned(),
            upload: "https://www.googleapis.com/upload/youtube/v3".to_owned(),
        }
    }
}

/// The OAuth client of a Google Cloud "Desktop app" client.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Client {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error(
        "no YouTube OAuth client at {}. In a Google Cloud project, enable the YouTube Data API v3, \
         create an OAuth client of type \"Desktop app\", download its JSON and save it there, \
         or point SHOT_YOUTUBE_CLIENT at it",
        .0.display()
    )]
    Setup(PathBuf),
    #[error("{}: {message}", path.display())]
    BadClient { path: PathBuf, message: String },
    #[error("YouTube sign-in did not finish: {0}")]
    SignIn(String),
    #[error("the YouTube sign-in expired or was revoked, sign in again")]
    Expired,
    #[error("Keychain: {0}")]
    Store(String),
    #[error("token request failed: {0}")]
    Http(String),
}

/// `$SHOT_YOUTUBE_CLIENT`, or `~/Library/Application Support/Shot Studio/youtube-client.json`.
pub fn client_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SHOT_YOUTUBE_CLIENT") {
        return PathBuf::from(path);
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/Shot Studio/youtube-client.json")
}

impl Client {
    /// Reads the JSON Google's console downloads (`{"installed": {...}}`) or a plain
    /// `{"client_id", "client_secret"}`.
    pub fn load(path: &Path) -> Result<Self, AuthError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AuthError::Setup(path.to_owned()));
            }
            Err(e) => {
                return Err(AuthError::BadClient {
                    path: path.to_owned(),
                    message: e.to_string(),
                });
            }
        };
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum File {
            Downloaded { installed: Client },
            Plain(Client),
        }
        match serde_json::from_str(&text) {
            Ok(File::Downloaded { installed: client } | File::Plain(client)) => Ok(client),
            Err(_) => Err(AuthError::BadClient {
                path: path.to_owned(),
                message: "not a Desktop app OAuth client: expected client_id and client_secret \
                          under \"installed\""
                    .to_owned(),
            }),
        }
    }
}

/// Keeps the refresh token between runs.
pub trait TokenStore: Send {
    fn load(&self) -> Result<Option<String>, AuthError>;
    fn save(&self, refresh_token: &str) -> Result<(), AuthError>;
    fn clear(&self) -> Result<(), AuthError>;
}

/// The refresh token as a generic password in the login Keychain, one per OAuth client.
#[cfg(target_os = "macos")]
pub struct Keychain {
    account: String,
}

#[cfg(target_os = "macos")]
impl Keychain {
    pub const SERVICE: &str = "Shot Studio YouTube";

    pub fn new(client: &Client) -> Self {
        Self {
            account: client.client_id.clone(),
        }
    }
}

#[cfg(target_os = "macos")]
impl TokenStore for Keychain {
    fn load(&self) -> Result<Option<String>, AuthError> {
        use security_framework::passwords::get_generic_password;
        // errSecItemNotFound
        const NOT_FOUND: i32 = -25300;
        match get_generic_password(Self::SERVICE, &self.account) {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|e| AuthError::Store(e.to_string())),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(AuthError::Store(e.to_string())),
        }
    }

    fn save(&self, refresh_token: &str) -> Result<(), AuthError> {
        security_framework::passwords::set_generic_password(
            Self::SERVICE,
            &self.account,
            refresh_token.as_bytes(),
        )
        .map_err(|e| AuthError::Store(e.to_string()))
    }

    fn clear(&self) -> Result<(), AuthError> {
        const NOT_FOUND: i32 = -25300;
        match security_framework::passwords::delete_generic_password(Self::SERVICE, &self.account) {
            Err(e) if e.code() != NOT_FOUND => Err(AuthError::Store(e.to_string())),
            _ => Ok(()),
        }
    }
}

/// A PKCE verifier and its S256 challenge.
struct Pkce {
    verifier: String,
    challenge: String,
}

impl Pkce {
    fn new() -> Self {
        let verifier = random_token();
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

/// 32 random bytes, base64url: 43 characters, the shortest verifier RFC 7636 allows.
fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system random source works");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The token endpoint's answer.
#[derive(Deserialize)]
pub(crate) struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

/// Signs in through the browser and returns the tokens. `open` shows the consent page, for
/// example with `open <url>`; the page redirects to a one-shot server on 127.0.0.1.
pub(crate) fn sign_in(
    agent: &ureq::Agent,
    client: &Client,
    endpoints: &Endpoints,
    open: &mut dyn FnMut(&str) -> std::io::Result<()>,
    timeout: Duration,
) -> Result<Tokens, AuthError> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| AuthError::SignIn(e.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|e| AuthError::SignIn(e.to_string()))?
        .port();
    let redirect = format!("http://127.0.0.1:{port}");
    let pkce = Pkce::new();
    let state = random_token();
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &client.client_id)
        .append_pair("redirect_uri", &redirect)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .finish();
    open(&format!("{}?{query}", endpoints.auth))
        .map_err(|e| AuthError::SignIn(format!("could not open the browser: {e}")))?;
    let code = wait_for_code(&listener, &state, timeout)?;
    let tokens = token_request(
        agent,
        &endpoints.token,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("code_verifier", &pkce.verifier),
            ("redirect_uri", &redirect),
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
        ],
    )?;
    if tokens.refresh_token.is_none() {
        return Err(AuthError::SignIn(
            "Google sent no refresh token, sign in again".to_owned(),
        ));
    }
    Ok(tokens)
}

/// A fresh access token for a stored refresh token. A revoked or expired refresh token is
/// [`AuthError::Expired`].
pub(crate) fn refresh(
    agent: &ureq::Agent,
    client: &Client,
    endpoints: &Endpoints,
    refresh_token: &str,
) -> Result<String, AuthError> {
    token_request(
        agent,
        &endpoints.token,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
        ],
    )
    .map(|t| t.access_token)
}

fn token_request(
    agent: &ureq::Agent,
    url: &str,
    form: &[(&str, &str)],
) -> Result<Tokens, AuthError> {
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    let mut response = agent
        .post(url)
        .content_type("application/x-www-form-urlencoded")
        .send(body)
        .map_err(|e| AuthError::Http(e.to_string()))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| AuthError::Http(e.to_string()))?;
    if status == 200 {
        return serde_json::from_str(&text).map_err(|e| AuthError::Http(e.to_string()));
    }
    #[derive(Deserialize)]
    struct Failure {
        error: String,
    }
    match serde_json::from_str::<Failure>(&text) {
        Ok(f) if f.error == "invalid_grant" => Err(AuthError::Expired),
        Ok(f) => Err(AuthError::Http(format!("{status} {}", f.error))),
        Err(_) => Err(AuthError::Http(format!("{status}"))),
    }
}

/// Accepts connections until one carries the redirect with our `state`, then answers it.
fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    timeout: Duration,
) -> Result<String, AuthError> {
    let failed = |e: std::io::Error| AuthError::SignIn(e.to_string());
    listener.set_nonblocking(true).map_err(failed)?;
    let deadline = Instant::now() + timeout;
    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err(AuthError::SignIn(
                        "the browser did not come back in time".to_owned(),
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(e) => return Err(failed(e)),
        };
        if let Some(result) = answer_redirect(stream, state) {
            return result;
        }
    }
}

/// Reads one request. `None` when it is not the redirect (a favicon request, a stray tab).
fn answer_redirect(mut stream: TcpStream, state: &str) -> Option<Result<String, AuthError>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).ok()?;
    let target = line.split_whitespace().nth(1)?;
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut code = None;
    let mut error = None;
    let mut returned_state = None;
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            "state" => returned_state = Some(value.into_owned()),
            _ => {}
        }
    }
    if code.is_none() && error.is_none() {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        return None;
    }
    let result = match (code, error) {
        _ if returned_state.as_deref() != Some(state) => Err(AuthError::SignIn(
            "the redirect carried the wrong state".to_owned(),
        )),
        (Some(code), None) => Ok(code),
        (_, Some(error)) => Err(AuthError::SignIn(error)),
        (None, None) => unreachable!("checked above"),
    };
    let page = match &result {
        Ok(_) => "Shot Studio is signed in to YouTube. You can close this tab.",
        Err(_) => "Shot Studio was not signed in to YouTube. You can close this tab.",
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_downloaded_client_file_and_explains_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.json");
        assert!(matches!(Client::load(&path), Err(AuthError::Setup(_))));
        let message = Client::load(&path).unwrap_err().to_string();
        assert!(message.contains("Desktop app") && message.contains("SHOT_YOUTUBE_CLIENT"));
        std::fs::write(
            &path,
            r#"{"installed":{"client_id":"id.apps.googleusercontent.com","project_id":"p","client_secret":"s","redirect_uris":["http://localhost"]}}"#,
        )
        .unwrap();
        assert_eq!(
            Client::load(&path).unwrap(),
            Client {
                client_id: "id.apps.googleusercontent.com".to_owned(),
                client_secret: "s".to_owned()
            }
        );
    }

    // Writes to the login Keychain of whoever runs it, under a test account it removes again.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn the_keychain_keeps_and_forgets_the_refresh_token() {
        let store = Keychain::new(&Client {
            client_id: "shot-ship-keychain-test".to_owned(),
            client_secret: String::new(),
        });
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.save("r1").unwrap();
        store.save("r2").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("r2"));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }
}
