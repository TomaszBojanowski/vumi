use adw::prelude::*;

// Working ID, will change together with the final app name.
const APP_ID: &str = "io.github.tomaszbojanowski.MusePlayer";

fn main() -> gtk::glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &adw::Application) {
    let versions = format!(
        "GTK {}.{}.{} · libadwaita {}.{}.{}",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        adw::major_version(),
        adw::minor_version(),
        adw::micro_version(),
    );

    let status = adw::StatusPage::builder()
        .icon_name("audio-x-generic-symbolic")
        .title("muse-player")
        .description(versions)
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&status));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("muse-player")
        .default_width(900)
        .default_height(600)
        .content(&toolbar)
        .build();
    window.present();
}
