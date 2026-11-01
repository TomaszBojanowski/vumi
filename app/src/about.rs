//! "About" sections (album description, artist biography): HTML from the
//! server turned into plain text, long texts collapsed behind "Show More".

use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Last.fm texts are HTML ending with a "Read more on Last.fm" link: keep the text only.
pub(crate) fn plain_text(html: &str) -> String {
    let text = match html.rfind("<a ") {
        Some(i) => &html[..i],
        None => html,
    };
    let mut out = String::new();
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .trim()
        .to_string()
}

/// Shows the section with `text`. "Show More" appears only while the text
/// needs more than four lines at the current width, also after a resize.
pub(crate) fn fill(section: &gtk::Box, text_label: &gtk::Label, more: &gtk::Button, text: &str) {
    text_label.set_label(text);
    more.set_visible(false);
    section.set_visible(true);

    // At most one check waits for the next frame, however fast the window is resized.
    let pending = Rc::new(Cell::new(false));
    schedule_check(text_label, more, &pending);

    // The scrolled page reports every change of its width: check again then.
    if let Some(scrolled) = text_label
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_downcast::<gtk::ScrolledWindow>()
    {
        let label = text_label.clone();
        let button = more.clone();
        scrolled
            .hadjustment()
            .connect_page_size_notify(move |_| schedule_check(&label, &button, &pending));
    }

    let label = text_label.clone();
    more.connect_clicked(move |button| {
        let expanded = label.ellipsize() == gtk::pango::EllipsizeMode::None;
        if expanded {
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_lines(4);
            button.set_label(&gettext("Show More"));
        } else {
            label.set_ellipsize(gtk::pango::EllipsizeMode::None);
            label.set_lines(-1);
            button.set_label(&gettext("Show Less"));
        }
    });
}

/// On the next frame, when the label has its real width, counts how many
/// lines the whole text needs there and shows the button only if it is more than four.
fn schedule_check(label: &gtk::Label, button: &gtk::Button, pending: &Rc<Cell<bool>>) {
    if pending.replace(true) {
        return;
    }
    let button = button.clone();
    let pending = pending.clone();
    label.add_tick_callback(move |label, _| {
        if label.width() == 0 {
            return glib::ControlFlow::Continue;
        }
        let layout = label.create_pango_layout(Some(&label.text()));
        layout.set_width(label.width() * gtk::pango::SCALE);
        layout.set_wrap(gtk::pango::WrapMode::Word);
        button.set_visible(layout.line_count() > 4);
        pending.set(false);
        glib::ControlFlow::Break
    });
}
