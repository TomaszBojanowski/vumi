//! Album covers: fetched from the server at display size, cached on disk,
//! decoded off the main thread. Only a few downloads run at once.

use gtk::{gdk, glib};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tokio::sync::Semaphore;

use crate::runtime;

/// Pixel size requested from the server: twice the 160 px card, sharp on HiDPI screens.
const SIZE: u32 = 320;

/// At most this many covers are downloaded at the same time.
const MAX_DOWNLOADS: usize = 6;

/// Inside Flatpak this is ~/.var/app/<app id>/cache/muse-player/covers.
fn cache_dir() -> PathBuf {
    glib::user_cache_dir().join("muse-player").join("covers")
}

fn downloads() -> &'static Semaphore {
    static DOWNLOADS: OnceLock<Semaphore> = OnceLock::new();
    DOWNLOADS.get_or_init(|| Semaphore::new(MAX_DOWNLOADS))
}

/// Which server and account the covers belong to ("user@url"). Known even
/// offline, because the main view sets it from the saved login.
fn current_server() -> &'static Mutex<String> {
    static SERVER: OnceLock<Mutex<String>> = OnceLock::new();
    SERVER.get_or_init(|| Mutex::new(String::new()))
}

pub(crate) fn set_server(key: String) {
    if let Ok(mut server) = current_server().lock() {
        *server = key;
    }
}

/// A stable, collision-free file name: MD5 of server, cover id and size.
/// (Rust's own hasher may change between compiler versions; MD5 never does.)
fn cache_path(server: &str, id: &str) -> PathBuf {
    let digest = md5::compute(format!("{server}\n{id}\n{SIZE}"));
    cache_dir().join(format!("{digest:x}.img"))
}

/// Disk cache first; the server only when connected (`client` is Some).
/// Returns None if there is no usable image.
pub(crate) async fn load(client: Option<subsonic::Client>, id: String) -> Option<gdk::Texture> {
    let server = current_server()
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();

    let result = runtime()
        .spawn(async move {
            let path = cache_path(&server, &id);

            if let Ok(bytes) = tokio::fs::read(&path).await {
                return decode(bytes).await;
            }

            // Not cached and not connected (yet): nothing to show for now.
            let client = client?;

            let _permit = downloads().acquire().await.ok()?;
            let bytes = match client.cover_art(&id, SIZE).await {
                Ok(bytes) => bytes,
                Err(e) => {
                    eprintln!("Could not fetch cover {id}: {e}");
                    return None;
                }
            };

            // Cache only what decodes, so an error reply is never stored as an image.
            let Some(texture) = decode(bytes.clone()).await else {
                let start = String::from_utf8_lossy(&bytes[..bytes.len().min(120)]).to_string();
                eprintln!("Cover {id}: {} bytes that are not an image: {start}", bytes.len());
                return None;
            };
            if let Some(dir) = path.parent() {
                let _ = tokio::fs::create_dir_all(dir).await;
            }
            let _ = tokio::fs::write(&path, &bytes).await;
            Some(texture)
        })
        .await;
    result.ok().flatten()
}

/// Decoding a JPEG takes a few milliseconds, so it runs on a worker thread.
async fn decode(bytes: Vec<u8>) -> Option<gdk::Texture> {
    tokio::task::spawn_blocking(move || {
        gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok()
    })
    .await
    .ok()
    .flatten()
}

/// Loads a cover in the background and hands it to `apply` on the main thread,
/// but only if `owner` still exists by then: the page or card may have been
/// closed while the cover was downloading. Checks that depend on the widget
/// (was it reused for another album?) belong in `apply`.
pub(crate) fn load_for<W>(
    owner: &W,
    client: Option<subsonic::Client>,
    id: String,
    apply: impl FnOnce(&W, Option<gtk::gdk::Texture>) + 'static,
) where
    W: gtk::glib::object::IsA<gtk::glib::Object>,
{
    let owner = gtk::glib::object::ObjectExt::downgrade(owner);
    gtk::glib::spawn_future_local(async move {
        let texture = load(client, id).await;
        if let Some(owner) = owner.upgrade() {
            apply(&owner, texture);
        }
    });
}
