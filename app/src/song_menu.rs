//! The menu of a song row (album pages, search results, favourites): a ⋮
//! button at the end of the row.

use adw::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::{gio, glib};
use std::cell::Cell;
use std::rc::Rc;

use crate::main_view::MainView;
use crate::player;

/// Gives `row` a ⋮ menu for the song at `index` in `songs`; the menu is
/// under that button only, by design (no right-click menu). The list is
/// shared, not copied; the song is taken from it when the menu is used.
/// `go_to_album` is false on the album's own page.
pub(crate) fn attach(
    row: &adw::ActionRow,
    songs: &Rc<[subsonic::Song]>,
    index: usize,
    client: Option<subsonic::Client>,
    go_to_album: bool,
) {
    let songs = songs.clone();
    let button = menu_button(
        row,
        move || songs.get(index).cloned().zip(client.clone()),
        go_to_album,
    );
    row.add_suffix(&button);
}

/// Asks for the song a menu is about, and the client, when the menu is used.
type Current = Rc<dyn Fn() -> Option<(subsonic::Song, subsonic::Client)>>;

/// The ⋮ button with the song menu, for the end of a row. Its actions live on
/// `owner` (the row, or what the row holds) and ask `current` for the song
/// and the client only when one of them is used, so a row that is reused for
/// another song needs no new menu.
///
/// The menu itself (its actions, its items and the popover) is made on the
/// first click: most rows are never clicked there, and the menu costs several
/// times what the rest of a row does.
pub(crate) fn menu_button(
    owner: &impl IsA<gtk::Widget>,
    current: impl Fn() -> Option<(subsonic::Song, subsonic::Client)> + 'static,
    go_to_album: bool,
) -> gtk::MenuButton {
    let button = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text(gettext("More"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    let owner = owner.upcast_ref::<gtk::Widget>().downgrade();
    let current: Current = Rc::new(current);
    let made = Cell::new(false);
    // Called each time the menu is about to open; only the first time counts.
    button.set_create_popup_func(move |button| {
        if made.replace(true) {
            return;
        }
        if let Some(owner) = owner.upgrade() {
            button.set_menu_model(Some(&menu(&owner, &current, go_to_album)));
        }
    });
    button
}

/// The actions of the song menu, put on `row`, and the menu that uses them.
fn menu(row: &gtk::Widget, current: &Current, go_to_album: bool) -> gio::Menu {
    let actions = gio::SimpleActionGroup::new();
    let add = |name: &str, run: Box<dyn Fn(&gtk::Widget, subsonic::Song, subsonic::Client)>| {
        let action = gio::SimpleAction::new(name, None);
        let current = current.clone();
        let row_weak = row.downgrade();
        action.connect_activate(move |_, _| {
            let (Some(row), Some((song, client))) = (row_weak.upgrade(), current()) else {
                return;
            };
            run(&row, song, client);
        });
        actions.add_action(&action);
    };

    add(
        "play-next",
        Box::new(|row, song, client| {
            player::get().play_next(client, song);
            toast(row, &gettext("Will play next"));
        }),
    );
    add(
        "add-to-queue",
        Box::new(|row, song, client| {
            player::get().enqueue(client, song);
            toast(row, &gettext("Added to the queue"));
        }),
    );
    add(
        "add-to-playlist",
        Box::new(|row, song, client| choose_playlist(row, song, client)),
    );
    add("play-similar", Box::new(|row, song, client| play_similar(row, song, client)));
    add(
        "go-to-album",
        Box::new(|row, song, _| {
            let Some(album_id) = song.album_id.clone() else {
                return;
            };
            let fallback = subsonic::Album {
                id: album_id.clone(),
                name: song.album.clone().unwrap_or_default(),
                artist: song.artist.clone(),
                year: None,
                song_count: None,
                created: None,
                cover_art: song.cover_art.clone(),
                artist_id: None,
                starred: None,
            };
            if let Some(main) = row
                .ancestor(MainView::static_type())
                .and_downcast::<MainView>()
            {
                main.open_album_from(row, &album_id, fallback);
            }
        }),
    );
    add(
        "go-to-artist",
        Box::new(|row, song, client| {
            let Some(artist_id) = song.artist_id.clone() else {
                return;
            };
            let name = song.artist.clone().unwrap_or_default();
            if let Some(main) = row
                .ancestor(MainView::static_type())
                .and_downcast::<MainView>()
            {
                main.open_artist_from(row, &artist_id, &name, client);
            }
        }),
    );
    add("info", Box::new(|row, song, _| show_info(row, &song)));
    row.insert_action_group("song", Some(&actions));

    // Grouped as in the picture: queue, radio, navigation, information.
    let menu = gio::Menu::new();
    let queue = gio::Menu::new();
    queue.append(Some(&gettext("Play Next")), Some("song.play-next"));
    queue.append(Some(&gettext("Add to Queue")), Some("song.add-to-queue"));
    queue.append(Some(&gettext("Add to Playlist…")), Some("song.add-to-playlist"));
    menu.append_section(None, &queue);
    let radio = gio::Menu::new();
    radio.append(Some(&gettext("Play Similar")), Some("song.play-similar"));
    menu.append_section(None, &radio);
    let navigation = gio::Menu::new();
    // On the album's own page, going to the album would lead nowhere.
    if go_to_album {
        navigation.append(Some(&gettext("Go to Album")), Some("song.go-to-album"));
    }
    navigation.append(Some(&gettext("Go to Artist")), Some("song.go-to-artist"));
    menu.append_section(None, &navigation);
    let info = gio::Menu::new();
    info.append(Some(&gettext("Song Information")), Some("song.info"));
    menu.append_section(None, &info);
    menu
}

/// Play Similar: this song, then songs that sound like it (AudioMuse-AI).
fn play_similar(row: &gtk::Widget, song: subsonic::Song, client: subsonic::Client) {
    let row = row.downgrade();
    let command = player::get().command_id();
    glib::spawn_future_local(async move {
        let request = client.clone();
        let id = song.id.clone();
        let result = crate::runtime()
            .spawn(async move { request.similar_songs(&id, 50).await })
            .await;
        // Something else was started meanwhile: this answer is stale.
        if !player::get().is_current(command) {
            return;
        }
        let similar = match result {
            Ok(Ok(songs)) => songs,
            _ => Vec::new(),
        };
        if similar.is_empty() {
            if let Some(row) = row.upgrade() {
                toast(&row, &gettext("No similar songs found"));
            }
            return;
        }
        let seed_id = song.id.clone();
        let mut songs = vec![song];
        songs.extend(similar.into_iter().filter(|s| s.id != seed_id));
        player::get().play(client, songs.into(), 0);
    });
}

/// Add to Playlist: a dialog with a field for a new playlist and the list of
/// the existing ones; a click on one adds the song to it.
fn choose_playlist(parent: &gtk::Widget, song: subsonic::Song, client: subsonic::Client) {
    let Some(main) = parent
        .ancestor(MainView::static_type())
        .and_downcast::<MainView>()
    else {
        return;
    };

    let dialog = adw::Dialog::builder()
        .title(gettext("Add to Playlist"))
        .content_width(420)
        .content_height(520)
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(18)
        .margin_end(18)
        .build();

    // A new playlist, made with this song, on Enter.
    let entry = adw::EntryRow::builder()
        .title(gettext("New Playlist"))
        .show_apply_button(true)
        .build();
    let new_group = adw::PreferencesGroup::new();
    new_group.add(&entry);
    content.append(&new_group);
    {
        let dialog = dialog.downgrade();
        let main = main.downgrade();
        let song = song.clone();
        let client = client.clone();
        entry.connect_apply(move |entry| {
            let name = entry.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            let (Some(dialog), Some(main)) = (dialog.upgrade(), main.upgrade()) else {
                return;
            };
            dialog.close();
            let song_id = song.id.clone();
            let request = client.clone();
            glib::spawn_future_local(async move {
                let result = crate::runtime()
                    .spawn(async move { request.create_playlist(&name, &[song_id.as_str()]).await })
                    .await;
                match result {
                    Ok(Ok(playlist)) => {
                        // Translators: {name} is the name of a playlist.
                        toast(&main, &gettext("Added to “{name}”").replace("{name}", &playlist.name));
                        main.sync_playlists();
                    }
                    _ => toast(&main, &gettext("Could not add to the playlist")),
                }
            });
        });
    }

    // The existing playlists.
    let playlists = main.playlists();
    if !playlists.is_empty() {
        let group = adw::PreferencesGroup::builder()
            .title(gettext("Playlists"))
            .build();
        for playlist in playlists {
            let row = adw::ActionRow::builder()
                .title(playlist.name.as_str())
                .use_markup(false)
                .activatable(true)
                .build();
            let count = playlist.song_count.unwrap_or(0);
            // Translators: {n} is a number of tracks.
            row.set_subtitle(
                &ngettext("{n} track", "{n} tracks", count).replace("{n}", &count.to_string()),
            );
            row.add_suffix(&gtk::Image::from_icon_name("list-add-symbolic"));

            let dialog = dialog.downgrade();
            let main = main.downgrade();
            let song_id = song.id.clone();
            let client = client.clone();
            let id = playlist.id.clone();
            let name = playlist.name.clone();
            row.connect_activated(move |_| {
                let (Some(dialog), Some(main)) = (dialog.upgrade(), main.upgrade()) else {
                    return;
                };
                dialog.close();
                let request = client.clone();
                let id = id.clone();
                let song_id = song_id.clone();
                let name = name.clone();
                glib::spawn_future_local(async move {
                    let result = crate::runtime()
                        .spawn(async move { request.add_to_playlist(&id, &[song_id.as_str()]).await })
                        .await;
                    match result {
                        Ok(Ok(())) => {
                            toast(&main, &gettext("Added to “{name}”").replace("{name}", &name));
                            main.sync_playlists();
                        }
                        _ => toast(&main, &gettext("Could not add to the playlist")),
                    }
                });
            });
            group.add(&row);
        }
        content.append(&group);
    }

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&scroll));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(parent));
}

