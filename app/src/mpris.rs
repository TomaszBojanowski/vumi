//! MPRIS: the player on the session bus (org.mpris.MediaPlayer2), so GNOME
//! Shell's media controls, the lock screen and the media keys can show and
//! control it. Built on GIO's own D-Bus support, on the GTK main loop.

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::OnceCell;

use crate::player;

const BUS_NAME: &str = "org.mpris.MediaPlayer2.Vumi";
const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

/// The two interfaces, as the MPRIS specification describes them.
const XML: &str = r#"<node>
  <interface name="org.mpris.MediaPlayer2">
    <method name="Raise"/>
    <method name="Quit"/>
    <property name="CanQuit" type="b" access="read"/>
    <property name="CanRaise" type="b" access="read"/>
    <property name="HasTrackList" type="b" access="read"/>
    <property name="Identity" type="s" access="read"/>
    <property name="DesktopEntry" type="s" access="read"/>
    <property name="SupportedUriSchemes" type="as" access="read"/>
    <property name="SupportedMimeTypes" type="as" access="read"/>
  </interface>
  <interface name="org.mpris.MediaPlayer2.Player">
    <method name="Next"/>
    <method name="Previous"/>
    <method name="Pause"/>
    <method name="PlayPause"/>
    <method name="Stop"/>
    <method name="Play"/>
    <method name="Seek">
      <arg direction="in" name="Offset" type="x"/>
    </method>
    <method name="SetPosition">
      <arg direction="in" name="TrackId" type="o"/>
      <arg direction="in" name="Position" type="x"/>
    </method>
    <method name="OpenUri">
      <arg direction="in" name="Uri" type="s"/>
    </method>
    <signal name="Seeked">
      <arg name="Position" type="x"/>
    </signal>
    <property name="PlaybackStatus" type="s" access="read"/>
    <property name="Rate" type="d" access="read"/>
    <property name="Metadata" type="a{sv}" access="read"/>
    <property name="Volume" type="d" access="readwrite"/>
    <property name="Position" type="x" access="read"/>
    <property name="MinimumRate" type="d" access="read"/>
    <property name="MaximumRate" type="d" access="read"/>
    <property name="CanGoNext" type="b" access="read"/>
    <property name="CanGoPrevious" type="b" access="read"/>
    <property name="CanPlay" type="b" access="read"/>
    <property name="CanPause" type="b" access="read"/>
    <property name="CanSeek" type="b" access="read"/>
    <property name="CanControl" type="b" access="read"/>
  </interface>
</node>"#;

thread_local! {
    static CONNECTION: OnceCell<gio::DBusConnection> = const { OnceCell::new() };
}

/// Puts the player on the session bus. Call once, at startup; `app` is
/// raised ("Raise") or quit ("Quit") when asked to.
pub(crate) fn start(app: &adw::Application) {
    let app = app.downgrade();
    let _ = gio::bus_own_name(
        gio::BusType::Session,
        BUS_NAME,
        gio::BusNameOwnerFlags::NONE,
        move |connection, _| register(&connection, app.clone()),
        |_, _| {},
        |_, name| eprintln!("MPRIS: could not own {name}"),
    );
}

fn register(connection: &gio::DBusConnection, app: glib::WeakRef<adw::Application>) {
    // Every jump of the position is announced once, from Player::seek.
    {
        let connection = connection.clone();
        player::get().connect_seeked(move |seconds| emit_seeked(&connection, seconds));
    }
    let node = match gio::DBusNodeInfo::for_xml(XML) {
        Ok(node) => node,
        Err(e) => {
            eprintln!("MPRIS: bad interface description: {e}");
            return;
        }
    };
    for name in [ROOT, PLAYER] {
        let Some(interface) = node.lookup_interface(name) else {
            continue;
        };
        let app = app.clone();
        let result = connection
            .register_object(PATH, &interface)
            .method_call(move |_, _, _, interface, method, params, invocation| {
                // D-Bus allows calls without an interface name; the player's methods
                // are matched by their name alone.
                handle_method(&app, interface.unwrap_or_default(), method, &params);
                invocation.return_value(None);
            })
            .property(|_, _, _, interface, property| get_property(interface, property))
            .set_property(|_, _, _, _, property, value| set_property(property, &value))
            .build();
        if let Err(e) = result {
            eprintln!("MPRIS: could not register {name}: {e}");
        }
    }
    CONNECTION.with(|cell| {
        let _ = cell.set(connection.clone());
    });

    // Every change of track or state goes out to GNOME Shell and the others;
    // a change of the volume goes out alone.
    player::get().connect_changed(emit_changed);
    player::get().connect_volume_changed(emit_volume);
}

