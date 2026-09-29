//! The Background portal: asks GNOME to let the app go on without a window.
//! GNOME may ask the user once, then lists the app under "Background Apps" in
//! the system menu, where it can also be closed.

use gettextrs::gettext;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

/// Asks for permission to run in the background. The answer comes later and is
/// not needed here: with or without it, the music goes on until the app quits.
pub(crate) fn request() {
    glib::spawn_future_local(async move {
        // The app is already on the session bus (GTK connects at startup).
        let Some(connection) = gio::Application::default().and_then(|app| app.dbus_connection())
        else {
            eprintln!("Background portal: not on the session bus");
            return;
        };
        let options = glib::VariantDict::new(None);
        options.insert_value(
            "reason",
            &gettext("Music keeps playing after the window is closed").to_variant(),
        );
        options.insert_value("autostart", &false.to_variant());
        options.insert_value("dbus-activatable", &false.to_variant());
        // (parent window, options): no parent handle, the request stands alone.
        let params = glib::Variant::tuple_from_iter(["".to_variant(), options.end()]);
        if let Err(e) = connection
            .call_future(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.Background",
                "RequestBackground",
                Some(&params),
                None,
                gio::DBusCallFlags::NONE,
                -1,
            )
            .await
        {
            eprintln!("Background portal: {e}");
        }
    });
}
