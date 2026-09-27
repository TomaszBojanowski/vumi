//! Main view: the library will live here. For now it shows the connection
//! state and an offline banner with a Retry button.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;

use crate::APP_NAME;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/main_view.blp")]
    pub struct MainView {
        #[template_child]
        pub banner: gtk::TemplateChild<adw::Banner>,
        #[template_child]
        pub status: gtk::TemplateChild<adw::StatusPage>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MainView {
        // Must match "template $MusePlayerMainView" in the Blueprint file.
        const NAME: &'static str = "MusePlayerMainView";
        type Type = super::MainView;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MainView {}
    impl WidgetImpl for MainView {}
    impl NavigationPageImpl for MainView {}
}

glib::wrapper! {
    pub struct MainView(ObjectSubclass<imp::MainView>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Buildable, gtk::ConstraintTarget;
}

impl MainView {
    pub(crate) fn new() -> Self {
        let view: Self = glib::Object::new();
        view.set_title(APP_NAME);
        view
    }

    pub(crate) fn show_connecting(&self) {
        self.imp()
            .status
            .set_description(Some(&gettext("Connecting…")));
    }

    pub(crate) fn show_connected(&self, r: &subsonic::Response) {
        let imp = self.imp();
        imp.banner.set_revealed(false);
        // Translators: {server} is the server software (e.g. navidrome), {version} its version.
        let text = gettext("Connected to {server} {version}")
            .replace("{server}", r.server_type.as_deref().unwrap_or("server"))
            .replace("{version}", r.server_version.as_deref().unwrap_or(""));
        imp.status.set_description(Some(&text));
    }

    pub(crate) fn show_offline(&self, message: &str) {
        let imp = self.imp();
        imp.banner.set_title(message);
        imp.banner.set_revealed(true);
        imp.status.set_description(Some(&gettext("Offline")));
    }

    /// Runs `f` when the user presses Retry in the offline banner.
    pub(crate) fn connect_retry<F: Fn() + 'static>(&self, f: F) {
        self.imp().banner.connect_button_clicked(move |_| f());
    }
}
