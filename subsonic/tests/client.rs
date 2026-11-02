//! Tests for the API client against a local mock HTTP server (wiremock): what
//! each call sends, and how the client reads what comes back. The responses in
//! `fixtures/` are written in the shape Navidrome sends, extra fields included.

use std::collections::HashMap;

use serde_json::{Value, json};
use subsonic::{Client, Error, Starrable};
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Match, Mock, MockBuilder, MockServer, Request, ResponseTemplate};

const USER: &str = "anna";
const PASSWORD: &str = "sesame";

/// A response from `fixtures/`, sent as JSON.
macro_rules! fixture {
    ($name:literal) => {
        ResponseTemplate::new(200).set_body_raw(
            include_str!(concat!("fixtures/", $name, ".json")),
            "application/json",
        )
    };
}

/// A mock server and a client that talks to it.
async fn start() -> (MockServer, Client) {
    let server = MockServer::start().await;
    let client = Client::new(&server.uri(), USER, PASSWORD);
    (server, client)
}

/// A GET of the endpoint `name`, signed with the test user's credentials.
/// A request that is not signed correctly gets no answer from this mock.
fn endpoint(name: &str) -> MockBuilder {
    Mock::given(method("GET"))
        .and(path(format!("/rest/{name}")))
        .and(SignedBy::new(USER, PASSWORD))
}

/// A successful response as Navidrome sends it, with `fields` added.
fn ok_with(fields: Value) -> ResponseTemplate {
    let mut response = json!({
        "status": "ok",
        "version": "1.16.1",
        "type": "navidrome",
        "serverVersion": "0.58.0 (13a8af6b)",
        "openSubsonic": true,
    });
    let fields = fields.as_object().expect("fields must be a JSON object");
    response.as_object_mut().unwrap().extend(fields.clone());
    ResponseTemplate::new(200).set_body_json(json!({ "subsonic-response": response }))
}

/// A failed response: Subsonic sends errors with HTTP 200, inside the body.
fn failed(code: i32, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "subsonic-response": {
            "status": "failed",
            "version": "1.16.1",
            "type": "navidrome",
            "serverVersion": "0.58.0 (13a8af6b)",
            "openSubsonic": true,
            "error": { "code": code, "message": message },
        }
    }))
}

/// A raw body with HTTP 200, for responses that are not what the client expects.
fn raw(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.to_owned(), "application/json")
}

/// The error of a call that must fail. The client's types have no Debug, so
/// `unwrap_err` is not available.
fn error<T>(result: subsonic::Result<T>) -> Error {
    match result {
        Ok(_) => panic!("the call succeeded, but an error was expected"),
        Err(e) => e,
    }
}

/// The query parameters of a URL; for a repeated key, the last value.
fn params(url: &reqwest::Url) -> HashMap<String, String> {
    url.query_pairs().into_owned().collect()
}

/// Every value of a query parameter, in order.
fn all_values(url: &reqwest::Url, key: &str) -> Vec<String> {
    url.query_pairs()
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .collect()
}

/// The URLs of all requests the server has received, in order.
async fn received(server: &MockServer) -> Vec<reqwest::Url> {
    let requests = server.received_requests().await.expect("recording is on");
    requests.into_iter().map(|r| r.url).collect()
}

/// Checks Subsonic token auth: `t` = md5(password + `s`), the fixed client
/// parameters, and no password anywhere. The error says what is wrong.
fn check_auth(url: &reqwest::Url, user: &str, password: &str) -> Result<(), String> {
    let params = params(url);
    let get = |key: &str| params.get(key).map(String::as_str);
    if get("u") != Some(user) {
        return Err(format!("u is {:?}, expected {user:?}", get("u")));
    }
    let salt = get("s").ok_or("no salt (s)")?;
    // The Subsonic documentation asks for a salt of at least six characters.
    if salt.len() < 6 {
        return Err(format!("salt {salt:?} is shorter than six characters"));
    }
    let token = get("t").ok_or("no token (t)")?;
    let expected = format!("{:x}", md5::compute(format!("{password}{salt}")));
    if token != expected {
        return Err(format!(
            "token {token:?} is not md5(password + salt) = {expected:?}"
        ));
    }
    for (key, value) in [("v", "1.16.1"), ("c", "vumi"), ("f", "json")] {
        if get(key) != Some(value) {
            return Err(format!("{key} is {:?}, expected {value:?}", get(key)));
        }
    }
    if params.contains_key("p") {
        return Err("the password is sent as p".into());
    }
    if params.values().any(|value| value.contains(password)) {
        return Err("the password is in the query".into());
    }
    Ok(())
}

/// Matches only requests signed with token auth for these credentials.
struct SignedBy {
    user: String,
    password: String,
}

impl SignedBy {
    fn new(user: &str, password: &str) -> Self {
        Self {
            user: user.into(),
            password: password.into(),
        }
    }
}

impl Match for SignedBy {
    fn matches(&self, request: &Request) -> bool {
        check_auth(&request.url, &self.user, &self.password).is_ok()
    }
}

fn ids<T>(items: &[T], id: impl Fn(&T) -> &str) -> Vec<&str> {
    items.iter().map(id).collect()
}

mod auth {
    use super::*;

