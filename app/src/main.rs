use adw::prelude::*;

// Working ID, will change together with the final app name.
const APP_ID: &str = "io.github.tomaszbojanowski.MusePlayer";

fn main() -> gtk::glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &adw::Application) {
    let nav = adw::NavigationView::new();
    let login_page = build_login_page();
    let welcome_page = build_welcome_page(&nav, &login_page);
    nav.push(&welcome_page);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("muse-player")
        .default_width(900)
        .default_height(600)
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

fn build_login_page() -> adw::NavigationPage {
    let url_row = adw::EntryRow::builder().title("Server address").build();
    let user_row = adw::EntryRow::builder().title("Username").build();
    let password_row = adw::PasswordEntryRow::builder().title("Password").build();

    let group = adw::PreferencesGroup::new();
    group.add(&url_row);
    group.add(&user_row);
    group.add(&password_row);

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
    form.append(&connect_button);

    let clamp = adw::Clamp::builder().maximum_size(400).child(&form).build();

    let status = adw::StatusPage::builder()
        .icon_name("network-server-symbolic")
        .css_classes(["compact"])
        .title("Connect to Your Server")
        .description("Navidrome or another Subsonic-compatible server")
        .child(&clamp)
        .build();

    // For now the button only reports what was typed (never the password).
    let url_for_click = url_row.clone();
    let user_for_click = user_row.clone();
    connect_button.connect_clicked(move |_| {
        println!(
            "Connect to {} as {}",
            url_for_click.text(),
            user_for_click.text()
        );
    });

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&status));

    adw::NavigationPage::builder()
        .title("Connect")
        .child(&toolbar)
        .build()
}
