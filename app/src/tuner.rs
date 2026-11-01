//! The Tuner card of the Start page (tuner.blp): the year chosen on the scale
//! of years, how many albums the library has from it, and ‹ › for the year
//! with albums before and after it.

use adw::subclass::prelude::*;
use gettextrs::ngettext;
use gtk::glib;
use gtk::prelude::*;
use std::collections::BTreeMap;

use crate::year_scale::YearScale;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/tuner.blp")]
    pub struct Tuner {
        #[template_child]
        pub year_label: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub count_label: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub prev_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub next_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub scale: gtk::TemplateChild<YearScale>,
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

    /// The year, its count and the ‹ › that lead somewhere.
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
    }
}
