//! What a screen reader says for Vumi's own parts. A card or a row says what it
//! is in one name ("Adele, 5 albums"), the states that matter go with it, and
//! pictures that only decorate stay out of it (GNOME's accessibility guidelines).
//!
//! The screen's separators become commas: a voice pauses at a comma, while
//! " • " (between artists, from the server) or " · " (between details) is read
//! as a word or not at all.

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
