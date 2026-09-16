//! Bundled cce-icons glyphs, tinted for the bar.
//!
//! The stat modules draw a cce-icons glyph with their value superimposed on it,
//! and the glyph has to wear the same color as the number — `module
//! { text_color }`, the battery's accent when it is low or charging, the
//! volume's `disabled_color` while muted. `cce_ui::upload_icon` cannot do that:
//! a `Prim::Image` carries alpha but no color, and the bundled artwork is
//! white. So this rasterizes the SVG itself, multiplies the pixels by the
//! color, and uploads the result — one texture per `(name, px, color)`,
//! cached for the life of the process (the key space is a handful of colors
//! times one size, so nothing is ever freed).
//!
//! The artwork comes from the cce-icons crate via [`cce_ui::icons_dir`]
//! (`$CCE_ICONS_DIR`, else `~/projects/cce/cce-icons/svg`). A glyph that is
//! missing or unparsable yields `None` — the caller keeps its text readout as
//! the fallback — and is logged once, since the miss is cached too.

use std::collections::HashMap;
use std::sync::Mutex;

/// Rasterize `<name>.svg` from cce-icons at `px` on its longer side, tinted
/// to `rgb` (raw sRGB, like every text color here — uploaded images are
/// sampled as sRGB), and upload it as a renderer texture. Returns the image
/// id plus the pixel size for `PaintCtx::image`.
pub(crate) fn tinted_icon(name: &str, px: u32, rgb: [u8; 3]) -> Option<(u32, u32, u32)> {
    type Key = (String, u32, [u8; 3]);
    static CACHE: Mutex<Option<HashMap<Key, Option<(u32, u32, u32)>>>> = Mutex::new(None);
    let key = (name.to_string(), px, rgb);
    let mut guard = CACHE.lock().unwrap();
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some(hit) = cache.get(&key) {
        return *hit;
    }
    let loaded = (|| {
        let path = format!("{}/{name}.svg", cce_ui::icons_dir());
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("[icons] {path}: {e} — falling back to the text readout");
                return None;
            }
        };
        let (mut rgba, w, h) = cce_ui::rasterize_svg(&data, px).or_else(|| {
            log::warn!("[icons] {path}: unparsable SVG — falling back to the text readout");
            None
        })?;
        // The artwork is white, so multiplying is tinting; anything the
        // glyph shades darker (a cut-through keyhole) stays proportionally
        // darker in the tint.
        for p in rgba.chunks_exact_mut(4) {
            for (c, &t) in p[..3].iter_mut().zip(rgb.iter()) {
                *c = ((*c as u16 * t as u16 + 127) / 255) as u8;
            }
        }
        Some((cce_ui::vk::upload_rgba(rgba, w, h), w, h))
    })();
    cache.insert(key, loaded);
    loaded
}

/// The `[u8; 3]` a text color becomes for tinting — the same conversion
/// `StyledLabel` applies to its color, so glyph and number match exactly.
pub(crate) fn tint_of(color: [f32; 4]) -> [u8; 3] {
    [
        (color[0] * 255.0).round() as u8,
        (color[1] * 255.0).round() as u8,
        (color[2] * 255.0).round() as u8,
    ]
}
