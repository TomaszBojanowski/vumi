//! Local library database (SQLite). The grid is filled from here at startup,
//! before the server is even contacted; syncing keeps it up to date.
//! All functions here block, so callers run them on a worker thread.

use gtk::glib;
use rusqlite::{params, Connection};
use std::path::PathBuf;

/// Inside Flatpak this is ~/.var/app/<app id>/data/muse-player/library.db
fn db_path() -> PathBuf {
    glib::user_data_dir().join("muse-player").join("library.db")
}

fn open() -> rusqlite::Result<Connection> {
    let path = db_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
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
    Ok(conn)
}

/// Albums of one server, in the order the server listed them.
pub(crate) fn load_albums(server: &str) -> rusqlite::Result<Vec<subsonic::Album>> {
    let conn = open()?;
    let mut stmt = conn.prepare(
        "SELECT id, name, artist, year, song_count, cover_art
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
        })
    })?;
    let albums = rows.collect::<rusqlite::Result<Vec<_>>>();
    albums
}

/// Replaces the stored albums of one server in a single transaction.
pub(crate) fn save_albums(server: &str, albums: &[subsonic::Album]) -> rusqlite::Result<()> {
    let mut conn = open()?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM albums WHERE server = ?1", params![server])?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO albums (server, position, id, name, artist, year, song_count, cover_art)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
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
                a.cover_art
            ])?;
        }
    }
    tx.commit()
}
