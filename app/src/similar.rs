//! Similar songs as the server gives them. getSimilarSongs works on any server
//! that answers it: with AudioMuse-AI full lists, through Navidrome's metadata
//! agents (Last.fm and the like) thinner ones, sometimes none. So a radio starts
//! only with enough songs, the "Similar to …" shelf shows only with enough
//! albums, and while the server gives none at all the features are hidden. That
//! is checked again after every sync: they come back by themselves (AudioMuse-AI
//! installed later, say).

use std::cell::{Cell, RefCell};

/// A radio needs at least this many songs besides the one it starts from.
pub(crate) const MIN_RADIO: usize = 5;

/// The "Similar to …" shelf needs at least this many albums.
pub(crate) const MIN_SHELF: usize = 3;

/// Songs asked for when checking whether the server gives any.
const PROBE_COUNT: u32 = 10;

/// What a radio can do with the server's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Radio {
    Play,
    /// 1 to MIN_RADIO - 1 songs: nothing plays, a note says why.
    TooFew,
    NoneFound,
}

/// The verdict for a radio from `seed`, given the songs the server `found`:
/// the seed itself and repeats do not count.
pub(crate) fn radio(seed: &str, found: &[subsonic::Song]) -> Radio {
    let mut ids: Vec<&str> = found
        .iter()
        .map(|song| song.id.as_str())
        .filter(|id| *id != seed)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    match ids.len() {
        0 => Radio::NoneFound,
        n if n < MIN_RADIO => Radio::TooFew,
        _ => Radio::Play,
    }
}

/// Whether the shelf shows with `albums` albums.
pub(crate) fn shelf_shows(albums: usize) -> bool {
    albums >= MIN_SHELF
}

/// The server's answers to the probe, in order (Some(n): n songs, None: the
/// request failed): Some(true) as soon as one gave songs, Some(false) when all
/// answered and gave none, None (keep what is known) otherwise.
pub(crate) fn probe_verdict(answers: &[Option<usize>]) -> Option<bool> {
    if answers.iter().any(|a| a.is_some_and(|n| n > 0)) {
        return Some(true);
    }
    if !answers.is_empty() && answers.iter().all(|a| *a == Some(0)) {
        return Some(false);
    }
    None
}

/// Hears whether the server gives similar songs; false: no longer wanted.
type Watcher = Box<dyn Fn(bool) -> bool>;

thread_local! {
    /// Whether the server gives similar songs; true until a check says not.
    static AVAILABLE: Cell<bool> = const { Cell::new(true) };
    static WATCHERS: RefCell<Vec<Watcher>> = const { RefCell::new(Vec::new()) };
}

/// Whether the features are shown: the server gave similar songs at the last
/// check, or has not been checked yet.
pub(crate) fn available() -> bool {
    AVAILABLE.with(Cell::get)
}

/// `watcher` hears every change of available() until it returns false.
pub(crate) fn watch(watcher: impl Fn(bool) -> bool + 'static) {
    WATCHERS.with(|watchers| watchers.borrow_mut().push(Box::new(watcher)));
}

fn set_available(available: bool) {
    if AVAILABLE.with(|a| a.replace(available)) == available {
        return;
    }
    eprintln!(
        "Similar songs: {}",
        if available {
            "the server gives them"
        } else {
            "the server gives none, the features are hidden"
        }
    );
    // Taken out while they run, so one may add another.
    let watchers = WATCHERS.with(|w| std::mem::take(&mut *w.borrow_mut()));
    let kept: Vec<Watcher> = watchers.into_iter().filter(|w| w(available)).collect();
    WATCHERS.with(|w| {
        let mut w = w.borrow_mut();
        let added = std::mem::take(&mut *w);
        *w = kept;
        w.extend(added);
    });
}

/// After a sync: asks the server for songs similar to `seeds` (up to three,
/// one after another, in the background) and shows or hides the features by
/// the answers (probe_verdict). One request when the server gives songs.
pub(crate) fn check(client: subsonic::Client, seeds: Vec<String>) {
    if seeds.is_empty() {
        return;
    }
    gtk::glib::spawn_future_local(async move {
        let mut answers = Vec::new();
        for seed in seeds.into_iter().take(3) {
            let request = client.clone();
            let found = crate::runtime()
                .spawn(async move { request.similar_songs(&seed, PROBE_COUNT).await })
                .await;
            let answer = match found {
                Ok(Ok(songs)) => Some(songs.len()),
                Ok(Err(e)) => {
                    eprintln!("Could not check similar songs: {e}");
                    None
                }
                Err(_) => None,
            };
            answers.push(answer);
            if answer.is_some_and(|n| n > 0) || answer.is_none() {
                break;
            }
        }
        if let Some(available) = probe_verdict(&answers) {
            set_available(available);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn songs(ids: &[&str]) -> Vec<subsonic::Song> {
        ids.iter()
            .map(|id| subsonic::Song {
                id: (*id).into(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn a_radio_needs_five_songs_besides_its_first() {
        assert_eq!(radio("s", &songs(&[])), Radio::NoneFound);
        assert_eq!(radio("s", &songs(&["s"])), Radio::NoneFound);
        assert_eq!(radio("s", &songs(&["a"])), Radio::TooFew);
        assert_eq!(
            radio("s", &songs(&["s", "a", "b", "c", "d"])),
            Radio::TooFew
        );
        assert_eq!(radio("s", &songs(&["a", "b", "c", "d", "e"])), Radio::Play);
        let fifty: Vec<String> = (0..50).map(|n| n.to_string()).collect();
        let fifty: Vec<&str> = fifty.iter().map(String::as_str).collect();
        assert_eq!(radio("s", &songs(&fifty)), Radio::Play);
    }

    #[test]
    fn repeats_do_not_count_for_a_radio() {
        assert_eq!(
            radio("s", &songs(&["a", "a", "b", "b", "c"])),
            Radio::TooFew
        );
    }

    #[test]
    fn the_shelf_needs_three_albums() {
        assert!(!shelf_shows(0));
        assert!(!shelf_shows(2));
        assert!(shelf_shows(3));
        assert!(shelf_shows(8));
    }

    #[test]
    fn the_server_gives_similar_songs_once_one_answer_has_some() {
        assert_eq!(probe_verdict(&[Some(10)]), Some(true));
        assert_eq!(probe_verdict(&[Some(0), Some(3)]), Some(true));
    }

    #[test]
    fn the_server_gives_none_only_when_every_answer_is_empty() {
        assert_eq!(probe_verdict(&[Some(0), Some(0), Some(0)]), Some(false));
        assert_eq!(probe_verdict(&[Some(0), None]), None);
        assert_eq!(probe_verdict(&[None]), None);
        assert_eq!(probe_verdict(&[]), None);
    }
}