    #[tokio::test]
    async fn every_request_is_signed_with_a_salted_token() {
        let (server, client) = start().await;
        Mock::given(method("GET"))
            .and(path("/rest/ping"))
            .respond_with(fixture!("ping"))
            .expect(1)
            .mount(&server)
            .await;

        client.ping().await.unwrap();

        let url = &received(&server).await[0];
        let params = params(url);
        assert_eq!(params["u"], USER);
        assert!(
            params["s"].len() >= 6,
            "salt {:?} is too short",
            params["s"]
        );
        let expected = format!("{:x}", md5::compute(format!("{PASSWORD}{}", params["s"])));
        assert_eq!(params["t"], expected, "t must be md5(password + salt)");
        assert_eq!(params["v"], "1.16.1");
        assert_eq!(params["c"], "vumi");
        assert_eq!(params["f"], "json");
        assert!(!params.contains_key("p"), "the password must not be sent");
        assert!(!url.as_str().contains(PASSWORD), "the password is in {url}");
    }

    #[tokio::test]
    async fn the_salt_is_new_for_every_request() {
        let (server, client) = start().await;
        endpoint("ping")
            .respond_with(fixture!("ping"))
            .expect(2)
            .mount(&server)
            .await;

        client.ping().await.unwrap();
        client.ping().await.unwrap();

        let urls = received(&server).await;
        let (first, second) = (params(&urls[0]), params(&urls[1]));
        assert_ne!(first["s"], second["s"], "the salt was reused");
        assert_ne!(first["t"], second["t"], "the token was reused");
    }

    #[tokio::test]
    async fn the_salt_is_128_random_bits() {
        let (server, client) = start().await;
        endpoint("ping")
            .respond_with(fixture!("ping"))
            .expect(1)
            .mount(&server)
            .await;

        client.ping().await.unwrap();

        let salt = params(&received(&server).await[0])["s"].clone();
        assert_eq!(salt.len(), 32, "salt {salt:?}");
        assert!(salt.chars().all(|c| c.is_ascii_hexdigit()), "salt {salt:?}");
    }

    #[tokio::test]
    async fn credentials_with_special_characters_are_encoded() {
        let server = MockServer::start().await;
        let (user, password) = ("anna maria+1&co", "pä ss&wörd=?#%");
        Mock::given(method("GET"))
            .and(path("/rest/ping"))
            .and(SignedBy::new(user, password))
            .respond_with(fixture!("ping"))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::new(&server.uri(), user, password);
        client.ping().await.unwrap();
    }

    #[tokio::test]
    async fn calls_go_to_rest_under_the_base_address() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/music/rest/ping"))
            .and(SignedBy::new(USER, PASSWORD))
            .respond_with(fixture!("ping"))
            .expect(1)
            .mount(&server)
            .await;

        // A path prefix (a reverse proxy) and trailing slashes.
        let client = Client::new(&format!("{}/music//", server.uri()), USER, PASSWORD);
        client.ping().await.unwrap();
    }

    #[test]
    fn the_stream_url_is_signed_and_asks_for_the_original_file() {
        let client = Client::new("https://music.example.org/", USER, PASSWORD);
        let url = client.stream_url("song-1").unwrap();

        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("music.example.org"));
        assert_eq!(url.path(), "/rest/stream");
        check_auth(&url, USER, PASSWORD).unwrap();
        let params = params(&url);
        assert_eq!(params["id"], "song-1");
        assert_eq!(params["format"], "raw");
    }

    #[test]
    fn the_server_key_is_user_at_address_without_trailing_slashes() {
        let client = Client::new("https://music.example.org//", USER, PASSWORD);
        assert_eq!(client.server_key(), "anna@https://music.example.org");
        assert_eq!(
            subsonic::server_key("https://music.example.org/", USER),
            client.server_key()
        );
    }

    #[tokio::test]
    async fn an_address_without_a_scheme_is_invalid() {
        let client = Client::new("music.example.org", USER, PASSWORD);
        let e = error(client.ping().await);
        assert!(matches!(e, Error::InvalidAddress(_)), "got {e:?}");
        let e = error(client.stream_url("song-1"));
        assert!(matches!(e, Error::InvalidAddress(_)), "got {e:?}");
    }

    /// "localhost:4533" is what many people type for Navidrome. It parses as
    /// a URL with the scheme "localhost"; only http and https are taken, so
    /// it is an invalid address, not an unreachable server.
    #[tokio::test]
    async fn an_address_with_a_port_but_no_scheme_is_invalid() {
        let client = Client::new("localhost:4533", USER, PASSWORD);
        let e = error(client.ping().await);
        assert!(matches!(e, Error::InvalidAddress(_)), "got {e:?}");
        let e = error(client.stream_url("song-1"));
        assert!(matches!(e, Error::InvalidAddress(_)), "got {e:?}");
    }
}

mod ping {
    use super::*;

