use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "vumi";

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
            Error::BadResponse(describe(&e.without_url()))
        } else {
            Error::Unreachable(describe(&e.without_url()))
        }
    }
}

/// The error with every cause under it ("error sending request: operation
/// timed out"): the top line alone rarely says what went wrong. The URL is
/// already gone (it carries the login token); the causes below never have it.
fn describe(e: &reqwest::Error) -> String {
    let mut text = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Cheap to clone: everything inside is shared (Arc), nothing is copied.
#[derive(Clone)]
pub struct Client {
    base_url: Arc<str>,
    user: Arc<str>,
    password: Arc<str>,
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
    /// getOpenSubsonicExtensions: what the server adds to the Subsonic API.
    #[serde(rename = "openSubsonicExtensions")]
    open_subsonic_extensions: Option<Vec<Extension>>,
    pub status: String,
    pub version: String,
    #[serde(rename = "type")]
    pub server_type: Option<String>,
    pub server_version: Option<String>,
    pub error: Option<ApiError>,
    pub album_list2: Option<AlbumList>,
    pub search_result3: Option<SearchResult3>,
    pub similar_songs: Option<SimilarSongs>,
    pub album: Option<AlbumWithSongs>,
    pub album_info: Option<AlbumInfo>,
    pub artist: Option<ArtistWithAlbums>,
    pub artist_info2: Option<ArtistInfo>,
    pub artists: Option<ArtistIndexes>,
    pub playlists: Option<Playlists>,
    pub playlist: Option<PlaylistWithSongs>,
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
    pub created: Option<String>,
    pub artist_id: Option<String>,
    pub starred: Option<String>,
    pub cover_art: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumWithSongs {
    pub artist_id: Option<String>,
    pub starred: Option<String>,
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
#[derive(Clone)]
pub struct Song {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub track: Option<u32>,
    pub disc_number: Option<u32>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub artist_id: Option<String>,
    pub duration: Option<u32>,
    pub suffix: Option<String>,
    pub bit_rate: Option<u32>,
    pub cover_art: Option<String>,
    pub bit_depth: Option<u32>,
    pub sampling_rate: Option<u32>,
    pub starred: Option<String>,
    /// OpenSubsonic: the composer(s) as one display string.
    #[serde(rename = "displayComposer")]
    pub composer: Option<String>,
}

impl Client {
    pub fn new(base_url: &str, user: &str, password: &str) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .expect("failed to create HTTP client");
        Self {
            base_url: base_url.trim_end_matches('/').into(),
            user: user.into(),
            password: password.into(),
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
            ("u", self.user.to_string()),
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

    /// A list of albums the server ranks: `kind` is a getAlbumList2 type,
    /// e.g. "frequent" (most played) or "recent" (played last).
    pub async fn album_list_of(&self, kind: &str, size: u32) -> Result<Vec<Album>> {
        let response = self
            .call(
                "getAlbumList2",
                &[("type", kind.to_string()), ("size", size.to_string())],
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

    /// One page of songs from the whole library: search3 with an empty query
    /// lists everything (Navidrome and other OpenSubsonic servers support this).
    pub async fn song_page(&self, count: u32, offset: u32) -> Result<Vec<Song>> {
        let response = self
            .call(
                "search3",
                &[
                    ("query", String::new()),
                    ("artistCount", "0".to_string()),
                    ("albumCount", "0".to_string()),
                    ("songCount", count.to_string()),
                    ("songOffset", offset.to_string()),
                ],
            )
            .await?;
        Ok(response.search_result3.map(|r| r.song).unwrap_or_default())
    }

    /// Songs that sound like the song (or album, or artist) `id`. With the
    /// AudioMuse-AI plugin in Navidrome, the similarity comes from analysing the
    /// audio itself; without it, from whatever the server uses (e.g. Last.fm).
    pub async fn similar_songs(&self, id: &str, count: u32) -> Result<Vec<Song>> {
        let response = self
            .call(
                "getSimilarSongs",
                &[("id", id.to_string()), ("count", count.to_string())],
            )
            .await?;
        Ok(response.similar_songs.map(|s| s.song).unwrap_or_default())
    }

    /// Every song on the server, fetched in pages of 500.
    pub async fn all_songs(&self) -> Result<Vec<Song>> {
        const PAGE: u32 = 500;
        let mut all = Vec::new();
        loop {
            let page = self.song_page(PAGE, all.len() as u32).await?;
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

/// Something that can be added to the favourites on the server.
pub enum Starrable<'a> {
    Song(&'a str),
    Album(&'a str),
}

impl Client {
    /// Adds to (`starred` = true) or removes from the favourites on the server.
    pub async fn set_starred(&self, item: Starrable<'_>, starred: bool) -> Result<()> {
        let (key, id) = match item {
            Starrable::Song(id) => ("id", id),
            Starrable::Album(id) => ("albumId", id),
        };
        let endpoint = if starred { "star" } else { "unstar" };
        self.call(endpoint, &[(key, id.to_string())]).await?;
        Ok(())
    }

    /// Tells the server about a song being played: `submission` = false marks
    /// it as playing now, true records a finished listen (play count, history,
    /// Last.fm or ListenBrainz if the server forwards them). `time` is when the
    /// listen began, in milliseconds since the Unix epoch.
    pub async fn scrobble(&self, id: &str, submission: bool, time: Option<u64>) -> Result<()> {
        let mut params = vec![("id", id.to_string()), ("submission", submission.to_string())];
        if let Some(time) = time {
            params.push(("time", time.to_string()));
        }
        self.call("scrobble", &params).await?;
        Ok(())
    }

    /// Names of the OpenSubsonic extensions the server supports. A plain
    /// Subsonic server answers with an error: then it has none.
    pub async fn extensions(&self) -> Result<Vec<String>> {
        let response = self.call("getOpenSubsonicExtensions", &[]).await?;
        Ok(response
            .open_subsonic_extensions
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.name)
            .collect())
    }

    /// Live playback state for the server's "now playing" (OpenSubsonic
    /// playbackReport extension): `state` is "starting", "playing", "paused"
    /// or "stopped". Never counted as a listen: `scrobble` does that.
    pub async fn report_playback(&self, id: &str, state: &str, position_ms: u64) -> Result<()> {
        self.call(
            "reportPlayback",
            &[
                ("mediaId", id.to_string()),
                ("mediaType", "song".to_string()),
                ("positionMs", position_ms.to_string()),
                ("state", state.to_string()),
                ("ignoreScrobble", "true".to_string()),
            ],
        )
        .await?;
        Ok(())
    }
}

/// Extra album information the server gathers, e.g. from Last.fm.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AlbumInfo {
    /// HTML text, usually ending with a "Read more on Last.fm" link.
    pub notes: Option<String>,
    pub last_fm_url: Option<String>,
}

impl Client {
    pub async fn album_info(&self, id: &str) -> Result<AlbumInfo> {
        let response = self.call("getAlbumInfo2", &[("id", id.to_string())]).await?;
        Ok(response.album_info.unwrap_or_default())
    }
}

/// One artist as getArtists lists them: name, how many albums, and the id of
/// their picture (fetched like a cover).
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub album_count: Option<u32>,
    pub cover_art: Option<String>,
}

/// getArtists groups artists under index letters ("A", "B", ...).
#[derive(Deserialize)]
pub struct ArtistIndexes {
    #[serde(default)]
    pub index: Vec<ArtistIndex>,
}

#[derive(Deserialize)]
pub struct ArtistIndex {
    pub name: String,
    #[serde(default)]
    pub artist: Vec<Artist>,
}

impl Client {
    /// Every artist on the server, in the server's order (by index letter,
    /// then name).
    pub async fn artists(&self) -> Result<Vec<Artist>> {
        let response = self.call("getArtists", &[]).await?;
        Ok(response
            .artists
            .map(|indexes| indexes.index.into_iter().flat_map(|i| i.artist).collect())
            .unwrap_or_default())
    }
}

/// An artist with all of their albums.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistWithAlbums {
    pub id: String,
    pub name: String,
    pub cover_art: Option<String>,
    #[serde(default)]
    pub album: Vec<Album>,
}

/// Extra artist information the server gathers, e.g. from Last.fm.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ArtistInfo {
    /// HTML text, usually ending with a "Read more on Last.fm" link.
    pub biography: Option<String>,
}

impl Client {
    pub async fn artist(&self, id: &str) -> Result<ArtistWithAlbums> {
        let response = self.call("getArtist", &[("id", id.to_string())]).await?;
        response
            .artist
            .ok_or_else(|| Error::BadResponse("missing artist in response".into()))
    }

    pub async fn artist_info(&self, id: &str) -> Result<ArtistInfo> {
        let response = self.call("getArtistInfo2", &[("id", id.to_string())]).await?;
        Ok(response.artist_info2.unwrap_or_default())
    }
}

/// Songs found by search3; with an empty query, a page of the whole library.
#[derive(Deserialize)]
pub struct SearchResult3 {
    #[serde(default)]
    pub song: Vec<Song>,
}

/// Songs that sound like a given song, album or artist (getSimilarSongs).
#[derive(Deserialize)]
pub struct SimilarSongs {
    #[serde(default)]
    pub song: Vec<Song>,
}

/// One playlist as getPlaylists lists them.
#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub owner: Option<String>,
    pub public: Option<bool>,
    pub song_count: Option<u32>,
    pub duration: Option<u32>,
    pub created: Option<String>,
    pub changed: Option<String>,
    pub cover_art: Option<String>,
}

#[derive(Deserialize)]
pub struct Playlists {
    #[serde(default)]
    pub playlist: Vec<Playlist>,
}

/// A playlist with its songs (getPlaylist).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistWithSongs {
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub song_count: Option<u32>,
    pub duration: Option<u32>,
    pub cover_art: Option<String>,
    #[serde(default)]
    pub entry: Vec<Song>,
}

impl Client {
    /// Every playlist the user can see.
    pub async fn playlists(&self) -> Result<Vec<Playlist>> {
        let response = self.call("getPlaylists", &[]).await?;
        Ok(response.playlists.map(|p| p.playlist).unwrap_or_default())
    }

    /// One playlist with its songs, in playlist order.
    pub async fn playlist(&self, id: &str) -> Result<PlaylistWithSongs> {
        let response = self.call("getPlaylist", &[("id", id.to_string())]).await?;
        response
            .playlist
            .ok_or_else(|| Error::BadResponse("missing playlist in response".into()))
    }

    /// Creates a playlist with the given songs; returns it.
    pub async fn create_playlist(&self, name: &str, song_ids: &[&str]) -> Result<PlaylistWithSongs> {
        let mut params = vec![("name", name.to_string())];
        params.extend(song_ids.iter().map(|id| ("songId", id.to_string())));
        let response = self.call("createPlaylist", &params).await?;
        response
            .playlist
            .ok_or_else(|| Error::BadResponse("missing playlist in response".into()))
    }

    /// Appends songs to a playlist.
    pub async fn add_to_playlist(&self, id: &str, song_ids: &[&str]) -> Result<()> {
        let mut params = vec![("playlistId", id.to_string())];
        params.extend(song_ids.iter().map(|id| ("songIdToAdd", id.to_string())));
        self.call("updatePlaylist", &params).await?;
        Ok(())
    }
}

/// One entry of getOpenSubsonicExtensions.
#[derive(Debug, serde::Deserialize)]
struct Extension {
    name: String,
}
