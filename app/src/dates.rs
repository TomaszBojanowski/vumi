//! When something happened, in words: "today", "a week ago", "in August".

use gettextrs::{gettext, ngettext};
use gtk::glib;

/// How long ago `date` (ISO 8601, as the server sends a star's date) was, by
/// the calendar: today, yesterday, 2-6 days, a week (7-13 days), 2-3 weeks;
/// further back the month (within a year) or the year. Empty when the date
/// cannot be read.
pub(crate) fn starred_ago(date: &str) -> String {
    match glib::DateTime::now_local() {
        Ok(now) => starred_ago_at(date, &now),
        Err(_) => String::new(),
    }
}

/// starred_ago as seen at `now` (local time).
fn starred_ago_at(date: &str, now: &glib::DateTime) -> String {
    let Ok(then) = glib::DateTime::from_iso8601(date, None).and_then(|then| then.to_local()) else {
        return String::new();
    };
    // The number of the day in local time, so "yesterday" follows midnight.
    let day =
        |time: &glib::DateTime| (time.to_unix() + time.utc_offset().as_seconds()).div_euclid(86400);
    let days = (day(now) - day(&then)).max(0);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A local time, written as the server would send it.
    fn local(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> String {
        glib::DateTime::from_local(year, month, day, hour, minute, 0.0)
            .unwrap()
            .format_iso8601()
            .unwrap()
            .to_string()
    }

    #[test]
    fn days_go_by_the_calendar_then_weeks_months_and_years() {
        let now = glib::DateTime::from_local(2026, 11, 2, 0, 10, 0.0).unwrap();
        // No translations in tests: the English text.
        assert_eq!(starred_ago_at(&local(2026, 11, 2, 0, 5), &now), "today");
        // Twenty minutes ago, but before midnight.
        assert_eq!(
            starred_ago_at(&local(2026, 11, 1, 23, 50), &now),
            "yesterday"
        );
        assert_eq!(
            starred_ago_at(&local(2026, 10, 29, 12, 0), &now),
            "4 days ago"
        );
        assert_eq!(
            starred_ago_at(&local(2026, 10, 26, 12, 0), &now),
            "a week ago"
        );
        assert_eq!(
            starred_ago_at(&local(2026, 10, 12, 12, 0), &now),
            "3 weeks ago"
        );
        assert_eq!(
            starred_ago_at(&local(2026, 8, 15, 12, 0), &now),
            "in August"
        );
        assert_eq!(starred_ago_at(&local(2025, 8, 15, 12, 0), &now), "in 2025");
    }

    #[test]
    fn a_date_that_cannot_be_read_gives_nothing_and_the_future_is_today() {
        let now = glib::DateTime::from_local(2026, 11, 2, 12, 0, 0.0).unwrap();
        assert_eq!(starred_ago_at("not a date", &now), "");
        assert_eq!(starred_ago_at(&local(2026, 11, 5, 12, 0), &now), "today");
    }
}