/// Song Information: what the server knows about the file.
fn show_info(parent: &gtk::Widget, song: &subsonic::Song) {
    let group = adw::PreferencesGroup::new();
    let add = |title: String, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            let row = adw::ActionRow::builder()
                .title(title)
                .use_markup(false)
                .subtitle_selectable(true)
                .css_classes(["property"])
                .build();
            // After use_markup is off: "&" in names is plain text.
            row.set_subtitle(&value);
            group.add(&row);
        }
    };
    add(gettext("Title"), Some(song.title.clone()));
    add(gettext("Artist"), song.artist.clone());
    add(gettext("Composer"), song.composer.clone());
    add(gettext("Album"), song.album.clone());
    add(gettext("Track"), song.track.map(|n| n.to_string()));
    add(gettext("Disc"), song.disc_number.map(|n| n.to_string()));
    add(
        gettext("Duration"),
        song.duration.map(|d| format!("{}:{:02}", d / 60, d % 60)),
    );
    add(gettext("Format"), song.suffix.as_ref().map(|s| s.to_uppercase()));
    add(gettext("Bit Depth"), song.bit_depth.map(|b| format!("{b} bit")));
    add(
        gettext("Sample Rate"),
        song.sampling_rate
            .map(|r| format!("{} kHz", (r as f64 / 1000.0).to_string().replace('.', ","))),
    );
    add(gettext("Bit Rate"), song.bit_rate.map(|r| format!("{r} kbps")));

    let page = adw::PreferencesPage::new();
    page.add(&group);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&page));
    let dialog = adw::Dialog::builder()
        .title(gettext("Song Information"))
        .content_width(420)
        .content_height(560)
        .child(&toolbar)
        .build();
    dialog.present(Some(parent));
}

/// A short note at the bottom of the window.
fn toast(widget: &impl IsA<gtk::Widget>, text: &str) {
    if let Some(toasts) = widget
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()
    {
        toasts.add_toast(crate::plain_toast(text));
    }
}
