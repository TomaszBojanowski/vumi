use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "muse-player";

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub struct Client {
    base_url: String,
    user: String,
    password: String,
    http: reqwest::blocking::Client,
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
pub struct Album {
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub year: Option<i32>,
    pub song_count: Option<u32>,
}

impl Client {
    pub fn new(base_url: &str, user: &str, password: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            user: user.to_string(),
            password: password.to_string(),
            http: reqwest::blocking::Client::new(),
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

    /// Calls a Subsonic endpoint and turns a non-"ok" status into an error.
    fn call(&self, endpoint: &str, extra: &[(&'static str, String)]) -> Result<Response> {
        let mut params = self.auth_params();
        params.extend_from_slice(extra);
        let url = reqwest::Url::parse_with_params(
            &format!("{}/rest/{}", self.base_url, endpoint),
            &params,
        )?;
        let envelope: Envelope = self.http.get(url).send()?.error_for_status()?.json()?;
        let response = envelope.response;
        if response.status != "ok" {
            let msg = match &response.error {
                Some(e) => format!("server error {}: {}", e.code, e.message),
                None => format!("unexpected status: {}", response.status),
            };
            return Err(msg.into());
        }
        Ok(response)
    }

    pub fn ping(&self) -> Result<Response> {
        self.call("ping", &[])
    }

    /// Albums sorted by name; `size` is how many, `offset` is where to start.
    pub fn album_list(&self, size: u32, offset: u32) -> Result<Vec<Album>> {
        let response = self.call(
            "getAlbumList2",
            &[
                ("type", "alphabeticalByName".to_string()),
                ("size", size.to_string()),
                ("offset", offset.to_string()),
            ],
        )?;
        Ok(response.album_list2.map(|l| l.album).unwrap_or_default())
    }
}
