//! The Preferences dialog (Ctrl+,). Every switch is bound straight to GSettings,
//! so the rest of the app reads the same values and nothing needs saving here.

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::gio;
use gtk::glib;
use adw::prelude::*;
use std::cell::RefCell;

use crate::main_view::{Connection, MainView};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/preferences.blp")]
    pub struct Preferences {
        #[template_child]
        pub server_address: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub server_status: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub server_dot: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub server_warning: gtk::TemplateChild<gtk::Image>,
        /// The main view and the handler that listens to its connection
        /// state, taken off again when the dialog closes.
        pub listening: RefCell<Option<(glib::WeakRef<MainView>, glib::SignalHandlerId)>>,
        #[template_child]
        pub user_row: gtk::TemplateChild<adw::ActionRow>,
        #[template_child]
        pub logout_row: gtk::TemplateChild<adw::ButtonRow>,
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
        pub sync_row: gtk::TemplateChild<adw::ButtonRow>,
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
            self.user_row.set_subtitle(&settings.string("username"));
            self.obj().setup_library();

            // Logging out leaves the account behind: the dialog goes with it.
            let dialog = self.obj().downgrade();
            self.logout_row.connect_activated(move |_| {
                if let Some(dialog) = dialog.upgrade() {
                    dialog.close();
                }
            });
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
        dialog
    }
}

impl Preferences {
    /// The Server row: the address and, under it, whether the server answers.
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
        self.imp()
            .listening
            .replace(Some((main.downgrade(), handler)));
        self.connect_closed(|dialog| {
            if let Some((main, handler)) = dialog.imp().listening.take() {
                if let Some(main) = main.upgrade() {
                    main.disconnect(handler);
                }
            }
        });

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
            Connection::Connected { server: Some(server), version } => {
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
        imp.server_address
            .set_label(&gio::Settings::new(crate::APP_ID).string("server-url"));
        imp.server_status.set_label(&status);
        imp.server_dot
            .set_visible(matches!(connection, Connection::Connected { .. }));
        imp.server_warning
            .set_visible(matches!(connection, Connection::Offline(_)));
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
    /// The Library group: a note when a sync starts, the size of the cover
    /// cache (worked out in the background) and the button that clears it.
    fn setup_library(&self) {
        let imp = self.imp();

        let dialog = self.downgrade();
        imp.sync_row.connect_activated(move |_| {
            if let Some(dialog) = dialog.upgrade() {
                dialog.add_toast(crate::plain_toast(&gettext("Syncing the library…")));
            }
        });

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
                dialog.imp().covers_row.set_subtitle(&glib::format_size(size));
            }
        });
    }
}
