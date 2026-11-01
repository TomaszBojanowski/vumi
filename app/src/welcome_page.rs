//! Welcome carousel, shown only until the user completes it once.
//! The layout lives in welcome_page.blp; this file only holds the behaviour.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, pgettext};
use gtk::{gio, glib};

use crate::APP_NAME;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/welcome_page.blp")]
    pub struct WelcomePage {
        #[template_child]
        pub carousel: gtk::TemplateChild<adw::Carousel>,
        #[template_child]
        pub welcome_status: gtk::TemplateChild<adw::StatusPage>,
        #[template_child]
        pub next_button: gtk::TemplateChild<gtk::Button>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for WelcomePage {
        // Must match "template $VumiWelcomePage" in the Blueprint file.
        const NAME: &'static str = "VumiWelcomePage";
        type Type = super::WelcomePage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for WelcomePage {}
    impl WidgetImpl for WelcomePage {}
    impl NavigationPageImpl for WelcomePage {}
}

glib::wrapper! {
    pub struct WelcomePage(ObjectSubclass<imp::WelcomePage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl WelcomePage {
    pub(crate) fn new(
        nav: &adw::NavigationView,
        login_page: &adw::NavigationPage,
        settings: &gio::Settings,
    ) -> Self {
        let page: Self = glib::Object::new();
        let imp = page.imp();

        page.set_title(APP_NAME);
        // Translators: {app_name} is the name of this application.
        let welcome = gettext("Welcome to {app_name}").replace("{app_name}", APP_NAME);
        imp.welcome_status.set_title(&welcome);

        // The button label follows the carousel: "Get Started" on the last page.
        let button = imp.next_button.get();
        imp.carousel.connect_page_changed(move |carousel, index| {
            let last = index + 1 == carousel.n_pages();
            let label = if last {
                gettext("Get Started")
            } else {
                pgettext("welcome", "Next")
            };
            button.set_label(&label);
        });

        // Next page, or the login screen when we are on the last one.
        let carousel = imp.carousel.get();
        let nav = nav.clone();
        let login_page = login_page.clone();
        let settings = settings.clone();
        imp.next_button.connect_clicked(move |_| {
            let current = carousel.position().round() as u32;
            if current + 1 < carousel.n_pages() {
                carousel.scroll_to(&carousel.nth_page(current + 1), true);
            } else {
                let _ = settings.set_boolean("welcome-shown", true);
                nav.push(&login_page);
            }
        });

        page
    }
}
