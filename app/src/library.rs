//! Local library database (SQLite). The grid, album pages and search are
//! filled from here, before the server is even contacted; syncing keeps it up
//! to date. All functions here block, so callers run them on a worker thread.
//!
//! Schema changes are migrations keyed by PRAGMA user_version, each in its own
//! transaction: an interrupted migration leaves the database as it was, and the
//! next start simply runs it again.

use gtk::glib;
use rusqlite::{Connection, params};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// Inside Flatpak this is ~/.var/app/<app id>/data/vumi/library.db
fn db_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = tests::DATABASE.with(|path| path.borrow().clone()) {
        return path;
    }
    glib::user_data_dir().join("vumi").join("library.db")
}

thread_local! {
    /// This thread's connection, opened at its first query and kept. Each
    /// worker thread has its own: a sync writing thousands of songs in one
    /// transaction does not hold up the reads of other threads (WAL).
    static CONNECTION: RefCell<Option<(PathBuf, Connection)>> = const { RefCell::new(None) };
}

/// The databases whose schema is up to date: checked and migrated once in
/// the life of the process, by the first connection (at startup, when the
/// local library is read).
static PREPARED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Runs `f` with this thread's connection to the database.
fn with_connection<T>(
    f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
) -> rusqlite::Result<T> {
    let path = db_path();
    CONNECTION.with(|cell| {
        let mut cell = cell.borrow_mut();
        let open = cell.as_ref().is_some_and(|(open, _)| *open == path);
        if !open {
            *cell = Some((path.clone(), connect(&path)?));
        }
        let (_, conn) = cell.as_mut().expect("the connection was just opened");
        f(conn)
    })
}

fn connect(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut conn = Connection::open(path)?;
    let mut prepared = PREPARED.lock().unwrap_or_else(PoisonError::into_inner);
    if !prepared.iter().any(|done| done == path) {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS albums (
                 server     TEXT NOT NULL,
                 position   INTEGER NOT NULL,
                 id         TEXT NOT NULL,
                 name       TEXT NOT NULL,
                 artist     TEXT,
                 year       INTEGER,
                 song_count INTEGER,
                 cover_art  TEXT,
                 PRIMARY KEY (server, id)
             );",
        )?;
        migrate(&mut conn)?;
        prepared.push(path.to_path_buf());
    }
    Ok(conn)
}

/// Brings an older database up to date, one version at a time.
fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version < 1 {
        // Version 1: when the album was added to the server ("Recently added").
        let tx = conn.transaction()?;
        tx.execute_batch(
            "ALTER TABLE albums ADD COLUMN created TEXT;
             PRAGMA user_version = 1;",
        )?;
        tx.commit()?;
    }
    if version < 2 {
        // Version 2: every song, so album pages open already filled.
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE songs (
                 server        TEXT NOT NULL,
                 position      INTEGER NOT NULL,
                 id            TEXT NOT NULL,
                 album_id      TEXT,
                 title         TEXT NOT NULL,
                 artist        TEXT,
                 album         TEXT,
                 track         INTEGER,
                 disc_number   INTEGER,
                 duration      INTEGER,
                 suffix        TEXT,
                 bit_rate      INTEGER,
                 bit_depth     INTEGER,
                 sampling_rate INTEGER,
                 cover_art     TEXT,
                 starred       TEXT,
                 PRIMARY KEY (server, id)
             );
             CREATE INDEX songs_by_album ON songs (server, album_id);
             PRAGMA user_version = 2;",
        )?;
        tx.commit()?;
    }
    if version < 3 {
        // Version 3: the album's star and artist id, so album pages can be
        // built from here without waiting for the server.
        let tx = conn.transaction()?;
        tx.execute_batch(
            "ALTER TABLE albums ADD COLUMN artist_id TEXT;
             ALTER TABLE albums ADD COLUMN starred TEXT;
             PRAGMA user_version = 3;",
        )?;
        tx.commit()?;
    }
    if version < 4 {
        // Version 4: full-text search (FTS5). The text is stored folded (see
        // fold), and filled in by the next sync.
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE VIRTUAL TABLE albums_fts USING fts5(
                 server UNINDEXED, id UNINDEXED, text,
                 tokenize = 'unicode61 remove_diacritics 2'
             );
             CREATE VIRTUAL TABLE songs_fts USING fts5(
                 server UNINDEXED, id UNINDEXED, text,
                 tokenize = 'unicode61 remove_diacritics 2'
             );
             PRAGMA user_version = 4;",
        )?;
        tx.commit()?;
    }
    if version < 5 {
        // Version 5: the artist id of every song, for "Go to Artist". Filled
        // in by the next sync, which writes all songs anew.
        let tx = conn.transaction()?;
        tx.execute_batch(
            "ALTER TABLE songs ADD COLUMN artist_id TEXT;
             PRAGMA user_version = 5;",
        )?;
        tx.commit()?;
    }
    if version < 6 {
        // Version 6: the composer of every song (classical albums). Filled in
        // by the next sync.
        let tx = conn.transaction()?;
        tx.execute_batch(
            "ALTER TABLE songs ADD COLUMN composer TEXT;
             PRAGMA user_version = 6;",
        )?;
        tx.commit()?;
    }
    if version < 7 {
        // Version 7: when each account was last synced (Preferences). A
        // table of its own: nothing that reads albums or songs sees it.
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE sync_state (
                 server    TEXT NOT NULL PRIMARY KEY,
                 synced_at INTEGER NOT NULL
             );
             PRAGMA user_version = 7;",
        )?;
        tx.commit()?;
    }
    if version < 8 {
        // Version 8: the end of the server's last scan, as it was at the last
        // full sync: unchanged, the library needs no full sync (last_scan).
        let tx = conn.transaction()?;
        tx.execute_batch(
            "ALTER TABLE sync_state ADD COLUMN last_scan TEXT;
             PRAGMA user_version = 8;",
        )?;
        tx.commit()?;
    }
    Ok(())
}

