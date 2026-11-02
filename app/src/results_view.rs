//! Albums as cards and songs as a list, with the playing song marked: used by
//! the search results, the Favorites page and playlist pages.
//!
//! The songs are a list view over a model, so rows exist only for the songs
//! on screen (and a few hundred around them) and are reused while scrolling.
//! The album cards and the "Songs" heading are the first item of that list:
//! they scroll away with the songs, as one page.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use crate::album_card::AlbumCard;
use crate::main_view::MainView;
use crate::player;
use crate::song_row::SongRow;

/// What a view is asked to show: the albums, the songs and the client for
/// covers, playing and the song menu.
pub(crate) type Content = (
    Vec<Arc<subsonic::Album>>,
    Vec<subsonic::Song>,
    Option<subsonic::Client>,
);

/// What a page says about a song on its row; empty for nothing.
type Note = Rc<dyn Fn(&subsonic::Song) -> String>;

/// One item of the list: what stands above the songs, or a song (boxed: a
/// header item would otherwise take a song's size).
enum Entry {
    Header,
    Song(Box<subsonic::Song>),
}

/// The albums on screen and the account they came from.
type Shown = (Vec<Arc<subsonic::Album>>, Option<String>);

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/results_view.blp")]
    pub struct ResultsView {
        #[template_child]
        pub stack: gtk::TemplateChild<gtk::Stack>,
        /// The album cards and the "Songs" heading; the child of the first row.
        #[template_child]
        pub header: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub albums_section: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub albums_box: gtk::TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub albums_count: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub songs_count: gtk::TemplateChild<gtk::Label>,
        /// Whether the headings say how many albums and songs there are.
        pub show_counts: Cell<bool>,
        #[template_child]
        pub header_top: gtk::TemplateChild<adw::Bin>,
        #[template_child]
        pub songs_title: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub songs_row: gtk::TemplateChild<gtk::Box>,
        /// A narrow view: less room around the list, and what stands at the
        /// end of the "Songs" heading goes under it.
        pub narrow: Cell<bool>,
        #[template_child]
        pub songs_end: gtk::TemplateChild<adw::Bin>,
        /// A few words to show on each song's row before its length, if the
        /// page wants any (when the song was starred, say).
        pub note: RefCell<Option<Note>>,
        #[template_child]
        pub songs_list: gtk::TemplateChild<gtk::ListView>,
        #[template_child]
        pub empty_page: gtk::TemplateChild<adw::StatusPage>,
        /// What the list shows: the header, then the songs.
        pub entries: OnceCell<gio::ListStore>,
        /// The list item that holds the header now. A widget has one parent,
        /// so it is taken out of there before another item gets it.
        pub header_item: RefCell<Option<glib::WeakRef<gtk::ListItem>>>,
        /// The song rows that exist, to mark the one that is playing.
        pub rows: RefCell<Vec<glib::WeakRef<SongRow>>>,
        /// The songs listed: a click on a song plays it, then the ones after it.
        pub songs: RefCell<Option<Rc<[subsonic::Song]>>>,
        /// For playing, and for what the song menu does.
        pub client: RefCell<Option<subsonic::Client>>,
        /// Asked for while the view was not on screen: shown when it gets there.
        pub pending: RefCell<Option<Content>>,
        /// The albums on screen and the account they came from (the songs are
        /// in `songs`): the same content again is not built again.
        pub shown: RefCell<Option<super::Shown>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ResultsView {
        // Must match "template $VumiResultsView" in the Blueprint file.
        const NAME: &'static str = "VumiResultsView";
        type Type = super::ResultsView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ResultsView {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_list();
            // The playing song is marked, as on album pages. The search results
            // and the Favorites live as long as the app, but a playlist page has
            // a view of its own: it stops listening when the page is closed.
            let view = self.obj().downgrade();
            let listener = player::get().connect_changed(move || {
                if let Some(view) = view.upgrade() {
                    view.mark_playing();
                }
            });
            self.obj()
                .connect_destroy(move |_| player::get().disconnect(listener));

            // What was asked for while the view was hidden is built now.
            self.obj().connect_map(|view| {
                let pending = view.imp().pending.take();
                if let Some(content) = pending {
                    view.fill(content);
                }
            });
        }
    }
    impl WidgetImpl for ResultsView {}
    impl BinImpl for ResultsView {}
}

