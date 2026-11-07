//! What may be changed in a playlist, and the checks that keep its songs
//! whole on the server.
//!
//! Subsonic has no call to move a song: a move sends the whole new list,
//! which replaces the old one (createPlaylist with the playlist's id). So a
//! list is changed only when the server gave all of it (Navidrome leaves out
//! songs whose file is missing, but counts them), only after a fresh fetch
//! shows the same list as the page, and only with a new list that has the
//! same songs as before (a move) or one song put back (Undo). Removing a
//! song uses its own call (updatePlaylist), by an index that is right because
//! of the same checks.

/// What the logged-in user may do with a playlist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rights {
    /// Rename and delete it.
    pub(crate) manage: bool,
    /// Remove and move its songs.
    pub(crate) songs: bool,
}

/// The rights of `user` on a playlist made by `owner`. `readonly` (an
/// OpenSubsonic field) says whether its songs can be changed: Navidrome sets
/// it for smart playlists, ones kept in step with a file and other users'
/// playlists. A server that says neither is asked anyway: its refusal comes
/// as a note.
pub(crate) fn rights(owner: Option<&str>, readonly: Option<bool>, user: &str) -> Rights {
    // Navidrome takes the user name in any case at login.
    let own = owner.is_none_or(|owner| owner.to_lowercase() == user.to_lowercase());
    Rights {
        manage: own,
        songs: readonly.map_or(own, |readonly| !readonly),
    }
}

/// Whether the server gave every song of the playlist (`given` of
/// `song_count`). Navidrome leaves out songs whose file is missing or in a
/// library the user cannot see, but counts them; the songs of such a list
/// are not changed, or a replace would drop the ones not shown.
pub(crate) fn whole(given: usize, song_count: Option<u32>) -> bool {
    song_count.is_none_or(|count| count as usize == given)
}

/// One change of the songs, as the page asks for it. Indexes count from 0
/// in the list before the change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Edit<T> {
    /// Takes out the song at this index.
    Remove(usize),
    /// Moves the song at `from` so that it stands at `to`.
    Move { from: usize, to: usize },
    /// Puts a removed song back at `index` (Undo).
    Restore { index: usize, song: T },
}

impl<T: Clone> Edit<T> {
    /// The list after the change; None when the change does not fit it.
    pub(crate) fn apply(&self, list: &[T]) -> Option<Vec<T>> {
        let mut after = list.to_vec();
        match self {
            Edit::Remove(index) => {
                if *index >= after.len() {
                    return None;
                }
                after.remove(*index);
            }
            Edit::Move { from, to } => {
                if *from >= after.len() || *to >= after.len() || from == to {
                    return None;
                }
                let song = after.remove(*from);
                after.insert(*to, song);
            }
            Edit::Restore { index, song } => {
                if *index > after.len() {
                    return None;
                }
                after.insert(*index, song.clone());
            }
        }
        Some(after)
    }

    /// The same change with `song` mapped (a song to its id, say).
    pub(crate) fn map<'a, U>(&'a self, song: impl FnOnce(&'a T) -> U) -> Edit<U> {
        match self {
            Edit::Remove(index) => Edit::Remove(*index),
            Edit::Move { from, to } => Edit::Move {
                from: *from,
                to: *to,
            },
            Edit::Restore { index, song: s } => Edit::Restore {
                index: *index,
                song: song(s),
            },
        }
    }
}

/// Whether `new` may replace `old` on the server for `edit`: never empty;
/// a move keeps every song as many times as before, only the order changes;
/// a restore is `old` with that one song back in its place. Anything else
/// (a mistake in building the list) is not sent: it could cut the playlist.
/// A removal never replaces the list.
pub(crate) fn safe_replace(edit: &Edit<&str>, old: &[&str], new: &[&str]) -> bool {
    if new.is_empty() || edit.apply(old).as_deref() != Some(new) {
        return false;
    }
    match edit {
        Edit::Remove(_) => false,
        Edit::Move { .. } => {
            let (mut a, mut b) = (old.to_vec(), new.to_vec());
            a.sort_unstable();
            b.sort_unstable();
            a == b
        }
        Edit::Restore { index, song } => {
            new.len() == old.len() + 1 && new.get(*index) == Some(song) && {
                let mut back = new.to_vec();
                back.remove(*index);
                back == old
            }
        }
    }
}

