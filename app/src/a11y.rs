//! What a screen reader says for Vumi's own parts. A card or a row says what it
//! is in one name ("Adele, 5 albums"), the states that matter go with it, and
//! pictures that only decorate stay out of it (GNOME's accessibility guidelines).
//!
//! The screen's separators become commas: a voice pauses at a comma, while
//! " • " (between artists, from the server) or " · " (between details) is read
//! as a word or not at all.

use gettextrs::pgettext;

/// Whether a song is the player's current one, and how.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Now {
    #[default]
    Not,
    Playing,
    Paused,
}

/// A song row's name: `shown` (all the row shows, joined) and the states that
/// hold, "Água de Beber, Astrud Gilberto, 2:18, playing, favorite".
pub(crate) fn song(shown: &str, now: Now, favorite: bool) -> String {
    let now = match now {
        // Translators: said by a screen reader after a song's name, "Song,
        // Artist, 2:18, playing"; an adjective for the song.
        Now::Playing => Some(pgettext("song state", "playing")),
        // Translators: as "playing", for the current song while paused.
        Now::Paused => Some(pgettext("song state", "paused")),
        Now::Not => None,
    };
    // Translators: said by a screen reader after a song's name when it is a
    // favourite; an adjective for the song.
    let favorite = favorite.then(|| pgettext("song state", "favorite"));
    join(
        [Some(shown), now.as_deref(), favorite.as_deref()]
            .into_iter()
            .flatten(),
    )
}

/// `text` with the screen's separators as commas.
pub(crate) fn spoken(text: &str) -> String {
    text.replace(" • ", ", ").replace(" · ", ", ")
}

/// The parts of one name, joined by commas, the empty ones left out:
/// "19, Adele".
pub(crate) fn join<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(spoken)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_its_parts_with_commas() {
        assert_eq!(join(["19", "Adele"]), "19, Adele");
        assert_eq!(join(["Adele", "5 albums"]), "Adele, 5 albums");
    }

    #[test]
    fn empty_parts_are_left_out() {
        assert_eq!(join(["Night Songs", ""]), "Night Songs");
        assert_eq!(join(["", " ", "Calm Indie"]), "Calm Indie");
    }

    #[test]
    fn a_song_says_what_it_shows_then_its_states() {
        let shown = join(["Água de Beber", "Astrud Gilberto", "2:18"]);
        assert_eq!(
            song(&shown, Now::Not, false),
            "Água de Beber, Astrud Gilberto, 2:18"
        );
        assert_eq!(
            song(&shown, Now::Playing, true),
            "Água de Beber, Astrud Gilberto, 2:18, playing, favorite"
        );
        assert_eq!(
            song(&shown, Now::Paused, false),
            "Água de Beber, Astrud Gilberto, 2:18, paused"
        );
        assert_eq!(song("City Sleep", Now::Not, true), "City Sleep, favorite");
    }

    #[test]
    fn the_screens_separators_are_read_as_commas() {
        assert_eq!(
            join([
                "Adiemus - Songs Of Sanctuary",
                "Adiemus • Karl Jenkins • London Philharmonic Orchestra"
            ]),
            "Adiemus - Songs Of Sanctuary, Adiemus, Karl Jenkins, London Philharmonic Orchestra"
        );
        assert_eq!(
            spoken("Stan Getz • Astrud Gilberto · António Carlos Jobim"),
            "Stan Getz, Astrud Gilberto, António Carlos Jobim"
        );
        // A dot inside a word or a name stays.
        assert_eq!(spoken("Vol. 1·2"), "Vol. 1·2");
    }
}
