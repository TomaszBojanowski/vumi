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
    decode_entities(&out).trim().to_string()
}

/// "&amp;", "&lt;", "&#233;", "&#xF6;" and the like, each read once ("&amp;lt;"
/// is the text "&lt;"); an unknown one stays as it is.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        // The shortest entity that ends within a few characters.
        let decoded = rest[1..].find(';').filter(|&end| end <= 8).and_then(|end| {
            let name = &rest[1..=end];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => {
                    let number = name.strip_prefix('#')?;
                    let code = match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => number.parse().ok()?,
                    };
                    char::from_u32(code)
                }
            }?;
            Some((c, end + 2))
        });
        match decoded {
            Some((c, length)) => {
                out.push(c);
                rest = &rest[length..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_and_the_last_fm_link_go() {
        let html =
            "A record <b>about</b> the sea.\n<a href=\"https://last.fm\">Read more on Last.fm</a>";
        assert_eq!(plain_text(html), "A record about the sea.");
    }

    #[test]
    fn each_entity_is_decoded_once() {
        // "&amp;lt;" is the text "&lt;", not "<".
        assert_eq!(plain_text("Tom &amp;lt; Jerry"), "Tom &lt; Jerry");
        assert_eq!(plain_text("Simon &amp; Garfunkel"), "Simon & Garfunkel");
        assert_eq!(
            plain_text("&quot;Hi&quot; &#39;there&#39; &lt;3 &gt;"),
            "\"Hi\" 'there' <3 >"
        );
    }

    #[test]
    fn numeric_entities_are_decoded_and_unknown_ones_kept() {
        assert_eq!(plain_text("Caf&#233; &#x2014; Bj&#xF6;rk"), "Café — Björk");
        assert_eq!(plain_text("a &unknown; b & c"), "a &unknown; b & c");
    }
}
