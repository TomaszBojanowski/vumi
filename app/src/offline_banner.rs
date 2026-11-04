//! The offline banner: one under the header bar of every page (the sections,
//! album, artist and playlist pages, search results), all showing the same
//! connection state, with Retry. The main view sets the state (show()); a page
//! only adds its banner (attach()).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

struct State {
    revealed: bool,
    title: String,
    button: Option<String>,
}

thread_local! {
    static BANNERS: RefCell<Vec<glib::WeakRef<adw::Banner>>> = const { RefCell::new(Vec::new()) };
    static STATE: RefCell<State> = const {
        RefCell::new(State { revealed: false, title: String::new(), button: None })
    };
    static ON_RETRY: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

fn apply(banner: &adw::Banner, state: &State) {
    banner.set_title(&state.title);
    // No label, no button (while an attempt the user asked for is on its way).
    banner.set_button_label(state.button.as_deref());
    banner.set_revealed(state.revealed);
}

/// The page's own ToolbarView: its child, or inside a bin or an overlay or
/// two (a breakpoint bin, a cover-colour layer). The shallowest one, so never
/// one inside its content.
fn toolbar_of(page: &adw::NavigationPage) -> Option<adw::ToolbarView> {
    let mut level: Vec<gtk::Widget> = page.child().into_iter().collect();
    for _ in 0..4 {
        let mut next = Vec::new();
        for widget in &level {
            if let Ok(toolbar) = widget.clone().downcast::<adw::ToolbarView>() {
                return Some(toolbar);
            }
            let mut child = widget.first_child();
            while let Some(c) = child {
                child = c.next_sibling();
                next.push(c);
            }
        }
        level = next;
    }
    None
}

/// Puts a banner under the header bar of `page`.
pub(crate) fn attach(page: &impl IsA<adw::NavigationPage>) {
    let Some(toolbar) = toolbar_of(page.as_ref()) else {
        return;
    };
    let banner = adw::Banner::builder()
        // The title can be a message from the server: plain text.
        .use_markup(false)
        .build();
    STATE.with(|state| apply(&banner, &state.borrow()));
    banner.connect_button_clicked(|_| {
        let retry = ON_RETRY.with(|r| r.borrow().clone());
        if let Some(retry) = retry {
            retry();
        }
    });
    toolbar.add_top_bar(&banner);
    BANNERS.with(|banners| {
        let mut banners = banners.borrow_mut();
        banners.retain(|b| b.upgrade().is_some());
        banners.push(banner.downgrade());
    });
}

/// What every banner shows: hidden, or `title` with a Retry button labelled
/// `button` (none while an attempt is on its way).
pub(crate) fn show(revealed: bool, title: &str, button: Option<&str>) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.revealed = revealed;
        // Hidden banners keep their text, so it does not change while they slide away.
        if revealed {
            state.title = title.to_string();
            state.button = button.map(str::to_string);
        }
        BANNERS.with(|banners| {
            for banner in banners.borrow().iter().filter_map(|b| b.upgrade()) {
                apply(&banner, &state);
            }
        });
    });
}

/// What Retry does (the main view: an attempt to connect now).
pub(crate) fn set_retry(retry: impl Fn() + 'static) {
    ON_RETRY.with(|r| r.replace(Some(Rc::new(retry))));
}
