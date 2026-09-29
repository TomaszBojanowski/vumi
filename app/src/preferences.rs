//! The Preferences dialog (Ctrl+,). Every switch is bound straight to GSettings,
//! so the rest of the app reads the same values and nothing needs saving here.

use adw::subclass::prelude::*;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/preferences.blp")]
    pub struct Preferences {
        #[template_child]
        pub endless_radio: gtk::TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub background_playback: gtk::TemplateChild<adw::SwitchRow>,
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
            settings
                .bind("endless-radio", &*self.endless_radio, "active")
                .build();
            settings
                .bind("background-playback", &*self.background_playback, "active")
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
