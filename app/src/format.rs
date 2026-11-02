//! Durations, counts and audio units as text, written the same way on every
//! page.

use gettextrs::{gettext, ngettext, pgettext};

/// Seconds as "m:ss": the length of a track or a place in it.
pub fn clock(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A total length, rounded to the nearest minute: "47 minutes" for most
/// albums, "6 h 9 min" for long ones.
pub fn long_duration(seconds: u32) -> String {
    let minutes = (seconds + 30) / 60;
    if minutes < 60 {
        // Translators: {n} is a number of minutes.
        ngettext("{n} minute", "{n} minutes", minutes).replace("{n}", &minutes.to_string())
    } else {
        hours_and_minutes(minutes)
    }
}

/// A duration in few characters, a started minute counted as a whole one:
/// "12 min", "1 h 5 min".
pub fn short_duration(seconds: u32) -> String {
    let minutes = seconds.div_ceil(60);
    if minutes < 60 {
        // Translators: a short duration in minutes, e.g. "12 min".
        gettext("{n} min").replace("{n}", &minutes.to_string())
    } else {
        hours_and_minutes(minutes)
    }
}

fn hours_and_minutes(minutes: u32) -> String {
    // Translators: a long duration, e.g. "6 h 9 min".
    gettext("{hours} h {minutes} min")
        .replace("{hours}", &(minutes / 60).to_string())
        .replace("{minutes}", &(minutes % 60).to_string())
}

/// "12 tracks".
pub fn tracks_text(n: u32) -> String {
    // Translators: {n} is a number of tracks.
    ngettext("{n} track", "{n} tracks", n).replace("{n}", &grouped(n))
}

/// "3 albums".
pub fn albums_text(n: u32) -> String {
    // Translators: {n} is a number of albums.
    ngettext("{n} album", "{n} albums", n).replace("{n}", &grouped(n))
}

/// A count with its thousands set apart as the language does it: from 1,234
/// on in English, from 10 000 on in Polish.
pub fn grouped(n: u32) -> String {
    let digits = n.to_string();
    // Translators: the fewest digits a number has before its thousands are
    // set apart: "4" groups 1,234 already (English), "5" leaves 1234 as it is
    // and groups 12 345 (Polish). A number, nothing else.
    let fewest = pgettext("digit grouping", "4").trim().parse().unwrap_or(4);
    if digits.len() < fewest {
        return digits;
    }
    // Translators: what separates thousands in a large number ("11,265").
    let separator = pgettext("thousands separator", ",");
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push_str(&separator);
        }
        out.push(digit);
    }
    out
}

/// Audio bit depth: "24 bit".
pub fn bits_text(depth: u32) -> String {
    // Translators: audio bit depth, e.g. "24 bit".
    gettext("{n} bit").replace("{n}", &depth.to_string())
}

/// Sample rate in kHz, with one decimal when it has one: "96 kHz", "44.1 kHz".
pub fn khz_text(rate: u32) -> String {
    let whole = rate / 1000;
    let fraction = (rate % 1000) / 100;
    if fraction == 0 {
        // Translators: sample rate, e.g. "96 kHz".
        gettext("{khz} kHz").replace("{khz}", &whole.to_string())
    } else {
        // Translators: sample rate with one decimal, e.g. "44.1 kHz"; use your decimal separator.
        gettext("{whole}.{fraction} kHz")
            .replace("{whole}", &whole.to_string())
            .replace("{fraction}", &fraction.to_string())
    }
}

/// Audio bit rate: "320 kbps".
pub fn kbps_text(kbps: u32) -> String {
    // Translators: audio bit rate, e.g. "320 kbps".
    gettext("{kbps} kbps").replace("{kbps}", &kbps.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // No translations in tests: the texts are the English ones.

    #[test]
    fn clock_shows_minutes_and_two_digit_seconds() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(9), "0:09");
        assert_eq!(clock(245), "4:05");
        assert_eq!(clock(3725), "62:05");
    }

    #[test]
    fn long_duration_rounds_to_the_nearest_minute() {
        assert_eq!(long_duration(29), "0 minutes");
        assert_eq!(long_duration(30), "1 minute");
        assert_eq!(long_duration(47 * 60 + 29), "47 minutes");
        assert_eq!(long_duration(47 * 60 + 30), "48 minutes");
        assert_eq!(long_duration(59 * 60 + 30), "1 h 0 min");
        assert_eq!(long_duration(6 * 3600 + 9 * 60), "6 h 9 min");
    }

    #[test]
    fn short_duration_counts_a_started_minute() {
        assert_eq!(short_duration(0), "0 min");
        assert_eq!(short_duration(1), "1 min");
        assert_eq!(short_duration(12 * 60), "12 min");
        assert_eq!(short_duration(12 * 60 + 1), "13 min");
        assert_eq!(short_duration(59 * 60 + 1), "1 h 0 min");
        assert_eq!(short_duration(65 * 60), "1 h 5 min");
    }

    #[test]
    fn counts_take_the_plural_and_group_thousands() {
        assert_eq!(tracks_text(1), "1 track");
        assert_eq!(tracks_text(12), "12 tracks");
        assert_eq!(tracks_text(1234), "1,234 tracks");
        assert_eq!(albums_text(1), "1 album");
        assert_eq!(albums_text(0), "0 albums");
    }

    #[test]
    fn thousands_are_grouped_as_the_language_groups_them() {
        // English groups from four digits on.
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1234), "1,234");
        assert_eq!(grouped(11_333), "11,333");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn audio_units() {
        assert_eq!(bits_text(24), "24 bit");
        assert_eq!(khz_text(96_000), "96 kHz");
        assert_eq!(khz_text(44_100), "44.1 kHz");
        assert_eq!(khz_text(88_200), "88.2 kHz");
        assert_eq!(kbps_text(320), "320 kbps");
    }
}
