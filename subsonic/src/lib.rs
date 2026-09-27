use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "muse-player";

/// Subsonic error code for a wrong username or password.
const WRONG_CREDENTIALS: i32 = 40;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("wrong username or password")]
    WrongCredentials,
    #[error("server error {code}: {message}")]
    Server { code: i32, message: String },
    #[error("cannot reach the server: {0}")]
    Unreachable(String),
    #[error("unexpected response from the server: {0}")]
    BadResponse(String),
    #[error("invalid server address: {0}")]
    InvalidAddress(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        if let Some(status) = e.status() {
            Error::BadResponse(format!("HTTP {status}"))
        } else if e.is_decode() {
            Error::BadResponse(e.without_url().to_string())
        } else {
            Error::Unreachable(e.without_url().to_string())
        }
    }
}

/// Cheap to clone: the HTTP client inside is shared.
#[derive(Clone)]
pub struct Client {
    base_url: String,
    user: String,
    password: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "subsonic-response")]
    response: Response,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub status: String,
    pub version: String,
    #[serde(rename = "type")]
    pub server_type: Option<String>,
    pub server_version: Option<String>,
    pub error: Option<ApiError>,
    pub album_list2: Option<AlbumList>,
    pub album: Option<AlbumWithSongs>,
}

#[derive(Deserialize)]
pub struct ApiError {
    pub code: i32,
    pub message: String,
}

#[derive(Deserialize)]
pub struct AlbumList {
    #[serde(default)]
    pub album: Vec<Album>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(Clone, PartialEq)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub year: Option<i32>,
    pub song_count: Option<u32>,
    pub cover_art: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumWithSongs {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    #[serde(default)]
    pub song: Vec<Song>,
    #[serde(default)]
    pub disc_titles: Vec<DiscTitle>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub track: Option<u32>,
    pub disc_number: Option<u32>,
    pub duration: Option<u32>,
    pub suffix: Option<String>,
    pub bit_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub sampling_rate: Option<u32>,
}

impl Client {
    pub fn new(base_url: &str, user: &str, password: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .expect("failed to create HTTP client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            user: user.to_string(),
            password: password.to_string(),
            http,
        }
    }

    /// Subsonic token auth: t = md5(password + salt), so the password never goes over the wire.
    fn auth_params(&self) -> Vec<(&'static str, String)> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let salt = format!("{nanos:x}");
        let token = format!("{:x}", md5::compute(format!("{}{}", self.password, salt)));
        vec![
            ("u", self.user.clone()),
            ("t", token),
            ("s", salt),
            ("v", API_VERSION.to_string()),
            ("c", CLIENT_NAME.to_string()),
            ("f", "json".to_string()),
        ]
    }

    /// Builds a full endpoint URL with auth and extra query parameters.
    fn build_url(&self, endpoint: &str, extra: &[(&'static str, String)]) -> Result<reqwest::Url> {
        let mut params = self.auth_params();
        params.extend_from_slice(extra);
        reqwest::Url::parse_with_params(&format!("{}/rest/{}", self.base_url, endpoint), &params)
            .map_err(|e| Error::InvalidAddress(e.to_string()))
    }

    /// Calls a Subsonic endpoint and turns a non-"ok" status into a typed error.
    async fn call(&self, endpoint: &str, extra: &[(&'static str, String)]) -> Result<Response> {
        let url = self.build_url(endpoint, extra)?;
        let envelope: Envelope = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let response = envelope.response;
        if response.status == "ok" {
            return Ok(response);
        }
        Err(match response.error {
            Some(e) if e.code == WRONG_CREDENTIALS => Error::WrongCredentials,
            Some(e) => Error::Server {
                code: e.code,
                message: e.message,
            },
            None => Error::BadResponse(format!("status {}", response.status)),
        })
    }

    pub async fn ping(&self) -> Result<Response> {
        self.call("ping", &[]).await
    }

    /// Albums sorted by name; `size` is how many, `offset` is where to start.
    pub async fn album_list(&self, size: u32, offset: u32) -> Result<Vec<Album>> {
        let response = self
            .call(
                "getAlbumList2",
                &[
                    ("type", "alphabeticalByName".to_string()),
                    ("size", size.to_string()),
                    ("offset", offset.to_string()),
                ],
            )
            .await?;
        Ok(response.album_list2.map(|l| l.album).unwrap_or_default())
    }

    /// One album with its track list.
    pub async fn album(&self, id: &str) -> Result<AlbumWithSongs> {
        let response = self.call("getAlbum", &[("id", id.to_string())]).await?;
        response
            .album
            .ok_or_else(|| Error::BadResponse("missing album in response".into()))
    }

    /// Stream URL for a song; format=raw asks for the original file, no transcoding.
    /// No network call here, so it stays synchronous.
    pub fn stream_url(&self, song_id: &str) -> Result<reqwest::Url> {
        self.build_url(
            "stream",
            &[("id", song_id.to_string()), ("format", "raw".to_string())],
        )
    }
}

impl Client {
    /// Every album on the server, fetched in pages of 500 (the Subsonic maximum).
    pub async fn all_albums(&self) -> Result<Vec<Album>> {
        const PAGE: u32 = 500;
        let mut all = Vec::new();
        loop {
            let page = self.album_list(PAGE, all.len() as u32).await?;
            let done = page.len() < PAGE as usize;
            all.extend(page);
            if done {
                return Ok(all);
            }
        }
    }
}

impl Client {
    /// Cover image bytes, scaled by the server so the longer side is `size` pixels.
    pub async fn cover_art(&self, id: &str, size: u32) -> Result<Vec<u8>> {
        let url = self.build_url(
            "getCoverArt",
            &[("id", id.to_string()), ("size", size.to_string())],
        )?;
        let bytes = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok(bytes.to_vec())
    }
}

/// Identifies one account on one server, e.g. for local caches: "user@url".
pub fn server_key(base_url: &str, user: &str) -> String {
    format!("{}@{}", user, base_url.trim_end_matches('/'))
}

impl Client {
    pub fn server_key(&self) -> String {
        server_key(&self.base_url, &self.user)
    }
}

/// Name of one disc of a multi-disc album (OpenSubsonic).
#[derive(Deserialize, Clone)]
pub struct DiscTitle {
    pub disc: u32,
    pub title: String,
}
