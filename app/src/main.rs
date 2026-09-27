use adw::prelude::*;
use gtk::glib;
use std::sync::OnceLock;
use tokio::runtime::Runtime;

// Working ID, will change together with the final app name.
const APP_ID: &str = "io.github.tomaszbojanowski.MusePlayer";

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

/// One shared tokio runtime for all network work, off the GTK main thread.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to start tokio runtime"))
}

fn build_ui(app: &adw::Application) {
    let nav = adw::NavigationView::new();
    let login_page = build_login_page(&nav);
    let welcome_page = build_welcome_page(&nav, &login_page);
    nav.push(&welcome_page);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("muse-player")
        .default_width(900)
        .default_height(700)
        .content(&nav)
        .build();
    window.present();
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
) -> adw::NavigationPage {
    let carousel = adw::Carousel::builder().hexpand(true).vexpand(true).build();
    carousel.append(&feature_page(
        "audio-x-generic-symbolic",
        "Welcome to muse-player",
        "A fast music player for your own music server.",
    ));
    carousel.append(&feature_page(
        "view-grid-symbolic",
        "Your Library, Instantly",
        "Browse thousands of albums without waiting. Everything stays smooth, even with huge collections.",
    ));
    carousel.append(&feature_page(
        "system-search-symbolic",
        "Discover Music",
        "Similar songs, endless radio and search by mood, powered by AudioMuse-AI.",
    ));
    carousel.append(&feature_page(
        "audio-speakers-symbolic",
        "Hear Every Detail",
        "Original quality streaming, gapless playback and bit-perfect output.",
    ));

    let dots = adw::CarouselIndicatorDots::builder().carousel(&carousel).build();

    let next_button = gtk::Button::builder()
        .label("Next")
        .halign(gtk::Align::Center)
        .css_classes(["suggested-action", "pill"])
        .build();

    // The button label follows the carousel: "Get Started" on the last page.
    let button_for_label = next_button.clone();
    carousel.connect_page_changed(move |carousel, index| {
        let last = index + 1 == carousel.n_pages();
        button_for_label.set_label(if last { "Get Started" } else { "Next" });
    });

    // Next page, or the login screen when we are on the last one.
    let carousel_for_click = carousel.clone();
    let nav = nav.clone();
    let login_page = login_page.clone();
    next_button.connect_clicked(move |_| {
        let c = &carousel_for_click;
        let current = c.position().round() as u32;
        if current + 1 < c.n_pages() {
            c.scroll_to(&c.nth_page(current + 1), true);
        } else {
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
        .title("muse-player")
        .child(&toolbar)
        .build()
}

fn build_login_page(nav: &adw::NavigationView) -> adw::NavigationPage {
    let url_row = adw::EntryRow::builder().title("Server address").build();
    let user_row = adw::EntryRow::builder().title("Username").build();
    let password_row = adw::PasswordEntryRow::builder().title("Password").build();

    let group = adw::PreferencesGroup::new();
    group.add(&url_row);
    group.add(&user_row);
    group.add(&password_row);

    // Hidden until a connection attempt fails.
    let error_label = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(["error"])
        .visible(false)
        .build();

    let connect_button = gtk::Button::builder()
        .label("Connect")
        .halign(gtk::Align::Center)
        .css_classes(["suggested-action", "pill"])
        .build();

    let form = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .build();
    form.append(&group);
    form.append(&error_label);
    form.append(&connect_button);

    let clamp = adw::Clamp::builder().maximum_size(400).child(&form).build();

    let status = adw::StatusPage::builder()
        .icon_name("network-server-symbolic")
        .css_classes(["compact"])
        .title("Connect to Your Server")
        .description("Navidrome or another Subsonic-compatible server")
        .child(&clamp)
        .build();

    let nav = nav.clone();
    let group_for_click = group.clone();
    let error_for_click = error_label.clone();
    connect_button.connect_clicked(move |button| {
        let url = url_row.text().trim().to_string();
        let user = user_row.text().trim().to_string();
        let password = password_row.text().to_string();

        // Busy state: spinner instead of the label, form locked.
        button.set_sensitive(false);
        button.set_child(Some(&adw::Spinner::new()));
        group_for_click.set_sensitive(false);
        error_for_click.set_visible(false);

        let button = button.clone();
        let group = group_for_click.clone();
        let error_label = error_for_click.clone();
        let nav = nav.clone();

        // Runs on the GTK main loop; only the network call goes to tokio.
        glib::spawn_future_local(async move {
            let client = subsonic::Client::new(&url, &user, &password);
            let result = runtime().spawn(async move { client.ping().await }).await;

            button.set_label("Connect");
            button.set_sensitive(true);
            group.set_sensitive(true);

            match result {
                Ok(Ok(response)) => nav.replace(&[connected_page(&response)]),
                Ok(Err(e)) => {
                    error_label.set_label(&error_message(&e));
                    error_label.set_visible(true);
                }
                Err(_) => {
                    error_label.set_label("Something went wrong. Please try again.");
                    error_label.set_visible(true);
                }
            }
        });
    });

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&status));

    adw::NavigationPage::builder()
        .title("Connect")
        .child(&toolbar)
        .build()
}

/// Human-readable message for each kind of connection error.
fn error_message(e: &subsonic::Error) -> String {
    use subsonic::Error::*;
    match e {
        WrongCredentials => "Wrong username or password.".into(),
        Unreachable(_) => "Cannot reach the server. Check the address and your connection.".into(),
        InvalidAddress(_) => "Invalid server address. It should start with http:// or https://".into(),
        BadResponse(_) => "The server answered, but it does not look like a Subsonic server.".into(),
        Server { message, .. } => format!("Server error: {message}"),
    }
}

/// Placeholder shown after a successful login; the library will live here.
fn connected_page(r: &subsonic::Response) -> adw::NavigationPage {
    let status = adw::StatusPage::builder()
        .icon_name("emblem-ok-symbolic")
        .title("Connected")
        .description(format!(
            "{} {}",
            r.server_type.as_deref().unwrap_or("Server"),
            r.server_version.as_deref().unwrap_or("")
        ))
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&status));

    adw::NavigationPage::builder()
        .title("muse-player")
        .child(&toolbar)
        .build()
}
