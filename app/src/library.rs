//! Local library database (SQLite). The grid and album pages are filled from
//! here, before the server is even contacted; syncing keeps it up to date.
//! All functions here block, so callers run them on a worker thread.
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
    Ok(())
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

/// Replaces the stored albums of one server in a single transaction.
pub(crate) fn save_albums(server: &str, albums: &[Arc<subsonic::Album>]) -> rusqlite::Result<()> {
    let mut conn = open()?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM albums WHERE server = ?1", params![server])?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO albums
                 (server, position, id, name, artist, year, song_count, cover_art, created,
                  artist_id, starred)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
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
        }
    }
    tx.commit()
}

/// Replaces the stored songs of one server in a single transaction.
pub(crate) fn save_songs(server: &str, songs: &[subsonic::Song]) -> rusqlite::Result<()> {
    let mut conn = open()?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM songs WHERE server = ?1", params![server])?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO songs
                 (server, position, id, album_id, title, artist, album, track,
                  disc_number, duration, suffix, bit_rate, bit_depth, sampling_rate,
                  cover_art, starred)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        )?;
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
        }
    }
    tx.commit()
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
    let rows = stmt.query_map(params![server, album_id], |row| {
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
    })?;
    let songs = rows.collect::<rusqlite::Result<Vec<_>>>();
    songs
}
