//! The Preferences dialog (Ctrl+,). Every switch is bound straight to GSettings,
//! so the rest of the app reads the same values and nothing needs saving here.

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::gio;
use gtk::glib;
use adw::prelude::*;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/preferences.blp")]
    pub struct Preferences {
        #[template_child]
        pub server_row: gtk::TemplateChild<adw::ActionRow>,
        #[template_child]
        pub user_row: gtk::TemplateChild<adw::ActionRow>,
        #[template_child]
        pub logout_row: gtk::TemplateChild<adw::ButtonRow>,
        #[template_child]
        pub endless_radio: gtk::TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub background_playback: gtk::TemplateChild<adw::SwitchRow>,
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
        // Must match "template $MusePlayerPreferences" in the Blueprint file.
        const NAME: &'static str = "MusePlayerPreferences";
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
            self.server_row
                .set_subtitle(&settings.string("server-url"));
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
    pub(crate) fn new() -> Self {
        glib::Object::new()
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
                dialog.add_toast(adw::Toast::new(&gettext("Syncing the library…")));
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
                        dialog.add_toast(adw::Toast::new(&gettext(
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
