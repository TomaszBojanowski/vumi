//! The Preferences dialog (Ctrl+,). Every switch is bound straight to GSettings,
//! so the rest of the app reads the same values and nothing needs saving here.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext, pgettext};
use gtk::gio;
use gtk::glib;
use std::cell::{Cell, RefCell};

use crate::main_view::{Connection, MainView};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/preferences.blp")]
    pub struct Preferences {
        #[template_child]
        pub avatar: gtk::TemplateChild<adw::Avatar>,
        #[template_child]
        pub user_name: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub server_address: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub server_status: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub server_dot: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub server_warning: gtk::TemplateChild<gtk::Image>,
        /// The main view and the handlers that listen to it (connection,
        /// library), taken off again when the dialog closes.
        pub listening: RefCell<Option<(glib::WeakRef<MainView>, Vec<glib::SignalHandlerId>)>>,
        #[template_child]
        pub logout_side: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub logout_below: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub library_row: gtk::TemplateChild<adw::ActionRow>,
        #[template_child]
        pub sync_button: gtk::TemplateChild<gtk::Button>,
        /// The Library row: its counts line, when the library was last
        /// synced (Unix time) and whether a sync runs now.
        pub counts: RefCell<String>,
        pub synced_at: Cell<Option<i64>>,
        pub syncing: Cell<bool>,
        #[template_child]
        pub endless_radio: gtk::TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub background_playback: gtk::TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub color_scheme: gtk::TemplateChild<adw::ToggleGroup>,
        #[template_child]
        pub cover_colors: gtk::TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub replay_gain: gtk::TemplateChild<adw::ToggleGroup>,
        #[template_child]
        pub report_listens: gtk::TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub covers_row: gtk::TemplateChild<adw::ActionRow>,
        #[template_child]
        pub clear_covers: gtk::TemplateChild<gtk::Button>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Preferences {
        // Must match "template $VumiPreferences" in the Blueprint file.
        const NAME: &'static str = "VumiPreferences";
        type Type = super::Preferences;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Preferences {
        fn constructed(&self) {
            self.parent_constructed();
            // The bindings keep the settings object alive for as long as needed.
            let settings = gio::Settings::new(crate::APP_ID);
            let user = settings.string("username");
            self.avatar.set_text(Some(&user));
            self.user_name.set_label(&user);
            self.server_address
                .set_label(&settings.string("server-url"));
            self.obj().setup_covers();

            // One of the two shows, by the width of the dialog.
            for button in [&*self.logout_side, &*self.logout_below] {
                let dialog = self.obj().downgrade();
                button.connect_clicked(move |_| {
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.confirm_logout();
                    }
                });
            }
            settings
                .bind("endless-radio", &*self.endless_radio, "active")
                .build();
            settings
                .bind("background-playback", &*self.background_playback, "active")
                .build();
            settings
                .bind("report-listens", &*self.report_listens, "active")
                .build();
            settings
                .bind("color-scheme", &*self.color_scheme, "active")
                .build();
            settings
                .bind("cover-colors", &*self.cover_colors, "active")
                .build();
            settings
                .bind("replay-gain", &*self.replay_gain, "active")
                .build();
        }
    }

    impl WidgetImpl for Preferences {}
    impl AdwDialogImpl for Preferences {}
    impl PreferencesDialogImpl for Preferences {}
}

