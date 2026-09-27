//! Login page: server address, username and password, with clear errors.
//! The layout lives in login_page.blp; this file only holds the behaviour.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::{error_message, runtime, secrets, MainView};

mod imp {
    use super::*;

    /// Widgets from the Blueprint file that the code needs to reach.
    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/login_page.blp")]
    pub struct LoginPage {
        #[template_child]
        pub group: gtk::TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub url_row: gtk::TemplateChild<adw::EntryRow>,
        #[template_child]
        pub user_row: gtk::TemplateChild<adw::EntryRow>,
        #[template_child]
        pub password_row: gtk::TemplateChild<adw::PasswordEntryRow>,
        #[template_child]
        pub error_label: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub connect_button: gtk::TemplateChild<gtk::Button>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LoginPage {
        // Must match "template $MusePlayerLoginPage" in the Blueprint file.
        const NAME: &'static str = "MusePlayerLoginPage";
        type Type = super::LoginPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for LoginPage {}
    impl WidgetImpl for LoginPage {}
    impl NavigationPageImpl for LoginPage {}
}

glib::wrapper! {
    pub struct LoginPage(ObjectSubclass<imp::LoginPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl LoginPage {
    pub(crate) fn new(nav: &adw::NavigationView, settings: &gio::Settings, main: &MainView) -> Self {
        let page: Self = glib::Object::new();
        let imp = page.imp();

        // Pre-filled with the last server that connected successfully.
        let saved_url = settings.string("server-url").to_string();
        let saved_user = settings.string("username").to_string();
        imp.url_row.set_text(&saved_url);
        imp.user_row.set_text(&saved_user);

        // Fill in the saved password in the background; the window never waits for it.
        if !saved_url.is_empty() && !saved_user.is_empty() {
            let password_row = imp.password_row.get();
            glib::spawn_future_local(async move {
                let result = runtime()
                    .spawn(async move { secrets::load_password(&saved_url, &saved_user).await })
                    .await;
                match result {
                    Ok(Ok(Some(password))) => password_row.set_text(&password),
                    Ok(Ok(None)) => {}
                    Ok(Err(e)) => eprintln!("Could not read the keyring: {e}"),
                    Err(_) => {}
                }
            });
        }

        // A weak reference, so the button does not keep its own page alive forever.
        let page_weak = page.downgrade();
        let nav = nav.clone();
        let settings = settings.clone();
        let main = main.clone();
        imp.connect_button.connect_clicked(move |button| {
            if let Some(page) = page_weak.upgrade() {
                page.try_connect(button, &nav, &settings, &main);
            }
        });

        page
    }

    fn try_connect(
        &self,
        button: &gtk::Button,
        nav: &adw::NavigationView,
        settings: &gio::Settings,
        main: &MainView,
    ) {
        let imp = self.imp();
        let url = imp.url_row.text().trim().to_string();
        let user = imp.user_row.text().trim().to_string();
        let password = imp.password_row.text().to_string();

        // Busy state: spinner instead of the label, form locked.
        button.set_sensitive(false);
        button.set_child(Some(&adw::Spinner::new()));
        imp.group.set_sensitive(false);
        imp.error_label.set_visible(false);

        let page = self.clone();
        let button = button.clone();
        let nav = nav.clone();
        let settings = settings.clone();
        let main = main.clone();

        // Runs on the GTK main loop; only the network call goes to tokio.
        glib::spawn_future_local(async move {
            let client = subsonic::Client::new(&url, &user, &password);
            let result = runtime().spawn({ let client = client.clone(); async move { client.ping().await } }).await;

            let imp = page.imp();
            button.set_label(&gettext("Connect"));
            button.set_sensitive(true);
            imp.group.set_sensitive(true);

            match result {
                Ok(Ok(response)) => {
                    // Saved only after success, so a typo is never remembered.
                    let _ = settings.set_string("server-url", &url);
                    let _ = settings.set_string("username", &user);
                    runtime().spawn(async move {
                        if let Err(e) = secrets::save_password(&url, &user, &password).await {
                            eprintln!("Could not save the password: {e}");
                        }
                    });
                    main.show_connected(&response, client);
                    nav.replace(&[main.clone().upcast()]);
                }
                Ok(Err(e)) => {
                    imp.error_label.set_label(&error_message(&e));
                    imp.error_label.set_visible(true);
                }
                Err(_) => {
                    imp.error_label
                        .set_label(&gettext("Something went wrong. Please try again."));
                    imp.error_label.set_visible(true);
                }
            }
        });
    }
}
