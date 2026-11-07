mod a11y;
mod about;
mod album_card;
mod album_page;
mod artist_avatar;
mod artist_card;
mod artist_page;
mod background;
mod continue_tile;
mod cover_image;
mod covers;
mod dates;
mod eq_bars;
mod favorites;
mod favorites_header;
mod format;
mod grid_fill;
mod library;
mod login_page;
mod main_view;
mod mini_player;
mod mpris;
mod now_playing;
mod offline_banner;
mod play_row;
#[cfg(test)]
mod playbin_tests;
mod player;
mod playlist_edit;
mod playlist_page;
mod preferences;
mod queue_page;
mod reconnect;
mod results_view;
mod resume;
mod search_page;
mod secrets;
mod shelf;
mod similar;
mod song_menu;
mod song_row;
mod start_page;
mod steady_bin;
mod tint;
mod tint_layer;
mod tuner;
mod welcome_page;
mod year_scale;

use adw::prelude::*;
use gettextrs::{LocaleCategory, gettext};
use gtk::{gio, glib};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

use login_page::LoginPage;
use main_view::MainView;
use mini_player::MiniPlayer;
use now_playing::NowPlaying;
use welcome_page::WelcomePage;

// The ID of the settings, the D-Bus names and the Flatpak, and the name shown.
pub const APP_ID: &str = "io.github.tomaszbojanowski.Vumi";
const APP_NAME: &str = "Vumi";

const GETTEXT_PACKAGE: &str = "vumi";
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
    app.connect_startup(|app| {
        load_css();
        // SIGTERM (the session ends, or the system stops the app): where
        // playback is gets saved, then the app quits cleanly.
        let app = app.downgrade();
        resume::save_on_terminate(move || {
            if let Some(app) = app.upgrade() {
                app.quit();
            }
        });
    });
    app.connect_shutdown(|_| {
        resume::save_now();
        player::shutdown()
    });
    // Logging out or shutting down: GNOME asks the app first (GTK registers
    // with the session manager by itself, through the portal inside Flatpak),
    // before anything is stopped.
    app.connect_query_end(|_| {
        resume::save_now();
        player::get().save_volume_now();
    });
    app.connect_activate(build_ui);
    app.run()
}

/// The app's settings: one object for the whole run (on each thread), so
/// busy paths (the player asks for Repeat at every track) make no new
/// object, and change signals stay connected.
fn settings() -> gio::Settings {
    thread_local! {
        static SETTINGS: gio::Settings = gio::Settings::new(APP_ID);
    }
    SETTINGS.with(Clone::clone)
}

/// One shared tokio runtime for all network work, off the GTK main thread.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to start tokio runtime"))
}