glib::wrapper! {
    pub struct Preferences(ObjectSubclass<imp::Preferences>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Preferences {
    pub(crate) fn new(main: &MainView) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.setup_connection(main);
        dialog.setup_library(main);
        // The main view outlives the dialog: its handlers go as it closes.
        dialog.connect_closed(|dialog| {
            if let Some((main, handlers)) = dialog.imp().listening.take()
                && let Some(main) = main.upgrade()
            {
                for handler in handlers {
                    main.disconnect(handler);
                }
            }
        });
        dialog
    }
}

impl Preferences {
    /// Keeps a handler connected to the main view, to take it off on closing.
    fn listen(&self, main: &MainView, handler: glib::SignalHandlerId) {
        let mut listening = self.imp().listening.borrow_mut();
        match listening.as_mut() {
            Some((_, handlers)) => handlers.push(handler),
            None => *listening = Some((main.downgrade(), vec![handler])),
        }
    }

    /// The account row: under the address, whether the server answers.
    /// What the main view knows shows at once; the server is then asked
    /// again, so the dialog does not repeat an answer from hours ago.
    fn setup_connection(&self, main: &MainView) {
        self.show_connection(&main.connection());

        // The main view may learn something new while the dialog is open (it
        // was still connecting, or Retry was pressed): it says so, and the
        // dialog listens for as long as it is open.
        let dialog = self.downgrade();
        let handler = main.connect_connection_changed(move |main| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.show_connection(&main.connection());
            }
        });
        self.listen(main, handler);

        let Some(client) = main.client() else {
            return;
        };
        let dialog = self.downgrade();
        glib::spawn_future_local(async move {
            let result = crate::runtime()
                .spawn(async move { client.ping().await })
                .await;
            let Some(dialog) = dialog.upgrade() else {
                return;
            };
            match result {
                Ok(Ok(response)) => dialog.show_connection(&Connection::from_response(&response)),
                Ok(Err(e)) => {
                    dialog.show_connection(&Connection::Offline(crate::error_message(&e)))
                }
                Err(_) => {}
            }
        });
    }

    fn show_connection(&self, connection: &Connection) {
        let imp = self.imp();
        let status = match connection {
            Connection::None | Connection::Connecting => gettext("Checking the connection…"),
            Connection::Connected {
                server: Some(server),
                version,
            } => {
                // Translators: {server} is the server software (e.g. Navidrome), {version} its version.
                gettext("Connected · {server} {version}")
                    .replace("{server}", &capitalized(server))
                    // The number alone: Navidrome adds a build id, "0.64.2 (10114574)".
                    .replace(
                        "{version}",
                        version
                            .as_deref()
                            .and_then(|version| version.split_whitespace().next())
                            .unwrap_or(""),
                    )
                    .trim_end()
                    .to_string()
            }
            Connection::Connected { server: None, .. } => gettext("Connected"),
            Connection::Offline(message) => message.clone(),
        };
        imp.server_status.set_label(&status);
        imp.server_dot
            .set_visible(matches!(connection, Connection::Connected { .. }));
        imp.server_warning
            .set_visible(matches!(connection, Connection::Offline(_)));
    }
}

