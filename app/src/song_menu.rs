//! The menu of a song row (album pages, search results, favourites): a ⋮
//! button at the end of the row, and only there, by design (no right-click
//! menu).

use adw::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::favorites::{self, Favorite};
use crate::main_view::MainView;
use crate::player;

/// Asks for the song a menu is about, and the client (None offline), when the
/// menu is used.
pub(crate) type Current = Rc<dyn Fn() -> Option<(subsonic::Song, Option<subsonic::Client>)>>;

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
    current: impl Fn() -> Option<(subsonic::Song, Option<subsonic::Client>)> + 'static,
    go_to_album: bool,
) -> gtk::Button {
    let owner = owner.upcast_ref::<gtk::Widget>().downgrade();
    let current: Current = Rc::new(current);
    let made = Cell::new(false);
    more_button(move || {
        let owner = owner.upgrade()?;
        if !made.replace(true) {
            install_actions(&owner, &current);
        }
        // Whether the song is a favourite now: the item says what it will do.
        let starred = current().is_some_and(|(song, _)| {
            favorites::is_starred(&Favorite::Song(song.id.clone()), song.starred.is_some())
        });
        Some(menu(go_to_album, starred))
    })
}

/// A ⋮ button that opens a menu: a button and its image, where a
/// gtk::MenuButton is five widgets (it sits in every row of a list). `menu`
/// gives the menu each time it is about to open, so an item can say what is
/// true then; the popover is made on the first click. While the menu is
/// open, the button looks pressed, as a menu button does.
///
/// The popover hangs on the widget the button sits in and points at the
/// button: hung on the button itself, it would keep the button lit as if
/// under the pointer while the pointer is in the menu (a menu button's
/// popover is not inside its button either).
pub(crate) fn more_button(menu: impl Fn() -> Option<gio::Menu> + 'static) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text(gettext("More"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    button.update_property(&[
        gtk::accessible::Property::Label(&gettext("More")),
        gtk::accessible::Property::HasPopup(true),
    ]);
    button.update_state(&[gtk::accessible::State::Expanded(Some(false))]);
    let popover: Rc<RefCell<Option<gtk::PopoverMenu>>> = Rc::default();
    let slot = popover.clone();
    button.connect_clicked(move |button| {
        let Some(model) = menu() else {
            return;
        };
        let Some(parent) = button.parent() else {
            return;
        };
        let mut slot = slot.borrow_mut();
        let popover = slot.get_or_insert_with(|| {
            let popover = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
            popover.set_parent(&parent);
            let button = button.downgrade();
            popover.connect_closed(move |_| {
                if let Some(button) = button.upgrade() {
                    button.unset_state_flags(gtk::StateFlags::CHECKED);
                    button.update_state(&[gtk::accessible::State::Expanded(Some(false))]);
                }
            });
            popover
        });
        popover.set_menu_model(Some(&model));
        if let Some(bounds) = button.compute_bounds(&parent) {
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                bounds.x() as i32,
                bounds.y() as i32,
                bounds.width() as i32,
                bounds.height() as i32,
            )));
        }
        button.set_state_flags(gtk::StateFlags::CHECKED, false);
        button.update_state(&[gtk::accessible::State::Expanded(Some(true))]);
        popover.popup();
    });
    // The popover goes when the button does.
    button.connect_destroy(move |_| {
        if let Some(popover) = popover.borrow_mut().take() {
            popover.unparent();
        }
    });
    button
}

/// What a song menu item does with the row, its song and the client. Offline
/// (no client) the album, the artist and the information still open; what
/// needs the server says so.
type SongAction = Box<dyn Fn(&gtk::Widget, subsonic::Song, Option<subsonic::Client>)>;

