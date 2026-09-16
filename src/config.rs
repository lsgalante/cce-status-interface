//! Config access: cached KDL config lookup, color/font/dimension readers,
//! and resolution of the ccectl binary.
//!
//! Every key is read by an explicit JSON pointer — the canonical nesting in
//! config.kdl. A key at a non-canonical location simply does not resolve.
//! (The legacy fuzzy search that split snake_case keys across nesting was
//! deleted after a release of quiet fallback-warning logs.)
//!
//! Color space: text colors stay **raw sRGB** — they end up as `[u8; 3]` for
//! cosmic-text (see `StyledLabel`), which expects sRGB. Quad/box colors are
//! converted with [`cce_ui::color::srgb_to_linear`] because the Vulkan pipeline
//! samples them in linear space.

pub(crate) fn get_cached_config() -> serde_json::Value {
    cce_ui::config::cached_config()
}

fn cfg_f32(pointer: &str) -> Option<f32> {
    get_cached_config().pointer(pointer).and_then(|v| v.as_f64()).map(|n| n as f32)
}

fn cfg_string(pointer: &str) -> Option<String> {
    get_cached_config().pointer(pointer).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// A text color: raw sRGB RGB with alpha forced to 1.0 (cosmic-text consumes
/// text colors as sRGB `[u8; 3]`; alpha is not carried by the text path).
pub(crate) fn text_color_from(val: &serde_json::Value, pointer: &str) -> Option<[f32; 4]> {
    let s = val.pointer(pointer)?.as_str()?;
    let [r, g, b, _] = cce_ui::color::parse_hex_rgba(s)?;
    Some([r, g, b, 1.0])
}

/// A quad/box color: RGB converted sRGB→linear for the Vulkan pipeline, alpha
/// kept raw.
pub(crate) fn quad_color_from(val: &serde_json::Value, pointer: &str) -> Option<[f32; 4]> {
    let s = val.pointer(pointer)?.as_str()?;
    cce_ui::color::parse_hex_rgba_linear(s)
}

fn cfg_text_color(pointer: &str) -> Option<[f32; 4]> {
    text_color_from(&get_cached_config(), pointer)
}

fn cfg_quad_color(pointer: &str) -> Option<[f32; 4]> {
    quad_color_from(&get_cached_config(), pointer)
}

pub(crate) fn read_normal_color_from_config() -> Option<[f32; 4]> {
    // App-native `module { text_color }` first (a TEXT color — raw sRGB, per
    // the color-space split in CLAUDE.md), then the shared status key.
    cfg_text_color("/module/text_color")
        .or_else(|| cfg_text_color("/style/status/normal_color"))
}

pub(crate) fn read_disabled_color_from_config() -> Option<[f32; 4]> {
    cfg_text_color("/style/status/disabled_color")
}

pub(crate) fn read_status_font_from_config() -> String {
    // App-native `module { font }` first (a family; an embedded size ranks
    // below module { font_size } in the size chain), then the shared key.
    if let Some(font_str) = cfg_string("/module/font") {
        return font_str;
    }
    if let Some(font_str) = cfg_string("/style/status/font") {
        return font_str;
    }

    // The third rung used to be fontconfig's `status-interface` alias — a cce
    // invention squatting in fontconfig's family namespace, and one this
    // precedence chain had already demoted to a last resort. It is gone along
    // with the settings app's Fonts page; the DE's families now live in the
    // shared config's `fonts { }` block.
    cce_ui::layout::read_preferred_fonts().0
}

pub(crate) fn read_status_height_from_config() -> f32 {
    // App-native `module { height }` first — the compositor reads the same
    // key for the arrange pass's segment height and reserved strip, so the
    // two sides always agree — then the shared layout { bar_height }.
    cfg_f32("/module/height")
        .or_else(|| cfg_f32("/layout/bar_height"))
        .unwrap_or(28.0)
}

pub(crate) fn read_status_font_size_from_config() -> f32 {
    // App-native `module { font_size }` wins over everything — including the
    // size embedded in the shared font string ("Chivo Mono 14"), which stays
    // the fallback along with the shared status_font_size key.
    if let Some(size) = cfg_f32("/module/font_size") {
        return size;
    }
    if let Some(font_str) = cfg_string("/module/font") {
        let (_, parsed_size) = cce_ui::layout::parse_font_string(&font_str);
        if let Some(size) = parsed_size {
            return size;
        }
    }
    if let Some(font_str) = cfg_string("/style/status/font") {
        let (_, parsed_size) = cce_ui::layout::parse_font_string(&font_str);
        if let Some(size) = parsed_size {
            return size;
        }
    }

    cfg_f32("/style/status/font_size").unwrap_or(11.0)
}

pub(crate) fn read_status_padding_from_config() -> f32 {
    // App-native `module { padding }` first (the text inset inside each
    // module box — bar-side only), then the shared status key.
    cfg_f32("/module/padding")
        .or_else(|| cfg_f32("/style/status/padding"))
        .unwrap_or(8.0)
}

pub(crate) fn read_status_module_spacing_from_config() -> f32 {
    // App-native `module { spacing }` first (the same value the compositor
    // reads for the gap BETWEEN segments — this reader only matters for the
    // intra-surface layout of a multi-module bar), then the shared key.
    cfg_f32("/module/spacing")
        .or_else(|| cfg_f32("/style/status/module_spacing"))
        .unwrap_or(8.0)
}


/// The DE-wide light angle, canonical at `window_manager { light_source_position }`.
/// Radians normally; a value above 2π is taken as legacy degrees (e.g. `135`)
/// and converted. (This unifies the two previous readers, one of which only
/// degree-converted integer values.)
pub(crate) fn light_source_position_from(val: &serde_json::Value) -> f32 {
    let raw = val
        .pointer("/window_manager/light_source_position")
        .and_then(|v| v.as_f64())
        .map(|f| f as f32)
        .unwrap_or(2.356_194_5); // 135°, the compositor default
    if raw > 2.0 * std::f32::consts::PI {
        raw.to_radians()
    } else {
        raw
    }
}

pub(crate) fn read_light_source_position_from_config() -> f32 {
    light_source_position_from(&get_cached_config())
}

pub(crate) fn read_status_background_blur_from_config() -> f32 {
    cfg_f32("/style/status/background_blur").unwrap_or(0.0)
}

pub(crate) fn read_status_box_background_color_from_config() -> Option<[f32; 4]> {
    // App-native `module { background_color }` first (a quad color — the
    // sRGB→linear conversion applies, per the color-space split in
    // CLAUDE.md), then the shared status key, then the built-in default.
    let mut color = cfg_quad_color("/module/background_color")
        .or_else(|| cfg_quad_color("/style/status/background_color"))
        .unwrap_or_else(|| {
            let c = cce_ui::color::srgb_to_linear(0x15 as f32 / 255.0);
            let b = cce_ui::color::srgb_to_linear(0x20 as f32 / 255.0);
            [c, c, b, 0.9]
        });

    let blur = read_status_background_blur_from_config();

    // Scale RGB by (1.0 - blur) to apply tint factor while keeping alpha as full opacity for the blur shader
    color[0] *= 1.0 - blur;
    color[1] *= 1.0 - blur;
    color[2] *= 1.0 - blur;

    Some(color)
}

/// `module { droplet "sag=0.45 belly=0.75 gleam=1.2 ..." }` — the water-drop
/// module style ([`cce_ui::scene::paint::Prim::Droplet`]). The key's PRESENCE
/// turns the style on (an empty string takes every default); its value is
/// whitespace-separated `k=v` pairs onto [`DropletSpec`]'s fields, in the DE's
/// spec-string idiom. Unknown keys warn and are skipped, so a typo shows up in
/// the log instead of silently reverting one knob.
pub(crate) fn read_droplet_from_config() -> Option<cce_ui::scene::paint::DropletSpec> {
    // The parser is shared with the compositor (whose scenefx droplet node
    // refracts the backdrop behind each segment from the same spec).
    Some(cce_ui::scene::paint::DropletSpec::parse(&cfg_string("/module/droplet")?))
}

/// `module { text_raise }` — lifts module text above vertical center by this
/// many logical px (negative lowers it). Bar-side only; every module's text
/// baseline funnels through `centered_text_y`, which subtracts this.
pub(crate) fn read_text_raise_from_config() -> f32 {
    cfg_f32("/module/text_raise").unwrap_or(0.0)
}

/// `module { icon_size }` — the glyph height, logical px, for the modules
/// that read out as a cce-icons glyph with their value on it (cpu,
/// brightness, volume, battery). Default: the bar height less 6, so the
/// glyph sits inside the bubble with a 3px breath above and below.
pub(crate) fn read_icon_size_from_config() -> f32 {
    cfg_f32("/module/icon_size")
        .unwrap_or_else(|| read_status_height_from_config() - 6.0)
        .max(4.0)
}

/// `module { icon_font_size }` — the size of the number superimposed on a
/// glyph. Defaults to three quarters of the module font size: "100" at the
/// full size overhangs a bar-height glyph on both sides, and the glyph is
/// what carries the unit now, so the number can afford to be smaller.
pub(crate) fn read_icon_font_size_from_config(font_size: f32) -> f32 {
    cfg_f32("/module/icon_font_size").unwrap_or(font_size * 0.75).max(1.0)
}

/// `module { icon_weight }` — OpenType weight of the number on a glyph
/// (default 700, bold; 400 = the face's regular). Stroke width is what a
/// digit of that size has to spare, so the number is heavier than the
/// labels around it.
pub(crate) fn read_icon_weight_from_config() -> Option<u16> {
    let w = cfg_f32("/module/icon_weight").unwrap_or(700.0).clamp(1.0, 1000.0) as u16;
    Some(w)
}

/// `module { icon_pocket }` — opacity 0-1 (default 0.6) of the feathered
/// pool under the digits, in the number's contrast color: the notch the
/// number sits in, cut into the glyph so the digits are never read against
/// the glyph's own tint. 0 disables it.
pub(crate) fn read_icon_pocket_from_config() -> f32 {
    cfg_f32("/module/icon_pocket").unwrap_or(0.6).clamp(0.0, 1.0)
}

/// `module { icon_alpha }` — opacity 0-1 of the glyph under the number
/// (default 0.4). The glyph is tinted the number's color, so it is this
/// ghosting alone that keeps the digits legible on it.
pub(crate) fn read_icon_alpha_from_config() -> f32 {
    cfg_f32("/module/icon_alpha").unwrap_or(0.4).clamp(0.0, 1.0)
}

/// `module { text_contrast }` — adaptive contrast strength (0 = off, the
/// default; 1 = full). The compositor's per-segment `backdrop` measurement
/// drives the scrim through it: the ground darkens only as far as a backdrop
/// the configured text color cannot carry demands, and its strength tracks
/// how badly the text is losing. Bar-side only.
///
/// On its own it makes the scrim appear only when the backdrop earns it;
/// alongside `module { text_scrim }` it deepens that resting ground.
pub(crate) fn read_text_contrast_from_config() -> f32 {
    cfg_f32("/module/text_contrast").unwrap_or(0.0).clamp(0.0, 1.0)
}

/// `module { text_scrim }` — opacity 0-1 of a dark feathered pool drawn
/// inside each module box, beneath everything the module paints (0 = off,
/// the default). It darkens the ground the glyphs sit on rather than
/// decorating the letterforms, which is what survives a busy backdrop
/// without putting a rim on every letterform.
///
/// The DE's one text-contrast treatment, and `module { text_contrast }`
/// applies on top: the scrim rests at this opacity and deepens toward
/// opaque as the measured backdrop demands more.
pub(crate) fn read_text_scrim_from_config() -> f32 {
    cfg_f32("/module/text_scrim").unwrap_or(0.0).clamp(0.0, 1.0)
}

/// `module { text_scrim_feather }` — how far, in logical px, the scrim fades
/// out from its solid core (default: a quarter of the box's height, which
/// keeps the whole gradient inside the box at any bar height).
///
/// The feather is drawn OUTSIDE the core rect, so the core is inset by this
/// much: feather and inset are the same number, and the pool reaches the
/// box's edge exactly.
pub(crate) fn read_text_scrim_feather_from_config() -> Option<f32> {
    cfg_f32("/module/text_scrim_feather").map(|v| v.max(0.0))
}

pub(crate) fn read_status_box_corner_radius_from_config() -> f32 {
    // App-native ONLY (~/.config/cce/cce-status-interface/config.kdl,
    // merged over the shared config by cce-ui): `module { corner_radius }`.
    // Unlike the other module styling keys there is deliberately no shared
    // `style { status ... }` fallback — the radius is purely bar-side
    // cosmetics (the old `status_box_corner_radius` rung was removed).
    cfg_f32("/module/corner_radius").unwrap_or(4.0)
}

/// Bevel treatment for the module boxes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum StatusBoxBevel {
    /// A lit plate lip — the box rises out of the bar.
    Raised,
    /// A carved recess rim — the box sinks into the bar.
    Inset,
}

