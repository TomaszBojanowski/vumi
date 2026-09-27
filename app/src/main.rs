mod login_page;
mod secrets;

use adw::prelude::*;
use gettextrs::{gettext, LocaleCategory};
use gtk::{gio, glib};
use std::sync::OnceLock;
use tokio::runtime::Runtime;

use login_page::LoginPage;

// Working ID and name, will change together with the final app name.
pub const APP_ID: &str = "io.github.tomaszbojanowski.MusePlayer";
const APP_NAME: &str = "muse-player";

const GETTEXT_PACKAGE: &str = "muse-player";
// Set by Meson at build time; the fallback matches the Flatpak prefix.
const LOCALEDIR: &str = match option_env!("LOCALEDIR") {
    Some(dir) => dir,
    None => "/app/share/locale",
};

fn main() -> glib::ExitCode {
    // Translations: use the system language and our installed catalogs.
    // SAFETY: called first in main, before tokio or GTK start any threads.
    unsafe { gettextrs::setlocale(LocaleCategory::LcAll, "") };
    gettextrs::bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("failed to bind text domain");
    gettextrs::bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8")
        .expect("failed to set text domain encoding");
    gettextrs::textdomain(GETTEXT_PACKAGE).expect("failed to set text domain");

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

/// One shared tokio runtime for all network work, off the GTK main thread.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to start tokio runtime"))
}

/// The main view with its offline banner. The library will live here.
#[derive(Clone)]
struct MainView {
    page: adw::NavigationPage,
    status: adw::StatusPage,
    banner: adw::Banner,
}

impl MainView {
    fn new() -> Self {
        let banner = adw::Banner::builder().button_label(gettext("Retry")).build();
        let status = adw::StatusPage::builder()
            .icon_name("audio-x-generic-symbolic")
            .title(gettext("Library"))
            .build();

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.add_top_bar(&banner);
        toolbar.set_content(Some(&status));

        let page = adw::NavigationPage::builder()
            .title(APP_NAME)
            .child(&toolbar)
            .build();
        Self { page, status, banner }
    }

    fn show_connecting(&self) {
        self.status.set_description(Some(&gettext("Connecting…")));
    }

    fn show_connected(&self, r: &subsonic::Response) {
        self.banner.set_revealed(false);
        // Translators: {server} is the server software (e.g. navidrome), {version} its version.
        let text = gettext("Connected to {server} {version}")
            .replace("{server}", r.server_type.as_deref().unwrap_or("server"))
            .replace("{version}", r.server_version.as_deref().unwrap_or(""));
        self.status.set_description(Some(&text));
    }

    fn show_offline(&self, message: &str) {
        self.banner.set_title(message);
        self.banner.set_revealed(true);
        self.status.set_description(Some(&gettext("Offline")));
    }
}

fn build_ui(app: &adw::Application) {
    let settings = gio::Settings::new(APP_ID);

    let nav = adw::NavigationView::new();
    let main = MainView::new();
    let login_page: adw::NavigationPage = LoginPage::new(&nav, &settings, &main).upcast();

    // With saved credentials the main view shows up at once and the
    // connection happens in the background. Otherwise: welcome or login.
    let saved_url = settings.string("server-url").to_string();
    let saved_user = settings.string("username").to_string();
    if !saved_url.is_empty() && !saved_user.is_empty() {
        nav.push(&main.page);
        auto_connect(&nav, &login_page, &main, saved_url, saved_user);
    } else if settings.boolean("welcome-shown") {
        nav.push(&login_page);
    } else {
        let welcome_page = build_welcome_page(&nav, &login_page, &settings);
        nav.push(&welcome_page);
    }

    // "Retry" in the offline banner reconnects with the saved credentials.
    {
        let nav = nav.clone();
        let login_page = login_page.clone();
        let main_for_retry = main.clone();
        let settings = settings.clone();
        main.banner.connect_button_clicked(move |_| {
            auto_connect(
                &nav,
                &login_page,
                &main_for_retry,
                settings.string("server-url").to_string(),
                settings.string("username").to_string(),
            );
        });
    }

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(APP_NAME)
        .content(&nav)
        .build();

    // Restore the window size and state from the last session.
    window.set_default_size(settings.int("window-width"), settings.int("window-height"));
    if settings.boolean("window-maximized") {
        window.maximize();
    }

    // Save them when the window closes.
    window.connect_close_request(move |window| {
        let (width, height) = window.default_size();
        let _ = settings.set_int("window-width", width);
        let _ = settings.set_int("window-height", height);
        let _ = settings.set_boolean("window-maximized", window.is_maximized());
        glib::Propagation::Proceed
    });

    window.present();
}