/// A count with its thousands set apart as the language does it: from 1,234
/// on in English, from 10 000 on in Polish.
fn grouped(n: u32) -> String {
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

/// How long ago `then` (Unix time) was, in words: minutes and hours within
/// a day, then days by the calendar, and from a week on the date itself.
fn time_ago(then: i64) -> String {
    let (Ok(now), Ok(then_local)) = (
        glib::DateTime::now_local(),
        glib::DateTime::from_unix_local(then),
    ) else {
        return String::new();
    };
    let seconds = (now.to_unix() - then).max(0);
    if seconds < 60 {
        return gettext("just now");
    }
    if seconds < 3600 {
        let n = (seconds / 60) as u32;
        return ngettext("{n} minute ago", "{n} minutes ago", n).replace("{n}", &n.to_string());
    }
    if seconds < 86400 {
        let n = (seconds / 3600) as u32;
        return ngettext("{n} hour ago", "{n} hours ago", n).replace("{n}", &n.to_string());
    }
    // The number of the day in local time, so "yesterday" follows midnight.
    let day =
        |time: &glib::DateTime| (time.to_unix() + time.utc_offset().as_seconds()).div_euclid(86400);
    match (day(&now) - day(&then_local)).max(1) {
        1 => gettext("yesterday"),
        n @ 2..=6 => {
            ngettext("{n} day ago", "{n} days ago", n as u32).replace("{n}", &n.to_string())
        }
        _ => then_local
            .format("%x")
            .map(|date| date.to_string())
            .unwrap_or_default(),
    }
}

/// "navidrome" as "Navidrome": servers name themselves in lower case.
fn capitalized(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl Preferences {
    /// Log Out asks first: the queue and the saved password go with it.
    fn confirm_logout(&self) {
        let alert = adw::AlertDialog::new(
            Some(&gettext("Log Out?")),
            Some(&gettext(
                "Playback stops and the saved password is removed. The library downloaded from this server stays on this device.",
            )),
        );
        alert.add_responses(&[
            ("cancel", &gettext("Cancel")),
            ("logout", &gettext("Log Out")),
        ]);
        alert.set_response_appearance("logout", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        let dialog = self.downgrade();
        alert.connect_response(Some("logout"), move |_, _| {
            if let Some(dialog) = dialog.upgrade() {
                // app.logout (main.rs); the account is left behind, and the
                // dialog goes with it.
                let _ = dialog.activate_action("app.logout", None);
                dialog.close();
            }
        });
        alert.present(Some(self));
    }

    /// The Library row: how much the local library holds, when it was last
    /// synced, and a button that syncs it now (as Ctrl+R does).
    fn setup_library(&self, main: &MainView) {
        let imp = self.imp();

        let main_weak = main.downgrade();
        imp.sync_button.connect_clicked(move |_| {
            if let Some(main) = main_weak.upgrade() {
                main.sync_now();
            }
        });

        // A sync starts or ends, or the artists arrive: the row follows.
        let dialog = self.downgrade();
        let handler = main.connect_library_changed(move |main| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.update_library(main);
            }
        });
        self.listen(main, handler);
        self.update_library(main);

        // "5 minutes ago" grows old while the dialog stays open.
        let dialog = self.downgrade();
        glib::timeout_add_seconds_local(30, move || match dialog.upgrade() {
            Some(dialog) if dialog.imp().listening.borrow().is_some() => {
                dialog.show_library();
                glib::ControlFlow::Continue
            }
            _ => glib::ControlFlow::Break,
        });
    }

    /// Reads the numbers of the Library row anew: albums, songs and the time
    /// of the last sync from the database, the artists from the main view
    /// (they are not stored; none until they are fetched).
    fn update_library(&self, main: &MainView) {
        let imp = self.imp();
        imp.syncing.set(main.is_syncing());
        imp.sync_button
            .set_sensitive(!main.is_syncing() && main.client().is_some());
        self.show_library();

        let server = main.server_key();
        let artists = main.artist_count() as u32;
        let dialog = self.downgrade();
        glib::spawn_future_local(async move {
            let stats = crate::runtime()
                .spawn_blocking(move || crate::library::stats(&server))
                .await;
            let (Some(dialog), Ok(Ok(stats))) = (dialog.upgrade(), stats) else {
                return;
            };
            let mut parts = vec![
                ngettext("{n} album", "{n} albums", stats.albums)
                    .replace("{n}", &grouped(stats.albums)),
            ];
            if artists > 0 {
                parts.push(
                    ngettext("{n} artist", "{n} artists", artists)
                        .replace("{n}", &grouped(artists)),
                );
            }
            parts.push(
                // Translators: {n} is a number of tracks.
                ngettext("{n} track", "{n} tracks", stats.songs)
                    .replace("{n}", &grouped(stats.songs)),
            );
            dialog.imp().counts.replace(parts.join(" · "));
            dialog.imp().synced_at.set(stats.synced_at);
            dialog.show_library();
        });
    }

    /// The two lines under "Library", from what update_library last read.
    fn show_library(&self) {
        let imp = self.imp();
        let second = if imp.syncing.get() {
            gettext("Syncing the library…")
        } else {
            // Translators: {time} is how long ago, e.g. "5 minutes ago", "yesterday", "never".
            gettext("Last sync: {time}").replace(
                "{time}",
                &match imp.synced_at.get() {
                    Some(then) => time_ago(then),
                    None => gettext("never"),
                },
            )
        };
        let counts = imp.counts.borrow();
        let text = if counts.is_empty() {
            second
        } else {
            format!("{counts}\n{second}")
        };
        // The row reads its subtitle as markup.
        imp.library_row
            .set_subtitle(&glib::markup_escape_text(&text));
    }

    /// The Local Data group: the size of the cover cache (worked out in the
    /// background) and the button that clears it.
    fn setup_covers(&self) {
        let imp = self.imp();

        let dialog = self.downgrade();
        imp.clear_covers.connect_clicked(move |button| {
            button.set_sensitive(false);
            let dialog = dialog.clone();
            let button = button.downgrade();
            glib::spawn_future_local(async move {
                let result = crate::runtime()
                    .spawn_blocking(crate::covers::clear_cache)
                    .await;
                if let Some(dialog) = dialog.upgrade() {
                    dialog.update_cache_size();
                    if !matches!(result, Ok(Ok(()))) {
                        dialog.add_toast(crate::plain_toast(&gettext(
                            "Could not clear the cover cache",
                        )));
                    }
                }
                if let Some(button) = button.upgrade() {
                    button.set_sensitive(true);
                }
            });
        });

        self.update_cache_size();
    }

    /// Shows the size of the cover cache, e.g. "48.2 MB".
    fn update_cache_size(&self) {
        let dialog = self.downgrade();
        glib::spawn_future_local(async move {
            let size = crate::runtime()
                .spawn_blocking(crate::covers::cache_size)
                .await
                .unwrap_or(0);
            if let Some(dialog) = dialog.upgrade() {
                dialog
                    .imp()
                    .covers_row
                    .set_subtitle(&glib::format_size(size));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_are_grouped_as_the_language_groups_them() {
        // No translations in tests: English groups from four digits on.
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1234), "1,234");
        assert_eq!(grouped(11_333), "11,333");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
