//! Bundled cce-icons glyphs, tinted for the bar.
//!
//! Every symbol the bar draws is a cce-icons glyph: the stat readouts' units,
//! the OSD's, the tray's stand-in for an item that brings no icon of its own,
//! and the in-surface menu's marks and page chevrons. A glyph has to wear
//! the colour of the text beside it — `module { text_color }`, the battery's
//! accent when it is low or charging, the volume's `disabled_color` while
//! muted — and a `Prim::Image` carries alpha but no colour, so the tint is
//! baked into the texture: `cce_ui::upload_icon_tinted`, cached per `(name,
//! px, tint)` and keyed on the renderer epoch, so a reconnect (cce-ui
//! repairs a lost transport by opening a new session, and a new renderer,
//! around the same `Application`) re-uploads rather than drawing ids the
//! new renderer never had. Until 2026-10-05 this module rasterized and
//! uploaded the SVGs itself through a cache that outlived the renderer, and
//! `renderer_init` had to empty it by hand.
//!
//! The artwork comes from the cce-icons crate via [`cce_ui::icons_dir`]
//! (`$CCE_ICONS_DIR`, else `~/projects/cce/cce-icons/svg`). A glyph that is
//! missing or unparsable yields `None` — the caller keeps its text readout as
//! the fallback — and is logged once per name.

use std::collections::HashSet;
use std::sync::Mutex;

/// `<name>.svg` from cce-icons, `side` LOGICAL px on its longer side, tinted
/// `rgb` (raw sRGB, like every text colour here, so a glyph matches the label
/// beside it). Returns the image id and the glyph's logical size.
pub(crate) fn glyph(name: &str, side: f32, rgb: [u8; 3]) -> Option<(u32, f32, f32)> {
    let scale = cce_ui::scale::scale_factor();
    let px = (side * scale).round().max(1.0) as u32;
    match cce_ui::upload_icon_tinted(name, px, rgb) {
        Some((image, w, h)) => Some((image, w as f32 / scale, h as f32 / scale)),
        None => {
            static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
            let mut warned = WARNED.lock().unwrap();
            if warned.get_or_insert_with(HashSet::new).insert(name.to_string()) {
                log::warn!(
                    "[icons] {}/{name}.svg could not be loaded — drawing without it",
                    cce_ui::icons_dir()
                );
            }
            None
        }
    }
}
