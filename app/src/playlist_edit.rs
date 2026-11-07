//! What the logged-in user may change in a playlist.

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
}
