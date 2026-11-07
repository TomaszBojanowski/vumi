//! Albums as cards and songs as a list, with the playing song marked: used by
//! the search results, the Favorites page and playlist pages.
//!
//! The songs are a list view over a model, so rows exist only for the songs
//! on screen (and a few hundred around them) and are reused while scrolling.
//! The album cards and the "Songs" heading are the first item of that list:
//! they scroll away with the songs, as one page.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use crate::album_card::AlbumCard;
use crate::main_view::MainView;
use crate::player;
use crate::playlist_edit::Edit;
use crate::song_row::SongRow;

/// What a playlist page lets its list do with the songs (playlist_page.rs):
/// Remove, Move Up and Move Down in the ⋮ menu, and dragging by a handle.
/// Indexes count the songs from 0.
pub(crate) struct Editing {
    pub(crate) remove: Box<dyn Fn(usize)>,
    pub(crate) move_to: Box<dyn Fn(usize, usize)>,
}

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
        /// A playlist whose songs can be changed (set_editing).
        pub(crate) editing: RefCell<Option<Rc<super::Editing>>>,
        /// Whether changes can be made now (not offline, say).
        pub edits_on: Cell<bool>,
        /// The songs as they were when a drag began: a change meanwhile
        /// makes the index it carries point at another song.
        pub dragged_from: RefCell<Option<Rc<[subsonic::Song]>>>,
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
            let (Some(songs), Some(index)) = (songs, index) else {
                return;
            };
            // Found offline (search and Favorites read the local database).
            let Some(client) = client else {
                crate::cannot_play(&view);
                return;
            };
            {
                let player = player::get();
                // Started from this list, the queue is this list: a song in
                // it twice is then told apart by its place.
                let clicked = player::clicked(&player.queue(), &songs, index);
                if clicked.is_some_and(|clicked| player.toggle_if_current(clicked)) {
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
        // Connected after the list's own handler (through the selection
        // model): by then the items know their new places.
        let view = self.downgrade();
        entries.connect_items_changed(move |_, _, _, _| {
            if let Some(view) = view.upgrade() {
                view.update_ends();
            }
        });
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
                let first_view = self.downgrade();
                let first_row = row.downgrade();
                // The menu asks for the song only when it is used: the row
                // shows another one by then.
                let menu = crate::song_menu::menu_button_with(
                    &row,
                    move || {
                        let song = row_weak.upgrade()?.song()?;
                        let client = view.upgrade()?.imp().client.borrow().clone();
                        Some((song, client))
                    },
                    true,
                    move || {
                        first_view
                            .upgrade()?
                            .playlist_section(&first_row.upgrade()?)
                    },
                );
                row.add_suffix(&menu);
                self.add_drop_target(&row);
                imp.rows.borrow_mut().push(row.downgrade());
                item.set_child(Some(&row));
                row
            }
        };
        if let Some(handle) = row.set_handle(imp.editing.borrow().is_some(), imp.edits_on.get()) {
            self.add_drag_source(&row, &handle);
        }
        item.set_activatable(true);
        item.set_selectable(false);
        item.set_focusable(true);
        // A screen reader hears the item's name, which the row makes.
        row.set_list_item(item);

        let subtitle = [song.artist.as_deref(), song.album.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        row.set_song(song, &subtitle);
        let note = imp.note.borrow().as_ref().map(|note| note(song));
        row.set_note(note.as_deref().unwrap_or(""));

        self.set_ends(&row);
        self.mark_row(&row);
    }

    /// The songs look like one card: the first and the last row round its
    /// corners (style.css), by the place the row's song is at now. The
    /// classes go on the song row itself: the list's own row widget must not
    /// be touched while it is being filled (GTK has not taken hold of it yet).
    fn set_ends(&self, row: &SongRow) {
        let count = self
            .imp()
            .entries
            .get()
            .map_or(0, |entries| entries.n_items());
        let position = row
            .imp()
            .item
            .borrow()
            .as_ref()
            .and_then(|item| item.upgrade())
            .filter(|item| item.item().is_some())
            .map(|item| item.position());
        let (first, last) = match position {
            // The header is the first item: the songs start after it.
            Some(position) if position >= 1 => (position == 1, position + 1 == count),
            _ => (false, false),
        };
        for (class, on) in [("first", first), ("last", last)] {
            if on {
                row.add_css_class(class);
            } else {
                row.remove_css_class(class);
            }
        }
    }

    /// After every change of the list (a song moved, removed, put back, the
    /// list filled again): each row that exists gets the ends of its place
    /// now. GTK fills again only rows whose item changed, so a row whose song
    /// merely moved would keep the corners of where it was.
    fn update_ends(&self) {
        let rows: Vec<SongRow> = self
            .imp()
            .rows
            .borrow()
            .iter()
            .filter_map(|row| row.upgrade())
            .collect();
        for row in rows {
            self.set_ends(&row);
        }
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

    /// Lets the songs be changed (a playlist page whose songs can be), or
    /// not (None). The rows on screen follow at once.
    pub(crate) fn set_editing(&self, editing: Option<Rc<Editing>>) {
        let imp = self.imp();
        imp.editing.replace(editing);
        self.update_handles();
    }

    /// Whether changes can be made now: off while offline (the handles and
    /// the menu items are dimmed).
    pub(crate) fn set_edits_on(&self, on: bool) {
        self.imp().edits_on.set(on);
        self.update_handles();
    }

    fn update_handles(&self) {
        let imp = self.imp();
        let shown = imp.editing.borrow().is_some();
        let on = imp.edits_on.get();
        let rows: Vec<SongRow> = imp
            .rows
            .borrow()
            .iter()
            .filter_map(|row| row.upgrade())
            .collect();
        for row in rows {
            if let Some(handle) = row.set_handle(shown, on) {
                self.add_drag_source(&row, &handle);
            }
        }
    }

    /// The index of the song a row shows now (the header is the first item).
    fn index_of(row: &SongRow) -> Option<usize> {
        let item = row.imp().item.borrow().as_ref()?.upgrade()?;
        item.item()?;
        (item.position() as usize).checked_sub(1)
    }

    /// The ⋮ menu's first section on a playlist whose songs can be changed:
    /// Move Up (not on the first song), Move Down (not on the last) and
    /// Remove from Playlist. Dimmed while changes cannot be made.
    fn playlist_section(&self, row: &SongRow) -> Option<gio::Menu> {
        let imp = self.imp();
        imp.editing.borrow().as_ref()?;
        let index = Self::index_of(row)?;
        let count = imp.songs.borrow().as_ref().map_or(0, |songs| songs.len());
        let actions = row.list_actions().unwrap_or_else(|| {
            let actions = gio::SimpleActionGroup::new();
            for name in ["move-up", "move-down", "remove"] {
                let action = gio::SimpleAction::new(name, None);
                let view = self.downgrade();
                let row = row.downgrade();
                action.connect_activate(move |_, _| {
                    let (Some(view), Some(row)) = (view.upgrade(), row.upgrade()) else {
                        return;
                    };
                    let editing = view.imp().editing.borrow().clone();
                    let (Some(editing), Some(index)) = (editing, Self::index_of(&row)) else {
                        return;
                    };
                    match name {
                        "move-up" if index > 0 => (editing.move_to)(index, index - 1),
                        "move-down" => (editing.move_to)(index, index + 1),
                        "remove" => (editing.remove)(index),
                        _ => {}
                    }
                });
                actions.add_action(&action);
            }
            row.insert_action_group("playlist", Some(&actions));
            row.set_list_actions(&actions);
            actions
        });
        for name in ["move-up", "move-down", "remove"] {
            if let Some(action) = actions
                .lookup_action(name)
                .and_downcast::<gio::SimpleAction>()
            {
                action.set_enabled(imp.edits_on.get());
            }
        }
        let section = gio::Menu::new();
        if index > 0 {
            section.append(Some(&gettext("Move Up")), Some("playlist.move-up"));
        }
        if index + 1 < count {
            section.append(Some(&gettext("Move Down")), Some("playlist.move-down"));
        }
        section.append(
            Some(&gettext("Remove from Playlist")),
            Some("playlist.remove"),
        );
        Some(section)
    }

    /// Dragging a row by its handle: it carries its index.
    fn add_drag_source(&self, row: &SongRow, handle: &gtk::Image) {
        let source = gtk::DragSource::builder()
            .actions(gtk::gdk::DragAction::MOVE)
            .build();
        let view = self.downgrade();
        let row = row.downgrade();
        source.connect_prepare(move |source, _, _| {
            let (view, row) = (view.upgrade()?, row.upgrade()?);
            let imp = view.imp();
            if imp.editing.borrow().is_none() || !imp.edits_on.get() {
                return None;
            }
            let index = Self::index_of(&row)?;
            imp.dragged_from.replace(imp.songs.borrow().clone());
            row.set_drag_icon(source);
            Some(gtk::gdk::ContentProvider::for_value(
                &(index as u32).to_value(),
            ))
        });
        // Every drag ends here: dropped, cancelled with Esc, dropped outside
        // the list. What was kept for the drop goes.
        let view = self.downgrade();
        source.connect_drag_end(move |_, _, _| {
            if let Some(view) = view.upgrade() {
                view.imp().dragged_from.replace(None);
            }
        });
        handle.add_controller(source);
    }

    /// A song dropped on a row takes that row's place, as in the queue.
    fn add_drop_target(&self, row: &SongRow) {
        let target = gtk::DropTarget::new(u32::static_type(), gtk::gdk::DragAction::MOVE);
        let view = self.downgrade();
        let row_weak = row.downgrade();
        target.connect_drop(move |_, value, _, _| {
            let (Some(view), Some(row)) = (view.upgrade(), row_weak.upgrade()) else {
                return false;
            };
            let imp = view.imp();
            let dragged_from = imp.dragged_from.take();
            let editing = imp.editing.borrow().clone();
            // The list changed under the drag: the index is of the old one.
            let same = match (&dragged_from, imp.songs.borrow().as_ref()) {
                (Some(from), Some(now)) => Rc::ptr_eq(from, now),
                _ => false,
            };
            let (Ok(from), Some(target), Some(editing)) =
                (value.get::<u32>(), Self::index_of(&row), editing)
            else {
                return false;
            };
            if !same || !imp.edits_on.get() {
                return false;
            }
            match crate::playlist_edit::dropped::<()>(from as usize, target) {
                Some(Edit::Move { from, to }) => {
                    (editing.move_to)(from, to);
                    true
                }
                _ => false,
            }
        });
        row.add_controller(target);
    }

    /// Changes the songs listed as `edit` does, row by row: the list keeps
    /// its place and the rows around stay. The songs played on a click
    /// follow.
    pub(crate) fn apply_edit(&self, edit: &Edit<subsonic::Song>) {
        let imp = self.imp();
        let (Some(entries), Some(songs)) = (imp.entries.get(), imp.songs.borrow().clone()) else {
            return;
        };
        let Some(after) = edit.apply(&songs) else {
            return;
        };
        let new =
            |song: &subsonic::Song| glib::BoxedAnyObject::new(Entry::Song(Box::new(song.clone())));
        match edit {
            Edit::Remove(index) => entries.remove(*index as u32 + 1),
            Edit::Move { from, to } => {
                entries.remove(*from as u32 + 1);
                entries.insert(*to as u32 + 1, &new(&after[*to]));
            }
            Edit::Restore { index, song } => entries.insert(*index as u32 + 1, &new(song)),
        }
        imp.songs.replace(Some(after.as_slice().into()));
        imp.songs_count.set_label(&after.len().to_string());
        if after.is_empty() {
            self.fill((Vec::new(), Vec::new(), imp.client.borrow().clone()));
        }
    }

    /// The songs listed, in order.
    pub(crate) fn songs(&self) -> Option<Rc<[subsonic::Song]>> {
        self.imp().songs.borrow().clone()
    }

    /// Puts the keyboard focus on the song at `index` (after a move).
    pub(crate) fn focus_song(&self, index: usize) {
        self.imp()
            .songs_list
            .scroll_to(index as u32 + 1, gtk::ListScrollFlags::FOCUS, None);
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
