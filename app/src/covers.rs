//! Album covers: fetched from the server at display size, cached on disk,
//! decoded off the main thread. Only a few downloads run at once.

use gtk::{gdk, glib};
use std::path::PathBuf;
use std::sync::OnceLock;
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

/// A file name that is safe whatever characters the server uses in ids.
fn file_name(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!("{safe}-{SIZE}.img")
}

/// Disk cache first; the server only when connected (`client` is Some).
/// Returns None if there is no usable image.
pub(crate) async fn load(client: Option<subsonic::Client>, id: String) -> Option<gdk::Texture> {
    let result = runtime()
        .spawn(async move {
            let path = cache_dir().join(file_name(&id));

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
            let texture = decode(bytes.clone()).await?;
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
