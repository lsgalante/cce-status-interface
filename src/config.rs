//! Config access: cached KDL config lookup, color/font/dimension readers,
//! and resolution of the cce/ccectl/cce-cloud binaries.

pub(crate) fn parse_json(content: &str) -> serde_json::Value {
    cce_ui::config::parse_kdl_to_json(content)
}

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

pub(crate) fn get_cached_config() -> serde_json::Value {
    cce_ui::config::cached_config()
}

pub(crate) fn get_cached_config_content() -> String {
    cce_ui::config::cached_config_content()
}

pub(crate) fn read_normal_color_from_config() -> Option<[f32; 4]> {
    let content = get_cached_config_content();
    parse_srgb_color_from_key(&content, "status_normal_color")
}

pub(crate) fn read_disabled_color_from_config() -> Option<[f32; 4]> {
    let content = get_cached_config_content();
    parse_srgb_color_from_key(&content, "disabled_color")
}

pub(crate) fn parse_srgb_color_from_key(content: &str, key: &str) -> Option<[f32; 4]> {
    let val = parse_json(content);
    if let Some(s) = json_find_key(&val, key).and_then(|v| v.as_str()) {
        if let Some(rgb) = parse_hex(s) {
            let r = rgb[0] as f32 / 255.0;
            let g = rgb[1] as f32 / 255.0;
            let b = rgb[2] as f32 / 255.0;
            return Some([r, g, b, 1.0]);
        }
    }
    None
}

pub(crate) fn read_status_font_from_config() -> String {
    let val = get_cached_config();
    if let Some(font_str) = json_find_key(&val, "status_font").and_then(|v| v.as_str()) {
        return font_str.to_string();
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
    let val = get_cached_config();
    json_find_key(&val, "bar_height").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(28.0)
}

pub(crate) fn read_status_font_size_from_config() -> f32 {
    let val = get_cached_config();
    
    if let Some(font_str) = json_find_key(&val, "status_font").and_then(|v| v.as_str()) {
        let (_, parsed_size) = cce_ui::layout::parse_font_string(font_str);
        if let Some(size) = parsed_size {
            return size;
        }
    }
    
    json_find_key(&val, "status_font_size").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(11.0)
}

pub(crate) fn read_status_padding_from_config() -> f32 {
    let val = get_cached_config();
    json_find_key(&val, "status_padding").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(8.0)
}

pub(crate) fn read_status_module_spacing_from_config() -> f32 {
    let val = get_cached_config();
    json_find_key(&val, "status_module_spacing").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(8.0)
}

pub(crate) fn read_separator_color_from_config() -> Option<[f32; 4]> {
    let content = get_cached_config_content();
    parse_color_from_key(&content, "status_separator_color")
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

pub(crate) fn read_bg_color_from_config() -> Option<[f32; 4]> {
    let content = get_cached_config_content();
    parse_color_from_key(&content, "background_color")
        .or_else(|| parse_color_from_key(&content, "low_color"))
        .or_else(|| parse_color_from_key(&content, "desktop_gap_color"))
}

pub(crate) fn parse_color_from_key(content: &str, key: &str) -> Option<[f32; 4]> {
    let val = parse_json(content);
    if let Some(s) = json_find_key(&val, key).and_then(|v| v.as_str()) {
        if let Some(rgba) = parse_hex_rgba(s) {
            let r = (rgba[0] as f32 / 255.0).powf(2.2);
            let g = (rgba[1] as f32 / 255.0).powf(2.2);
            let b = (rgba[2] as f32 / 255.0).powf(2.2);
            let a = rgba[3] as f32 / 255.0;
            return Some([r, g, b, a]);
        }
    }
    None
}

pub(crate) fn parse_hex(s: &str) -> Option<[u8; 3]> {
    cce_ui::color::parse_hex_bytes(s).map(|[r, g, b, _]| [r, g, b])
}
pub(crate) fn parse_hex_rgba(s: &str) -> Option<[u8; 4]> {
    cce_ui::color::parse_hex_bytes(s)
}

pub(crate) fn parse_rgba_color_from_key(content: &str, key: &str) -> Option<[f32; 4]> {
    let val = parse_json(content);
    if let Some(s) = json_find_key(&val, key).and_then(|v| v.as_str()) {
        if let Some(rgba) = parse_hex_rgba(s) {
            let r = (rgba[0] as f32 / 255.0).powf(2.2);
            let g = (rgba[1] as f32 / 255.0).powf(2.2);
            let b = (rgba[2] as f32 / 255.0).powf(2.2);
            let a = rgba[3] as f32 / 255.0;
            return Some([r, g, b, a]);
        }
    }
    None
}

pub(crate) fn read_status_background_blur_from_config() -> f32 {
    let val = get_cached_config();
    json_find_key(&val, "status_background_blur").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(0.0)
}

pub(crate) fn read_status_box_background_color_from_config() -> Option<[f32; 4]> {
    let content = get_cached_config_content();
    let mut color = parse_rgba_color_from_key(&content, "status_background_color")
        .unwrap_or_else(|| {
            let r = (0x15 as f32 / 255.0).powf(2.2);
            let g = (0x15 as f32 / 255.0).powf(2.2);
            let b = (0x20 as f32 / 255.0).powf(2.2);
            [r, g, b, 0.9]
        });

    let blur = read_status_background_blur_from_config();
    
    // Scale RGB by (1.0 - blur) to apply tint factor while keeping alpha as full opacity for the blur shader
    color[0] *= 1.0 - blur;
    color[1] *= 1.0 - blur;
    color[2] *= 1.0 - blur;

    Some(color)
}

pub(crate) fn read_status_box_corner_radius_from_config() -> f32 {
    let val = get_cached_config();
    json_find_key(&val, "status_box_corner_radius").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(4.0)
}

pub(crate) fn get_cce_cloud_cmd() -> String {
    if let Ok(home) = std::env::var("HOME") {
        let path = format!("{}/.local/bin/cce-cloud", home);
        if std::path::Path::new(&path).exists() {
            return path;
        }
    }
    "cce-cloud".to_string()
}

pub(crate) fn get_cce_cmd() -> String {
    if let Ok(home) = std::env::var("HOME") {
        let path = format!("{}/.local/bin/cce", home);
        if std::path::Path::new(&path).exists() {
            return path;
        }
    }
    "cce".to_string()
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
