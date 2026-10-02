//! Album covers: fetched from the server at a size that suits the screen
//! (see SIZE), cached on disk,
//! decoded off the main thread. Only a few downloads run at once.

use gtk::prelude::*;
use gtk::{gdk, glib};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use tokio::sync::Semaphore;

use crate::runtime;

/// Pixel size requested from the server (the longer side). The largest covers
/// are about 256 px in the app's own units (the player card is 252, a card in
/// the album grid up to about 250), so 320 px has a pixel for every device
/// pixel up to a screen scale of 1.25; a screen with a larger scale gets
/// 640 px. Each size has its own files in the cache.
static SIZE: AtomicU32 = AtomicU32::new(320);

/// The size to ask for now.
fn size() -> u32 {
    SIZE.load(Ordering::Relaxed)
}

/// Picks the size by the largest scale among the screens. Main thread only.
fn update_size() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let scale = display
        .monitors()
        .iter::<gdk::Monitor>()
        .flatten()
        .map(|monitor| monitor.scale())
        .fold(1.0, f64::max);
    SIZE.store(if 256.0 * scale > 320.0 { 640 } else { 320 }, Ordering::Relaxed);
}

/// At most this many covers are downloaded at the same time.
const MAX_DOWNLOADS: usize = 6;

/// Inside Flatpak this is ~/.var/app/<app id>/cache/vumi/covers.
fn cache_dir() -> PathBuf {
    glib::user_cache_dir().join("vumi").join("covers")
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
    let digest = md5::compute(format!("{server}\n{id}\n{}", size()));
    cache_dir().join(format!("{digest:x}.img"))
}

/// Disk cache first; the server only when connected (`client` is Some).
/// Returns None if there is no usable image.
pub(crate) async fn load(client: Option<subsonic::Client>, id: String) -> Option<gdk::Texture> {
    let server = current_server()
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default();

    let size = size();
    let key = format!("{server}\n{id}\n{size}");
    if let Some(texture) = memory_get(&key) {
        return Some(texture);
    }

    let result = runtime()
        .spawn(async move {
            let path = cache_path(&server, &id);

            if let Ok(bytes) = tokio::fs::read(&path).await {
                if let Some(texture) = decode(bytes).await {
                    return Some(texture);
                }
                // A broken file (a disk error, or a save cut off by an older
                // version): remove it and fetch the cover again below.
                eprintln!("Cover {id}: cached file is broken, fetching it again");
                let _ = tokio::fs::remove_file(&path).await;
            }

            // Not cached and not connected (yet): nothing to show for now.
            let client = client?;

            let _permit = downloads().acquire().await.ok()?;
            let bytes = match client.cover_art(&id, size).await {
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
            // Written to a temporary file of its own and renamed: a rename is
            // atomic, so a save cut off halfway never leaves a broken file under
            // the real name, and two downloads of the same cover never write
            // into one temporary file.
            let temporary = path.with_extension(format!("part-{}", fastrand::u64(..)));
            if tokio::fs::write(&temporary, &bytes).await.is_ok() {
                let _ = tokio::fs::rename(&temporary, &path).await;
            }
            Some(texture)
        })
        .await;
    let texture = result.ok().flatten();
    if let Some(texture) = &texture {
        memory_put(key, texture.clone());
    }
    texture
}

/// Covers come back often (the Start page, the album grid, the player card,
/// MPRIS); decoded textures are kept in memory up to this many bytes of pixels,
/// and the least recently used go first.
const MEMORY_LIMIT: usize = 128 * 1024 * 1024;

#[derive(Default)]
struct Memory {
    textures: std::collections::HashMap<String, gdk::Texture>,
    order: std::collections::VecDeque<String>,
    bytes: usize,
}

// Shared by all threads: clearing the cache runs on a worker thread.
static MEMORY: std::sync::LazyLock<std::sync::Mutex<Memory>> =
    std::sync::LazyLock::new(Default::default);

fn texture_bytes(texture: &gdk::Texture) -> usize {
    use gtk::gdk::prelude::TextureExt as _;
    texture.width().max(0) as usize * texture.height().max(0) as usize * 4
}

fn memory_get(key: &str) -> Option<gdk::Texture> {
    let mut memory = MEMORY.lock().ok()?;
    let texture = memory.textures.get(key)?.clone();
    if let Some(index) = memory.order.iter().position(|k| k == key) {
        if let Some(k) = memory.order.remove(index) {
            memory.order.push_back(k);
        }
    }
    Some(texture)
}

fn memory_put(key: String, texture: gdk::Texture) {
    let Ok(mut memory) = MEMORY.lock() else {
        return;
    };
    if memory.textures.contains_key(&key) {
        return;
    }
    memory.bytes += texture_bytes(&texture);
    memory.order.push_back(key.clone());
    memory.textures.insert(key, texture);
    while memory.bytes > MEMORY_LIMIT {
        let Some(oldest) = memory.order.pop_front() else {
            break;
        };
        if let Some(old) = memory.textures.remove(&oldest) {
            memory.bytes -= texture_bytes(&old);
        }
    }
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
    update_size();
    let owner = gtk::glib::object::ObjectExt::downgrade(owner);
    gtk::glib::spawn_future_local(async move {
        let texture = load(client, id).await;
        if let Some(owner) = owner.upgrade() {
            apply(&owner, texture);
        }
    });
}

/// Shows `texture` in `picture`, a cover in its square frame. The picture
/// draws the texture itself, at the screen's own resolution, and whole
/// (`content-fit: contain` in the templates): a cover that is not square is
/// never cropped, and the rest of the frame is then a neutral dark
/// (style.css: picture.letterbox).
pub(crate) fn show(picture: &gtk::Picture, texture: Option<&gdk::Texture>) {
    picture.set_paintable(texture);
    if texture.is_some_and(|texture| texture.width() != texture.height()) {
        picture.add_css_class("letterbox");
    } else {
        picture.remove_css_class("letterbox");
    }
}

/// Where the cover `id` is cached on disk, if it is. MPRIS hands this path to
/// GNOME Shell, which shows the cover in its media controls.
pub(crate) fn cached_path(id: &str) -> Option<PathBuf> {
    let server = current_server().lock().ok()?.clone();
    let path = cache_path(&server, id);
    path.exists().then_some(path)
}

/// Size of the cover cache on disk, in bytes (Preferences). Blocks: call it
/// on a worker thread.
pub(crate) fn cache_size() -> u64 {
    std::fs::read_dir(cache_dir())
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.metadata().ok())
                .filter(|meta| meta.is_file())
                .map(|meta| meta.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Removes every cached cover (Preferences); covers are fetched again when
/// needed. Blocks: call it on a worker thread.
pub(crate) fn clear_cache() -> std::io::Result<()> {
    if let Ok(mut memory) = MEMORY.lock() {
        *memory = Memory::default();
    }
    for entry in std::fs::read_dir(cache_dir())?.flatten() {
        if entry.path().is_file() {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}
