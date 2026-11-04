//! Search results: the shared results view (results_view.rs) on a page of its
//! own, shown on top of the library; the sidebar search field updates it.

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use std::sync::Arc;

use crate::results_view::ResultsView;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/search_page.blp")]
    pub struct SearchPage {
        #[template_child]
        pub results: gtk::TemplateChild<ResultsView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SearchPage {
        // Must match "template $VumiSearchPage" in the Blueprint file.
        const NAME: &'static str = "VumiSearchPage";
        type Type = super::SearchPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            // The results view is used in this template, so its type must exist.
            ResultsView::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SearchPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.results.set_empty(
                "system-search-symbolic",
                &gettext("No Results"),
                &gettext("Try other words"),
            );
        }
    }
    impl WidgetImpl for SearchPage {}
    impl NavigationPageImpl for SearchPage {}
}

glib::wrapper! {
    pub struct SearchPage(ObjectSubclass<imp::SearchPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SearchPage {
    pub(crate) fn new() -> Self {
        let page: Self = glib::Object::new();
        crate::offline_banner::attach(&page);
        page
    }

    /// Shows what a search found.
    pub(crate) fn show(
        &self,
        albums: &[Arc<subsonic::Album>],
        songs: Vec<subsonic::Song>,
        client: Option<subsonic::Client>,
    ) {
        self.imp().results.show(albums, songs, client);
    }
}