fn handle_method(
    app: &glib::WeakRef<adw::Application>,
    interface: &str,
    method: &str,
    params: &glib::Variant,
) {
    let player = player::get();
    match (interface, method) {
        (ROOT, "Raise") => {
            if let Some(app) = app.upgrade() {
                app.activate();
            }
        }
        (ROOT, "Quit") => {
            if let Some(app) = app.upgrade() {
                app.quit();
            }
        }
        (_, "Next") => player.next(),
        (_, "Previous") => player.previous(),
        (_, "PlayPause") => player.toggle_pause(),
        (_, "Play") => {
            if !player.is_playing() {
                player.toggle_pause();
            }
        }
        (_, "Pause") => {
            if player.is_playing() {
                player.toggle_pause();
            }
        }
        (_, "Stop") => player.stop(),
        (_, "Seek") => {
            // Offset in microseconds, from the current position.
            if let Some(offset) = params.child_value(0).get::<i64>() {
                let target = player.position_secs().unwrap_or(0.0) + offset as f64 / 1e6;
                // Past the end of the track: like Next (the MPRIS specification).
                // Before its start, seek() goes to the start.
                if track_length().is_some_and(|length| target > length) {
                    player.next();
                } else {
                    player.seek(target);
                }
            }
        }
        (_, "SetPosition") => {
            // Only for the track the client meant: a late command about the
            // previous track must not move this one (the MPRIS specification).
            let wanted = params.child_value(0).get::<glib::variant::ObjectPath>();
            let current = player.current().and_then(|song| track_path(&song.id));
            if wanted.is_none() || wanted != current {
                return;
            }
            // Position in microseconds, from the start of the track.
            if let Some(position) = params.child_value(1).get::<i64>() {
                let target = position as f64 / 1e6;
                // Outside the track: ignored (the MPRIS specification).
                if target < 0.0 || track_length().is_some_and(|length| target > length) {
                    return;
                }
                player.seek(target);
            }
        }
        _ => {}
    }
}

/// The length of the current track in seconds, as Metadata gives it out.
fn track_length() -> Option<f64> {
    player::get()
        .current()?
        .duration
        .map(|seconds| seconds as f64)
}

/// "Playing", "Paused" or "Stopped" (nothing loaded, ended, or failed).
fn status() -> &'static str {
    let player = player::get();
    if player.is_playing() {
        "Playing"
    } else if player.is_paused() {
        "Paused"
    } else {
        "Stopped"
    }
}

/// The current track: title, artist, album, length and, if cached, the cover.
fn metadata() -> glib::Variant {
    let dict = glib::VariantDict::new(None);
    if let Some(song) = player::get().current() {
        if let Some(path) = track_path(&song.id) {
            dict.insert_value("mpris:trackid", &path.to_variant());
        }
        dict.insert_value("xesam:title", &song.title.to_variant());
        if let Some(artist) = &song.artist {
            dict.insert_value("xesam:artist", &vec![artist.clone()].to_variant());
        }
        if let Some(album) = &song.album {
            dict.insert_value("xesam:album", &album.to_variant());
        }
        if let Some(seconds) = song.duration {
            dict.insert_value("mpris:length", &(seconds as i64 * 1_000_000).to_variant());
        }
        if let Some(uri) = song
            .cover_art
            .as_deref()
            .and_then(crate::covers::cached_path)
            .and_then(|path| glib::filename_to_uri(path, None).ok())
        {
            dict.insert_value("mpris:artUrl", &uri.to_variant());
        }
    }
    dict.end()
}

fn get_property(interface: &str, property: &str) -> glib::Variant {
    let player = player::get();
    let loaded = player.current().is_some();
    match (interface, property) {
        (ROOT, "CanQuit") | (ROOT, "CanRaise") => true.to_variant(),
        (ROOT, "HasTrackList") => false.to_variant(),
        (ROOT, "Identity") => crate::APP_NAME.to_variant(),
        (ROOT, "DesktopEntry") => crate::APP_ID.to_variant(),
        (ROOT, "SupportedUriSchemes") | (ROOT, "SupportedMimeTypes") => {
            Vec::<String>::new().to_variant()
        }
        (_, "PlaybackStatus") => status().to_variant(),
        (_, "Rate") | (_, "MinimumRate") | (_, "MaximumRate") => 1.0f64.to_variant(),
        // As on the app's own slider, from 0 to 1.
        (_, "Volume") => player.volume().to_variant(),
        (_, "Metadata") => metadata(),
        (_, "Position") => ((player.position_secs().unwrap_or(0.0) * 1e6) as i64).to_variant(),
        (_, "CanGoNext") => player.has_next().to_variant(),
        (_, "CanGoPrevious") | (_, "CanPlay") | (_, "CanPause") | (_, "CanSeek") => {
            loaded.to_variant()
        }
        (_, "CanControl") => true.to_variant(),
        _ => false.to_variant(),
    }
}

