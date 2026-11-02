//! Tints from the dominant colour of an album cover.
//! Every tinted widget gets its own style class and rule, removed again
//! when the widget is destroyed, so pages never share or steal colours.
//! The rules are written for the style the app shows (Preferences can force
//! light or dark), and again when it changes.

use gtk::gdk;
use gtk::prelude::*;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeMap;

thread_local! {
    /// One app-wide stylesheet holding the rules of all tinted widgets alive now.
    static PROVIDER: OnceCell<gtk::CssProvider> = const { OnceCell::new() };
    static RULES: RefCell<BTreeMap<u64, Rule>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// What one tinted widget shows, in the colour of a cover.
#[derive(Clone, Copy)]
enum Tint {
    /// The colour from the top of a page down (page_rule).
    Page((u8, u8, u8)),
    /// The now-playing pill (pill_rule).
    Pill((u8, u8, u8)),
    /// A flat card (card_rule).
    Card((u8, u8, u8)),
}

/// The tint of one widget and the style class it is drawn for.
struct Rule {
    class: String,
    tint: Tint,
}

impl Rule {
    fn css(&self, dark: bool) -> String {
        match self.tint {
            Tint::Page(color) => page_rule(&self.class, color, dark),
            Tint::Pill(color) => pill_rule(&self.class, color),
            Tint::Card(color) => card_rule(&self.class, color, dark),
        }
    }
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
pub(crate) fn apply(widget: &impl IsA<gtk::Widget>, color: (u8, u8, u8)) {
    let id = next_id();
    let class = format!("album-tint-{id}");
    widget.add_css_class(&class);
    let tint = Tint::Page(color);
    RULES.with(|rules| rules.borrow_mut().insert(id, Rule { class, tint }));
    reload();

    // The page is gone: drop its rule too.
    widget.connect_destroy(move |_| {
        RULES.with(|rules| rules.borrow_mut().remove(&id));
        reload();
    });
}

/// The colour from the top of a page down, fading into the page's own
/// background (album pages, Favorites). Softer in the light style, or dark
/// covers drown the title.
fn page_rule(class: &str, (r, g, b): (u8, u8, u8), dark: bool) -> String {
    let (top, middle) = if dark { (0.6, 0.3) } else { (0.35, 0.15) };
    format!(
        ".{class} {{ background-image: linear-gradient(to bottom, \
         rgba({r}, {g}, {b}, {top}), rgba({r}, {g}, {b}, {middle}) 240px, \
         transparent 440px); }}"
    )
}

/// The now-playing pill: the cover colour mixed into the window background,
/// so the text keeps its normal colour and stays readable on any cover. Only
/// the progress line and the play button carry the colour at full strength.
fn pill_rule(class: &str, (r, g, b): (u8, u8, u8)) -> String {
    let color = format!("rgb({r}, {g}, {b})");
    // Two classes, so these win over the plain .now-playing-pill rules.
    [
        format!(".now-playing-pill.{class} {{ background-color: color-mix(in srgb, {color} 30%, var(--window-bg-color)); }}"),
        format!(".now-playing-pill.{class} .pill-progress highlight {{ background-color: color-mix(in srgb, {color} 30%, currentColor); }}"),
        format!(".now-playing-pill.{class} .pill-play {{ background-color: color-mix(in srgb, {color} 45%, transparent); }}"),
    ]
    .join("\n")
}

/// Adwaita's window backgrounds, against which a card's share of colour is
/// chosen; the rule itself mixes with the theme's own colour.
const LIGHT_WINDOW: (u8, u8, u8) = (250, 250, 251);
const DARK_WINDOW: (u8, u8, u8) = (34, 34, 38);

/// The relative luminance of an sRGB colour (WCAG), 0 to 1.
fn luminance((r, g, b): (f64, f64, f64)) -> f64 {
    let linear = |v: f64| {
        let v = v / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// The luminance of `share` percent of `color` mixed into `window`, as
/// color-mix in srgb mixes them.
fn mixed_luminance(color: (u8, u8, u8), window: (u8, u8, u8), share: u32) -> f64 {
    let t = f64::from(share) / 100.0;
    let mix = |c: u8, w: u8| f64::from(c) * t + f64::from(w) * (1.0 - t);
    luminance((
        mix(color.0, window.0),
        mix(color.1, window.1),
        mix(color.2, window.2),
    ))
}

/// How much of the cover colour a card takes, in percent: 35 in the dark
/// style and 20 in the light one, less where that would make the card too
/// light for its text (dark style) or too dark (light style). The limits keep
/// the orange year at about 4.5:1 or more against the card in the dark style
/// and 3.5:1 in the light one, and the dimmed count near what it has on a
/// plain card.
pub(crate) fn card_share(color: (u8, u8, u8), dark: bool) -> u32 {
    let (most, window) = if dark {
        (35, DARK_WINDOW)
    } else {
        (20, LIGHT_WINDOW)
    };
    (0..=most)
        .rev()
        .find(|&share| {
            let luminance = mixed_luminance(color, window, share);
            if dark {
                luminance <= 0.09
            } else {
                luminance >= 0.70
            }
        })
        .unwrap_or(0)
}

/// The flat colour of a card (the Tuner on the Start page): the cover colour
/// mixed into the window background (card_share).
pub(crate) fn card_rule(class: &str, (r, g, b): (u8, u8, u8), dark: bool) -> String {
    let share = card_share((r, g, b), dark);
    format!(
        ".{class} {{ background-color: color-mix(in srgb, rgb({r}, {g}, {b}) {share}%, \
         var(--window-bg-color)); }}"
    )
}

/// "Cover Colors" in Preferences.
fn enabled() -> bool {
    use gtk::gio::prelude::*;
    crate::settings().boolean("cover-colors")
}

/// Loads the rules for the style the app shows, or none when cover colours
/// are off. The rules are kept either way, so turning the option back on
/// shows them again at once. A media query in the rules would follow the
/// system's style instead of one chosen in Preferences.
pub(crate) fn reload() {
    let style = adw::StyleManager::default();
    let css = if enabled() {
        let dark = style.is_dark();
        RULES.with(|rules| {
            rules
                .borrow()
                .values()
                .map(|rule| rule.css(dark))
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
            // Another style: the rules again, written for it.
            style.connect_dark_notify(|_| reload());
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

    fn set(&self, tint: Tint) {
        let class = self.class.clone();
        RULES.with(|rules| rules.borrow_mut().insert(self.id, Rule { class, tint }));
        reload();
    }

    /// The now-playing pill in the cover's colour (pill_rule).
    pub(crate) fn set_solid(&self, color: (u8, u8, u8)) {
        self.set(Tint::Pill(color));
    }

    /// The colour behind the top of a page, as on album pages (apply), for a
    /// page whose colour changes (Favorites).
    pub(crate) fn set_page(&self, color: (u8, u8, u8)) {
        self.set(Tint::Page(color));
    }

    /// The flat colour of a card (card_rule).
    pub(crate) fn set_card(&self, color: (u8, u8, u8)) {
        self.set(Tint::Card(color));
    }

    /// Back to the default colours (no cover).
    pub(crate) fn clear(&self) {
        RULES.with(|rules| rules.borrow_mut().remove(&self.id));
        reload();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_rule_has_no_media_query() {
        // A media query follows the system's style, not one forced in
        // Preferences: the rule is written for the style shown.
        let dark = page_rule("tint-1", (200, 100, 50), true);
        let light = page_rule("tint-1", (200, 100, 50), false);
        for rule in [&dark, &light] {
            assert!(!rule.contains("@media"), "{rule}");
            assert!(rule.starts_with(".tint-1 {"), "{rule}");
            assert!(rule.contains("rgba(200, 100, 50, "), "{rule}");
        }
        assert!(dark.contains(", 0.6)") && dark.contains(", 0.3)"), "{dark}");
        assert!(
            light.contains(", 0.35)") && light.contains(", 0.15)"),
            "{light}"
        );
    }

    #[test]
    fn the_card_takes_more_of_the_cover_in_the_dark_style() {
        let dark = card_rule("tint-1", (200, 100, 50), true);
        let light = card_rule("tint-1", (200, 100, 50), false);
        for rule in [&dark, &light] {
            assert!(rule.starts_with(".tint-1 {"), "{rule}");
            assert!(rule.contains("rgb(200, 100, 50)"), "{rule}");
            assert!(rule.contains("var(--window-bg-color)"), "{rule}");
            assert!(!rule.contains("@media"), "{rule}");
        }
        assert!(dark.contains(" 35%"), "{dark}");
        assert!(light.contains(" 20%"), "{light}");
    }

    /// The luminance of the card's colour over Adwaita's window.
    fn card(color: (u8, u8, u8), dark: bool) -> f64 {
        let window = if dark { DARK_WINDOW } else { LIGHT_WINDOW };
        mixed_luminance(color, window, card_share(color, dark))
    }

    #[test]
    fn a_light_cover_takes_less_in_the_dark_style() {
        for color in [(255, 255, 255), (240, 230, 120), (180, 200, 220)] {
            let share = card_share(color, true);
            assert!(share < 35, "{color:?}: {share}");
            assert!(share > 0, "{color:?}: {share}");
            assert!(card(color, true) <= 0.09, "{color:?}");
        }
        assert_eq!(card_share((30, 60, 120), true), 35);
    }

    #[test]
    fn a_dark_cover_takes_less_in_the_light_style() {
        for color in [(0, 0, 0), (60, 60, 60), (90, 20, 20)] {
            let share = card_share(color, false);
            assert!(share < 20, "{color:?}: {share}");
            assert!(share > 0, "{color:?}: {share}");
            assert!(card(color, false) >= 0.70, "{color:?}");
        }
        assert_eq!(card_share((240, 200, 120), false), 20);
    }
}