glib::wrapper! {
    pub struct ResultsView(ObjectSubclass<imp::ResultsView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ResultsView {
    /// Connects the list to its store and tells it how to fill a row.
    fn setup_list(&self) {
        let imp = self.imp();
        let entries = gio::ListStore::new::<glib::BoxedAnyObject>();

        // A row holds the header or a song, and when it is reused the other
        // may come: what the row holds is settled in bind, not in setup.
        let factory = gtk::SignalListItemFactory::new();
        let view = self.downgrade();
        factory.connect_bind(move |_, item| {
            let (Some(view), Some(item)) = (view.upgrade(), item.downcast_ref::<gtk::ListItem>())
            else {
                return;
            };
            let Some(object) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            match &*object.borrow::<Entry>() {
                Entry::Header => view.bind_header(item),
                Entry::Song(song) => view.bind_song(item, song),
            }
        });
        let view = self.downgrade();
        factory.connect_unbind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            match item.child().and_downcast::<SongRow>() {
                // Off screen: no song, so it is not marked as the playing one.
                Some(row) => row.clear(),
                // The header leaves, free for the item that shows it next.
                // Unless that item has it already: when the model is replaced
                // the new first item may be filled before this one is let go,
                // and the header is then no longer this item's to give up.
                None => {
                    let Some(view) = view.upgrade() else {
                        return;
                    };
                    let imp = view.imp();
                    let holds = imp
                        .header_item
                        .borrow()
                        .as_ref()
                        .and_then(|held| held.upgrade())
                        .is_some_and(|held| held == *item);
                    if holds {
                        item.set_child(gtk::Widget::NONE);
                        imp.header_item.replace(None);
                    }
                }
            }
        });

        // A click on a song plays it, then the songs listed after it; on the
        // track in progress it pauses or resumes.
        let view = self.downgrade();
        imp.songs_list.connect_activate(move |_, position| {
            let Some(view) = view.upgrade() else {
                return;
            };
            let imp = view.imp();
            let songs = imp.songs.borrow().clone();
            let client = imp.client.borrow().clone();
            // The header is the first item: the songs start after it.
            let index = (position as usize).checked_sub(1);
            if let (Some(songs), Some(client), Some(index)) = (songs, client, index) {
                let player = player::get();
                let clicked = songs.get(index).map(|song| song.id.as_str());
                if clicked.is_some_and(|id| player.toggle_if_current(player::Clicked::Song(id))) {
                    return;
                }
                player.play(client, songs, index, player::QueueOf::Songs);
            }
        });

        // A click (or Enter) on an album card opens the album.
        let view = self.downgrade();
        imp.albums_box.connect_child_activated(move |_, child| {
            let Some(view) = view.upgrade() else {
                return;
            };
            let album = view
                .imp()
                .shown
                .borrow()
                .as_ref()
                .and_then(|(albums, _)| albums.get(child.index() as usize).cloned());
            let main = child
                .ancestor(MainView::static_type())
                .and_downcast::<MainView>();
            if let (Some(album), Some(main)) = (album, main) {
                main.open_album_from(child, &album.id, (*album).clone());
            }
        });

        imp.songs_list
            .set_model(Some(&gtk::NoSelection::new(Some(entries.clone()))));
        imp.songs_list.set_factory(Some(&factory));
        let _ = imp.entries.set(entries);
    }

    /// The first row: the album cards and the "Songs" heading.
    fn bind_header(&self, item: &gtk::ListItem) {
        let imp = self.imp();
        let header = imp.header.get();
        let holder = imp.header_item.take().and_then(|held| held.upgrade());
        if let Some(holder) = holder.filter(|holder| holder != item) {
            holder.set_child(gtk::Widget::NONE);
        }
        item.set_activatable(false);
        item.set_selectable(false);
        // The cards inside take the focus, not the row around them.
        item.set_focusable(false);
        // The item may have held a song before.
        item.set_accessible_label("");
        item.set_child(Some(&header));
        imp.header_item.replace(Some(item.downgrade()));
    }

    /// A song row: made once for the list item, then filled for each song
    /// that comes to it.
    fn bind_song(&self, item: &gtk::ListItem, song: &subsonic::Song) {
        let imp = self.imp();
        let row = match item.child().and_downcast::<SongRow>() {
            Some(row) => row,
            None => {
                let row = SongRow::new();
                let view = self.downgrade();
                let row_weak = row.downgrade();
                // The menu asks for the song only when it is used: the row
                // shows another one by then.
                let menu = crate::song_menu::menu_button(
                    &row,
                    move || {
                        let song = row_weak.upgrade()?.song()?;
                        let client = view.upgrade()?.imp().client.borrow().clone()?;
                        Some((song, client))
                    },
                    true,
                );
                row.add_suffix(&menu);
                imp.rows.borrow_mut().push(row.downgrade());
                item.set_child(Some(&row));
                row
            }
        };
        item.set_activatable(true);
        item.set_selectable(false);
        item.set_focusable(true);
        item.set_accessible_label(&crate::song_row::accessible_label(song));

        let subtitle = [song.artist.as_deref(), song.album.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        row.set_song(song, &subtitle);
        let note = imp.note.borrow().as_ref().map(|note| note(song));
        row.set_note(note.as_deref().unwrap_or(""));

        // The songs look like one card: the first and the last row round
        // its corners (style.css). The classes go on the song row itself: the
        // list's own row widget must not be touched while it is being filled
        // (GTK has not taken hold of it yet).
        let last = imp.entries.get().map_or(0, |entries| entries.n_items());
        for (class, on) in [
            ("first", item.position() == 1),
            ("last", item.position() + 1 == last),
        ] {
            if on {
                row.add_css_class(class);
            } else {
                row.remove_css_class(class);
            }
        }
        self.mark_row(&row);
    }

    /// Whether the "Albums" and "Songs" headings say how many there are
    /// (the Favorites page).
    pub(crate) fn set_show_counts(&self, show: bool) {
        let imp = self.imp();
        imp.show_counts.set(show);
        imp.albums_count.set_visible(show);
        imp.songs_count.set_visible(show);
    }

    /// Puts `widget` above everything else in the view (a page header); it
    /// scrolls away with the list. None takes it out again.
    pub(crate) fn set_header_top(&self, widget: Option<&impl IsA<gtk::Widget>>) {
        let top = &self.imp().header_top;
        top.set_child(widget);
        top.set_visible(widget.is_some());
    }

    /// Puts `widget` at the end of the "Songs" heading (a sort switch, say).
    /// None takes it out again.
    pub(crate) fn set_songs_end(&self, widget: Option<&impl IsA<gtk::Widget>>) {
        let end = &self.imp().songs_end;
        end.set_child(widget);
        end.set_visible(widget.is_some());
    }

    /// What to say about each song on its row, before its length; None for
    /// nothing. The rows on screen follow at once.
    pub(crate) fn set_note(&self, note: Option<impl Fn(&subsonic::Song) -> String + 'static>) {
        let imp = self.imp();
        imp.note.replace(note.map(|note| Rc::new(note) as Note));
        let note = imp.note.borrow().clone();
        for row in imp.rows.borrow().iter().filter_map(|row| row.upgrade()) {
            let text = match (&note, row.song()) {
                (Some(note), Some(song)) => note(&song),
                _ => String::new(),
            };
            row.set_note(&text);
        }
    }

    /// A narrow view (the Favorites page below its breakpoint): 12 px around
    /// the list instead of 24 (style.css), so two album cards fit side by
    /// side as in the album grid, and the widget at the end of the "Songs"
    /// heading goes under it, as wide as the list.
    pub(crate) fn set_narrow(&self, narrow: bool) {
        let imp = self.imp();
        if imp.narrow.replace(narrow) == narrow {
            return;
        }
        if narrow {
            self.add_css_class("narrow");
        } else {
            self.remove_css_class("narrow");
        }
        let end = imp.songs_end.get();
        if narrow {
            imp.songs_row.remove(&end);
            end.set_halign(gtk::Align::Fill);
            imp.songs_title.append(&end);
        } else {
            imp.songs_title.remove(&end);
            end.set_halign(gtk::Align::End);
            imp.songs_row.append(&end);
        }
    }

    /// What the empty page says (search and favourites differ).
    pub(crate) fn set_empty(&self, icon: &str, title: &str, description: &str) {
        let page = &self.imp().empty_page;
        page.set_icon_name(Some(icon));
        page.set_title(title);
        page.set_description(Some(description));
    }

    /// Shows albums and songs. A click on an album opens it; a click on a song
    /// plays it, then the songs listed after it. A view that is not on screen
    /// (Favorites at startup, say) only remembers them, and builds its cards
    /// and rows when it is opened: many never open it, and it is asked to show
    /// the same thing more than once while the library syncs.
    pub(crate) fn show(
        &self,
        albums: &[Arc<subsonic::Album>],
        songs: Vec<subsonic::Song>,
        client: Option<subsonic::Client>,
    ) {
        let content = (albums.to_vec(), songs, client);
        if self.is_mapped() {
            self.fill(content);
        } else {
            self.imp().pending.replace(Some(content));
        }
    }

    fn fill(&self, (albums, songs, client): Content) {
        let imp = self.imp();

        // The same as on screen: nothing to build.
        let account = client.as_ref().map(|client| client.server_key());
        let same = imp
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|(shown_albums, shown_account)| {
                *shown_albums == albums
                    && *shown_account == account
                    && imp
                        .songs
                        .borrow()
                        .as_deref()
                        .is_some_and(|shown| *shown == *songs)
            });
        if same {
            // The account connected again: its songs play with the new client.
            imp.client.replace(client);
            return;
        }
        imp.shown.replace(Some((albums.clone(), account)));

        // A card straight in its cell, as in the album grid: the cell is
        // what lights up and takes the focus; a click or Enter on it opens
        // the album (setup_list).
        imp.albums_box.remove_all();
        for album in &albums {
            let card = AlbumCard::new();
            card.set_album(album, client.clone());
            imp.albums_box.append(&card);
        }
        imp.albums_section.set_visible(!albums.is_empty());
        imp.songs_title.set_visible(!songs.is_empty());
        imp.albums_count.set_label(&albums.len().to_string());
        imp.songs_count.set_label(&songs.len().to_string());

        let nothing = albums.is_empty() && songs.is_empty();
        let mut objects = Vec::with_capacity(songs.len() + 1);
        if !nothing {
            objects.push(glib::BoxedAnyObject::new(Entry::Header));
        }
        objects.extend(
            songs
                .iter()
                .cloned()
                .map(|song| glib::BoxedAnyObject::new(Entry::Song(Box::new(song)))),
        );
        imp.songs.replace(Some(songs.into()));
        imp.client.replace(client);
        if let Some(entries) = imp.entries.get() {
            // One change notification for the whole list, not one per song.
            entries.splice(0, entries.n_items(), &objects);
        }
        // The rows of the old model are gone: forget them here, not only when
        // the player next changes.
        imp.rows.borrow_mut().retain(|row| row.upgrade().is_some());

        imp.stack
            .set_visible_child_name(if nothing { "empty" } else { "results" });
    }

    /// Bars and the highlight on the song that is playing (resting bars while
    /// paused), as on album pages. Only rows that exist need it; a row that
    /// comes into view is marked as it is filled.
    fn mark_playing(&self) {
        let mut rows = self.imp().rows.borrow_mut();
        rows.retain(|row| row.upgrade().is_some());
        for row in rows.iter().filter_map(|row| row.upgrade()) {
            self.mark_row(&row);
        }
    }

    fn mark_row(&self, row: &SongRow) {
        let player = player::get();
        let current = player.current().filter(|_| player.is_active());
        let playing = match (current, row.song()) {
            (Some(current), Some(song)) => current.id == song.id,
            _ => false,
        };
        row.set_playing(playing, !player.is_playing());
    }
}