/// Volume is the only property a client may set. The player announces the
/// change itself (emit_changed), and the slider in the app follows it.
fn set_property(property: &str, value: &glib::Variant) -> bool {
    match (property, value.get::<f64>()) {
        // Anything that is not a number (NaN, infinity) is refused; the
        // player keeps the rest between 0 and 1.
        ("Volume", Some(volume)) if volume.is_finite() => {
            player::get().set_volume(volume);
            true
        }
        _ => false,
    }
}

/// PropertiesChanged: what GNOME Shell shows follows the player.
fn emit_changed() {
    CONNECTION.with(|cell| {
        let Some(connection) = cell.get() else {
            return;
        };
        let player = player::get();
        let loaded = player.current().is_some();
        let changed = glib::VariantDict::new(None);
        changed.insert_value("PlaybackStatus", &status().to_variant());
        changed.insert_value("Metadata", &metadata());
        changed.insert_value("CanGoNext", &player.has_next().to_variant());
        changed.insert_value("CanGoPrevious", &loaded.to_variant());
        changed.insert_value("CanPlay", &loaded.to_variant());
        changed.insert_value("CanPause", &loaded.to_variant());
        changed.insert_value("CanSeek", &loaded.to_variant());
        changed.insert_value("Volume", &player.volume().to_variant());
        let params = glib::Variant::tuple_from_iter([
            PLAYER.to_variant(),
            changed.end(),
            Vec::<String>::new().to_variant(),
        ]);
        let _ = connection.emit_signal(
            None,
            PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            Some(&params),
        );
        // The cover of a new track is often still downloading at this point:
        // fetch it, and send the metadata again once it is there.
        fetch_missing_cover();
    });
}

/// PropertiesChanged with the volume alone (the slider, the scroll wheel, a
/// client).
fn emit_volume(volume: f64) {
    CONNECTION.with(|cell| {
        let Some(connection) = cell.get() else {
            return;
        };
        let changed = glib::VariantDict::new(None);
        changed.insert_value("Volume", &volume.to_variant());
        let params = glib::Variant::tuple_from_iter([
            PLAYER.to_variant(),
            changed.end(),
            Vec::<String>::new().to_variant(),
        ]);
        let _ = connection.emit_signal(
            None,
            PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            Some(&params),
        );
    });
}

/// Seeked: the position jumped (the slider, Previous, resuming, a client).
fn emit_seeked(connection: &gio::DBusConnection, seconds: f64) {
    let _ = connection.emit_signal(
        None,
        PATH,
        PLAYER,
        "Seeked",
        Some(&(((seconds * 1e6) as i64),).to_variant()),
    );
}

/// If the current track has a cover that is not on disk yet, loads it (from the
/// server if need be) and sends the metadata again once it is there.
fn fetch_missing_cover() {
    let player = player::get();
    let Some(id) = player.current().and_then(|song| song.cover_art) else {
        return;
    };
    if crate::covers::cached_path(&id).is_some() {
        return;
    }
    let client = player.client();
    glib::spawn_future_local(async move {
        let _ = crate::covers::load(client, id.clone()).await;
        // Only if it really arrived, and still for the same track: no loop.
        let same_track = player::get()
            .current()
            .and_then(|song| song.cover_art)
            .as_deref()
            == Some(id.as_str());
        if same_track && crate::covers::cached_path(&id).is_some() {
            emit_changed();
        }
    });
}

/// The D-Bus object path that stands for a track (mpris:trackid). A path may
/// hold only letters, digits and "_", so other characters become "_".
fn track_path(song_id: &str) -> Option<glib::variant::ObjectPath> {
    let id: String = song_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    glib::variant::ObjectPath::try_from(format!("/io/github/tomaszbojanowski/Vumi/track/t{id}"))
        .ok()
}
