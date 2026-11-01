//! When something happened, in words: "today", "a week ago", "in August".

use gettextrs::{gettext, ngettext};
use gtk::glib;

/// How long ago `date` (ISO 8601, as the server sends a star's date) was, by
/// the calendar: today, yesterday, 2-6 days, a week (7-13 days), 2-3 weeks;
/// further back the month (within a year) or the year. Empty when the date
/// cannot be read.
pub(crate) fn starred_ago(date: &str) -> String {
    let (Ok(then), Ok(now)) = (
        glib::DateTime::from_iso8601(date, None).and_then(|then| then.to_local()),
        glib::DateTime::now_local(),
    ) else {
        return String::new();
    };
    // The number of the day in local time, so "yesterday" follows midnight.
    let day =
        |time: &glib::DateTime| (time.to_unix() + time.utc_offset().as_seconds()).div_euclid(86400);
    let days = (day(&now) - day(&then)).max(0);
    match days {
        0 => gettext("today"),
        1 => gettext("yesterday"),
        2..=6 => {
            ngettext("{n} day ago", "{n} days ago", days as u32).replace("{n}", &days.to_string())
        }
        7..=13 => gettext("a week ago"),
        14..=27 => {
            let weeks = (days / 7) as u32;
            ngettext("{n} week ago", "{n} weeks ago", weeks).replace("{n}", &weeks.to_string())
        }
        _ => {
            let months = (now.year() - then.year()) * 12 + now.month() - then.month();
            if months < 12 {
                month(then.month())
            } else {
                // Translators: {year} is a year, e.g. "in 2024".
                gettext("in {year}").replace("{year}", &then.year().to_string())
            }
        }
    }
}

/// "in August": each month a string of its own, as languages inflect them.
fn month(month: i32) -> String {
    match month {
        1 => gettext("in January"),
        2 => gettext("in February"),
        3 => gettext("in March"),
        4 => gettext("in April"),
        5 => gettext("in May"),
        6 => gettext("in June"),
        7 => gettext("in July"),
        8 => gettext("in August"),
        9 => gettext("in September"),
        10 => gettext("in October"),
        11 => gettext("in November"),
        _ => gettext("in December"),
    }
}