    #[tokio::test]
    async fn ping_reports_the_server_and_its_version() {
        let (server, client) = start().await;
        endpoint("ping")
            .respond_with(fixture!("ping"))
            .expect(1)
            .mount(&server)
            .await;

        let response = client.ping().await.unwrap();

        assert_eq!(response.status, "ok");
        assert_eq!(response.version, "1.16.1");
        assert_eq!(response.server_type.as_deref(), Some("navidrome"));
        assert_eq!(
            response.server_version.as_deref(),
            Some("0.58.0 (13a8af6b)")
        );
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn ping_accepts_a_plain_subsonic_server() {
        let (server, client) = start().await;
        endpoint("ping")
            .respond_with(raw(
                r#"{"subsonic-response": {"status": "ok", "version": "1.15.0"}}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let response = client.ping().await.unwrap();

        assert_eq!(response.version, "1.15.0");
        assert!(response.server_type.is_none());
        assert!(response.server_version.is_none());
    }
}

mod albums {
    use super::*;

    /// A page of albums named after their ids.
    fn album_page(ids: impl IntoIterator<Item = String>) -> ResponseTemplate {
        let albums: Vec<Value> = ids
            .into_iter()
            .map(|id| json!({ "id": id, "name": id, "songCount": 1, "duration": 1, "created": "" }))
            .collect();
        ok_with(json!({ "albumList2": { "album": albums } }))
    }

    #[tokio::test]
    async fn album_list_asks_for_a_page_by_name_and_reads_the_albums() {
        let (server, client) = start().await;
        endpoint("getAlbumList2")
            .and(query_param("type", "alphabeticalByName"))
            .and(query_param("size", "10"))
            .and(query_param("offset", "20"))
            .respond_with(fixture!("getAlbumList2"))
            .expect(1)
            .mount(&server)
            .await;

        let albums = client.album_list(10, 20).await.unwrap();

        assert_eq!(ids(&albums, |a| &a.id), ["album-1", "album-2"]);
        let full = &albums[0];
        assert_eq!(full.name, "Low Tide");
        assert_eq!(full.artist.as_deref(), Some("The Nightjars"));
        assert_eq!(full.artist_id.as_deref(), Some("artist-1"));
        assert_eq!(full.year, Some(2019));
        assert_eq!(full.song_count, Some(12));
        assert_eq!(full.created.as_deref(), Some("2025-11-03T10:15:42.123Z"));
        assert_eq!(full.starred.as_deref(), Some("2026-01-15T08:00:00Z"));
        assert_eq!(full.cover_art.as_deref(), Some("al-album-1_65f2a1c4"));
        // Fields a server may leave out.
        let sparse = &albums[1];
        assert_eq!(sparse.name, "Field Recordings");
        assert!(sparse.artist.is_none());
        assert!(sparse.artist_id.is_none());
        assert!(sparse.year.is_none());
        assert!(sparse.starred.is_none());
        assert!(sparse.cover_art.is_none());
        assert_eq!(sparse.song_count, Some(3));
    }

    #[tokio::test]
    async fn an_empty_or_missing_album_list_is_empty() {
        let (server, client) = start().await;
        // Navidrome sends an empty object when there is nothing to list.
        endpoint("getAlbumList2")
            .respond_with(ok_with(json!({ "albumList2": {} })))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        endpoint("getAlbumList2")
            .respond_with(ok_with(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        assert!(client.album_list(10, 0).await.unwrap().is_empty());
        assert!(client.album_list(10, 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn album_reads_tracks_discs_and_audio_details() {
        let (server, client) = start().await;
        endpoint("getAlbum")
            .and(query_param("id", "album-1"))
            .respond_with(fixture!("getAlbum"))
            .expect(1)
            .mount(&server)
            .await;

        let album = client.album("album-1").await.unwrap();

        assert_eq!(album.id, "album-1");
        assert_eq!(album.name, "Low Tide");
        assert_eq!(album.artist.as_deref(), Some("The Nightjars"));
        assert_eq!(album.artist_id.as_deref(), Some("artist-1"));
        assert_eq!(album.starred.as_deref(), Some("2026-01-15T08:00:00Z"));
        let discs: Vec<(u32, &str)> = album
            .disc_titles
            .iter()
            .map(|d| (d.disc, d.title.as_str()))
            .collect();
        assert_eq!(discs, [(1, "Studio"), (2, "Live at the Harbour")]);

        assert_eq!(ids(&album.song, |s| &s.id), ["song-1", "song-2", "song-3"]);
        let flac = &album.song[0];
        assert_eq!(flac.title, "Undertow");
        assert_eq!(flac.artist.as_deref(), Some("The Nightjars"));
        assert_eq!(flac.album.as_deref(), Some("Low Tide"));
        assert_eq!(flac.album_id.as_deref(), Some("album-1"));
        assert_eq!(flac.artist_id.as_deref(), Some("artist-1"));
        assert_eq!((flac.disc_number, flac.track), (Some(1), Some(1)));
        assert_eq!(flac.duration, Some(245));
        assert_eq!(flac.suffix.as_deref(), Some("flac"));
        assert_eq!(flac.bit_rate, Some(1571));
        assert_eq!(flac.bit_depth, Some(24));
        assert_eq!(flac.sampling_rate, Some(96000));
        assert_eq!(flac.cover_art.as_deref(), Some("mf-song-1_65f2a1c4"));
        assert_eq!(flac.starred.as_deref(), Some("2026-02-01T21:30:00Z"));
        assert_eq!(flac.composer.as_deref(), Some("Mara Quill • Ivo Penn"));
        let mp3 = &album.song[1];
        assert_eq!(mp3.bit_rate, Some(320));
        assert!(mp3.bit_depth.is_none());
        assert!(mp3.starred.is_none());
        let live = &album.song[2];
        assert_eq!((live.disc_number, live.track), (Some(2), Some(1)));
    }

    #[tokio::test]
    async fn album_info_reads_the_notes_and_the_last_fm_link() {
        let (server, client) = start().await;
        endpoint("getAlbumInfo2")
            .and(query_param("id", "album-1"))
            .respond_with(ok_with(json!({
                "albumInfo": {
                    "notes": "A record about the sea. <a href=\"https://www.last.fm/music/The+Nightjars/Low+Tide\">Read more on Last.fm</a>",
                    "musicBrainzId": "8e1d3b52-2f0b-4c5e-9d6a-1f0c2b7e5a11",
                    "lastFmUrl": "https://www.last.fm/music/The+Nightjars/Low+Tide",
                    "smallImageUrl": "https://music.example.org/share/img/al-album-1?size=300",
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let info = client.album_info("album-1").await.unwrap();

        assert!(info.notes.unwrap().starts_with("A record about the sea."));
    }

    #[tokio::test]
    async fn all_albums_reads_pages_until_a_short_one_and_drops_repeats() {
        let (server, client) = start().await;
        endpoint("getAlbumList2")
            .and(query_param("size", "500"))
            .and(query_param("offset", "0"))
            .respond_with(album_page((0..500).map(|i| format!("album-{i}"))))
            .expect(1)
            .mount(&server)
            .await;
        // The library changed between the pages: album-499 comes again.
        endpoint("getAlbumList2")
            .and(query_param("size", "500"))
            .and(query_param("offset", "500"))
            .respond_with(album_page(
                ["album-499", "album-500", "album-501"].map(String::from),
            ))
            .expect(1)
            .mount(&server)
            .await;

        let albums = client.all_albums().await.unwrap();

        assert_eq!(albums.len(), 502);
        let expected: Vec<String> = (0..502).map(|i| format!("album-{i}")).collect();
        assert_eq!(ids(&albums, |a| &a.id), expected);
    }

    #[tokio::test]
    async fn all_albums_stops_at_an_empty_page() {
        let (server, client) = start().await;
        endpoint("getAlbumList2")
            .and(query_param("offset", "0"))
            .respond_with(album_page((0..500).map(|i| format!("album-{i}"))))
            .expect(1)
            .mount(&server)
            .await;
        endpoint("getAlbumList2")
            .and(query_param("offset", "500"))
            .respond_with(ok_with(json!({ "albumList2": {} })))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(client.all_albums().await.unwrap().len(), 500);
    }
}

mod artists {
    use super::*;

    #[tokio::test]
    async fn artists_come_from_every_index_in_order() {
        let (server, client) = start().await;
        endpoint("getArtists")
            .respond_with(fixture!("getArtists"))
            .expect(1)
            .mount(&server)
            .await;

        let artists = client.artists().await.unwrap();

        assert_eq!(
            ids(&artists, |a| &a.id),
            ["artist-2", "artist-3", "artist-1"]
        );
        let names: Vec<&str> = artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Aurora Lane", "Ayla", "The Nightjars"]);
        assert_eq!(artists[0].album_count, Some(2));
        assert_eq!(artists[0].cover_art.as_deref(), Some("ar-artist-2_0"));
        assert!(artists[1].album_count.is_none());
        assert!(artists[1].cover_art.is_none());
        assert_eq!(artists[2].album_count, Some(4));
    }

    #[tokio::test]
    async fn no_artists_is_an_empty_list() {
        let (server, client) = start().await;
        endpoint("getArtists")
            .respond_with(ok_with(json!({ "artists": { "ignoredArticles": "The" } })))
            .expect(1)
            .mount(&server)
            .await;

        assert!(client.artists().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn artist_reads_the_artist_and_their_albums() {
        let (server, client) = start().await;
        endpoint("getArtist")
            .and(query_param("id", "artist-1"))
            .respond_with(fixture!("getArtist"))
            .expect(1)
            .mount(&server)
            .await;

        let artist = client.artist("artist-1").await.unwrap();

        assert_eq!(artist.id, "artist-1");
        assert_eq!(artist.name, "The Nightjars");
        assert_eq!(artist.cover_art.as_deref(), Some("ar-artist-1_0"));
        assert_eq!(ids(&artist.album, |a| &a.id), ["album-1", "album-4"]);
        assert_eq!(artist.album[1].name, "Night Ferry");
        assert_eq!(artist.album[1].year, Some(2023));
    }

    #[tokio::test]
    async fn artist_info_reads_the_biography() {
        let (server, client) = start().await;
        endpoint("getArtistInfo2")
            .and(query_param("id", "artist-1"))
            .respond_with(ok_with(json!({
                "artistInfo2": {
                    "biography": "A band from the coast.",
                    "musicBrainzId": "",
                    "smallImageUrl": "https://music.example.org/share/img/ar-artist-1?size=300",
                    "similarArtist": [{ "id": "artist-2", "name": "Aurora Lane" }],
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let info = client.artist_info("artist-1").await.unwrap();

        assert_eq!(info.biography.as_deref(), Some("A band from the coast."));
    }

    #[tokio::test]
    async fn missing_artist_info_is_empty() {
        let (server, client) = start().await;
        endpoint("getArtistInfo2")
            .respond_with(ok_with(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        assert!(
            client
                .artist_info("artist-1")
                .await
                .unwrap()
                .biography
                .is_none()
        );
    }
}

mod songs {
    use super::*;

    /// A page of songs named after their ids.
    fn song_page(ids: impl IntoIterator<Item = String>) -> ResponseTemplate {
        let songs: Vec<Value> = ids
            .into_iter()
            .map(|id| json!({ "id": id, "title": id, "isDir": false }))
            .collect();
        ok_with(json!({ "searchResult3": { "song": songs } }))
    }

    #[tokio::test]
    async fn song_page_lists_the_library_with_an_empty_search() {
        let (server, client) = start().await;
        endpoint("search3")
            .and(query_param("query", ""))
            .and(query_param("artistCount", "0"))
            .and(query_param("albumCount", "0"))
            .and(query_param("songCount", "100"))
            .and(query_param("songOffset", "200"))
            .respond_with(fixture!("search3"))
            .expect(1)
            .mount(&server)
            .await;

        let songs = client.song_page(100, 200).await.unwrap();

        assert_eq!(ids(&songs, |s| &s.id), ["song-7", "song-8"]);
        let full = &songs[0];
        assert_eq!(full.title, "Night Ferry");
        assert_eq!(full.album.as_deref(), Some("Night Ferry"));
        assert_eq!(full.suffix.as_deref(), Some("opus"));
        assert_eq!(full.sampling_rate, Some(48000));
        assert_eq!(full.track, Some(3));
        // Only id and title are required.
        let sparse = &songs[1];
        assert_eq!(sparse.title, "Untitled Demo");
        assert!(sparse.artist.is_none());
        assert!(sparse.album_id.is_none());
        assert!(sparse.duration.is_none());
        assert!(sparse.track.is_none());
        assert!(sparse.composer.is_none());
    }

    #[tokio::test]
    async fn an_empty_search_result_is_empty() {
        let (server, client) = start().await;
        endpoint("search3")
            .respond_with(ok_with(json!({ "searchResult3": {} })))
            .expect(1)
            .mount(&server)
            .await;

        assert!(client.song_page(100, 0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn all_songs_reads_pages_by_song_offset_and_drops_repeats() {
        let (server, client) = start().await;
        endpoint("search3")
            .and(query_param("songCount", "500"))
            .and(query_param("songOffset", "0"))
            .respond_with(song_page((0..500).map(|i| format!("song-{i}"))))
            .expect(1)
            .mount(&server)
            .await;
        endpoint("search3")
            .and(query_param("songCount", "500"))
            .and(query_param("songOffset", "500"))
            .respond_with(song_page(["song-0", "song-500"].map(String::from)))
            .expect(1)
            .mount(&server)
            .await;

        let songs = client.all_songs().await.unwrap();

        let expected: Vec<String> = (0..501).map(|i| format!("song-{i}")).collect();
        assert_eq!(ids(&songs, |s| &s.id), expected);
    }

    #[tokio::test]
    async fn similar_songs_asks_for_a_count_and_reads_the_songs() {
        let (server, client) = start().await;
        endpoint("getSimilarSongs")
            .and(query_param("id", "song-1"))
            .and(query_param("count", "20"))
            .respond_with(ok_with(json!({
                "similarSongs": {
                    "song": [
                        { "id": "song-7", "title": "Night Ferry", "artist": "The Nightjars" },
                        { "id": "song-12", "title": "Glasswater", "artist": "Aurora Lane" },
                    ]
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let songs = client.similar_songs("song-1", 20).await.unwrap();

        assert_eq!(ids(&songs, |s| &s.id), ["song-7", "song-12"]);
        assert_eq!(songs[1].artist.as_deref(), Some("Aurora Lane"));
    }

    #[tokio::test]
    async fn no_similar_songs_is_an_empty_list() {
        let (server, client) = start().await;
        endpoint("getSimilarSongs")
            .respond_with(ok_with(json!({ "similarSongs": {} })))
            .expect(1)
            .mount(&server)
            .await;

        assert!(client.similar_songs("song-1", 20).await.unwrap().is_empty());
    }
}

mod playlists {
    use super::*;

    #[tokio::test]
    async fn playlists_are_read_with_their_details() {
        let (server, client) = start().await;
        endpoint("getPlaylists")
            .respond_with(fixture!("getPlaylists"))
            .expect(1)
            .mount(&server)
            .await;

        let playlists = client.playlists().await.unwrap();

        assert_eq!(ids(&playlists, |p| &p.id), ["playlist-1", "playlist-2"]);
        let full = &playlists[0];
        assert_eq!(full.name, "Sunday Morning");
        assert_eq!(full.comment.as_deref(), Some("Slow and quiet"));
        assert_eq!(full.song_count, Some(24));
        assert_eq!(full.duration, Some(5521));
        assert_eq!(full.created.as_deref(), Some("2026-04-12T09:00:00.5Z"));
        assert_eq!(full.cover_art.as_deref(), Some("pl-playlist-1_66fa0b11"));
        let empty = &playlists[1];
        assert!(empty.comment.is_none());
        assert!(empty.cover_art.is_none());
        assert_eq!(empty.song_count, Some(0));
    }

    #[tokio::test]
    async fn no_playlists_is_an_empty_list() {
        let (server, client) = start().await;
        endpoint("getPlaylists")
            .respond_with(ok_with(json!({ "playlists": {} })))
            .expect(1)
            .mount(&server)
            .await;

        assert!(client.playlists().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn playlist_keeps_the_playlist_order() {
        let (server, client) = start().await;
        endpoint("getPlaylist")
            .and(query_param("id", "playlist-1"))
            .respond_with(fixture!("getPlaylist"))
            .expect(1)
            .mount(&server)
            .await;

        let playlist = client.playlist("playlist-1").await.unwrap();

        assert_eq!(playlist.id, "playlist-1");
        assert_eq!(playlist.name, "Sunday Morning");
        assert_eq!(playlist.comment.as_deref(), Some("Slow and quiet"));
        assert_eq!(playlist.song_count, Some(3));
        assert_eq!(playlist.duration, Some(812));
        assert_eq!(
            playlist.cover_art.as_deref(),
            Some("pl-playlist-1_66fa0b11")
        );
        assert_eq!(
            ids(&playlist.entry, |s| &s.id),
            ["song-7", "song-2", "song-1"]
        );
        assert_eq!(playlist.entry[0].title, "Night Ferry");
    }

    #[tokio::test]
    async fn create_playlist_sends_every_song_and_returns_the_playlist() {
        let (server, client) = start().await;
        endpoint("createPlaylist")
            .and(query_param("name", "Road Trip"))
            .respond_with(ok_with(json!({
                "playlist": {
                    "id": "playlist-9",
                    "name": "Road Trip",
                    "songCount": 2,
                    "duration": 443,
                    "entry": [
                        { "id": "song-1", "title": "Undertow" },
                        { "id": "song-2", "title": "Salt Lines" },
                    ],
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let playlist = client
            .create_playlist("Road Trip", &["song-1", "song-2"])
            .await
            .unwrap();

        assert_eq!(playlist.id, "playlist-9");
        assert_eq!(ids(&playlist.entry, |s| &s.id), ["song-1", "song-2"]);
        let url = &received(&server).await[0];
        assert_eq!(all_values(url, "songId"), ["song-1", "song-2"]);
    }

    #[tokio::test]
    async fn add_to_playlist_sends_every_song() {
        let (server, client) = start().await;
        endpoint("updatePlaylist")
            .and(query_param("playlistId", "playlist-1"))
            .respond_with(ok_with(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        client
            .add_to_playlist("playlist-1", &["song-3", "song-4"])
            .await
            .unwrap();

        let url = &received(&server).await[0];
        assert_eq!(all_values(url, "songIdToAdd"), ["song-3", "song-4"]);
    }
}

mod other_calls {
    use super::*;

    #[tokio::test]
    async fn starring_uses_id_for_songs_and_album_id_for_albums() {
        let (server, client) = start().await;
        endpoint("star")
            .and(query_param("id", "song-1"))
            .and(query_param_is_missing("albumId"))
            .respond_with(ok_with(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        endpoint("unstar")
            .and(query_param("albumId", "album-1"))
            .and(query_param_is_missing("id"))
            .respond_with(ok_with(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        client
            .set_starred(Starrable::Song("song-1"), true)
            .await
            .unwrap();
        client
            .set_starred(Starrable::Album("album-1"), false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn scrobble_sends_the_submission_and_the_time_when_given() {
        let (server, client) = start().await;
        endpoint("scrobble")
            .and(query_param("id", "song-1"))
            .and(query_param("submission", "true"))
            .and(query_param("time", "1759300000000"))
            .respond_with(ok_with(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        endpoint("scrobble")
            .and(query_param("id", "song-2"))
            .and(query_param("submission", "false"))
            .and(query_param_is_missing("time"))
            .respond_with(ok_with(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        client
            .scrobble("song-1", true, Some(1_759_300_000_000))
            .await
            .unwrap();
        client.scrobble("song-2", false, None).await.unwrap();
    }

    #[tokio::test]
    async fn report_playback_sends_the_state_and_position() {
        let (server, client) = start().await;
        endpoint("reportPlayback")
            .and(query_param("mediaId", "song-1"))
            .and(query_param("mediaType", "song"))
            .and(query_param("positionMs", "61500"))
            .and(query_param("state", "paused"))
            .and(query_param("ignoreScrobble", "true"))
            .respond_with(ok_with(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        client
            .report_playback("song-1", "paused", 61_500)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn extensions_are_listed_by_name() {
        let (server, client) = start().await;
        endpoint("getOpenSubsonicExtensions")
            .respond_with(ok_with(json!({
                "openSubsonicExtensions": [
                    { "name": "formPost", "versions": [1] },
                    { "name": "playbackReport", "versions": [1] },
                ]
            })))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        // A server that answers without the list.
        endpoint("getOpenSubsonicExtensions")
            .respond_with(ok_with(json!({})))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            client.extensions().await.unwrap(),
            ["formPost", "playbackReport"]
        );
        assert!(client.extensions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cover_art_returns_the_image_bytes() {
        let (server, client) = start().await;
        // The first bytes of a PNG file.
        let image = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        endpoint("getCoverArt")
            .and(query_param("id", "al-album-1"))
            .and(query_param("size", "600"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(image.clone(), "image/png"))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(client.cover_art("al-album-1", 600).await.unwrap(), image);
    }
}

mod errors {
    use super::*;

    #[tokio::test]
    async fn wrong_credentials_are_reported_as_such() {
        let (server, client) = start().await;
        endpoint("ping")
            .respond_with(failed(40, "Wrong username or password"))
            .expect(1)
            .mount(&server)
            .await;
        endpoint("getAlbum")
            .respond_with(failed(40, "Wrong username or password"))
            .expect(1)
            .mount(&server)
            .await;

        let e = error(client.ping().await);
        assert!(matches!(e, Error::WrongCredentials), "got {e:?}");
        let e = error(client.album("album-1").await);
        assert!(matches!(e, Error::WrongCredentials), "got {e:?}");
    }

    #[tokio::test]
    async fn other_subsonic_errors_keep_their_code_and_message() {
        let cases = [
            (0, "A generic error"),
            (10, "Required parameter is missing: id"),
            (
                20,
                "Incompatible Subsonic REST protocol version. Client must upgrade",
            ),
            (41, "Token authentication not supported for LDAP users"),
            (50, "User is not authorized for the given operation"),
            (70, "Album not found"),
        ];
        let (server, client) = start().await;
        for (code, message) in cases {
            endpoint("getAlbum")
                .and(query_param("id", format!("album-{code}")))
                .respond_with(failed(code, message))
                .expect(1)
                .mount(&server)
                .await;
        }

        for (code, message) in cases {
            match error(client.album(&format!("album-{code}")).await) {
                Error::Server {
                    code: c,
                    message: m,
                } => {
                    assert_eq!((c, m.as_str()), (code, message));
                }
                e => panic!("code {code}: got {e:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_failed_status_without_an_error_is_a_bad_response() {
        let (server, client) = start().await;
        endpoint("ping")
            .respond_with(raw(
                r#"{"subsonic-response": {"status": "failed", "version": "1.16.1"}}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let e = error(client.ping().await);
        assert!(
            matches!(&e, Error::BadResponse(m) if m.contains("failed")),
            "got {e:?}"
        );
    }

    /// The Subsonic API documents `message` as optional, but `ApiError`
    /// requires it: an error without one fails to parse, and the client
    /// reports BadResponse instead of the error the server sent.
    #[tokio::test]
    async fn an_error_without_a_message_keeps_its_code() {
        let (server, client) = start().await;
        endpoint("ping")
            .respond_with(raw(
                r#"{"subsonic-response": {"status": "failed", "version": "1.16.1", "error": {"code": 40}}}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;
        endpoint("getAlbum")
            .respond_with(raw(
                r#"{"subsonic-response": {"status": "failed", "version": "1.16.1", "error": {"code": 70}}}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let e = error(client.ping().await);
        assert!(matches!(e, Error::WrongCredentials), "got {e:?}");
        let e = error(client.album("album-1").await);
        assert!(matches!(e, Error::Server { code: 70, .. }), "got {e:?}");
    }

    #[tokio::test]
    async fn an_ok_response_without_the_requested_item_is_a_bad_response() {
        let (server, client) = start().await;
        for name in ["getAlbum", "getArtist", "getPlaylist", "createPlaylist"] {
            endpoint(name)
                .respond_with(ok_with(json!({})))
                .expect(1)
                .mount(&server)
                .await;
        }

        let results = [
            ("album", client.album("album-1").await.map(drop)),
            ("artist", client.artist("artist-1").await.map(drop)),
            ("playlist", client.playlist("playlist-1").await.map(drop)),
            (
                "playlist",
                client.create_playlist("New", &[]).await.map(drop),
            ),
        ];
        for (item, result) in results {
            let e = error(result);
            let expected = format!("missing {item} in response");
            assert!(
                matches!(&e, Error::BadResponse(m) if *m == expected),
                "got {e:?}"
            );
        }
    }

    #[tokio::test]
    async fn malformed_bodies_are_bad_responses() {
        let cases = [
            (
                "truncated JSON",
                r#"{"subsonic-response": {"status": "ok", "version": "1.16.1""#,
            ),
            (
                "an HTML page",
                "<html><body><h1>502 Bad Gateway</h1></body></html>",
            ),
            ("an empty body", ""),
            (
                "JSON without the envelope",
                r#"{"status": "ok", "version": "1.16.1"}"#,
            ),
            (
                "XML instead of JSON",
                r#"<?xml version="1.0" encoding="UTF-8"?><subsonic-response xmlns="http://subsonic.org/restapi" status="ok" version="1.16.1"/>"#,
            ),
            ("no version", r#"{"subsonic-response": {"status": "ok"}}"#),
            (
                "a number as a string",
                r#"{"subsonic-response": {"status": "ok", "version": "1.16.1", "albumList2": {"album": [{"id": "album-1", "name": "Low Tide", "year": "2019"}]}}}"#,
            ),
            (
                "an album without a name",
                r#"{"subsonic-response": {"status": "ok", "version": "1.16.1", "albumList2": {"album": [{"id": "album-1"}]}}}"#,
            ),
        ];
        for (case, body) in cases {
            let (server, client) = start().await;
            endpoint("getAlbumList2")
                .respond_with(raw(body))
                .expect(1)
                .mount(&server)
                .await;

            let e = error(client.album_list(10, 0).await);
            assert!(matches!(e, Error::BadResponse(_)), "{case}: got {e:?}");
        }
    }

    #[tokio::test]
    async fn http_errors_are_bad_responses_with_the_status() {
        for status in [401, 403, 404, 500, 502, 503] {
            let (server, client) = start().await;
            endpoint("ping")
                .respond_with(ResponseTemplate::new(status).set_body_string("error page"))
                .expect(1)
                .mount(&server)
                .await;

            let e = error(client.ping().await);
            let expected = format!("HTTP {status}");
            assert!(
                matches!(&e, Error::BadResponse(m) if m.starts_with(&expected)),
                "{status}: got {e:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_server_that_is_not_there_is_unreachable() {
        // A port that was free a moment ago: nothing listens there now.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let client = Client::new(&format!("http://127.0.0.1:{port}"), USER, PASSWORD);

        let e = error(client.ping().await);
        assert!(matches!(e, Error::Unreachable(_)), "got {e:?}");
    }

    #[tokio::test]
    async fn error_texts_never_carry_the_login_token() {
        // A decode error and an HTTP error, where the token is known.
        for response in [raw("not json"), ResponseTemplate::new(500)] {
            let (server, client) = start().await;
            endpoint("ping")
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;

            let text = error(client.ping().await).to_string();

            let params = params(&received(&server).await[0]);
            assert!(!text.contains(&params["t"]), "the token is in {text:?}");
            assert!(!text.contains(&params["s"]), "the salt is in {text:?}");
            assert!(!text.contains("/rest/"), "the URL is in {text:?}");
        }

        // A connection error, where the request never arrives.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let client = Client::new(&format!("http://127.0.0.1:{port}"), USER, PASSWORD);
        let text = error(client.ping().await).to_string();
        assert!(!text.contains("t="), "the token is in {text:?}");
        assert!(!text.contains("/rest/"), "the URL is in {text:?}");
    }

    #[tokio::test]
    async fn cover_art_http_errors_are_bad_responses() {
        let (server, client) = start().await;
        endpoint("getCoverArt")
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let e = error(client.cover_art("al-missing", 600).await);
        assert!(
            matches!(&e, Error::BadResponse(m) if m.starts_with("HTTP 404")),
            "got {e:?}"
        );
    }

    /// Subsonic servers answer getCoverArt errors (no such cover, wrong
    /// password) like any other: HTTP 200 with the error in a JSON body.
    /// `cover_art` reads it as the error it is, not as the image.
    #[tokio::test]
    async fn cover_art_reports_subsonic_errors() {
        let (server, client) = start().await;
        endpoint("getCoverArt")
            .and(query_param("id", "al-missing"))
            .respond_with(failed(70, "Artwork not found"))
            .expect(1)
            .mount(&server)
            .await;
        endpoint("getCoverArt")
            .and(query_param("id", "al-album-1"))
            .respond_with(failed(40, "Wrong username or password"))
            .expect(1)
            .mount(&server)
            .await;

        let e = error(client.cover_art("al-missing", 600).await);
        assert!(matches!(e, Error::Server { code: 70, .. }), "got {e:?}");
        let e = error(client.cover_art("al-album-1", 600).await);
        assert!(matches!(e, Error::WrongCredentials), "got {e:?}");
    }
}

mod library_changes {
    //! What a client asks before a sync: whether the library changed since
    //! (getScanStatus), and the stars, which change without a scan
    //! (getStarred2).
    use super::*;

    #[tokio::test]
    async fn the_scan_status_tells_whether_a_scan_runs_and_when_the_last_ended() {
        let (server, client) = start().await;
        endpoint("getScanStatus")
            .respond_with(ok_with(json!({
                "scanStatus": {
                    "scanning": false,
                    "count": 11333,
                    "folderCount": 749,
                    "lastScan": "2026-10-31T21:26:55.751080157Z",
                    "scanType": "quick",
                    "elapsedTime": 51751080157_u64
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let status = client.scan_status().await.unwrap();
        assert!(!status.scanning);
        assert_eq!(
            status.last_scan.as_deref(),
            Some("2026-10-31T21:26:55.751080157Z")
        );
    }

    #[tokio::test]
    async fn a_server_that_does_not_tell_the_last_scan_gives_none() {
        let (server, client) = start().await;
        endpoint("getScanStatus")
            .respond_with(ok_with(
                json!({ "scanStatus": { "scanning": true, "count": 10 } }),
            ))
            .mount(&server)
            .await;

        let status = client.scan_status().await.unwrap();
        assert!(status.scanning);
        assert_eq!(status.last_scan, None);
    }

    #[tokio::test]
    async fn a_server_without_the_scan_status_gives_an_error() {
        let (server, client) = start().await;
        endpoint("getScanStatus")
            .respond_with(failed(70, "Not found"))
            .mount(&server)
            .await;
        assert!(client.scan_status().await.is_err());

        let (server, client) = start().await;
        endpoint("getScanStatus")
            .respond_with(ok_with(json!({})))
            .mount(&server)
            .await;
        assert!(client.scan_status().await.is_err());
    }

    #[tokio::test]
    async fn the_starred_albums_and_songs_come_with_the_dates_of_their_stars() {
        let (server, client) = start().await;
        endpoint("getStarred2")
            .respond_with(ok_with(json!({
                "starred2": {
                    "artist": [{ "id": "ar1", "name": "Artist", "starred": "2026-01-01T00:00:00Z" }],
                    "album": [{
                        "id": "al1", "name": "Album", "artist": "Artist", "year": 1965,
                        "songCount": 3, "starred": "2026-02-01T00:00:00Z"
                    }],
                    "song": [
                        { "id": "s1", "title": "One", "albumId": "al1", "starred": "2026-03-01T00:00:00Z" },
                        { "id": "s2", "title": "Two", "albumId": "al2", "starred": "2026-04-01T00:00:00Z" }
                    ]
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let starred = client.starred().await.unwrap();
        assert_eq!(ids(&starred.album, |a| &a.id), ["al1"]);
        assert_eq!(
            starred.album[0].starred.as_deref(),
            Some("2026-02-01T00:00:00Z")
        );
        assert_eq!(ids(&starred.song, |s| &s.id), ["s1", "s2"]);
        assert_eq!(
            starred.song[1].starred.as_deref(),
            Some("2026-04-01T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn nothing_starred_gives_empty_lists() {
        let (server, client) = start().await;
        endpoint("getStarred2")
            .respond_with(ok_with(json!({ "starred2": {} })))
            .mount(&server)
            .await;

        let starred = client.starred().await.unwrap();
        assert!(starred.album.is_empty() && starred.song.is_empty());
    }
}