/// `style { status box_bevel="raised"|"inset" }`; absent, `"none"`, or any
/// other value keeps the flat boxes.
pub(crate) fn read_status_box_bevel_from_config() -> Option<StatusBoxBevel> {
    match cfg_string("/style/status/box_bevel")?.to_ascii_lowercase().as_str() {
        "raised" => Some(StatusBoxBevel::Raised),
        "inset" => Some(StatusBoxBevel::Inset),
        _ => None,
    }
}

/// `style { status box_bevel_depth=(f64)N }` — the roll width of the bevel lip
/// in logical px. The DE-wide `bevel_width` (~9px) is window-scale; module
/// boxes in a ~24px bar want a much tighter lip.
pub(crate) fn read_status_box_bevel_depth_from_config() -> f32 {
    cfg_f32("/style/status/box_bevel_depth").unwrap_or(3.0)
}

pub(crate) fn get_ccectl_cmd() -> String {
    if let Ok(home) = std::env::var("HOME") {
        let path = format!("{}/.local/bin/ccectl", home);
        if std::path::Path::new(&path).exists() {
            return path;
        }
    }
    "ccectl".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_rgba_close(actual: [f32; 4], expected: [f32; 4]) {
        for i in 0..4 {
            assert!(
                (actual[i] - expected[i]).abs() < 1e-3,
                "channel {} differs: actual {:?} vs expected {:?}",
                i,
                actual,
                expected
            );
        }
    }

    fn parse_kdl(content: &str) -> serde_json::Value {
        cce_ui::config::parse_kdl_to_json(content)
    }

    #[test]
    fn test_status_config() {
        let val = parse_kdl(
            r##"
style {
    status normal_color=(color)"#ccccd8" background_color=(color)"#151520e6" background_blur=(f64)0.8 font="Berkeley Mono 14"
}
"##,
        );

        assert_eq!(
            val.pointer("/style/status/background_color").and_then(|v| v.as_str()),
            Some("#151520e6")
        );
        assert_eq!(
            val.pointer("/style/status/background_blur").and_then(|v| v.as_f64()),
            Some(0.8)
        );
        assert_eq!(
            val.pointer("/style/status/font").and_then(|v| v.as_str()),
            Some("Berkeley Mono 14")
        );
    }

    #[test]
    fn test_box_bevel_config() {
        let val = parse_kdl(
            r##"
style {
    status box_bevel="raised" box_bevel_depth=(f64)2.5
}
"##,
        );
        assert_eq!(
            val.pointer("/style/status/box_bevel").and_then(|v| v.as_str()),
            Some("raised")
        );
        assert_eq!(
            val.pointer("/style/status/box_bevel_depth").and_then(|v| v.as_f64()),
            Some(2.5)
        );
    }

    // --- pointer-only lookup ---

    #[test]
    fn non_canonical_locations_do_not_resolve() {
        // A key parked somewhere other than its canonical nesting is simply
        // absent — the fuzzy search that used to find these is gone.
        let val = serde_json::json!({"stray": {"status_normal_color": "#222222"}});
        assert!(text_color_from(&val, "/style/status/normal_color").is_none());
    }

    // --- light_source_position ---

    #[test]
    fn light_source_position_canonical_and_units() {
        // Canonical location, radians as-is.
        let val = parse_kdl("window_manager {\n    light_source_position (f64)2.5\n}");
        assert!((light_source_position_from(&val) - 2.5).abs() < 1e-6);

        // A value above 2π is legacy degrees.
        let val = parse_kdl("window_manager {\n    light_source_position (f64)135.0\n}");
        assert!((light_source_position_from(&val) - 135.0f32.to_radians()).abs() < 1e-6);

        // Absent: the compositor's 135° default.
        let val = serde_json::json!({});
        assert!((light_source_position_from(&val) - 2.356_194_5).abs() < 1e-6);
    }

    // --- color space (spec: text = raw sRGB, quads = linearized) ---

    #[test]
    fn text_colors_stay_srgb_and_quad_colors_are_linearized() {
        let val = parse_kdl(
            r##"
style {
    status normal_color=(color)"#808080" background_color=(color)"#80808080"
}
"##,
        );
        let raw = 128.0f32 / 255.0;
        let linear = cce_ui::color::srgb_to_linear(raw);

        // Text color: raw sRGB, alpha forced to 1.0 (cosmic-text takes sRGB u8).
        let text = text_color_from(&val, "/style/status/normal_color").unwrap();
        assert_rgba_close(text, [raw, raw, raw, 1.0]);

        // Quad color: RGB linearized for the Vulkan pipeline, alpha raw.
        let quad = quad_color_from(&val, "/style/status/background_color").unwrap();
        assert_rgba_close(quad, [linear, linear, linear, raw]);
    }
}
