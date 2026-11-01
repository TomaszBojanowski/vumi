//! The Tuner card of the Start page (tuner.blp): the year chosen on the scale
//! of years, how many albums the library has from it, ‹ › for the year with
//! albums before and after it, "Play This Year", and the row of the year's
//! albums. The card takes its colour from the first album's cover.

use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::main_view::artist_sort_key;
use crate::shelf::LazyRow;
use crate::year_scale::YearScale;

/// How long the year has to stay before the row of its albums and the
/// card's colour follow it: dragging the needle over twenty years, holding
/// an arrow key or clicking ‹ › again and again builds the row once.
const ROW_DELAY: Duration = Duration::from_millis(200);

/// How many songs "Play This Year" plays.
const YEAR_SONGS: usize = 50;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/tuner.blp")]
    pub struct Tuner {
        #[template_child]
        pub card: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub header: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub year_label: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub count_label: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub buttons: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub prev_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub next_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub play_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub scale: gtk::TemplateChild<YearScale>,
        #[template_child]
        pub rows: gtk::TemplateChild<gtk::Stack>,
        /// The two rows of albums the stack crossfades between, named "a" and
        /// "b" in it.
        pub(crate) row_views: OnceCell<[LazyRow; 2]>,
        /// The library and the client (StartPage::update_tuner).
        pub albums: RefCell<Vec<Arc<subsonic::Album>>>,
        pub client: RefCell<Option<subsonic::Client>>,
        /// The year whose albums the row shows.
        pub shown_year: Cell<Option<i32>>,
        /// The row waits for the year to stay (ROW_DELAY).
        pub row_timeout: RefCell<Option<glib::SourceId>>,
        /// The card's colour: its rule, the cover it comes from, the colour
        /// (None: plain), and the colour the rule shows now (None: no rule
        /// written yet).
        pub tint: OnceCell<crate::tint::Dynamic>,
        pub tint_cover: RefCell<Option<String>>,
        pub tint_color: Cell<Option<(u8, u8, u8)>>,
        pub tint_shown: Cell<Option<Option<(u8, u8, u8)>>>,
        /// The buttons under the year (a narrow page) rather than beside it.
        pub compact: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Tuner {
        // Must match "template $VumiTuner" in the Blueprint file.
        const NAME: &'static str = "VumiTuner";
        type Type = super::Tuner;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            YearScale::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Tuner {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }

        fn dispose(&self) {
            if let Some(source) = self.row_timeout.take() {
                source.remove();
            }
            // The card's colour rule goes with the card.
            if let Some(tint) = self.tint.get() {
                tint.clear();
            }
        }
    }
    impl WidgetImpl for Tuner {}
    impl BinImpl for Tuner {}
}

