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

/// The colour of the cover: the hue most of its colourful pixels share.
/// Pixels are sorted by hue into bins, weighted by saturation and brightness
/// (greys and black barely count); the fullest bin, with its two neighbours,
/// gives the colour, so a purple-and-yellow cover comes out purple rather
/// than brown. A cover with hardly any colour gets its weighted average.
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

    const BINS: usize = 12;
    // Per bin: weighted r, g, b and the weight.
    let mut bins = [(0f32, 0f32, 0f32, 0f32); BINS];
    let (mut avg_r, mut avg_g, mut avg_b, mut avg_w) = (0f32, 0f32, 0f32, 0f32);
    // Every 7th pixel is plenty.
    for px in pixels.as_chunks::<4>().0.iter().step_by(7) {
        let (pb, pg, pr) = (px[0] as f32, px[1] as f32, px[2] as f32);
        let max = pr.max(pg).max(pb);
        let min = pr.min(pg).min(pb);
        let chroma = max - min;
        let saturation = if max > 0.0 { chroma / max } else { 0.0 };
        let weight = saturation * saturation * (max / 255.0);
        avg_r += pr * (weight + 0.001);
        avg_g += pg * (weight + 0.001);
        avg_b += pb * (weight + 0.001);
        avg_w += weight + 0.001;
        if chroma <= 0.0 {
            continue;
        }
        // Hue as 0..6 (the six sectors of the colour wheel).
        let hue = if max == pr {
            ((pg - pb) / chroma).rem_euclid(6.0)
        } else if max == pg {
            (pb - pr) / chroma + 2.0
        } else {
            (pr - pg) / chroma + 4.0
        };
        let bin = ((hue / 6.0 * BINS as f32) as usize).min(BINS - 1);
        bins[bin].0 += pr * weight;
        bins[bin].1 += pg * weight;
        bins[bin].2 += pb * weight;
        bins[bin].3 += weight;
    }

    let prev = |i: usize| (i + BINS - 1) % BINS;
    let next = |i: usize| (i + 1) % BINS;
    let score = |i: usize| bins[prev(i)].3 / 2.0 + bins[i].3 + bins[next(i)].3 / 2.0;
    let best = (0..BINS).max_by(|a, b| score(*a).total_cmp(&score(*b)))?;
    let (mut r, mut g, mut b, mut w) = (0f32, 0f32, 0f32, 0f32);
    for i in [prev(best), best, next(best)] {
        r += bins[i].0;
        g += bins[i].1;
        b += bins[i].2;
        w += bins[i].3;
    }
    // Hardly any colour on the cover: the weighted average of everything.
    if w < avg_w * 0.05 {
        return Some((
            (avg_r / avg_w) as u8,
            (avg_g / avg_w) as u8,
            (avg_b / avg_w) as u8,
        ));
    }
    Some(((r / w) as u8, (g / w) as u8, (b / w) as u8))
}

/// Gives `widget` a background gradient in `color` that belongs to it alone
/// (album pages).
pub(crate) fn apply(widget: &impl IsA<gtk::Widget>, (r, g, b): (u8, u8, u8)) {
    let id = next_id();
    let class = format!("album-tint-{id}");

    RULES.with(|rules| rules.borrow_mut().insert(id, page_rule(&class, (r, g, b))));
    reload();
    widget.add_css_class(&class);

    // The page is gone: drop its rule too.
    widget.connect_destroy(move |_| {
        RULES.with(|rules| rules.borrow_mut().remove(&id));
        reload();
    });
}

/// The colour from the top of a page down, fading into the page's own
/// background (album pages, Favorites).
fn page_rule(class: &str, (r, g, b): (u8, u8, u8)) -> String {
    format!(
        // Light style: a softer tint, or dark covers drown the title.
        ".{class} {{ background-image: linear-gradient(to bottom, \
         rgba({r}, {g}, {b}, 0.35), rgba({r}, {g}, {b}, 0.15) 240px, \
         transparent 440px); }} \
         @media (prefers-color-scheme: dark) {{ \
         .{class} {{ background-image: linear-gradient(to bottom, \
         rgba({r}, {g}, {b}, 0.6), rgba({r}, {g}, {b}, 0.3) 240px, \
         transparent 440px); }} }}"
    )
}

/// "Cover Colors" in Preferences.
fn enabled() -> bool {
    use gtk::gio::prelude::*;
    gtk::gio::Settings::new(crate::APP_ID).boolean("cover-colors")
}

/// Loads the rules, or none when cover colours are off. The rules are kept
/// either way, so turning the option back on shows them again at once.
pub(crate) fn reload() {
    let css = if enabled() {
        RULES.with(|rules| {
            rules
                .borrow()
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        })
    } else {
        String::new()
    };
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

/// A style class whose colour can change over time (the now-playing pill,
/// the Favorites page).
/// Unlike apply(), the same widget keeps one class and its rule is rewritten.
pub struct Dynamic {
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
            format!(".now-playing-pill.{c} .pill-progress highlight {{ background-color: color-mix(in srgb, {color} 30%, currentColor); }}"),
            format!(".now-playing-pill.{c} .pill-play {{ background-color: color-mix(in srgb, {color} 45%, transparent); }}"),
        ]
        .join("\n");
        RULES.with(|rules| rules.borrow_mut().insert(self.id, rule));
        reload();
    }

    /// The colour behind the top of a page, as on album pages (apply), for a
    /// page whose colour changes (Favorites).
    pub(crate) fn set_page(&self, color: (u8, u8, u8)) {
        let rule = page_rule(&self.class, color);
        RULES.with(|rules| rules.borrow_mut().insert(self.id, rule));
        reload();
    }

    /// Back to the default colours (no cover).
    pub(crate) fn clear(&self) {
        RULES.with(|rules| rules.borrow_mut().remove(&self.id));
        reload();
    }
}