/// The move made by dropping the song dragged from `from` on the row at
/// `target`: it takes that row's place, as in the queue. None when dropped
/// on itself.
pub(crate) fn dropped<T>(from: usize, target: usize) -> Option<Edit<T>> {
    (from != target).then_some(Edit::Move { from, to: target })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_may_do_all_others_nothing() {
        let all = Rights {
            manage: true,
            songs: true,
        };
        assert_eq!(rights(Some("anna"), Some(false), "anna"), all);
        assert_eq!(rights(Some("Anna"), Some(false), "anna"), all);
        assert_eq!(rights(Some("bob"), Some(true), "anna"), Rights::default());
    }

    #[test]
    fn a_smart_playlist_can_be_renamed_but_not_edited() {
        assert_eq!(
            rights(Some("anna"), Some(true), "anna"),
            Rights {
                manage: true,
                songs: false
            }
        );
    }

    #[test]
    fn without_readonly_the_owner_decides() {
        assert!(rights(Some("anna"), None, "anna").songs);
        assert!(!rights(Some("bob"), None, "anna").songs);
        assert!(rights(None, None, "anna").songs);
    }

    #[test]
    fn a_list_is_whole_when_every_counted_song_is_given() {
        assert!(whole(3, Some(3)));
        assert!(whole(3, None));
        // Navidrome: a missing file is counted, not given.
        assert!(!whole(2, Some(3)));
    }

    #[test]
    fn changes_apply_by_index_and_keep_repeats() {
        let list = ["a", "b", "a", "c"];
        assert_eq!(Edit::Remove(2).apply(&list), Some(vec!["a", "b", "c"]));
        assert_eq!(
            Edit::Move { from: 3, to: 0 }.apply(&list),
            Some(vec!["c", "a", "b", "a"])
        );
        assert_eq!(
            Edit::Move { from: 0, to: 2 }.apply(&list),
            Some(vec!["b", "a", "a", "c"])
        );
        assert_eq!(
            Edit::Restore {
                index: 1,
                song: "x"
            }
            .apply(&list),
            Some(vec!["a", "x", "b", "a", "c"])
        );
    }

    #[test]
    fn changes_that_do_not_fit_the_list_are_none() {
        let list = ["a", "b"];
        assert_eq!(Edit::Remove(2).apply(&list), None);
        assert_eq!(Edit::Move { from: 0, to: 2 }.apply(&list), None);
        assert_eq!(Edit::Move { from: 1, to: 1 }.apply(&list), None);
        assert_eq!(
            Edit::Restore {
                index: 3,
                song: "x"
            }
            .apply(&list),
            None
        );
    }

    #[test]
    fn undo_of_a_removal_gives_the_list_before_it() {
        let list = ["a", "b", "a", "c"];
        let after = Edit::Remove(2).apply(&list).unwrap();
        let back = Edit::Restore {
            index: 2,
            song: "a",
        }
        .apply(&after)
        .unwrap();
        assert_eq!(back, list);
    }

    #[test]
    fn a_move_may_replace_the_list() {
        let old = ["a", "b", "a", "c"];
        let edit = Edit::Move { from: 3, to: 1 };
        let new = edit.apply(&old).unwrap();
        assert!(safe_replace(&edit, &old, &new));
    }

    #[test]
    fn a_list_that_lost_or_gained_a_song_is_not_sent() {
        let old = ["a", "b", "a", "c"];
        let edit = Edit::Move { from: 3, to: 1 };
        assert!(!safe_replace(&edit, &old, &["a", "c", "b"]));
        assert!(!safe_replace(&edit, &old, &["a", "c", "b", "a", "a"]));
        assert!(!safe_replace(&edit, &old, &["a", "c", "b", "b"]));
        assert!(!safe_replace(&edit, &old, &[]));
        // Right songs, but not this move.
        assert!(!safe_replace(&edit, &old, &["c", "a", "b", "a"]));
    }

    #[test]
    fn a_restore_puts_back_exactly_one_song() {
        let old = ["a", "c"];
        let edit = Edit::Restore {
            index: 1,
            song: "b",
        };
        assert!(safe_replace(&edit, &old, &["a", "b", "c"]));
        assert!(!safe_replace(&edit, &old, &["a", "b"]));
        assert!(!safe_replace(&edit, &old, &["b", "a", "c"]));
    }

    #[test]
    fn a_removal_never_replaces_the_list() {
        let old = ["a", "b"];
        assert!(!safe_replace(&Edit::Remove(0), &old, &["b"]));
    }

    #[test]
    fn a_dropped_song_takes_the_place_of_its_row() {
        let list = ["a", "b", "c", "d"];
        let down = dropped(1, 3).unwrap().apply(&list);
        assert_eq!(down, Some(vec!["a", "c", "d", "b"]));
        let up = dropped(3, 0).unwrap().apply(&list);
        assert_eq!(up, Some(vec!["d", "a", "b", "c"]));
        assert_eq!(dropped::<&str>(2, 2), None);
    }
}