/// Notes that the library of `server` was synced in full just now, with the
/// end of the server's last scan as it was when the sync began
/// (scan_to_store).
pub(crate) fn mark_synced(server: &str, last_scan: Option<&str>) -> rusqlite::Result<()> {
    with_connection(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO sync_state (server, synced_at, last_scan)
             VALUES (?1, CAST(strftime('%s', 'now') AS INTEGER), ?2)",
            params![server, last_scan],
        )?;
        Ok(())
    })
}

/// The end of the server's last scan as stored with the last full sync of
/// `server` (scan_to_store), if any.
pub(crate) fn last_scan(server: &str) -> rusqlite::Result<Option<String>> {
    with_connection(|conn| {
        let stored = conn.query_row(
            "SELECT last_scan FROM sync_state WHERE server = ?1",
            params![server],
            |row| row.get(0),
        );
        match stored {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            other => other,
        }
    })
}

/// Notes that the library of `server` was checked just now and found as it
/// was (a light sync): the time moves, the stored scan stays.
pub(crate) fn mark_checked(server: &str) -> rusqlite::Result<()> {
    with_connection(|conn| {
        conn.execute(
            "UPDATE sync_state SET synced_at = CAST(strftime('%s', 'now') AS INTEGER)
             WHERE server = ?1",
            params![server],
        )?;
        Ok(())
    })
}

/// The stars of `server` as the server has them now: every other star goes.
pub(crate) fn set_stars(server: &str, starred: &subsonic::Starred) -> rusqlite::Result<()> {
    with_connection(|conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE albums SET starred = NULL WHERE server = ?1 AND starred IS NOT NULL",
            params![server],
        )?;
        tx.execute(
            "UPDATE songs SET starred = NULL WHERE server = ?1 AND starred IS NOT NULL",
            params![server],
        )?;
        {
            let mut album =
                tx.prepare("UPDATE albums SET starred = ?1 WHERE server = ?2 AND id = ?3")?;
            for a in &starred.album {
                album.execute(params![a.starred.as_deref().unwrap_or("1"), server, a.id])?;
            }
            let mut song =
                tx.prepare("UPDATE songs SET starred = ?1 WHERE server = ?2 AND id = ?3")?;
            for s in &starred.song {
                song.execute(params![s.starred.as_deref().unwrap_or("1"), server, s.id])?;
            }
        }
        tx.commit()
    })
}

/// Whether the library is as at the last full sync: the server scans no more
/// and its last scan ended when it had ended then (`stored`). A server that
/// does not tell (no status, no last scan) is taken as changed.
pub(crate) fn library_unchanged(
    stored: Option<&str>,
    status: Option<&subsonic::ScanStatus>,
) -> bool {
    match (stored, status) {
        (Some(stored), Some(status)) => {
            !status.scanning && status.last_scan.as_deref() == Some(stored)
        }
        _ => false,
    }
}

