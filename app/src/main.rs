mod album_card;
mod about;
mod album_page;
mod artist_page;
mod covers;
mod favorites;
mod library;
mod login_page;
mod main_view;
mod secrets;
mod tint;
mod welcome_page;

use adw::prelude::*;
use gettextrs::{gettext, LocaleCategory};
use gtk::{gio, glib};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

use login_page::LoginPage;
use main_view::MainView;
use welcome_page::WelcomePage;

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
    START.get_or_init(Instant::now);
    // Translations: use the system language and our installed catalogs.
    // SAFETY: called first in main, before tokio or GTK start any threads.
    unsafe { gettextrs::setlocale(LocaleCategory::LcAll, "") };
    gettextrs::bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("failed to bind text domain");
    gettextrs::bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8")
        .expect("failed to set text domain encoding");
    gettextrs::textdomain(GETTEXT_PACKAGE).expect("failed to set text domain");

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build_ui);
    app.run()
}

/// One shared tokio runtime for all network work, off the GTK main thread.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to start tokio runtime"))
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
        nav.push(&main);
        main.load_cached(subsonic::server_key(&saved_url, &saved_user));
        auto_connect(&nav, &login_page, &main, saved_url, saved_user);
    } else if settings.boolean("welcome-shown") {
        nav.push(&login_page);
    } else {
        nav.push(&WelcomePage::new(&nav, &login_page, &settings));
    }

    // "Retry" in the offline banner reconnects with the saved credentials.
    // A weak reference, so the view does not keep itself alive through its own banner.
    {
        let nav = nav.clone();
        let login_page = login_page.clone();
        let main_weak = main.downgrade();
        let settings = settings.clone();
        main.connect_retry(move || {
            if let Some(main) = main_weak.upgrade() {
                auto_connect(
                    &nav,
                    &login_page,
                    &main,
                    settings.string("server-url").to_string(),
                    settings.string("username").to_string(),
                );
            }
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
                let client = subsonic::Client::new(&url, &user, &password);
                let ping = client.ping().await;
                Some((client, ping))
            })
            .await;

        match result {
            Ok(Some((client, Ok(response)))) => main.show_connected(&response, client),
            Ok(Some((_, Err(subsonic::Error::WrongCredentials)))) | Ok(None) => {
                nav.replace(&[login_page]);
            }
            Ok(Some((_, Err(e)))) => main.show_offline(&error_message(&e)),
            Err(_) => main.show_offline(&gettext("Something went wrong. Please try again.")),
        }
    });
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

/// When the process started, to measure how fast the library shows up.
static START: OnceLock<Instant> = OnceLock::new();

fn since_start() -> Duration {
    START.get().map(Instant::elapsed).unwrap_or_default()
}

/// The app's own styles (style.css is built into the binary).
fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("style.css"));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
