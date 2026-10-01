//! The Background portal: asks GNOME to let the app go on without a window.
//! GNOME may ask the user once, then lists the app under "Background Apps" in
//! the system menu, where it can also be closed. The answer comes later, as a
//! Response signal; if it is a refusal while the window is hidden, the app quits.

use gettextrs::gettext;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};

thread_local! {
    /// Each request gets its own token, so answers cannot be mixed up.
    static TOKEN: Cell<u32> = const { Cell::new(0) };
    /// The listener for the pending answer; dropping it unsubscribes.
    static SUBSCRIPTION: RefCell<Option<gio::SignalSubscription>> = const { RefCell::new(None) };
}

/// Asks for permission to run in the background.
pub(crate) fn request() {
    glib::spawn_future_local(async move {
        // The app is already on the session bus (GTK connects at startup).
        let Some(connection) =
            gio::Application::default().and_then(|app| app.dbus_connection())
        else {
            eprintln!("Background portal: not on the session bus");
            return;
        };

        // The portal answers on a path made of our bus name and our token:
        // listen there first, so a quick answer is not missed.
        let token = TOKEN.with(|t| {
            t.set(t.get() + 1);
            format!("vumi_{}", t.get())
        });
        let Some(sender) = connection.unique_name() else {
            return;
        };
        let sender = sender.trim_start_matches(':').replace('.', "_");
        let request_path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
        let subscription = connection.subscribe_to_signal(
            Some("org.freedesktop.portal.Desktop"),
            Some("org.freedesktop.portal.Request"),
            Some("Response"),
            Some(&request_path),
            None,
            gio::DBusSignalFlags::NONE,
            |signal| {
                on_response(signal.parameters);
                // One answer per request: stop listening, but not from inside
                // the callback that is still running.
                glib::idle_add_local_once(|| {
                    SUBSCRIPTION.with(|s| s.borrow_mut().take());
                });
            },
        );
        SUBSCRIPTION.with(|s| s.replace(Some(subscription)));

        let options = glib::VariantDict::new(None);
        options.insert_value("handle_token", &token.to_variant());
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

/// Response(u response, a{sv} results): 0 means the user answered, and
/// results["background"] says whether running in the background is allowed.
fn on_response(params: &glib::Variant) {
    let answered = params.child_value(0).get::<u32>() == Some(0);
    let allowed = params
        .child_value(1)
        .get::<glib::VariantDict>()
        .and_then(|results| results.lookup::<bool>("background").ok().flatten())
        .unwrap_or(false);
    if answered && allowed {
        return;
    }
    // Refused (or the request failed): running on without a window would be
    // exactly what was refused, so the app quits if its window is hidden.
    eprintln!("Background portal: running in the background was not allowed");
    if let Some(app) = gio::Application::default() {
        let hidden = app
            .downcast_ref::<gtk::Application>()
            .and_then(|app| app.active_window())
            .is_some_and(|window| !window.is_visible());
        if hidden {
            app.quit();
        }
    }
}