/// What a full sync stores of the scan status read at its start: the end of
/// the last scan, but nothing while a scan runs (what the sync reads may be
/// half old, so the next start syncs everything again).
pub(crate) fn scan_to_store(status: Option<&subsonic::ScanStatus>) -> Option<String> {
    status
        .filter(|status| !status.scanning)
        .and_then(|status| status.last_scan.clone())
}

/// What the local library of one server holds.
pub(crate) struct Stats {
    pub albums: u32,
    pub songs: u32,
    /// When it was last synced (Unix time), if it ever was.
    pub synced_at: Option<i64>,
}

pub(crate) fn stats(server: &str) -> rusqlite::Result<Stats> {
    with_connection(|conn| {
        conn.query_row(
            "SELECT (SELECT COUNT(*) FROM albums WHERE server = ?1),
                    (SELECT COUNT(*) FROM songs WHERE server = ?1),
                    (SELECT synced_at FROM sync_state WHERE server = ?1)",
            params![server],
            |row| {
                Ok(Stats {
                    albums: row.get(0)?,
                    songs: row.get(1)?,
                    synced_at: row.get(2)?,
                })
            },
        )
    })
}

/// Text for the search index and for queries: lower case, and Polish letters as
/// their base letters ("Łódź" → "lodz"). SQLite folds most accents by itself
/// (remove_diacritics), but "ł" is a letter of its own in Unicode, not an "l"
/// with an accent, so all Polish letters are folded here alike.
fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'ą' => 'a',
            'ć' => 'c',
            'ę' => 'e',
            'ł' => 'l',
            'ń' => 'n',
            'ó' => 'o',
            'ś' => 's',
            'ź' | 'ż' => 'z',
            c => c,
        })
        .collect()
}

