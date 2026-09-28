//! Tints from the dominant colour of an album cover.
//! Every tinted widget gets its own style class and rule, removed again
//! when the widget is destroyed, so pages never share or steal colours.

use gtk::gdk;
use gtk::prelude::*;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeMap;

thread_local! {
    /// One app-wide stylesheet holding the rules of all tinted widgets alive now.
    static PROVIDER: OnceCell<gtk::CssProvider> = const { OnceCell::new() };
    static RULES: RefCell<BTreeMap<u64, String>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

fn next_id() -> u64 {
    NEXT_ID.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    })
}

/// Weighted average of the cover's pixels: colourful, bright pixels count most,
/// greys and black barely count, so the result is a colour rather than mud.
pub(crate) fn dominant_color(texture: &gdk::Texture) -> Option<(u8, u8, u8)> {
    let width = texture.width() as usize;
    let height = texture.height() as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let stride = width * 4;
    let mut pixels = vec![0u8; stride * height];
    // Default memory format: B, G, R, A bytes per pixel.
    texture.download(&mut pixels, stride);

    let (mut r, mut g, mut b, mut total) = (0f32, 0f32, 0f32, 0f32);
    // Every 7th pixel is plenty for an average colour.
    for px in pixels.chunks_exact(4).step_by(7) {
        let (pb, pg, pr) = (px[0] as f32, px[1] as f32, px[2] as f32);
        let max = pr.max(pg).max(pb);
        let min = pr.min(pg).min(pb);
        let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
        let weight = saturation * saturation * (max / 255.0) + 0.001;
        r += pr * weight;
        g += pg * weight;
        b += pb * weight;
        total += weight;
    }
    Some(((r / total) as u8, (g / total) as u8, (b / total) as u8))
}

/// Gives `widget` a background gradient in `color` that belongs to it alone
/// (album pages).
pub(crate) fn apply(widget: &impl IsA<gtk::Widget>, (r, g, b): (u8, u8, u8)) {
    let id = next_id();
    let class = format!("album-tint-{id}");

    RULES.with(|rules| {
        rules.borrow_mut().insert(
            id,
            format!(
                ".{class} {{ background-image: linear-gradient(to bottom, \
                 rgba({r}, {g}, {b}, 0.45), transparent 480px); }}"
            ),
        )
    });
    reload();
    widget.add_css_class(&class);

    // The page is gone: drop its rule too.
    widget.connect_destroy(move |_| {
        RULES.with(|rules| rules.borrow_mut().remove(&id));
        reload();
    });
}

fn reload() {
    let css = RULES.with(|rules| rules.borrow().values().cloned().collect::<Vec<_>>().join("\n"));
    PROVIDER.with(|cell| {
        let provider = cell.get_or_init(|| {
            let provider = gtk::CssProvider::new();
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
            provider
        });
        provider.load_from_string(&css);
    });
}

/// A style class whose colour can change over time (the now-playing pill).
/// Unlike apply(), the same widget keeps one class and its rule is rewritten.
pub(crate) struct Dynamic {
    id: u64,
    class: String,
}

impl Dynamic {
    pub(crate) fn new() -> Self {
        let id = next_id();
        Self {
            id,
            class: format!("dynamic-tint-{id}"),
        }
    }

    pub(crate) fn class(&self) -> &str {
        &self.class
    }

    /// A soft tint: the cover colour mixed into the window background, so the
    /// text keeps its normal colour and stays readable on any cover. Only the
    /// progress line and the play button carry the colour at full strength.
    pub(crate) fn set_solid(&self, (r, g, b): (u8, u8, u8)) {
        let c = &self.class;
        let color = format!("rgb({r}, {g}, {b})");
        // Two classes, so these win over the plain .now-playing-pill rules.
        let rule = [
            format!(".now-playing-pill.{c} {{ background-color: color-mix(in srgb, {color} 30%, var(--window-bg-color)); }}"),
            format!(".now-playing-pill.{c} .pill-progress highlight {{ background-color: color-mix(in srgb, {color} 75%, white); }}"),
            format!(".now-playing-pill.{c} .pill-play {{ background-color: color-mix(in srgb, {color} 45%, transparent); }}"),
        ]
        .join("\n");
        RULES.with(|rules| rules.borrow_mut().insert(self.id, rule));
        reload();
    }

    /// Back to the default colours (no cover).
    pub(crate) fn clear(&self) {
        RULES.with(|rules| rules.borrow_mut().remove(&self.id));
        reload();
    }
}
