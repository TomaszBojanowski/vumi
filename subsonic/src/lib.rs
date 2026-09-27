use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "muse-player";

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
pub struct Response {
    pub status: String,
    pub version: String,
    #[serde(rename = "type")]
    pub server_type: Option<String>,
    #[serde(rename = "serverVersion")]
    pub server_version: Option<String>,
    pub error: Option<ApiError>,
}

#[derive(Deserialize)]
pub struct ApiError {
    pub code: i32,
    pub message: String,
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

    pub fn ping(&self) -> Result<Response, Box<dyn std::error::Error>> {
        let url = reqwest::Url::parse_with_params(
            &format!("{}/rest/ping", self.base_url),
            &self.auth_params(),
        )?;
        let envelope: Envelope = self.http.get(url).send()?.error_for_status()?.json()?;
        Ok(envelope.response)
    }
}