/// Connects with the saved server, user and keyring password without
/// blocking the UI: the main view is already on screen while this runs.
fn auto_connect(
    nav: &adw::NavigationView,
    login_page: &adw::NavigationPage,
    main: &MainView,
    url: String,
    user: String,
) {
    main.show_connecting();
    let nav = nav.clone();
    let login_page = login_page.clone();
    let main = main.clone();

    glib::spawn_future_local(async move {
        // None means there is no usable saved password.
        let result = runtime()
            .spawn(async move {
                let password = match secrets::load_password(&url, &user).await {
                    Ok(Some(password)) => password,
                    Ok(None) => return None,
                    Err(e) => {
                        eprintln!("Could not read the keyring: {e}");
                        return None;
                    }
                };
                Some(subsonic::Client::new(&url, &user, &password).ping().await)
            })
            .await;

        match result {
            Ok(Some(Ok(response))) => main.show_connected(&response),
            Ok(Some(Err(subsonic::Error::WrongCredentials))) | Ok(None) => {
                nav.replace(&[login_page]);
            }
            Ok(Some(Err(e))) => main.show_offline(&error_message(&e)),
            Err(_) => main.show_offline(&gettext("Something went wrong. Please try again.")),
        }
    });
}

/// One page of the welcome carousel.
fn feature_page(icon: &str, title: &str, description: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(description)
        .hexpand(true)
        .vexpand(true)
        .build()
}

fn build_welcome_page(
    nav: &adw::NavigationView,
    login_page: &adw::NavigationPage,
    settings: &gio::Settings,
) -> adw::NavigationPage {
    let carousel = adw::Carousel::builder().hexpand(true).vexpand(true).build();
    carousel.append(&feature_page(
        "audio-x-generic-symbolic",
        // Translators: {app_name} is the name of this application.
        &gettext("Welcome to {app_name}").replace("{app_name}", APP_NAME),
        &gettext("A fast music player for your own music server."),
    ));
    carousel.append(&feature_page(
        "view-grid-symbolic",
        &gettext("Your Library, Instantly"),
        &gettext("Browse thousands of albums without waiting. Everything stays smooth, even with huge collections."),
    ));
    carousel.append(&feature_page(
        "system-search-symbolic",
        &gettext("Discover Music"),
        &gettext("Similar songs, endless radio and search by mood, powered by AudioMuse-AI."),
    ));
    carousel.append(&feature_page(
        "audio-speakers-symbolic",
        &gettext("Hear Every Detail"),
        &gettext("Original quality streaming, gapless playback and bit-perfect output."),
    ));

    let dots = adw::CarouselIndicatorDots::builder().carousel(&carousel).build();

    let next_button = gtk::Button::builder()
        .label(gettext("Next"))
        .halign(gtk::Align::Center)
        .css_classes(["suggested-action", "pill"])
        .build();

    // The button label follows the carousel: "Get Started" on the last page.
    let button_for_label = next_button.clone();
    carousel.connect_page_changed(move |carousel, index| {
        let last = index + 1 == carousel.n_pages();
        let label = if last { gettext("Get Started") } else { gettext("Next") };
        button_for_label.set_label(&label);
    });

    // Next page, or the login screen when we are on the last one.
    let carousel_for_click = carousel.clone();
    let nav = nav.clone();
    let login_page = login_page.clone();
    let settings = settings.clone();
    next_button.connect_clicked(move |_| {
        let c = &carousel_for_click;
        let current = c.position().round() as u32;
        if current + 1 < c.n_pages() {
            c.scroll_to(&c.nth_page(current + 1), true);
        } else {
            let _ = settings.set_boolean("welcome-shown", true);
            nav.push(&login_page);
        }
    });

    let bottom = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_bottom(24)
        .build();
    bottom.append(&dots);
    bottom.append(&next_button);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    content.append(&carousel);
    content.append(&bottom);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content));

    adw::NavigationPage::builder()
        .title(APP_NAME)
        .child(&toolbar)
        .build()
}

/// Human-readable message for each kind of connection error.
fn error_message(e: &subsonic::Error) -> String {
    use subsonic::Error::*;
    match e {
        WrongCredentials => gettext("Wrong username or password."),
        Unreachable(_) => gettext("Cannot reach the server. Check the address and your connection."),
        InvalidAddress(_) => gettext("Invalid server address. It should start with http:// or https://"),
        BadResponse(_) => gettext("The server answered, but it does not look like a Subsonic server."),
        // Translators: {message} is the error text sent by the server.
        Server { message, .. } => gettext("Server error: {message}").replace("{message}", message),
    }
}