/// "chas pav" → `"chas"* "pav"*`: every word must match the start of a word.
/// Each word is quoted, so characters FTS5 treats as syntax are just text.
fn fts_query(query: &str) -> Option<String> {
    let words: Vec<String> = fold(query)
        .split_whitespace()
        .map(|word| format!("\"{}\"*", word.replace('"', "")))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Albums of one server, in the order the server listed them.
pub(crate) fn load_albums(server: &str) -> rusqlite::Result<Vec<subsonic::Album>> {
    with_connection(|conn| {
        let mut stmt = conn.prepare_cached(
            "SELECT id, name, artist, year, song_count, cover_art, created, artist_id, starred
             FROM albums WHERE server = ?1 ORDER BY position",
        )?;
        let rows = stmt.query_map(params![server], |row| {
            Ok(subsonic::Album {
                id: row.get(0)?,
                name: row.get(1)?,
                artist: row.get(2)?,
                year: row.get(3)?,
                song_count: row.get(4)?,
                cover_art: row.get(5)?,
                created: row.get(6)?,
                artist_id: row.get(7)?,
                starred: row.get(8)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })
}

/// Replaces the stored albums of one server, and their search index, in a
/// single transaction.
pub(crate) fn save_albums(server: &str, albums: &[Arc<subsonic::Album>]) -> rusqlite::Result<()> {
    with_connection(|conn| save_albums_with(conn, server, albums))
}

fn save_albums_with(
    conn: &mut Connection,
    server: &str,
    albums: &[Arc<subsonic::Album>],
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM albums WHERE server = ?1", params![server])?;
    tx.execute("DELETE FROM albums_fts WHERE server = ?1", params![server])?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO albums
                 (server, position, id, name, artist, year, song_count, cover_art, created,
                  artist_id, starred)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
        let mut index =
            tx.prepare("INSERT INTO albums_fts (server, id, text) VALUES (?1, ?2, ?3)")?;
        for (position, a) in albums.iter().enumerate() {
            insert.execute(params![
                server,
                position as i64,
                a.id,
                a.name,
                a.artist,
                a.year,
                a.song_count,
                a.cover_art,
                a.created,
                a.artist_id,
                a.starred
            ])?;
            let text = fold(&format!("{} {}", a.name, a.artist.as_deref().unwrap_or("")));
            index.execute(params![server, a.id, text])?;
        }
    }
    tx.commit()
}

/// Replaces the stored songs of one server, and their search index, in a
/// single transaction.
pub(crate) fn save_songs(server: &str, songs: &[subsonic::Song]) -> rusqlite::Result<()> {
    with_connection(|conn| save_songs_with(conn, server, songs))
}

fn save_songs_with(
    conn: &mut Connection,
    server: &str,
    songs: &[subsonic::Song],
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM songs WHERE server = ?1", params![server])?;
    tx.execute("DELETE FROM songs_fts WHERE server = ?1", params![server])?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO songs
                 (server, position, id, album_id, title, artist, album, track,
                  disc_number, duration, suffix, bit_rate, bit_depth, sampling_rate,
                  cover_art, starred, artist_id, composer)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        )?;
        let mut index =
            tx.prepare("INSERT INTO songs_fts (server, id, text) VALUES (?1, ?2, ?3)")?;
        for (position, s) in songs.iter().enumerate() {
            insert.execute(params![
                server,
                position as i64,
                s.id,
                s.album_id,
                s.title,
                s.artist,
                s.album,
                s.track,
                s.disc_number,
                s.duration,
                s.suffix,
                s.bit_rate,
                s.bit_depth,
                s.sampling_rate,
                s.cover_art,
                s.starred,
                s.artist_id,
                s.composer
            ])?;
            let text = fold(&format!(
                "{} {} {} {}",
                s.title,
                s.artist.as_deref().unwrap_or(""),
                s.album.as_deref().unwrap_or(""),
                s.composer.as_deref().unwrap_or("")
            ));
            index.execute(params![server, s.id, text])?;
        }
    }
    tx.commit()
}

/// A song read from a row with these columns, in this order: id, title,
/// artist, track, disc_number, album, album_id, duration, suffix, bit_rate,
/// cover_art, bit_depth, sampling_rate, starred, artist_id, composer.
fn song_from_row(row: &rusqlite::Row) -> rusqlite::Result<subsonic::Song> {
    Ok(subsonic::Song {
        id: row.get(0)?,
        title: row.get(1)?,
        artist: row.get(2)?,
        track: row.get(3)?,
        disc_number: row.get(4)?,
        album: row.get(5)?,
        album_id: row.get(6)?,
        duration: row.get(7)?,
        suffix: row.get(8)?,
        bit_rate: row.get(9)?,
        cover_art: row.get(10)?,
        bit_depth: row.get(11)?,
        sampling_rate: row.get(12)?,
        starred: row.get(13)?,
        artist_id: row.get(14)?,
        composer: row.get(15)?,
    })
}

/// The songs of one album, disc by disc and track by track.
pub(crate) fn load_album_songs(
    server: &str,
    album_id: &str,
) -> rusqlite::Result<Vec<subsonic::Song>> {
    with_connection(|conn| {
        let mut stmt = conn.prepare_cached(ALBUM_SONGS)?;
        let rows = stmt.query_map(params![server, album_id], song_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })
}

/// The songs of each album of `album_ids`, in the album's order, read
/// together (the Start page asks for the albums of its tiles at once).
pub(crate) fn load_songs_by_album(
    server: &str,
    album_ids: &[String],
) -> rusqlite::Result<Vec<(String, Vec<subsonic::Song>)>> {
    with_connection(|conn| {
        let mut stmt = conn.prepare_cached(ALBUM_SONGS)?;
        album_ids
            .iter()
            .map(|album_id| {
                let songs = stmt
                    .query_map(params![server, album_id], song_from_row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok((album_id.clone(), songs))
            })
            .collect()
    })
}

/// Up to `count` songs picked at random from the albums of `year`, each once
/// ("Play This Year").
pub(crate) fn random_songs_of_year(
    server: &str,
    year: i32,
    count: usize,
) -> rusqlite::Result<Vec<subsonic::Song>> {
    with_connection(|conn| {
        let mut stmt = conn.prepare_cached(
            "SELECT id, title, artist, track, disc_number, album, album_id, duration,
                    suffix, bit_rate, cover_art, bit_depth, sampling_rate, starred, artist_id,
                    composer
             FROM songs
             WHERE server = ?1
               AND album_id IN (SELECT id FROM albums WHERE server = ?1 AND year = ?2)
             ORDER BY random() LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![server, year, count as i64], song_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })
}

/// The songs of one album, in the album's order.
const ALBUM_SONGS: &str = "SELECT id, title, artist, track, disc_number, album, album_id, duration,
        suffix, bit_rate, cover_art, bit_depth, sampling_rate, starred, artist_id,
        composer
 FROM songs WHERE server = ?1 AND album_id = ?2
 ORDER BY COALESCE(disc_number, 1), COALESCE(track, 0), position";

/// What a search (or the favourites) found: album ids (the albums themselves
/// are in memory) and
/// songs, best matches first.
pub(crate) struct SearchResults {
    pub(crate) album_ids: Vec<String>,
    pub(crate) songs: Vec<subsonic::Song>,
}

/// Searches albums and songs of one server; every word of `query` must match
/// the start of a word, ignoring case and accents ("zolw" finds "Żółw").
pub(crate) fn search(server: &str, query: &str, limit: usize) -> rusqlite::Result<SearchResults> {
    let Some(fts) = fts_query(query) else {
        return Ok(SearchResults {
            album_ids: Vec::new(),
            songs: Vec::new(),
        });
    };
    with_connection(|conn| search_with(conn, server, &fts, limit))
}

fn search_with(
    conn: &Connection,
    server: &str,
    fts: &str,
    limit: usize,
) -> rusqlite::Result<SearchResults> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM albums_fts
         WHERE albums_fts MATCH ?2 AND server = ?1
         ORDER BY rank LIMIT ?3",
    )?;
    let album_ids = stmt
        .query_map(params![server, fts, limit as i64], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;

    let mut stmt = conn.prepare_cached(
        "SELECT s.id, s.title, s.artist, s.track, s.disc_number, s.album, s.album_id,
                s.duration, s.suffix, s.bit_rate, s.cover_art, s.bit_depth,
                s.sampling_rate, s.starred, s.artist_id, s.composer
         FROM songs_fts f JOIN songs s ON s.server = f.server AND s.id = f.id
         WHERE songs_fts MATCH ?2 AND f.server = ?1
         ORDER BY f.rank LIMIT ?3",
    )?;
    let songs = stmt
        .query_map(params![server, fts, limit as i64], song_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(SearchResults { album_ids, songs })
}

/// Starred albums and songs of one server, most recently starred first.
pub(crate) fn load_starred(server: &str) -> rusqlite::Result<SearchResults> {
    with_connection(|conn| load_starred_with(conn, server))
}

fn load_starred_with(conn: &Connection, server: &str) -> rusqlite::Result<SearchResults> {
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM albums WHERE server = ?1 AND starred IS NOT NULL
         ORDER BY starred DESC",
    )?;
    let album_ids = stmt
        .query_map(params![server], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;

    let mut stmt = conn.prepare_cached(
        "SELECT id, title, artist, track, disc_number, album, album_id, duration,
                suffix, bit_rate, cover_art, bit_depth, sampling_rate, starred, artist_id,
                composer
         FROM songs WHERE server = ?1 AND starred IS NOT NULL
         ORDER BY starred DESC",
    )?;
    let songs = stmt
        .query_map(params![server], song_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(SearchResults { album_ids, songs })
}

/// Records a star, or its removal, locally right away, once the server has
/// confirmed it; the next sync brings the server's own record.
pub(crate) fn set_starred(
    server: &str,
    album: bool,
    id: &str,
    starred: bool,
) -> rusqlite::Result<()> {
    let table = if album { "albums" } else { "songs" };
    // The server keeps the date of the star; here any non-null value marks it.
    let value: Option<String> = starred.then(|| {
        glib::DateTime::now_utc()
            .ok()
            .and_then(|now| now.format_iso8601().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "1".to_string())
    });
    with_connection(|conn| {
        conn.execute(
            &format!("UPDATE {table} SET starred = ?1 WHERE server = ?2 AND id = ?3"),
            params![value, server, id],
        )?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// The database of the test running on this thread (db_path).
        pub(super) static DATABASE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    /// A database file of this test alone, in a directory removed afterwards.
    struct Temporary(PathBuf);

    impl Temporary {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("vumi-library-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("no temporary directory");
            DATABASE.with(|path| path.replace(Some(dir.join("library.db"))));
            Self(dir)
        }
    }

    impl Drop for Temporary {
        fn drop(&mut self) {
            DATABASE.with(|path| path.replace(None));
            CONNECTION.with(|cell| cell.replace(None));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SERVER: &str = "user@http://server";

    fn album(id: &str, name: &str, artist: &str, year: i32) -> Arc<subsonic::Album> {
        Arc::new(subsonic::Album {
            id: id.into(),
            name: name.into(),
            artist: Some(artist.into()),
            year: Some(year),
            song_count: None,
            created: None,
            artist_id: None,
            starred: None,
            cover_art: None,
        })
    }

    fn song(id: &str, title: &str, album_id: &str, disc: u32, track: u32) -> subsonic::Song {
        subsonic::Song {
            id: id.into(),
            title: title.into(),
            artist: Some("Artist".into()),
            track: Some(track),
            disc_number: Some(disc),
            album: Some(album_id.to_uppercase()),
            album_id: Some(album_id.into()),
            duration: Some(100),
            ..Default::default()
        }
    }

    fn ids(songs: &[subsonic::Song]) -> Vec<&str> {
        songs.iter().map(|song| song.id.as_str()).collect()
    }

    #[test]
    fn a_version_0_database_is_brought_up_to_date_and_keeps_its_albums() {
        let temporary = Temporary::new("migrations");
        // The first schema: albums only, no user_version.
        let old = Connection::open(temporary.0.join("library.db")).unwrap();
        old.execute_batch(
            "CREATE TABLE albums (
                 server TEXT NOT NULL, position INTEGER NOT NULL, id TEXT NOT NULL,
                 name TEXT NOT NULL, artist TEXT, year INTEGER, song_count INTEGER,
                 cover_art TEXT, PRIMARY KEY (server, id)
             );
             INSERT INTO albums VALUES ('user@http://server', 0, 'a', 'Żółw', 'Artist', 1999, 3, 'c');",
        )
        .unwrap();
        drop(old);

        let albums = load_albums(SERVER).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].name, "Żółw");
        assert_eq!(albums[0].year, Some(1999));
        assert_eq!(albums[0].created, None);
        with_connection(|conn| {
            let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            assert_eq!(version, 8);
            let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
            assert_eq!(mode, "wal");
            Ok(())
        })
        .unwrap();
        // Every later table is there and works.
        save_songs(SERVER, &[song("s", "Song", "a", 1, 1)]).unwrap();
        mark_synced(SERVER, Some("2026-10-31T21:26:55Z")).unwrap();
        let stats = stats(SERVER).unwrap();
        assert_eq!((stats.albums, stats.songs), (1, 1));
        assert!(stats.synced_at.is_some());
    }

    #[test]
    fn albums_and_songs_come_back_in_their_order() {
        let _temporary = Temporary::new("order");
        save_albums(
            SERVER,
            &[album("b", "B", "X", 2000), album("a", "A", "X", 2000)],
        )
        .unwrap();
        let names: Vec<_> = load_albums(SERVER)
            .unwrap()
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(names, ["B", "A"]);
        save_songs(
            SERVER,
            &[
                song("2-1", "Two one", "a", 2, 1),
                song("1-2", "One two", "a", 1, 2),
                song("1-1", "One one", "a", 1, 1),
                song("b-1", "Other", "b", 1, 1),
            ],
        )
        .unwrap();
        assert_eq!(
            ids(&load_album_songs(SERVER, "a").unwrap()),
            ["1-1", "1-2", "2-1"]
        );
        // Saving again replaces, it does not add.
        save_songs(SERVER, &[song("1-1", "One one", "a", 1, 1)]).unwrap();
        assert_eq!(ids(&load_album_songs(SERVER, "a").unwrap()), ["1-1"]);
        assert!(
            load_album_songs("other@http://server", "a")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn songs_by_album_come_album_after_album() {
        let _temporary = Temporary::new("grouped");
        save_songs(
            SERVER,
            &[
                song("a2", "A two", "a", 1, 2),
                song("b1", "B one", "b", 1, 1),
                song("a1", "A one", "a", 1, 1),
            ],
        )
        .unwrap();
        let wanted = ["b".to_string(), "none".to_string(), "a".to_string()];
        let found = load_songs_by_album(SERVER, &wanted).unwrap();
        let found: Vec<(&str, Vec<&str>)> = found
            .iter()
            .map(|(album, songs)| (album.as_str(), ids(songs)))
            .collect();
        assert_eq!(
            found,
            [("b", vec!["b1"]), ("none", vec![]), ("a", vec!["a1", "a2"])]
        );
    }

    #[test]
    fn random_songs_of_a_year_are_different_and_of_that_year_only() {
        let _temporary = Temporary::new("year");
        save_albums(
            SERVER,
            &[album("a", "A", "X", 1965), album("b", "B", "X", 1966)],
        )
        .unwrap();
        let mut songs: Vec<_> = (0..30)
            .map(|n| song(&format!("a{n}"), "S", "a", 1, n))
            .collect();
        songs.extend((0..30).map(|n| song(&format!("b{n}"), "S", "b", 1, n)));
        save_songs(SERVER, &songs).unwrap();

        let picked = random_songs_of_year(SERVER, 1965, 20).unwrap();
        assert_eq!(picked.len(), 20);
        assert!(
            picked
                .iter()
                .all(|song| song.album_id.as_deref() == Some("a"))
        );
        let unique: std::collections::HashSet<_> = picked.iter().map(|s| &s.id).collect();
        assert_eq!(unique.len(), 20);
        // Fewer songs than asked for: all of them.
        assert_eq!(random_songs_of_year(SERVER, 1966, 50).unwrap().len(), 30);
        assert!(random_songs_of_year(SERVER, 1967, 50).unwrap().is_empty());
    }

    #[test]
    fn search_folds_polish_letters_and_matches_the_start_of_words() {
        let _temporary = Temporary::new("search");
        save_albums(
            SERVER,
            &[
                album("z", "Żółw", "Łukasz", 2000),
                album("o", "Other", "Nobody", 2000),
            ],
        )
        .unwrap();
        save_songs(SERVER, &[song("s", "Chasing Pavements", "o", 1, 1)]).unwrap();
        assert_eq!(search(SERVER, "zolw", 10).unwrap().album_ids, ["z"]);
        assert_eq!(search(SERVER, "luka", 10).unwrap().album_ids, ["z"]);
        assert_eq!(ids(&search(SERVER, "chas pav", 10).unwrap().songs), ["s"]);
        assert!(
            search(SERVER, "pavements chasing x", 10)
                .unwrap()
                .songs
                .is_empty()
        );
        assert!(search(SERVER, "  ", 10).unwrap().album_ids.is_empty());
        // Quotes are text, not query syntax.
        assert!(search(SERVER, "\"", 10).unwrap().album_ids.is_empty());
    }

    #[test]
    fn stars_are_set_locally_and_read_newest_first() {
        let _temporary = Temporary::new("stars");
        save_albums(
            SERVER,
            &[album("a", "A", "X", 2000), album("b", "B", "X", 2000)],
        )
        .unwrap();
        let mut older = song("s1", "Older", "a", 1, 1);
        older.starred = Some("2020-01-01T00:00:00Z".into());
        let mut newer = song("s2", "Newer", "a", 1, 2);
        newer.starred = Some("2024-01-01T00:00:00Z".into());
        save_songs(SERVER, &[older, newer, song("s3", "Not", "a", 1, 3)]).unwrap();
        assert_eq!(ids(&load_starred(SERVER).unwrap().songs), ["s2", "s1"]);

        set_starred(SERVER, true, "b", true).unwrap();
        set_starred(SERVER, false, "s2", false).unwrap();
        let starred = load_starred(SERVER).unwrap();
        assert_eq!(starred.album_ids, ["b"]);
        assert_eq!(ids(&starred.songs), ["s1"]);
    }

    #[test]
    fn a_full_sync_keeps_the_last_scan_and_a_check_moves_only_the_time() {
        let _temporary = Temporary::new("scan");
        assert_eq!(last_scan(SERVER).unwrap(), None);
        mark_synced(SERVER, Some("2026-10-31T21:26:55Z")).unwrap();
        assert_eq!(
            last_scan(SERVER).unwrap().as_deref(),
            Some("2026-10-31T21:26:55Z")
        );
        // Back an hour, so the check shows it moved the time.
        with_connection(|conn| {
            conn.execute("UPDATE sync_state SET synced_at = synced_at - 3600", [])?;
            Ok(())
        })
        .unwrap();
        let before = stats(SERVER).unwrap().synced_at.unwrap();
        mark_checked(SERVER).unwrap();
        assert!(stats(SERVER).unwrap().synced_at.unwrap() >= before + 3600);
        assert_eq!(
            last_scan(SERVER).unwrap().as_deref(),
            Some("2026-10-31T21:26:55Z")
        );
        // A full sync during a scan stores none.
        mark_synced(SERVER, None).unwrap();
        assert_eq!(last_scan(SERVER).unwrap(), None);
    }

    #[test]
    fn stars_from_the_server_replace_the_local_ones() {
        let _temporary = Temporary::new("set-stars");
        let mut starred_album = (*album("a", "A", "X", 2000)).clone();
        starred_album.starred = Some("2020-01-01T00:00:00Z".into());
        save_albums(
            SERVER,
            &[Arc::new(starred_album), album("b", "B", "X", 2000)],
        )
        .unwrap();
        let mut old = song("s1", "Old", "a", 1, 1);
        old.starred = Some("2020-01-01T00:00:00Z".into());
        save_songs(
            SERVER,
            &[
                old,
                song("s2", "New", "a", 1, 2),
                song("s3", "Not", "a", 1, 3),
            ],
        )
        .unwrap();

        // Another account in the same database, with the same ids starred.
        const OTHER: &str = "other@http://server";
        let mut other_album = (*album("a", "A", "X", 2000)).clone();
        other_album.starred = Some("2021-01-01T00:00:00Z".into());
        save_albums(OTHER, &[Arc::new(other_album)]).unwrap();
        let mut other_song = song("s1", "Old", "a", 1, 1);
        other_song.starred = Some("2021-01-01T00:00:00Z".into());
        save_songs(OTHER, &[other_song]).unwrap();

        let mut b = (*album("b", "B", "X", 2000)).clone();
        b.starred = Some("2026-02-01T00:00:00Z".into());
        let mut s2 = song("s2", "New", "a", 1, 2);
        s2.starred = Some("2026-03-01T00:00:00Z".into());
        let starred = subsonic::Starred {
            album: vec![b],
            song: vec![s2],
        };
        set_stars(SERVER, &starred).unwrap();

        let found = load_starred(SERVER).unwrap();
        assert_eq!(found.album_ids, ["b"]);
        assert_eq!(ids(&found.songs), ["s2"]);
        assert_eq!(
            found.songs[0].starred.as_deref(),
            Some("2026-03-01T00:00:00Z")
        );
        // Another account's stars stay.
        let other = load_starred(OTHER).unwrap();
        assert_eq!(other.album_ids, ["a"]);
        assert_eq!(ids(&other.songs), ["s1"]);
    }

    #[test]
    fn the_library_counts_as_unchanged_only_after_the_same_finished_scan() {
        let status = |scanning: bool, last: Option<&str>| subsonic::ScanStatus {
            scanning,
            last_scan: last.map(Into::into),
        };
        let done = status(false, Some("2026-10-31T21:26:55Z"));
        assert!(library_unchanged(Some("2026-10-31T21:26:55Z"), Some(&done)));
        // A newer scan, a scan running, nothing stored, nothing told.
        assert!(!library_unchanged(
            Some("2026-10-30T10:00:00Z"),
            Some(&done)
        ));
        assert!(!library_unchanged(
            Some("2026-10-31T21:26:55Z"),
            Some(&status(true, Some("2026-10-31T21:26:55Z")))
        ));
        assert!(!library_unchanged(None, Some(&done)));
        assert!(!library_unchanged(Some("x"), Some(&status(false, None))));
        assert!(!library_unchanged(Some("x"), None));
        // What a full sync stores.
        assert_eq!(
            scan_to_store(Some(&done)).as_deref(),
            Some("2026-10-31T21:26:55Z")
        );
        assert_eq!(scan_to_store(Some(&status(true, Some("x")))), None);
        assert_eq!(scan_to_store(None), None);
    }

    #[test]
    fn folding_and_queries() {
        assert_eq!(fold("Łódź ĄĆĘŃÓŚŹŻ"), "lodz acenoszz");
        assert_eq!(
            fts_query("chas  pav").as_deref(),
            Some("\"chas\"* \"pav\"*")
        );
        assert_eq!(fts_query("a\"b").as_deref(), Some("\"ab\"*"));
        assert_eq!(fts_query("   "), None);
    }
}