fn build_ui(app: &adw::Application) {
    // Launched again while running: bring the existing window forward instead
    // of building a second one (with a second login and sync).
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

    let settings = crate::settings();

    let nav = adw::NavigationView::new();
    let main = MainView::new();
    let login_page: adw::NavigationPage = LoginPage::new(&nav, &settings, &main).upcast();

    // Connecting with the saved login: at start, on Retry in the offline
    // banner, and by itself while the server cannot be reached. A weak
    // reference, so the view does not keep itself alive through it.
    {
        let nav = nav.clone();
        let login_page = login_page.clone();
        let main_weak = main.downgrade();
        let settings = settings.clone();
        main.set_reconnect(move |quiet| {
            if let Some(main) = main_weak.upgrade() {
                auto_connect(
                    &nav,
                    &login_page,
                    &main,
                    settings.string("server-url").to_string(),
                    settings.string("username").to_string(),
                    quiet,
                );
            }
        });
    }
    // A failed request (an album page, a stream) makes the view ask the server.
    {
        let main_weak = main.downgrade();
        reconnect::set_failure_handler(move || {
            if let Some(main) = main_weak.upgrade() {
                main.check_connection();
            }
        });
        // The player found the server gone: asked again at once.
        let main_weak = main.downgrade();
        reconnect::set_lost_handler(move || {
            if let Some(main) = main_weak.upgrade() {
                main.server_lost();
            }
        });
    }
    // Playing while offline makes the view try to connect (at most every 10 s).
    {
        let main_weak = main.downgrade();
        reconnect::set_wake_handler(move || {
            if let Some(main) = main_weak.upgrade() {
                main.retry_soon();
            }
        });
    }
    // The network coming back starts an attempt at once; the network going
    // away makes the view ask the server.
    {
        let main_weak = main.downgrade();
        gtk::gio::NetworkMonitor::default().connect_network_changed(move |_, available| {
            if let Some(main) = main_weak.upgrade() {
                main.network_changed(available);
            }
        });
    }

    // With saved credentials the main view shows up at once and the
    // connection happens in the background. Otherwise: welcome or login.
    let saved_url = settings.string("server-url").to_string();
    let saved_user = settings.string("username").to_string();
    if !saved_url.is_empty() && !saved_user.is_empty() {
        nav.push(&main);
        main.load_cached(subsonic::server_key(&saved_url, &saved_user));
        main.retry_now(false);
    } else if settings.boolean("welcome-shown") {
        nav.push(&login_page);
    } else {
        nav.push(&WelcomePage::new(&nav, &login_page, &settings));
    }

    // Short in-app messages (e.g. a failed action) appear over the whole window.
    let toasts = adw::ToastOverlay::new();
    // Wherever a song plays, it shows its album's cover.
    {
        let main_weak = main.downgrade();
        covers::set_song_cover(move |song| match main_weak.upgrade() {
            Some(main) => main.song_cover(song),
            None => song.cover_art.clone(),
        });
    }
    // The now-playing card lives at the bottom of the sidebar.
    main.set_now_playing(&NowPlaying::new());

    // Narrow windows: a compact bar that opens the sidebar with the full card.
    let mini = MiniPlayer::new();
    let main_weak = main.downgrade();
    mini.connect_open(move || {
        if let Some(main) = main_weak.upgrade() {
            main.show_sidebar();
        }
    });
    main.set_mini_player(&mini);

    // Remember what plays, for "Continue listening" on the next start.
    resume::watch();

    // Media controls in GNOME Shell, on the lock screen and on media keys.
    mpris::start(app);

    // "Endless Radio" in the main menu, kept in GSettings. Not GSettings' own
    // action: that one cannot be disabled, and the item is hidden while the
    // server gives no similar songs (similar.rs; hidden-when in main_view.blp).
    {
        let settings = crate::settings();
        let action = gtk::gio::SimpleAction::new_stateful(
            "endless-radio",
            None,
            &settings.boolean("endless-radio").to_variant(),
        );
        action.connect_change_state(|action, value| {
            if let Some(value) = value {
                let _ =
                    crate::settings().set_boolean("endless-radio", value.get().unwrap_or(false));
                action.set_state(value);
            }
        });
        // Changed elsewhere (Preferences): the menu's check follows.
        let weak = action.downgrade();
        settings.connect_changed(Some("endless-radio"), move |settings, key| {
            if let Some(action) = weak.upgrade() {
                action.set_state(&settings.boolean(key).to_variant());
            }
        });
        action.set_enabled(crate::similar::available());
        let weak = action.downgrade();
        crate::similar::watch(move |available| {
            let Some(action) = weak.upgrade() else {
                return false;
            };
            action.set_enabled(available);
            true
        });
        app.add_action(&action);
    }
    app.add_action(&crate::settings().create_action("repeat"));

    // Style (Preferences): follow the system, or always light or dark.
    {
        let settings = crate::settings();
        let apply = |settings: &gtk::gio::Settings| {
            let scheme = match settings.uint("color-scheme") {
                1 => adw::ColorScheme::ForceLight,
                2 => adw::ColorScheme::ForceDark,
                _ => adw::ColorScheme::Default,
            };
            adw::StyleManager::default().set_color_scheme(scheme);
        };
        apply(&settings);
        settings.connect_changed(Some("color-scheme"), move |settings, _| apply(settings));
        // Cover Colors (Preferences): on and off at once, on every open page.
        settings.connect_changed(Some("cover-colors"), |_, _| crate::tint::reload());
        // Repeat (main menu): the track handed to GStreamer ahead follows at once.
        settings.connect_changed(Some("repeat"), |_, _| crate::player::get().repeat_changed());
    }

    // Playback errors appear in the window, not only in the terminal. The same
    // message again while it is still there (a slider dragged offline, in
    // several jumps) does not queue up behind it. With the window hidden
    // (playing in the background, media keys) a toast would never be seen: a
    // notification says it instead.
    let toasts_weak = toasts.downgrade();
    let last: std::cell::RefCell<Option<(String, glib::WeakRef<adw::Toast>)>> =
        std::cell::RefCell::default();
    player::get().set_error_handler(move |message| {
        let Some(toasts) = toasts_weak.upgrade() else {
            return;
        };
        let window = toasts.root().and_downcast::<gtk::Window>();
        if let Some(window) = window.filter(|window| !window.is_visible()) {
            if let Some(app) = window.application() {
                let notification = gtk::gio::Notification::new(message);
                app.send_notification(Some("playback-error"), &notification);
            }
            return;
        }
        let shown = last
            .borrow()
            .as_ref()
            .is_some_and(|(text, toast)| text == message && toast.upgrade().is_some());
        if shown {
            return;
        }
        let toast = plain_toast(message);
        last.replace(Some((message.to_owned(), toast.downgrade())));
        toasts.add_toast(toast);
    });
    toasts.set_child(Some(&nav));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(APP_NAME)
        .width_request(WINDOW_MIN_WIDTH)
        .height_request(294)
        .content(&toasts)
        .build();

    // Back to the window while offline: an attempt to connect.
    {
        let main_weak = main.downgrade();
        window.connect_is_active_notify(move |window| {
            if let (true, Some(main)) = (window.is_active(), main_weak.upgrade()) {
                main.retry_soon();
            }
        });
    }

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

    // Ctrl+F: to the search field.
    let search = gtk::gio::SimpleAction::new("search", None);
    let main_weak = main.downgrade();
    search.connect_activate(move |_, _| {
        if let Some(main) = main_weak.upgrade() {
            main.focus_search();
        }
    });
    window.add_action(&search);
    app.set_accels_for_action("win.search", &["<Control>f"]);

    // Preferences (Ctrl+,): a dialog over the window.
    let preferences = gtk::gio::SimpleAction::new("preferences", None);
    let window_weak = window.downgrade();
    let main_weak = main.downgrade();
    preferences.connect_activate(move |_, _| {
        if let (Some(window), Some(main)) = (window_weak.upgrade(), main_weak.upgrade()) {
            adw::prelude::AdwDialogExt::present(
                &crate::preferences::Preferences::new(&main),
                Some(&window),
            );
        }
    });
    app.add_action(&preferences);
    app.set_accels_for_action("app.preferences", &["<Control>comma"]);

    // Log out (Preferences): stop, forget the password, back to the login page.
    // The server address and the user stay filled in; the local library stays
    // too (it is kept per server and user).
    let logout = gtk::gio::SimpleAction::new("logout", None);
    {
        let nav = nav.clone();
        let settings = crate::settings();
        let login_page = login_page.clone();
        let main = main.downgrade();
        logout.connect_activate(move |_, _| {
            crate::player::get().reset();
            if let Some(main) = main.upgrade() {
                main.log_out();
            }
            let url = settings.string("server-url").to_string();
            let user = settings.string("username").to_string();
            let login = login_page.clone();
            gtk::glib::spawn_future_local(async move {
                let result = crate::runtime()
                    .spawn(async move { crate::secrets::delete_password(&url, &user).await })
                    .await;
                let error = match result {
                    Ok(Ok(())) => return,
                    Ok(Err(e)) => e.to_string(),
                    Err(e) => e.to_string(),
                };
                // The address and the user stay, so the password left in the
                // keyring would log in again at the next start: the login
                // page says so, as for a keyring it cannot read.
                eprintln!("Could not remove the saved password: {error}");
                if let Some(page) = login.downcast_ref::<LoginPage>() {
                    page.show_error(&gettext("Could not remove the password from the keyring"));
                }
            });
            if let Some(page) = login_page.downcast_ref::<LoginPage>() {
                page.clear_password();
            }
            nav.replace(std::slice::from_ref(&login_page));
        });
    }
    app.add_action(&logout);

    // Sync the library now (Preferences).
    let sync = gtk::gio::SimpleAction::new("sync", None);
    let main_for_sync = main.downgrade();
    sync.connect_activate(move |_, _| {
        if let Some(main) = main_for_sync.upgrade() {
            main.sync_now();
        }
    });
    app.add_action(&sync);
    app.set_accels_for_action("app.sync", &["<Control>r"]);

    // Quit for real: closing the window may only hide it while music plays.
    let quit = gtk::gio::SimpleAction::new("quit", None);
    let app_for_quit = app.downgrade();
    quit.connect_activate(move |_, _| {
        if let Some(app) = app_for_quit.upgrade() {
            app.quit();
        }
    });
    app.add_action(&quit);
    app.set_accels_for_action("app.quit", &["<Control>q"]);

    // About Vumi (main menu).
    let about = gtk::gio::SimpleAction::new("about", None);
    let window_for_about = window.downgrade();
    about.connect_activate(move |_, _| {
        if let Some(window) = window_for_about.upgrade() {
            show_about(&window);
        }
    });
    app.add_action(&about);

    // Playback from the keyboard, as in GNOME Music. Not plain Space: the
    // search entry needs it.
    let playback: [(&str, &str, fn()); 3] = [
        ("play-pause", "<Control>space", || {
            crate::player::get().toggle_pause();
        }),
        ("previous", "<Control>b", || {
            crate::player::get().previous();
        }),
        ("next", "<Control>n", || {
            crate::player::get().next();
        }),
    ];
    for (name, accel, run) in playback {
        let action = gtk::gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| run());
        app.add_action(&action);
        app.set_accels_for_action(&format!("app.{name}"), &[accel]);
    }

    // Keyboard Shortcuts (main menu, Ctrl+?).
    let shortcuts = gtk::gio::SimpleAction::new("shortcuts", None);
    let window_for_shortcuts = window.downgrade();
    shortcuts.connect_activate(move |_, _| {
        if let Some(window) = window_for_shortcuts.upgrade() {
            show_shortcuts(&window);
        }
    });
    app.add_action(&shortcuts);
    app.set_accels_for_action("app.shortcuts", &["<Control>question"]);

    // Closing the window while music is loaded (Preferences: "Play After the
    // Window Is Closed") only hides it: the music goes on, controlled from
    // GNOME Shell (MPRIS). Opening the app again, or Raise from Shell, shows it.
    {
        let settings = crate::settings();
        window.connect_close_request(move |window| {
            if settings.boolean("background-playback") && crate::player::get().is_active() {
                window.set_visible(false);
                crate::background::request();
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
    }
    // Hidden, and the music has stopped (end of the queue, Stop from Shell,
    // an error): nothing is left to do, so the app quits. Pausing does not
    // count. An error is shown as a notification first: the window's toast
    // would never be seen. Stopped because the server went away is not the
    // end: the app stays, and Play (media keys, GNOME Shell) goes on once the
    // connection is back.
    {
        let window_weak = window.downgrade();
        let app_weak = app.downgrade();
        crate::player::get().connect_changed(move || {
            let (Some(window), Some(app)) = (window_weak.upgrade(), app_weak.upgrade()) else {
                return;
            };
            // A failed stream waits for the server's answer: the message
            // (and so the notification) comes with it.
            let player = crate::player::get();
            if !window.is_visible()
                && !player.is_active()
                && !player.is_checking()
                && !player.waits_for_server()
            {
                if let Some(message) = player.failure() {
                    let notification = gtk::gio::Notification::new(&gettext("Playback Stopped"));
                    notification.set_body(Some(&message));
                    app.send_notification(Some("playback-error"), &notification);
                }
                app.quit();
            }
        });
    }

    window.present();
}

/// Connects with the saved server, user and keyring password without
/// blocking the UI: the main view is already on screen while this runs. A
/// `quiet` attempt (made by itself while offline) shows nothing on its way.
fn auto_connect(
    nav: &adw::NavigationView,
    login_page: &adw::NavigationPage,
    main: &MainView,
    url: String,
    user: String,
    quiet: bool,
) {
    if !quiet {
        main.show_connecting();
    }
    let session = main.session();
    let nav = nav.clone();
    let login_page = login_page.clone();
    let main = main.clone();

    glib::spawn_future_local(async move {
        let result = runtime()
            .spawn(async move {
                let password = match secrets::load_password(&url, &user).await {
                    Ok(Some(password)) => password,
                    Ok(None) => return Saved::Missing,
                    Err(e) => {
                        eprintln!("Could not read the keyring: {e}");
                        return Saved::Unreadable;
                    }
                };
                let client = subsonic::Client::new(&url, &user, &password);
                let ping = client.ping().await;
                Saved::Ping(client, Box::new(ping))
            })
            .await;

        main.attempt_done();
        // Logged out, or logged in again, while the ping was on its way: this
        // answer is about an account that is no longer the one in use.
        if main.session() != session {
            return;
        }
        let to_login = match result {
            Ok(Saved::Ping(client, ping)) => match *ping {
                Ok(response) => {
                    main.show_connected(&response, client);
                    false
                }
                // The saved password no longer fits (changed on the server
                // while Vumi was offline, say): the login page says why it
                // is there, with the password to type again.
                Err(e @ subsonic::Error::WrongCredentials) => {
                    if let Some(page) = login_page.downcast_ref::<LoginPage>() {
                        page.clear_password();
                        page.show_error(&error_message(&e));
                    }
                    true
                }
                Err(e) => {
                    main.show_offline(&e);
                    false
                }
            },
            Ok(Saved::Missing) => true,
            Ok(Saved::Unreadable) => {
                if let Some(page) = login_page.downcast_ref::<LoginPage>() {
                    page.show_error(&gettext("Could not read the password from the keyring"));
                }
                true
            }
            Err(_) => {
                main.show_offline_message(&gettext("Something went wrong. Please try again."));
                false
            }
        };
        if to_login {
            // Whoever logs in next may be someone else: this account's
            // library leaves the screen, as at Log Out.
            main.log_out();
            nav.replace(&[login_page]);
        }
    });
}

/// What the saved server, user and keyring password came to.
enum Saved {
    /// No password in the keyring: the login page.
    Missing,
    /// The keyring could not be read: the login page, saying so.
    Unreadable,
    /// The server's answer to the saved password.
    Ping(
        subsonic::Client,
        Box<Result<subsonic::Response, subsonic::Error>>,
    ),
}

/// Human-readable message for each kind of connection error.
fn error_message(e: &subsonic::Error) -> String {
    use subsonic::Error::*;
    match e {
        WrongCredentials => gettext("Wrong username or password."),
        Unreachable(_) => {
            gettext("Cannot reach the server. Check the address and your connection.")
        }
        InvalidAddress(_) => {
            gettext("Invalid server address. It should start with http:// or https://")
        }
        BadResponse(_) => {
            gettext("The server answered, but it does not look like a Subsonic server.")
        }
        // A server may send only its error code.
        Server { code, message } if message.trim().is_empty() => {
            // Translators: {code} is the number of the error sent by the server.
            gettext("Server error (code {code})").replace("{code}", &code.to_string())
        }
        // Translators: {message} is the error text sent by the server.
        Server { message, .. } => gettext("Server error: {message}").replace("{message}", message),
    }
}

/// error_message's short form for a toast, one line at the narrowest window;
/// the pages, the login page and Preferences keep the full sentences.
pub(crate) fn error_note(e: &subsonic::Error) -> String {
    use subsonic::Error::*;
    match e {
        WrongCredentials => gettext("Wrong username or password"),
        Unreachable(_) => gettext("Cannot reach the server"),
        InvalidAddress(_) => gettext("Invalid server address"),
        BadResponse(_) => gettext("Not a Subsonic-compatible server"),
        Server { .. } => error_message(e),
    }
}

/// Playing needs the server: a toast says so, and Vumi tries to connect
/// (unless it did lately). For the places that have no client yet.
pub(crate) fn cannot_play(near: &impl IsA<gtk::Widget>) {
    toast(near, &gettext("Cannot play without a connection"));
    reconnect::wake();
}

/// A short note at the bottom of the window `near` is in.
pub(crate) fn toast(near: &impl IsA<gtk::Widget>, text: &str) {
    if let Some(toasts) = near
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()
    {
        toasts.add_toast(plain_toast(text));
    }
}

/// The narrowest window, the smallest GNOME designs for (phones): every page
/// and dialog fits it.
const WINDOW_MIN_WIDTH: i32 = 360;

/// What a toast's title has in the narrowest window: the window less the
/// toast's own margins, padding and close button, 94 px (measured on
/// Adwaita's toast at 366 and at 360 px, 2026-11-06 and 07).
const TOAST_TITLE_WIDTH: i32 = WINDOW_MIN_WIDTH - 94;

/// A note with a button (Undo, say) at the bottom of the window `near` is
/// in; `clicked` runs when the button is used. Returns the toast, so the
/// note can be taken away when what it offers no longer can be done.
pub(crate) fn toast_with_button(
    near: &impl IsA<gtk::Widget>,
    text: &str,
    button: &str,
    clicked: impl Fn() + 'static,
) -> Option<adw::Toast> {
    let toasts = near
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()?;
    let toast = plain_toast(text);
    toast.set_button_label(Some(button));
    toast.connect_button_clicked(move |_| clicked());
    toasts.add_toast(toast.clone());
    Some(toast)
}

/// A toast with a name in it ("Added to “{name}”", `template` translated): the
/// name is cut with "…" inside its quotes until the whole text fits in one line
/// at the narrowest window, so the closing quote always shows. A screen reader
/// hears the whole name: announced, as a toast reads out only a plain title.
pub(crate) fn toast_with_name(near: &impl IsA<gtk::Widget>, template: &str, name: &str) {
    let Some(toasts) = near
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()
    else {
        return;
    };
    let full = template.replace("{name}", name);
    let shown = fit_name(
        template,
        name,
        &cluster_starts(&toasts, name),
        |text| toast_title_width(&toasts, text),
        TOAST_TITLE_WIDTH,
    );
    if shown == full {
        toasts.add_toast(plain_toast(&full));
        return;
    }
    // The title as Adwaita makes it: a "heading" label, cut at its end.
    let title = gtk::Label::builder()
        .label(shown.as_str())
        .css_classes(["heading"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .xalign(0.0)
        .build();
    toasts.add_toast(adw::Toast::builder().custom_title(&title).build());
    // From the window: Orca ignored it from the toast overlay (2026-11-06).
    if let Some(window) = toasts.root() {
        window
            .upcast_ref::<gtk::Widget>()
            .announce(&full, gtk::AccessibleAnnouncementPriority::Medium);
    }
}

/// The width of `text` as a toast's title (Adwaita's "heading": bold).
fn toast_title_width(widget: &impl IsA<gtk::Widget>, text: &str) -> i32 {
    let layout = widget.create_pango_layout(Some(text));
    let attrs = gtk::pango::AttrList::new();
    attrs.insert(gtk::pango::AttrInt::new_weight(gtk::pango::Weight::Bold));
    layout.set_attributes(Some(&attrs));
    layout.pixel_size().0
}

/// Where `name` may be cut without splitting a character as the eye sees it
/// (an emoji with its joiners, a letter with its accents): the starts of its
/// glyph clusters, as byte offsets, the first left out.
fn cluster_starts(widget: &impl IsA<gtk::Widget>, name: &str) -> Vec<usize> {
    let layout = widget.create_pango_layout(Some(name));
    let mut iter = layout.iter();
    let mut starts = Vec::new();
    loop {
        let index = iter.index() as usize;
        if index > 0 && index < name.len() && name.is_char_boundary(index) {
            starts.push(index);
        }
        if !iter.next_cluster() {
            break;
        }
    }
    starts.sort_unstable();
    starts.dedup();
    starts
}

/// `template` with `name`, or when that is wider than `max` (by `width`),
/// with the longest start of `name` that fits, cut at one of `cuts` (byte
/// offsets, ascending) and ended with "…" inside the template's quotes.
fn fit_name(
    template: &str,
    name: &str,
    cuts: &[usize],
    width: impl Fn(&str) -> i32,
    max: i32,
) -> String {
    let full = template.replace("{name}", name);
    if width(&full) <= max {
        return full;
    }
    let cut = |end: usize| template.replace("{name}", &format!("{}…", name[..end].trim_end()));
    // Longer starts are wider: the longest that fits, by halves.
    let (mut fitting, mut too_wide) = (0, cuts.len() + 1);
    while too_wide - fitting > 1 {
        let middle = (fitting + too_wide) / 2;
        if width(&cut(cuts[middle - 1])) <= max {
            fitting = middle;
        } else {
            too_wide = middle;
        }
    }
    if fitting == 0 {
        template.replace("{name}", "…")
    } else {
        cut(cuts[fitting - 1])
    }
}

/// A toast that shows `text` as it is. A toast reads its title as Pango markup
/// unless told otherwise, and then a "&" or "<" in a name from the server (a
/// playlist, an error) keeps the whole message from showing.
fn plain_toast(text: &str) -> adw::Toast {
    adw::Toast::builder().use_markup(false).title(text).build()
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

/// The About dialog: what the app is, who made it, what it builds on, and the
/// versions a bug report needs.
fn show_about(parent: &impl gtk::prelude::IsA<gtk::Widget>) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Vumi")
        .application_icon(APP_ID)
        .developer_name("Tomasz Bojanowski")
        .developers(["Tomasz Bojanowski"])
        .version(env!("CARGO_PKG_VERSION"))
        .website("https://vumi.bojanowski.me")
        .issue_url("https://github.com/TomaszBojanowski/vumi/issues")
        .copyright("© 2026 Tomasz Bojanowski")
        .license_type(gtk::License::Gpl30)
        .comments(gettext(
            "A music player for Navidrome and other Subsonic servers",
        ))
        .translator_credits(gettext("translator-credits"))
        .debug_info(debug_info())
        .debug_info_filename("vumi-debug.txt")
        .build();
    // A way to support the author; it sits with the dialog's other links.
    dialog.add_link(
        &gettext("Buy Me a Coffee"),
        "https://buymeacoffee.com/sojer2005",
    );
    dialog.add_acknowledgement_section(
        Some(&gettext("Built With")),
        &[
            "Navidrome https://www.navidrome.org",
            "AudioMuse-AI https://github.com/NeptuneHub/AudioMuse-AI",
        ],
    );
    dialog.present(Some(parent));
}

/// Versions of the app and the libraries under it, for bug reports.
fn debug_info() -> String {
    format!(
        "Vumi {}\nGTK {}.{}.{}\nlibadwaita {}.{}.{}\n{}\n",
        env!("CARGO_PKG_VERSION"),
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version(),
        gstreamer::version_string(),
    )
}

/// The Keyboard Shortcuts dialog. Each item reads its keys from the action
/// itself, so the list always matches what is really set.
fn show_shortcuts(parent: &impl gtk::prelude::IsA<gtk::Widget>) {
    let dialog = adw::ShortcutsDialog::new();
    let sections = [
        (
            gettext("Playback"),
            vec![
                (gettext("Play or Pause"), "app.play-pause"),
                (gettext("Previous Track"), "app.previous"),
                (gettext("Next Track"), "app.next"),
            ],
        ),
        (
            gettext("Library"),
            vec![
                (gettext("Search Library"), "win.search"),
                (gettext("Sync Library"), "app.sync"),
            ],
        ),
        (
            gettext("General"),
            vec![
                (gettext("Preferences"), "app.preferences"),
                (gettext("Keyboard Shortcuts"), "app.shortcuts"),
                (gettext("Quit"), "app.quit"),
            ],
        ),
    ];
    for (title, items) in sections {
        let section = adw::ShortcutsSection::new(Some(&title));
        for (label, action) in items {
            section.add(adw::ShortcutsItem::from_action(&label, action));
        }
        dialog.add(section);
    }
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cuts at every character, widths in characters: fit_name's rules
    /// without a screen (the app cuts at glyph clusters, measured in pixels).
    fn by_chars(name: &str) -> Vec<usize> {
        name.char_indices()
            .map(|(i, _)| i)
            .filter(|i| *i > 0)
            .collect()
    }

    fn chars(text: &str) -> i32 {
        text.chars().count() as i32
    }

    #[test]
    fn a_name_that_fits_stays_whole() {
        let template = "Added to “{name}”";
        assert_eq!(
            fit_name(template, "Calm", &by_chars("Calm"), chars, 30),
            "Added to “Calm”"
        );
    }

    #[test]
    fn a_long_name_is_cut_inside_its_quotes_as_long_as_it_fits() {
        let template = "Dodano do „{name}”";
        for name in [
            "Muzyka na bardzo długie zimowe wieczory przy kominku",
            "Ballady 🎸🎶 na długie zimowe wieczory 🔥",
            "Музыка для долгих зимних вечеров у камина",
        ] {
            let shown = fit_name(template, name, &by_chars(name), chars, 30);
            assert!(shown.ends_with("…”"), "{shown}");
            assert!(chars(&shown) <= 30, "{shown}");
            // The longest that fits: one character more would not.
            let kept = shown
                .trim_start_matches("Dodano do „")
                .trim_end_matches("…”");
            let next = name[kept.len()..].chars().next().unwrap();
            let longer = format!("Dodano do „{kept}{next}…”");
            assert!(name.starts_with(kept), "{shown}");
            assert!(chars(longer.trim_end()) > 30 || next == ' ', "{shown}");
        }
    }

    #[test]
    fn a_name_of_which_nothing_fits_leaves_the_ellipsis() {
        let template = "Saved as “{name}”";
        assert_eq!(
            fit_name(template, "Long name", &by_chars("Long name"), chars, 12),
            "Saved as “…”"
        );
    }

    #[test]
    fn toasts_have_the_short_error_texts_and_pages_the_full_ones() {
        let unreachable = subsonic::Error::Unreachable(String::new());
        assert_eq!(error_note(&unreachable), "Cannot reach the server");
        assert!(error_message(&unreachable).contains("Check the address"));
        assert_eq!(
            error_note(&subsonic::Error::WrongCredentials),
            "Wrong username or password"
        );
        assert_eq!(
            error_note(&subsonic::Error::InvalidAddress(String::new())),
            "Invalid server address"
        );
        assert_eq!(
            error_note(&subsonic::Error::BadResponse(String::new())),
            "Not a Subsonic-compatible server"
        );
        let server = subsonic::Error::Server {
            code: 70,
            message: "Song not found".into(),
        };
        assert_eq!(error_note(&server), error_message(&server));
    }

    /// A server that sends only a code still has its error named.
    #[test]
    fn a_server_error_without_a_message_gives_its_code() {
        for message in ["", "  "] {
            let e = subsonic::Error::Server {
                code: 70,
                message: message.into(),
            };
            assert!(error_message(&e).contains("70"), "{}", error_message(&e));
        }
    }

    #[test]
    fn a_server_error_with_a_message_shows_the_message() {
        let e = subsonic::Error::Server {
            code: 70,
            message: "Album not found".into(),
        };
        assert!(error_message(&e).contains("Album not found"));
    }
}
