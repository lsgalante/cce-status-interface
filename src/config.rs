//! Config access: cached KDL config lookup, color/font/dimension readers,
//! and resolution of the ccectl binary.
//!
//! Every key has an explicit JSON-pointer location (the canonical nesting in
//! config.kdl). Reads try the pointer first and fall back to the legacy fuzzy
//! search ([`json_find_key`]), warning once per key when only the fallback
//! hits — those warnings mean the key sits somewhere non-canonical in the
//! user's config (or collides with an unrelated key of the same name).
//!
//! Color space: text colors stay **raw sRGB** — they end up as `[u8; 3]` for
//! cosmic-text (see `StyledLabel`), which expects sRGB. Quad/box colors are
//! converted with [`cce_ui::color::srgb_to_linear`] because the Vulkan pipeline
//! samples them in linear space.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

pub(crate) fn get_cached_config() -> serde_json::Value {
    cce_ui::config::cached_config()
}

/// Legacy fuzzy lookup: exact key, then snake_case prefixes split across
/// nesting, then a depth-first search of every object. Kept only as the
/// fallback for configs that predate the canonical pointer locations.
pub(crate) fn json_find_key<'a>(val: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    fn find_recursive<'a>(val: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
        if let Some(obj) = val.as_object() {
            if let Some(v) = obj.get(key) {
                return Some(v);
            }
            let parts: Vec<&str> = key.split('_').collect();
            for i in 1..parts.len() {
                let (prefix_parts, suffix_parts) = parts.split_at(i);
                let prefix = prefix_parts.join("_");
                let suffix = suffix_parts.join("_");
                if let Some(sub_val) = obj.get(&prefix) {
                    if let Some(found) = find_recursive(sub_val, &suffix) {
                        return Some(found);
                    }
                }
            }
            for (_, sub_val) in obj.iter() {
                if let Some(found) = find_recursive(sub_val, key) {
                    return Some(found);
                }
            }
        }
        None
    }
    find_recursive(val, key)
}

fn warn_fuzzy_once(pointer: &str, legacy_key: &str) {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let warned = WARNED.get_or_init(|| Mutex::new(HashSet::new()));
    if warned.lock().map_or(false, |mut set| set.insert(pointer.to_string())) {
        log::warn!(
            "config key '{}' not found at its canonical location {} — \
             resolved by fuzzy search instead; consider moving it in config.kdl",
            legacy_key, pointer
        );
    }
}

/// Look up `pointer` in `val`, falling back to the legacy fuzzy search for
/// `legacy_key` (with a once-per-pointer warning when only the fallback hits).
pub(crate) fn pointer_or_fuzzy<'a>(
    val: &'a serde_json::Value,
    pointer: &str,
    legacy_key: &str,
) -> Option<&'a serde_json::Value> {
    if let Some(v) = val.pointer(pointer) {
        return Some(v);
    }
    let found = json_find_key(val, legacy_key);
    if found.is_some() {
        warn_fuzzy_once(pointer, legacy_key);
    }
    found
}

fn cfg_f32(pointer: &str, legacy_key: &str) -> Option<f32> {
    let val = get_cached_config();
    pointer_or_fuzzy(&val, pointer, legacy_key).and_then(|v| v.as_f64()).map(|n| n as f32)
}