/// Puts the actions a song menu can use on `row`, as the group "song":
/// play-next, add-to-queue, add-to-playlist, favorite, play-similar,
/// go-to-album, go-to-artist and info. The queue builds a menu of its own
/// from them.
pub(crate) fn install_actions(row: &gtk::Widget, current: &Current) {
    let actions = gio::SimpleActionGroup::new();
    let add = |name: &str, run: SongAction| {
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

    // Said only when it happened: offline the player says why not.
    add(
        "play-next",
        Box::new(|row, song, client| match client {
            Some(client) => {
                if player::get().play_next(client, song) {
                    crate::toast(row, &gettext("Will play next"));
                }
            }
            None => crate::cannot_play(row),
        }),
    );
    add(
        "add-to-queue",
        Box::new(|row, song, client| match client {
            Some(client) => {
                if player::get().enqueue(client, song) {
                    crate::toast(row, &gettext("Added to the queue"));
                }
            }
            None => crate::cannot_play(row),
        }),
    );
    add(
        "add-to-playlist",
        Box::new(|row, song, client| match client {
            Some(client) => choose_playlist(row, song, client),
            None => crate::toast(
                row,
                &crate::error_message(&subsonic::Error::Unreachable(String::new())),
            ),
        }),
    );
    // Adds the song to the favourites, or takes it out if it is one: the
    // star of a row shows only under the pointer, and a touch screen has none.
    add(
        "favorite",
        Box::new(|row, song, client| match client {
            Some(client) => {
                let item = Favorite::Song(song.id.clone());
                let starred = favorites::is_starred(&item, song.starred.is_some());
                favorites::set(row, client, item, !starred);
            }
            None => crate::toast(
                row,
                &crate::error_message(&subsonic::Error::Unreachable(String::new())),
            ),
        }),
    );
    add(
        "play-similar",
        Box::new(|row, song, client| match client {
            Some(client) => play_similar(row, song, client),
            None => crate::cannot_play(row),
        }),
    );
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
                genre: None,
                record_label: None,
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
}

/// The menu of a song in a list, for the actions above; `starred`: the song
/// is a favourite now.
fn menu(go_to_album: bool, starred: bool) -> gio::Menu {
    // Grouped as in the picture: queue, radio, navigation, information.
    let menu = gio::Menu::new();
    let queue = gio::Menu::new();
    queue.append(Some(&gettext("Play Next")), Some("song.play-next"));
    queue.append(Some(&gettext("Add to Queue")), Some("song.add-to-queue"));
    queue.append(
        Some(&gettext("Add to Playlist…")),
        Some("song.add-to-playlist"),
    );
    let favorite = if starred {
        gettext("Remove from Favorites")
    } else {
        gettext("Add to Favorites")
    };
    queue.append(Some(&favorite), Some("song.favorite"));
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

/// Play Similar: this song, then songs that sound like it.
fn play_similar(row: &gtk::Widget, song: subsonic::Song, client: subsonic::Client) {
    play_radio(row, client, song, || {});
}

/// Radio from `seed`: the song, then about 50 songs that sound like it
/// (AudioMuse-AI through Navidrome), as a list of songs. `done` runs once the
/// answer is there; a note near `widget` says why nothing plays (none found,
/// or the request failed). An answer that comes after something else was
/// started is dropped.
pub(crate) fn play_radio(
    widget: &impl IsA<gtk::Widget>,
    client: subsonic::Client,
    seed: subsonic::Song,
    done: impl FnOnce() + 'static,
) {
    let widget = widget.upcast_ref::<gtk::Widget>().downgrade();
    let command = player::get().command_id();
    glib::spawn_future_local(async move {
        let request = client.clone();
        let id = seed.id.clone();
        let result = crate::runtime()
            .spawn(async move { request.similar_songs(&id, 50).await })
            .await;
        done();
        // Something else was started meanwhile: this answer is stale.
        if !player::get().is_current(command) {
            return;
        }
        let Some(similar) = similar_found(widget.upgrade().as_ref(), result) else {
            return;
        };
        let seed_id = seed.id.clone();
        let mut songs = vec![seed];
        songs.extend(similar.into_iter().filter(|s| s.id != seed_id));
        player::get().play(client, songs.into(), 0, player::QueueOf::Songs);
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
                        crate::toast(
                            &main,
                            // Translators: {name} is the name of a playlist.
                            &gettext("Added to “{name}”").replace("{name}", &playlist.name),
                        );
                        main.sync_playlists();
                    }
                    _ => crate::toast(&main, &gettext("Could not add to the playlist")),
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
            row.set_subtitle(&crate::format::tracks_text(
                playlist.song_count.unwrap_or(0),
            ));
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
                        .spawn(
                            async move { request.add_to_playlist(&id, &[song_id.as_str()]).await },
                        )
                        .await;
                    match result {
                        Ok(Ok(())) => {
                            crate::toast(
                                &main,
                                &gettext("Added to “{name}”").replace("{name}", &name),
                            );
                            main.sync_playlists();
                        }
                        _ => crate::toast(&main, &gettext("Could not add to the playlist")),
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

/// The rows of Song Information, title and value, only those the song has.
fn song_details(song: &subsonic::Song) -> Vec<(String, String)> {
    use crate::format::{bits_text, clock, kbps_text, khz_text};
    let rows = [
        (gettext("Title"), Some(song.title.clone())),
        (gettext("Artist"), song.artist.clone()),
        (gettext("Composer"), song.composer.clone()),
        (gettext("Album"), song.album.clone()),
        (gettext("Track"), song.track.map(|n| n.to_string())),
        (gettext("Disc"), song.disc_number.map(|n| n.to_string())),
        (gettext("Duration"), song.duration.map(|d| clock(d.into()))),
        (
            gettext("Format"),
            song.suffix.as_ref().map(|s| s.to_uppercase()),
        ),
        // A lossy file tells 0 (or nothing) for its bit depth.
        (
            gettext("Bit Depth"),
            song.bit_depth.filter(|&b| b > 0).map(bits_text),
        ),
        (
            gettext("Sample Rate"),
            song.sampling_rate.filter(|&r| r > 0).map(khz_text),
        ),
        (
            gettext("Bit Rate"),
            song.bit_rate.filter(|&r| r > 0).map(kbps_text),
        ),
    ];
    rows.into_iter()
        .filter_map(|(title, value)| value.filter(|v| !v.is_empty()).map(|v| (title, v)))
        .collect()
}

/// Song Information: what the server knows about the file.
fn show_info(parent: &gtk::Widget, song: &subsonic::Song) {
    let group = adw::PreferencesGroup::new();
    for (title, value) in song_details(song) {
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

/// The songs a request for similar songs found, or None after a note in the
/// window near `widget`: why the request failed, or that the server found
/// none (an album not analysed yet, say).
pub(crate) fn similar_found(
    widget: Option<&impl IsA<gtk::Widget>>,
    result: Result<Result<Vec<subsonic::Song>, subsonic::Error>, tokio::task::JoinError>,
) -> Option<Vec<subsonic::Song>> {
    let note = match result {
        Ok(Ok(songs)) if !songs.is_empty() => return Some(songs),
        Ok(Ok(_)) => gettext("No similar songs found"),
        Ok(Err(e)) => {
            eprintln!("Could not fetch similar songs: {e}");
            crate::error_message(&e)
        }
        Err(_) => gettext("Something went wrong. Please try again."),
    };
    if let Some(widget) = widget {
        crate::toast(widget, &note);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(depth: Option<u32>, rate: Option<u32>, kbps: Option<u32>) -> subsonic::Song {
        subsonic::Song {
            id: "s".into(),
            title: "Title".into(),
            artist: Some("Artist".into()),
            track: Some(3),
            album: Some("Album".into()),
            duration: Some(125),
            suffix: Some("flac".into()),
            bit_rate: kbps,
            bit_depth: depth,
            sampling_rate: rate,
            ..Default::default()
        }
    }

    fn value<'a>(rows: &'a [(String, String)], title: &str) -> Option<&'a str> {
        rows.iter()
            .find(|(t, _)| t == title)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn units_and_decimals_come_from_the_translations() {
        // No translations in tests: what the English text gives.
        let rows = song_details(&song(Some(24), Some(44_100), Some(1429)));
        assert_eq!(value(&rows, "Bit Depth"), Some("24 bit"));
        assert_eq!(value(&rows, "Sample Rate"), Some("44.1 kHz"));
        assert_eq!(value(&rows, "Bit Rate"), Some("1429 kbps"));
        assert_eq!(value(&rows, "Duration"), Some("2:05"));
        assert_eq!(value(&rows, "Format"), Some("FLAC"));
        assert_eq!(value(&rows, "Disc"), None);
    }

    #[test]
    fn a_lossy_file_shows_no_bit_depth() {
        let rows = song_details(&song(Some(0), Some(48_000), Some(320)));
        assert_eq!(value(&rows, "Bit Depth"), None);
        assert_eq!(value(&rows, "Sample Rate"), Some("48 kHz"));
        assert_eq!(value(&rows, "Bit Rate"), Some("320 kbps"));
    }
}
