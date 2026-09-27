//! Page tint: a soft gradient in the dominant colour of the album cover.

use gtk::gdk;
use gtk::prelude::*;
use std::cell::OnceCell;

thread_local! {
    /// One app-wide stylesheet, rewritten whenever an album page gets its colour.
    static PROVIDER: OnceCell<gtk::CssProvider> = const { OnceCell::new() };
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

/// Sets the colour used by the "album-tint" style class.
pub(crate) fn apply((r, g, b): (u8, u8, u8)) {
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
        provider.load_from_string(&format!(
            ".album-tint {{ background-image: linear-gradient(to bottom, \
             rgba({r}, {g}, {b}, 0.45), transparent 480px); }}"
        ));
    });
}
