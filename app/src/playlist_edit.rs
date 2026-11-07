//! What the logged-in user may change in a playlist.

/// What the logged-in user may do with a playlist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rights {
    /// Add, remove and move its songs.
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
        songs: readonly.map_or(own, |readonly| !readonly),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_may_change_the_songs_others_not() {
        assert!(rights(Some("anna"), Some(false), "anna").songs);
        assert!(rights(Some("Anna"), Some(false), "anna").songs);
        assert!(!rights(Some("bob"), Some(true), "anna").songs);
    }

    #[test]
    fn a_smart_playlist_takes_no_songs() {
        assert!(!rights(Some("anna"), Some(true), "anna").songs);
    }

    #[test]
    fn without_readonly_the_owner_decides() {
        assert!(rights(Some("anna"), None, "anna").songs);
        assert!(!rights(Some("bob"), None, "anna").songs);
        assert!(rights(None, None, "anna").songs);
    }
}