glib::wrapper! {
    pub struct Tuner(ObjectSubclass<imp::Tuner>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Tuner {
    fn setup(&self) {
        let imp = self.imp();
        imp.scale.connect_changed(glib::clone!(
            #[weak(rename_to = tuner)]
            self,
            move |year| tuner.show_year(year)
        ));
        for (button, forward) in [(&imp.prev_button, false), (&imp.next_button, true)] {
            button.connect_clicked(glib::clone!(
                #[weak(rename_to = tuner)]
                self,
                move |_| tuner.imp().scale.step(forward)
            ));
        }
        imp.play_button.connect_clicked(glib::clone!(
            #[weak(rename_to = tuner)]
            self,
            move |_| tuner.play_year()
        ));
        imp.play_button.set_sensitive(false);

        let rows = [LazyRow::new(), LazyRow::new()];
        imp.rows.add_named(rows[0].widget(), Some("a"));
        imp.rows.add_named(rows[1].widget(), Some("b"));
        let _ = imp.row_views.set(rows);
        // The row faded out is emptied, so its cards do not stay alive; the
        // card's colour waited for the fade to end.
        imp.rows.connect_transition_running_notify(glib::clone!(
            #[weak(rename_to = tuner)]
            self,
            move |stack| {
                if !stack.is_transition_running() {
                    tuner.hidden_row().clear();
                    tuner.apply_tint();
                }
            }
        ));

        let tint = crate::tint::Dynamic::new();
        imp.card.add_css_class(tint.class());
        let _ = imp.tint.set(tint);

        // The orange of the year and the needle is darker in the light style.
        // The style the app shows decides (Preferences can force either); a
        // CSS media query follows the system instead.
        let style = adw::StyleManager::default();
        self.follow_style(style.is_dark());
        style.connect_dark_notify(glib::clone!(
            #[weak(rename_to = tuner)]
            self,
            move |style| tuner.follow_style(style.is_dark())
        ));
    }

    fn follow_style(&self, dark: bool) {
        if dark {
            self.add_css_class("tuner-dark");
        } else {
            self.remove_css_class("tuner-dark");
        }
    }

    /// The year chosen; None until there are albums with a year and a year
    /// has been set.
    pub(crate) fn year(&self) -> Option<i32> {
        self.imp().scale.year()
    }

    /// Puts the scale on `year`, or on the year with albums nearest to it.
    pub(crate) fn set_year(&self, year: i32) {
        self.imp().scale.set_year(year, false);
    }

    /// The albums per year of the library (year_scale::year_counts).
    pub(crate) fn set_counts(&self, counts: BTreeMap<i32, usize>) {
        let scale = &self.imp().scale;
        scale.set_counts(counts);
        // The year may have stayed while its count changed (a sync).
        if let Some(year) = scale.year() {
            self.show_year(year);
        }
    }

    /// The library and the client. The row follows at once if the albums of
    /// the year shown have changed (a sync, another account).
    pub(crate) fn set_library(
        &self,
        albums: Vec<Arc<subsonic::Album>>,
        client: Option<subsonic::Client>,
    ) {
        let imp = self.imp();
        imp.play_button.set_sensitive(client.is_some());
        imp.albums.replace(albums);
        imp.client.replace(client);
        if imp.shown_year.get().is_some() {
            self.show_row(false);
        }
    }

    /// Below the year rather than beside it: the buttons, on a narrow page.
    pub(crate) fn set_compact(&self, compact: bool) {
        let imp = self.imp();
        if imp.compact.replace(compact) == compact {
            return;
        }
        if compact {
            imp.header.set_orientation(gtk::Orientation::Vertical);
            imp.buttons.set_halign(gtk::Align::Start);
        } else {
            imp.header.set_orientation(gtk::Orientation::Horizontal);
            imp.buttons.set_halign(gtk::Align::Fill);
        }
    }

    /// The year, its count and the ‹ › that lead somewhere, at once; the row
    /// and the colour once the year stays.
    fn show_year(&self, year: i32) {
        let imp = self.imp();
        imp.year_label.set_label(&year.to_string());
        let n = imp.scale.count(year) as u32;
        imp.count_label.set_label(
            &ngettext(
                "{n} album from this year in the library",
                "{n} albums from this year in the library",
                n,
            )
            .replace("{n}", &n.to_string()),
        );
        imp.prev_button
            .set_sensitive(imp.scale.neighbour_of(year, false).is_some());
        imp.next_button
            .set_sensitive(imp.scale.neighbour_of(year, true).is_some());
        self.queue_row();
    }

    /// Shows the row of the year once it has stayed for ROW_DELAY; the first
    /// row at once.
    fn queue_row(&self) {
        let imp = self.imp();
        if let Some(source) = imp.row_timeout.take() {
            source.remove();
        }
        if imp.shown_year.get().is_none() {
            self.show_row(false);
            return;
        }
        let tuner = self.downgrade();
        let source = glib::timeout_add_local_once(ROW_DELAY, move || {
            if let Some(tuner) = tuner.upgrade() {
                // This source is ending: forget it rather than remove it.
                tuner.imp().row_timeout.take();
                tuner.show_row(true);
            }
        });
        imp.row_timeout.replace(Some(source));
    }

    /// Which of the two rows shows now: 0 for "a", 1 for "b".
    fn shown_index(&self) -> usize {
        usize::from(self.imp().rows.visible_child_name().as_deref() == Some("b"))
    }

    fn row(&self, index: usize) -> &LazyRow {
        &self
            .imp()
            .row_views
            .get()
            .expect("the rows are made in setup")[index]
    }

    fn hidden_row(&self) -> &LazyRow {
        self.row(1 - self.shown_index())
    }

    /// The albums of the year chosen in the hidden row, then the stack shows
    /// it: crossfading with `animate`, at once otherwise. The same albums
    /// stay as they are; their cards only fetch the covers they lack.
    fn show_row(&self, animate: bool) {
        let imp = self.imp();
        let Some(year) = self.year() else {
            imp.shown_year.set(None);
            self.row(0).clear();
            self.row(1).clear();
            self.show_tint(None);
            return;
        };
        let albums = albums_of_year(&imp.albums.borrow(), year);
        let client = imp.client.borrow().clone();
        let ids: Vec<String> = albums.iter().map(|album| album.id.clone()).collect();
        let shown = self.row(self.shown_index());
        if imp.shown_year.get() == Some(year) && shown.album_ids() == ids {
            if let Some(client) = client {
                shown.retry_covers(client);
            }
            self.show_tint(albums.first());
            return;
        }
        imp.shown_year.set(Some(year));
        let width = f64::from(imp.rows.width());
        let hidden = 1 - self.shown_index();
        self.row(hidden).set_albums(albums.clone(), client, width);
        let transition = if animate && self.is_mapped() {
            gtk::StackTransitionType::Crossfade
        } else {
            gtk::StackTransitionType::None
        };
        imp.rows
            .set_visible_child_full(["a", "b"][hidden], transition);
        if !imp.rows.is_transition_running() {
            self.hidden_row().clear();
        }
        self.show_tint(albums.first());
    }

    /// The card in the colour of `album`'s cover; plain without a cover.
    fn show_tint(&self, album: Option<&Arc<subsonic::Album>>) {
        let imp = self.imp();
        let cover = album.and_then(|album| album.cover_art.clone());
        if *imp.tint_cover.borrow() == cover && (cover.is_none() || imp.tint_color.get().is_some())
        {
            return;
        }
        imp.tint_cover.replace(cover.clone());
        let Some(id) = cover else {
            imp.tint_color.set(None);
            self.apply_tint();
            return;
        };
        let client = imp.client.borrow().clone();
        crate::covers::load_for(self, client, id.clone(), move |tuner, texture| {
            let imp = tuner.imp();
            // Another year has come meanwhile.
            if imp.tint_cover.borrow().as_deref() != Some(id.as_str()) {
                return;
            }
            imp.tint_color
                .set(texture.as_ref().and_then(crate::tint::dominant_color));
            tuner.apply_tint();
        });
    }

    /// Writes the rule of the card's colour, but not while the rows
    /// crossfade: loading the rules again restyles the whole window (some
    /// 50 ms), which would stall the fade. The end of the fade calls it
    /// again, and the colour then changes smoothly (style.css).
    fn apply_tint(&self) {
        let imp = self.imp();
        if imp.rows.is_transition_running() {
            return;
        }
        let color = imp.tint_color.get();
        if imp.tint_shown.replace(Some(color)) == Some(color) {
            return;
        }
        let Some(tint) = imp.tint.get() else {
            return;
        };
        match color {
            Some(color) => tint.set_card(color),
            None => tint.clear(),
        }
    }

    /// "Play This Year": YEAR_SONGS songs at random from the year's albums,
    /// read from the database, as a list of songs (not an album, so the
    /// history of played albums stays as it is).
    fn play_year(&self) {
        let imp = self.imp();
        let (Some(year), Some(client)) = (self.year(), imp.client.borrow().clone()) else {
            return;
        };
        let ids: Vec<String> = albums_of_year(&imp.albums.borrow(), year)
            .iter()
            .map(|album| album.id.clone())
            .collect();
        let server = client.server_key();
        let player = crate::player::get();
        let command = player.command_id();
        // One request at a time: the button waits for the answer.
        imp.play_button.set_sensitive(false);
        let tuner = self.downgrade();
        glib::spawn_future_local(async move {
            let read = crate::runtime()
                .spawn_blocking(move || crate::library::load_songs_of_albums(&server, &ids))
                .await;
            let songs = read
                .map_err(|error| error.to_string())
                .and_then(|songs| songs.map_err(|error| error.to_string()))
                .unwrap_or_else(|error| {
                    eprintln!("Could not read the songs of {year}: {error}");
                    Vec::new()
                });
            let Some(tuner) = tuner.upgrade() else {
                return;
            };
            let imp = tuner.imp();
            imp.play_button.set_sensitive(imp.client.borrow().is_some());
            // Something else was started meanwhile: this answer is stale.
            let player = crate::player::get();
            if !player.is_current(command) {
                return;
            }
            let songs = pick_songs(songs, YEAR_SONGS, |n| fastrand::usize(..n));
            if songs.is_empty() {
                crate::song_menu::toast(&tuner, &gettext("Could not play this year"));
                return;
            }
            player.play(client, songs.into(), 0, crate::player::QueueOf::Songs);
        });
    }
}

/// The albums of `year`, by artist and then by title, ignoring case and
/// accents (as the artists are sorted); albums without an artist come last.
pub(crate) fn albums_of_year(
    albums: &[Arc<subsonic::Album>],
    year: i32,
) -> Vec<Arc<subsonic::Album>> {
    let mut of_year: Vec<Arc<subsonic::Album>> = albums
        .iter()
        .filter(|album| album.year == Some(year))
        .cloned()
        .collect();
    of_year.sort_by_cached_key(|album| {
        let artist = album
            .artist
            .as_deref()
            .filter(|name| !name.trim().is_empty());
        (
            artist.is_none(),
            artist.map(artist_sort_key).unwrap_or_default(),
            artist_sort_key(&album.name),
            album.id.clone(),
        )
    });
    of_year
}

/// Up to `n` songs picked at random from `songs`, each song once (by id).
/// `random(k)` gives a number below `k`.
pub(crate) fn pick_songs(
    songs: Vec<subsonic::Song>,
    n: usize,
    mut random: impl FnMut(usize) -> usize,
) -> Vec<subsonic::Song> {
    let mut seen = HashSet::new();
    let mut songs: Vec<subsonic::Song> = songs
        .into_iter()
        .filter(|song| seen.insert(song.id.clone()))
        .collect();
    let n = n.min(songs.len());
    // The first n places of a shuffle (Fisher-Yates).
    for i in 0..n {
        let j = i + random(songs.len() - i);
        songs.swap(i, j);
    }
    songs.truncate(n);
    songs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(
        id: &str,
        name: &str,
        artist: Option<&str>,
        year: Option<i32>,
    ) -> Arc<subsonic::Album> {
        Arc::new(subsonic::Album {
            id: id.into(),
            name: name.into(),
            artist: artist.map(Into::into),
            year,
            song_count: None,
            created: None,
            artist_id: None,
            starred: None,
            cover_art: None,
        })
    }

    fn song(id: &str) -> subsonic::Song {
        subsonic::Song {
            id: id.into(),
            title: id.into(),
            artist: None,
            track: None,
            disc_number: None,
            album: None,
            album_id: None,
            artist_id: None,
            duration: None,
            suffix: None,
            bit_rate: None,
            cover_art: None,
            bit_depth: None,
            sampling_rate: None,
            starred: None,
            composer: None,
        }
    }

    fn ids(albums: &[Arc<subsonic::Album>]) -> Vec<&str> {
        albums.iter().map(|album| album.id.as_str()).collect()
    }

    #[test]
    fn albums_of_a_year_go_by_artist_then_by_title() {
        let albums = [
            album("help", "Help!", Some("The Beatles"), Some(1965)),
            album(
                "other",
                "Bringing It All Back Home",
                Some("Bob Dylan"),
                Some(1965),
            ),
            album("later", "Revolver", Some("The Beatles"), Some(1966)),
            album("soul", "Rubber Soul", Some("The Beatles"), Some(1965)),
            album("none", "Unknown", None, None),
            album(
                "blonde",
                "Highway 61 Revisited",
                Some("bob dylan"),
                Some(1965),
            ),
        ];
        assert_eq!(
            ids(&albums_of_year(&albums, 1965)),
            ["other", "blonde", "help", "soul"]
        );
        assert_eq!(ids(&albums_of_year(&albums, 1966)), ["later"]);
        assert!(albums_of_year(&albums, 1967).is_empty());
    }

    #[test]
    fn albums_of_a_year_ignore_accents_and_put_albums_without_an_artist_last() {
        let albums = [
            album("p", "A", Some("Pablo"), Some(2000)),
            album("blank", "B", Some(" "), Some(2000)),
            album("none", "A", None, Some(2000)),
            album("o", "A", Some("Óscar"), Some(2000)),
            album("n", "A", Some("Nina"), Some(2000)),
            album("l", "A", Some("Łukasz"), Some(2000)),
        ];
        assert_eq!(
            ids(&albums_of_year(&albums, 2000)),
            ["l", "n", "o", "p", "none", "blank"]
        );
    }

    #[test]
    fn picked_songs_are_at_most_n_different_ones_from_the_list() {
        let songs: Vec<_> = (0..200).map(|n| song(&n.to_string())).collect();
        let all: HashSet<String> = songs.iter().map(|s| s.id.clone()).collect();
        let mut rng = fastrand::Rng::with_seed(7);
        let picked = pick_songs(songs, 50, |k| rng.usize(..k));
        assert_eq!(picked.len(), 50);
        let unique: HashSet<String> = picked.iter().map(|s| s.id.clone()).collect();
        assert_eq!(unique.len(), 50);
        assert!(unique.is_subset(&all));
    }

    #[test]
    fn fewer_songs_than_n_are_all_picked_once() {
        let songs = vec![song("a"), song("b"), song("a"), song("c")];
        let mut rng = fastrand::Rng::with_seed(3);
        let picked = pick_songs(songs, 50, |k| rng.usize(..k));
        let mut picked: Vec<_> = picked.iter().map(|s| s.id.as_str()).collect();
        picked.sort();
        assert_eq!(picked, ["a", "b", "c"]);
        assert!(pick_songs(Vec::new(), 50, |_| 0).is_empty());
    }

    #[test]
    fn the_pick_follows_the_random_numbers() {
        let songs: Vec<_> = ["a", "b", "c", "d"].into_iter().map(song).collect();
        // Always the first one left: the list's own order.
        let picked = pick_songs(songs, 3, |_| 0);
        let picked: Vec<_> = picked.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(picked, ["a", "b", "c"]);
    }
}
