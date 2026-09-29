//! Local library database (SQLite). The grid, album pages and search are
//! filled from here, before the server is even contacted; syncing keeps it up
//! to date. All functions here block, so callers run them on a worker thread.
//!
//! Schema changes are migrations keyed by PRAGMA user_version, each in its own
//! transaction: an interrupted migration leaves the database as it was, and the
//! next start simply runs it again.

use gtk::glib;
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::Arc;

/// Inside Flatpak this is ~/.var/app/<app id>/data/muse-player/library.db
fn db_path() -> PathBuf {
    glib::user_data_dir().join("muse-player").join("library.db")
}

fn open() -> rusqlite::Result<Connection> {
    let path = db_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut conn = Connection::open(path)?;
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
    Ok(())
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
    let conn = open()?;
    let mut stmt = conn.prepare(
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
    let albums = rows.collect::<rusqlite::Result<Vec<_>>>();
    albums
}

/// Replaces the stored albums of one server, and their search index, in a
/// single transaction.
pub(crate) fn save_albums(
    server: &str,
    albums: &[Arc<subsonic::Album>],
) -> rusqlite::Result<()> {
    let mut conn = open()?;
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
    let mut conn = open()?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM songs WHERE server = ?1", params![server])?;
    tx.execute("DELETE FROM songs_fts WHERE server = ?1", params![server])?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO songs
                 (server, position, id, album_id, title, artist, album, track,
                  disc_number, duration, suffix, bit_rate, bit_depth, sampling_rate,
                  cover_art, starred)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
                s.starred
            ])?;
            let text = fold(&format!(
                "{} {} {}",
                s.title,
                s.artist.as_deref().unwrap_or(""),
                s.album.as_deref().unwrap_or("")
            ));
            index.execute(params![server, s.id, text])?;
        }
    }
    tx.commit()
}

/// A song read from a row with these columns, in this order: id, title,
/// artist, track, disc_number, album, album_id, duration, suffix, bit_rate,
/// cover_art, bit_depth, sampling_rate, starred.
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
    })
}

/// The songs of one album, disc by disc and track by track.
pub(crate) fn load_album_songs(
    server: &str,
    album_id: &str,
) -> rusqlite::Result<Vec<subsonic::Song>> {
    let conn = open()?;
    let mut stmt = conn.prepare(
        "SELECT id, title, artist, track, disc_number, album, album_id, duration,
                suffix, bit_rate, cover_art, bit_depth, sampling_rate, starred
         FROM songs WHERE server = ?1 AND album_id = ?2
         ORDER BY COALESCE(disc_number, 1), COALESCE(track, 0), position",
    )?;
    let rows = stmt.query_map(params![server, album_id], song_from_row)?;
    let songs = rows.collect::<rusqlite::Result<Vec<_>>>();
    songs
}

/// What a search found: album ids (the albums themselves are in memory) and
/// songs, best matches first.
#[allow(dead_code)] // Used by the search page, the next step.
pub(crate) struct SearchResults {
    pub(crate) album_ids: Vec<String>,
    pub(crate) songs: Vec<subsonic::Song>,
}

/// Searches albums and songs of one server; every word of `query` must match
/// the start of a word, ignoring case and accents ("zolw" finds "Żółw").
#[allow(dead_code)] // Used by the search page, the next step.
pub(crate) fn search(server: &str, query: &str, limit: usize) -> rusqlite::Result<SearchResults> {
    let Some(fts) = fts_query(query) else {
        return Ok(SearchResults {
            album_ids: Vec::new(),
            songs: Vec::new(),
        });
    };
    let conn = open()?;

    let mut stmt = conn.prepare(
        "SELECT id FROM albums_fts
         WHERE albums_fts MATCH ?2 AND server = ?1
         ORDER BY rank LIMIT ?3",
    )?;
    let album_ids = stmt
        .query_map(params![server, fts, limit as i64], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;

    let mut stmt = conn.prepare(
        "SELECT s.id, s.title, s.artist, s.track, s.disc_number, s.album, s.album_id,
                s.duration, s.suffix, s.bit_rate, s.cover_art, s.bit_depth,
                s.sampling_rate, s.starred
         FROM songs_fts f JOIN songs s ON s.server = f.server AND s.id = f.id
         WHERE songs_fts MATCH ?2 AND f.server = ?1
         ORDER BY f.rank LIMIT ?3",
    )?;
    let songs = stmt
        .query_map(params![server, fts, limit as i64], song_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(SearchResults { album_ids, songs })
}