fn cfg_string(pointer: &str, legacy_key: &str) -> Option<String> {
    let val = get_cached_config();
    pointer_or_fuzzy(&val, pointer, legacy_key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// A text color: raw sRGB RGB with alpha forced to 1.0 (cosmic-text consumes
/// text colors as sRGB `[u8; 3]`; alpha is not carried by the text path).
pub(crate) fn text_color_from(val: &serde_json::Value, pointer: &str, legacy_key: &str) -> Option<[f32; 4]> {
    let s = pointer_or_fuzzy(val, pointer, legacy_key)?.as_str()?;
    let [r, g, b, _] = cce_ui::color::parse_hex_rgba(s)?;
    Some([r, g, b, 1.0])
}

/// A quad/box color: RGB converted sRGB→linear for the Vulkan pipeline, alpha
/// kept raw.
pub(crate) fn quad_color_from(val: &serde_json::Value, pointer: &str, legacy_key: &str) -> Option<[f32; 4]> {
    let s = pointer_or_fuzzy(val, pointer, legacy_key)?.as_str()?;
    cce_ui::color::parse_hex_rgba_linear(s)
}

fn cfg_text_color(pointer: &str, legacy_key: &str) -> Option<[f32; 4]> {
    text_color_from(&get_cached_config(), pointer, legacy_key)
}

fn cfg_quad_color(pointer: &str, legacy_key: &str) -> Option<[f32; 4]> {
    quad_color_from(&get_cached_config(), pointer, legacy_key)
}

pub(crate) fn read_normal_color_from_config() -> Option<[f32; 4]> {
    // App-native `module { text_color }` first (a TEXT color — raw sRGB, per
    // the color-space split in CLAUDE.md), then the shared status key.
    cfg_text_color("/module/text_color", "module_text_color")
        .or_else(|| cfg_text_color("/style/status/normal_color", "status_normal_color"))
}

pub(crate) fn read_disabled_color_from_config() -> Option<[f32; 4]> {
    cfg_text_color("/style/status/disabled_color", "disabled_color")
}

pub(crate) fn read_status_font_from_config() -> String {
    // App-native `module { font }` first (a family; an embedded size ranks
    // below module { font_size } in the size chain), then the shared key.
    if let Some(font_str) = cfg_string("/module/font", "module_font") {
        return font_str;
    }
    if let Some(font_str) = cfg_string("/style/status/font", "status_font") {
        return font_str;
    }

    let font_conf_path = cce_ui::config::config_home().join("fontconfig").join("fonts.conf");
    if let Ok(content) = std::fs::read_to_string(&font_conf_path) {
        if let Some(font) = parse_font_for_alias(&content, "status-interface") {
            return font;
        }
    }
    "sans-serif".to_string()
}

pub(crate) fn read_status_height_from_config() -> f32 {
    // App-native `module { height }` first — the compositor reads the same
    // key for the arrange pass's segment height and reserved strip, so the
    // two sides always agree — then the shared layout { bar_height }.
    cfg_f32("/module/height", "module_height")
        .or_else(|| cfg_f32("/layout/bar_height", "bar_height"))
        .unwrap_or(28.0)
}

pub(crate) fn read_status_font_size_from_config() -> f32 {
    // App-native `module { font_size }` wins over everything — including the
    // size embedded in the shared font string ("Chivo Mono 14"), which stays
    // the fallback along with the shared status_font_size key.
    if let Some(size) = cfg_f32("/module/font_size", "module_font_size") {
        return size;
    }
    if let Some(font_str) = cfg_string("/module/font", "module_font") {
        let (_, parsed_size) = cce_ui::layout::parse_font_string(&font_str);
        if let Some(size) = parsed_size {
            return size;
        }
    }
    if let Some(font_str) = cfg_string("/style/status/font", "status_font") {
        let (_, parsed_size) = cce_ui::layout::parse_font_string(&font_str);
        if let Some(size) = parsed_size {
            return size;
        }
    }

    cfg_f32("/style/status/font_size", "status_font_size").unwrap_or(11.0)
}

pub(crate) fn read_status_padding_from_config() -> f32 {
    // App-native `module { padding }` first (the text inset inside each
    // module box — bar-side only), then the shared status key.
    cfg_f32("/module/padding", "module_padding")
        .or_else(|| cfg_f32("/style/status/padding", "status_padding"))
        .unwrap_or(8.0)
}

pub(crate) fn read_status_module_spacing_from_config() -> f32 {
    // App-native `module { spacing }` first (the same value the compositor
    // reads for the gap BETWEEN segments — this reader only matters for the
    // intra-surface layout of a multi-module bar), then the shared key.
    cfg_f32("/module/spacing", "module_spacing")
        .or_else(|| cfg_f32("/style/status/module_spacing", "status_module_spacing"))
        .unwrap_or(8.0)
}


pub(crate) fn parse_font_for_alias(content: &str, alias: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();
    for i in 0..lines.len() {
        let line = lines[i].trim();
        if line.contains("<test") && line.contains("name=\"family\"") && line.contains(&format!("<string>{}</string>", alias)) {
            for j in (i + 1)..(i + 6).min(lines.len()) {
                let next_line = lines[j].trim();
                if next_line.contains("<edit") {
                    for k in (j + 1)..(j + 6).min(lines.len()) {
                        let str_line = lines[k].trim();
                        if str_line.contains("<string>") && str_line.contains("</string>") {
                            if let Some(start) = str_line.find("<string>") {
                                if let Some(end) = str_line.find("</string>") {
                                    return Some(str_line[start + 8..end].to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// The DE-wide light angle, canonical at `window_manager { light_source_position }`.
/// Radians normally; a value above 2π is taken as legacy degrees (e.g. `135`)
/// and converted. (This unifies the two previous readers, one of which only
/// degree-converted integer values.)
pub(crate) fn light_source_position_from(val: &serde_json::Value) -> f32 {
    let raw = pointer_or_fuzzy(val, "/window_manager/light_source_position", "light_source_position")
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
    cfg_f32("/style/status/background_blur", "status_background_blur").unwrap_or(0.0)
}

pub(crate) fn read_status_box_background_color_from_config() -> Option<[f32; 4]> {
    // App-native `module { background_color }` first (a quad color — the
    // sRGB→linear conversion applies, per the color-space split in
    // CLAUDE.md), then the shared status key, then the built-in default.
    let mut color = cfg_quad_color("/module/background_color", "module_background_color")
        .or_else(|| cfg_quad_color("/style/status/background_color", "status_background_color"))
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

pub(crate) fn read_status_box_corner_radius_from_config() -> f32 {
    // App-native ONLY (~/.config/cce/cce-status-interface/config.kdl,
    // merged over the shared config by cce-ui): `module { corner_radius }`.
    // Unlike the other module styling keys there is deliberately no shared
    // `style { status ... }` fallback — the radius is purely bar-side
    // cosmetics (the old `status_box_corner_radius` rung was removed).
    cfg_f32("/module/corner_radius", "module_corner_radius").unwrap_or(4.0)
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
    match cfg_string("/style/status/box_bevel", "status_box_bevel")?.to_ascii_lowercase().as_str() {
        "raised" => Some(StatusBoxBevel::Raised),
        "inset" => Some(StatusBoxBevel::Inset),
        _ => None,
    }
}

/// `style { status box_bevel_depth=(f64)N }` — the roll width of the bevel lip
/// in logical px. The DE-wide `bevel_width` (~9px) is window-scale; module
/// boxes in a ~24px bar want a much tighter lip.
pub(crate) fn read_status_box_bevel_depth_from_config() -> f32 {
    cfg_f32("/style/status/box_bevel_depth", "status_box_bevel_depth").unwrap_or(3.0)
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

    // --- json_find_key (legacy fuzzy fallback) ---

    #[test]
    fn find_key_exact_match_at_top_level() {
        let val = serde_json::json!({"bar_height": 30.0});
        assert_eq!(json_find_key(&val, "bar_height").and_then(|v| v.as_f64()), Some(30.0));
    }

    #[test]
    fn find_key_splits_snake_case_across_nesting() {
        // "status_background_color" matches status { background_color }.
        let val = serde_json::json!({"style": {"status": {"background_color": "#101010"}}});
        assert_eq!(
            json_find_key(&val, "status_background_color").and_then(|v| v.as_str()),
            Some("#101010")
        );
    }

    #[test]
    fn find_key_exact_match_wins_over_split() {
        // An exact "status_font" key beats descending into status { font }.
        let val = serde_json::json!({
            "status_font": "Exact Font",
            "status": {"font": "Split Font"}
        });
        assert_eq!(
            json_find_key(&val, "status_font").and_then(|v| v.as_str()),
            Some("Exact Font")
        );
    }

    #[test]
    fn find_key_recurses_into_unrelated_parents() {
        // The key is found even under a parent the key name never mentions.
        let val = serde_json::json!({"unrelated": {"deeply": {"bar_height": 42.0}}});
        assert_eq!(json_find_key(&val, "bar_height").and_then(|v| v.as_f64()), Some(42.0));
    }

    #[test]
    fn find_key_miss_is_none() {
        let val = serde_json::json!({"style": {"status": {}}});
        assert!(json_find_key(&val, "nonexistent_key").is_none());
    }

    // --- pointer_or_fuzzy ---

    #[test]
    fn pointer_wins_over_fuzzy_match() {
        // With both a canonical and a stray same-named key, the pointer wins.
        let val = serde_json::json!({
            "style": {"status": {"normal_color": "#111111"}},
            "stray": {"status_normal_color": "#222222"}
        });
        assert_eq!(
            pointer_or_fuzzy(&val, "/style/status/normal_color", "status_normal_color")
                .and_then(|v| v.as_str()),
            Some("#111111")
        );
    }

    #[test]
    fn pointer_miss_falls_back_to_fuzzy() {
        let val = serde_json::json!({"stray": {"status_normal_color": "#222222"}});
        assert_eq!(
            pointer_or_fuzzy(&val, "/style/status/normal_color", "status_normal_color")
                .and_then(|v| v.as_str()),
            Some("#222222")
        );
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
        let text = text_color_from(&val, "/style/status/normal_color", "status_normal_color").unwrap();
        assert_rgba_close(text, [raw, raw, raw, 1.0]);

        // Quad color: RGB linearized for the Vulkan pipeline, alpha raw.
        let quad = quad_color_from(&val, "/style/status/background_color", "status_background_color").unwrap();
        assert_rgba_close(quad, [linear, linear, linear, raw]);
    }
}
