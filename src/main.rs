use std::collections::HashMap;
use std::sync::Arc;
use glyphon::{
    Attrs, Buffer, FontSystem, Metrics, TextArea, TextBounds,
};
use clear_ui::color;
use clear_ui::widget::{
    StyledLabel as Label, TextItem, Separator, Element,
    MouseButton, ElementState, MouseScrollDelta, KeyEvent,
};

#[derive(Debug, Clone)]
struct TrayPixmap {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct TrayItem {
    id: String,
    icon_name: Option<String>,
    icon_theme_path: Option<String>,
    pixmaps: Option<Vec<TrayPixmap>>,
    title: Option<String>,
}

#[derive(Debug, Clone)]
struct TrayIconBounds {
    id: String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    title: Option<String>,
}

#[derive(Debug, Clone)]
struct TagBounds {
    name: String,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Debug, Clone)]
struct LayoutBounds {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Debug, Clone)]
struct SystemStats {
    clock: String,
    memory: String,
    cpu: String,
    battery: String,
    battery_capacity: i32,
    battery_charging: bool,
    volume: String,
    volume_muted: bool,
    brightness: String,
}

#[derive(Debug, Clone)]
enum CustomEvent {
    TagsUpdated(String),
    LayoutUpdated(String),
    TitleUpdated(String),
    SystemStatsUpdated(SystemStats),
    TrayUpdated(TrayItem),
    TrayRemoved(String),
    LayoutMenuClosed(u32),
}

fn make_text_buffer(fs: &mut FontSystem, text: &str, size: f32, font_family: &str) -> Buffer {
    let metrics = Metrics::new(size, size * 1.4);
    let mut buf = Buffer::new(fs, metrics);
    let family = match font_family {
        "monospace" => glyphon::Family::Monospace,
        "sans-serif" => glyphon::Family::SansSerif,
        "serif" => glyphon::Family::Serif,
        _ => glyphon::Family::Name(font_family),
    };
    let attrs = Attrs::new().family(family);
    buf.set_text(fs, text, attrs, glyphon::Shaping::Advanced);
    buf.shape_until_scroll(fs, true);
    buf
}

fn parse_hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
    let s = hex.trim_start_matches('#');
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&s[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&s[4..6], 16).ok()? as f32 / 255.0;
        Some([r, g, b, 1.0])
    } else {
        None
    }
}

fn parse_tags(pango: &str) -> Vec<([f32; 4], String)> {
    let mut result = Vec::new();
    let mut remaining = pango;
    while let Some(start_span) = remaining.find("<span color='") {
        let color_start = start_span + "<span color='".len();
        if let Some(color_end) = remaining[color_start..].find("'") {
            let hex_color = &remaining[color_start..color_start + color_end];
            let tag_start = color_start + color_end + "'>".len();
            if let Some(tag_end) = remaining[tag_start..].find("</span>") {
                let tag_text = &remaining[tag_start..tag_start + tag_end];
                let color = parse_hex_to_rgba(hex_color).unwrap_or([0.8, 0.8, 0.8, 1.0]);
                result.push((color, tag_text.to_string()));
                remaining = &remaining[tag_start + tag_end + "</span>".len()..];
            } else {
                break;
            }
        } else {
            break;
        }
    }
    if result.is_empty() && !pango.is_empty() {
        // Fallback for plain text
        result.push(([0.8, 0.8, 0.8, 1.0], pango.to_string()));
    }
    result
}

fn read_cpu_ticks() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let first_line = stat.lines().next()?;
    if first_line.starts_with("cpu ") {
        let parts: Vec<u64> = first_line
            .split_whitespace()
            .skip(1)
            .filter_map(|s| s.parse::<u64>().ok())
            .collect();
        if parts.len() >= 4 {
            let idle = parts[3];
            let total: u64 = parts.iter().sum();
            return Some((total, idle));
        }
    }
    None
}

fn read_memory_usage() -> Option<String> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = 0.0;
    let mut free = 0.0;
    let mut buffers = 0.0;
    let mut cached = 0.0;
    for line in meminfo.lines() {
        if line.starts_with("MemTotal:") {
            total = line.split_whitespace().nth(1)?.parse::<f32>().ok()? / 1024.0 / 1024.0;
        } else if line.starts_with("MemFree:") {
            free = line.split_whitespace().nth(1)?.parse::<f32>().ok()? / 1024.0 / 1024.0;
        } else if line.starts_with("Buffers:") {
            buffers = line.split_whitespace().nth(1)?.parse::<f32>().ok()? / 1024.0 / 1024.0;
        } else if line.starts_with("Cached:") {
            cached = line.split_whitespace().nth(1)?.parse::<f32>().ok()? / 1024.0 / 1024.0;
        }
    }
    if total > 0.0 {
        let used = total - free - buffers - cached;
        Some(format!("Mem {:.1}G/{:.1}G", used, total))
    } else {
        None
    }
}

fn read_battery_details() -> Option<(String, i32, bool)> {
    for bat in &["BAT0", "BAT1"] {
        let cap_path = format!("/sys/class/power_supply/{}/capacity", bat);
        let status_path = format!("/sys/class/power_supply/{}/status", bat);
        if let Ok(cap_str) = std::fs::read_to_string(&cap_path) {
            let cap_trimmed = cap_str.trim();
            let cap = cap_trimmed.parse::<i32>().unwrap_or(0);
            let status = std::fs::read_to_string(&status_path).unwrap_or_default();
            let is_charging = status.trim() == "Charging";
            let charge_symbol = if is_charging { "⚡" } else { "Bat" };
            return Some((format!("{} {}%", charge_symbol, cap_trimmed), cap, is_charging));
        }
    }
    None
}

fn read_brightness() -> Option<String> {
    let dir = std::fs::read_dir("/sys/class/backlight").ok()?;
    for entry in dir {
        if let Ok(entry) = entry {
            let path = entry.path();
            let cur_path = path.join("brightness");
            let max_path = path.join("max_brightness");
            if cur_path.exists() && max_path.exists() {
                let cur_str = std::fs::read_to_string(cur_path).ok()?;
                let max_str = std::fs::read_to_string(max_path).ok()?;
                let cur = cur_str.trim().parse::<f32>().ok()?;
                let max = max_str.trim().parse::<f32>().ok()?;
                if max > 0.0 {
                    let pct = (cur / max * 100.0).round() as i32;
                    return Some(format!("Bri {}%", pct));
                }
            }
        }
    }
    None
}

struct RectWidget {
    x: f32, y: f32, w: f32, h: f32,
    color: [f32; 4],
}


struct StatusApp {
    // Status State
    tags: String,
    layout: String,
    title: String,
    stats: Option<SystemStats>,
    tray_items: HashMap<String, TrayItem>,
    cursor_pos: (f64, f64),
    hovered_tray_item: Option<String>,
    tray_item_bounds: Vec<TrayIconBounds>,
    tag_bounds: Vec<TagBounds>,
    layout_bounds: Option<LayoutBounds>,
    layout_menu_pid: Option<u32>,

    font_system: FontSystem,
    status_bar: clear_ui::widget::StatusBar,

    rects: Vec<RectWidget>,
    overlay_rects: Vec<RectWidget>,
    separators: Vec<Separator>,
    text_items: Vec<TextItem>,

    scale_factor: f64,
    width: u32,
    height: u32,
    needs_rebuild: bool,
    current_bg_color: [f32; 4],
    sender: calloop::channel::Sender<CustomEvent>,
}

impl StatusApp {
    fn rebuild_layout(&mut self) {
        let font_family = read_status_font_from_config();
        let font_size = read_status_font_size_from_config();
        let show_separators = read_status_separators_from_config();
        let padding = read_status_padding_from_config();
        let separator_color = read_separator_color_from_config().unwrap_or(color::STATUS_ACCENT);
        let normal_color = read_normal_color_from_config().unwrap_or(color::TEXT_FG);
        let sw_logical = self.width as f32;
        let sh_logical = self.height as f32;
        let _s = 1.0f32;
        let bar_h = read_status_height_from_config();
        eprintln!("[rebuild_layout] sw_logical={}, sh_logical={}, font_family={}, font_size={}", sw_logical, sh_logical, font_family, font_size);

        self.current_bg_color = read_bg_color_from_config().unwrap_or(color::STATUS_BG);
        eprintln!("[rebuild_layout] Using background color: {:?}", self.current_bg_color);

        self.rects.clear();
        self.overlay_rects.clear();
        self.separators.clear();
        self.text_items.clear();

        // 1. Background (using StatusBar widget)
        self.status_bar.set_rect(0.0, 0.0, sw_logical, bar_h);
        self.status_bar.set_bg_color(self.current_bg_color);
        
        let show_underline = read_status_underline_from_config();
        if show_underline {
            // 1b. Accent border at bottom of the status bar area
            self.rects.push(RectWidget {
                x: 0.0, y: bar_h - 2.0, w: sw_logical, h: 2.0,
                color: separator_color,
            });
        }

        self.tag_bounds.clear();
        self.layout_bounds = None;
        let mut left_x = 12.0;

        // 2. Tags
        let tags = parse_tags(&self.tags);
        for (col, text) in tags {
            let label_str = format!(" {} ", text);
            let label = Label::new_with_family(&mut self.font_system, &label_str, font_size, col, &font_family);
            let line_w = label.draw(&mut self.text_items, left_x, (bar_h - font_size * 1.4) / 2.0);
            self.tag_bounds.push(TagBounds {
                name: text.clone(),
                x: left_x,
                y: 0.0,
                w: line_w,
                h: bar_h,
            });
            left_x += line_w + 4.0;
        }

        // Spacing/separator before Layout
        if !self.layout.is_empty() {
            if show_separators {
                self.separators.push(Separator::new(
                    left_x + padding,
                    0.0,
                    1.0,
                    bar_h,
                    separator_color,
                ));
            }
            left_x += padding * 2.0;
        }

        // 3. Layout Mode
        if !self.layout.is_empty() {
            let label_str = self.layout.clone();
            let label = Label::new_with_family(&mut self.font_system, &label_str, font_size, normal_color, &font_family);
            let line_w = label.draw(&mut self.text_items, left_x, (bar_h - font_size * 1.4) / 2.0);
            eprintln!("[rebuild_layout] Layout Mode: '{}', x={}, y={}, w={}, h={}", self.layout, left_x, 0.0, line_w, bar_h);
            self.layout_bounds = Some(LayoutBounds {
                x: left_x,
                y: 0.0,
                w: line_w,
                h: bar_h,
            });
            left_x += line_w;
        }

        // Spacing/separator before Title
        if !self.title.is_empty() && self.title != "(none)" {
            if show_separators {
                self.separators.push(Separator::new(
                    left_x + padding,
                    0.0,
                    1.0,
                    bar_h,
                    separator_color,
                ));
            }
            left_x += padding * 2.0;
        }

        // 4. Focused Title (using StatusBar widget)
        if !self.title.is_empty() && self.title != "(none)" {
            let mut display_title = self.title.clone();
            if display_title.chars().count() > 40 {
                display_title = display_title.chars().take(37).collect::<String>() + "...";
            }
            self.status_bar.set_text_offset_x(left_x);
            self.status_bar.set_text_color(normal_color);
            self.status_bar.set_text(&display_title);
        } else {
            self.status_bar.set_text("");
        }

        // 5. Right Side Stats (CPU, Mem, Bat, Clock)
        let mut right_x = sw_logical - 12.0;
        if let Some(ref stats) = self.stats {
            let clock_w = 270.0;
            let battery_w = 80.0;
            let volume_w = 80.0;
            let brightness_w = 80.0;
            let memory_w = 140.0;
            let cpu_w = 90.0;

            // Clock
            let label = Label::new_with_family(&mut self.font_system, &stats.clock, font_size, normal_color, &font_family);
            right_x -= clock_w;
            let draw_x = right_x + (clock_w - label.w) / 2.0; // centered
            eprintln!("[rebuild_layout] Clock: x={}, w={}", draw_x, label.w);
            label.draw(&mut self.text_items, draw_x, (bar_h - font_size * 1.4) / 2.0);

            // Battery
            if !stats.battery.is_empty() {
                right_x -= padding * 2.0;
                if show_separators {
                    self.separators.push(Separator::new(
                        right_x + padding,
                        0.0,
                        1.0,
                        bar_h,
                        separator_color,
                    ));
                }
                let bat_color = if !stats.battery_charging && stats.battery_capacity > 10 {
                    normal_color
                } else {
                    color::TEXT_ACCENT
                };
                let label = Label::new_with_family(&mut self.font_system, &stats.battery, font_size, bat_color, &font_family);
                right_x -= battery_w;
                let draw_x = right_x + (battery_w - label.w) / 2.0; // centered
                eprintln!("[rebuild_layout] Battery: x={}, w={}", draw_x, label.w);
                label.draw(&mut self.text_items, draw_x, (bar_h - font_size * 1.4) / 2.0);
            }

            // Volume
            if !stats.volume.is_empty() {
                right_x -= padding * 2.0;
                if show_separators {
                    self.separators.push(Separator::new(
                        right_x + padding,
                        0.0,
                        1.0,
                        bar_h,
                        separator_color,
                    ));
                }
                let is_muted = stats.volume_muted;
                let color_val = if is_muted {
                    read_disabled_color_from_config().unwrap_or(color::TEXT_DIM)
                } else {
                    normal_color
                };
                let label = Label::new_with_family(&mut self.font_system, &stats.volume, font_size, color_val, &font_family)
                    .with_strikethrough(is_muted);
                right_x -= volume_w;
                let draw_x = right_x + (volume_w - label.w) / 2.0; // centered
                let start_y = (bar_h - font_size * 1.4) / 2.0;
                eprintln!("[rebuild_layout] Volume: x={}, w={}, is_muted={}", draw_x, label.w, is_muted);
                if let Some((sx, sy, sw_rect, sh_rect, scol)) = label.strikethrough_rect(draw_x, start_y, 1.0) {
                    eprintln!("[rebuild_layout] Strikethrough rect: sx={}, sy={}, sw={}, sh={}, scol={:?}", sx, sy, sw_rect, sh_rect, scol);
                    self.overlay_rects.push(RectWidget {
                        x: sx,
                        y: sy,
                        w: sw_rect,
                        h: sh_rect,
                        color: color::to_linear(scol),
                    });
                }
                label.draw(&mut self.text_items, draw_x, start_y);
            }

            // Brightness
            if !stats.brightness.is_empty() {
                right_x -= padding * 2.0;
                if show_separators {
                    self.separators.push(Separator::new(
                        right_x + padding,
                        0.0,
                        1.0,
                        bar_h,
                        separator_color,
                    ));
                }
                let label = Label::new_with_family(&mut self.font_system, &stats.brightness, font_size, normal_color, &font_family);
                right_x -= brightness_w;
                let draw_x = right_x + (brightness_w - label.w) / 2.0; // centered
                eprintln!("[rebuild_layout] Brightness: x={}, w={}", draw_x, label.w);
                label.draw(&mut self.text_items, draw_x, (bar_h - font_size * 1.4) / 2.0);
            }

            // Memory
            right_x -= padding * 2.0;
            if show_separators {
                self.separators.push(Separator::new(
                    right_x + padding,
                    0.0,
                    1.0,
                    bar_h,
                    separator_color,
                ));
            }
            let label = Label::new_with_family(&mut self.font_system, &stats.memory, font_size, normal_color, &font_family);
            right_x -= memory_w;
            let draw_x = right_x + (memory_w - label.w) / 2.0; // centered
            eprintln!("[rebuild_layout] Memory: x={}, w={}", draw_x, label.w);
            label.draw(&mut self.text_items, draw_x, (bar_h - font_size * 1.4) / 2.0);

            // CPU
            right_x -= padding * 2.0;
            if show_separators {
                self.separators.push(Separator::new(
                    right_x + padding,
                    0.0,
                    1.0,
                    bar_h,
                    separator_color,
                ));
            }
            let label = Label::new_with_family(&mut self.font_system, &stats.cpu, font_size, normal_color, &font_family);
            right_x -= cpu_w;
            let draw_x = right_x + (cpu_w - label.w) / 2.0; // centered
            eprintln!("[rebuild_layout] CPU: x={}, w={}", draw_x, label.w);
            label.draw(&mut self.text_items, draw_x, (bar_h - font_size * 1.4) / 2.0);
        }

        // 5b. System Tray Icons (render to the left of the CPU/stats block)
        self.tray_item_bounds.clear();
        if !self.tray_items.is_empty() {
            right_x -= padding * 2.0; // Separator padding
            if show_separators {
                self.separators.push(Separator::new(
                    right_x + padding,
                    0.0,
                    1.0,
                    bar_h,
                    separator_color,
                ));
            }
            let mut sorted_tray: Vec<&TrayItem> = self.tray_items.values().collect();
            sorted_tray.sort_by_key(|item| &item.id);

            right_x -= 8.0; // Right margin for tray block to match visual padding of other modules
            let len = sorted_tray.len();
            for (idx, item) in sorted_tray.iter().rev().enumerate() {
                let icon_size = 16.0;
                right_x -= icon_size;
                let x = right_x;
                let y = (bar_h - icon_size) / 2.0;

                // Record bounds for hit-testing
                eprintln!("[rebuild_layout] Tray icon bounds: id={}, x={}, y={}, w={}, h={}", item.id, x, y, icon_size, icon_size);
                self.tray_item_bounds.push(TrayIconBounds {
                    id: item.id.clone(),
                    x,
                    y,
                    w: icon_size,
                    h: icon_size,
                    title: item.title.clone(),
                });

                // Attempt to draw pixmap
                let mut drawn_pixmap = false;
                if let Some(ref pixmaps) = item.pixmaps {
                    if !pixmaps.is_empty() {
                        let target_pixel_width = (icon_size * self.scale_factor as f32) as i32;
                        if let Some(pixmap) = pixmaps.iter().min_by_key(|p| (p.width - target_pixel_width).abs()) {
                            if pixmap.width > 0 && pixmap.height > 0 {
                                // Calculate average brightness of visible pixels to see if we need to recolor
                                let mut total_brightness = 0.0;
                                let mut visible_pixel_count = 0;
                                for row in 0..pixmap.height {
                                    for col in 0..pixmap.width {
                                        let idx = ((row * pixmap.width + col) * 4) as usize;
                                        if idx + 3 < pixmap.pixels.len() {
                                            let a = pixmap.pixels[idx] as f32 / 255.0;
                                            if a > 0.1 {
                                                let r = pixmap.pixels[idx + 1] as f32 / 255.0;
                                                let g = pixmap.pixels[idx + 2] as f32 / 255.0;
                                                let b = pixmap.pixels[idx + 3] as f32 / 255.0;
                                                total_brightness += (r + g + b) / 3.0;
                                                visible_pixel_count += 1;
                                            }
                                        }
                                    }
                                }
                                
                                let avg_brightness = if visible_pixel_count > 0 {
                                    total_brightness / visible_pixel_count as f32
                                } else {
                                    0.5
                                };
                                let recolor_light = avg_brightness < 0.35;
 
                                let mut draw_w = pixmap.width;
                                let mut draw_h = pixmap.height;
                                if draw_w > 48 {
                                    draw_w = 48;
                                    draw_h = 48;
                                }
                                let pixel_w = icon_size / draw_w as f32;
                                let pixel_h = icon_size / draw_h as f32;
                                for row in 0..draw_h {
                                    for col in 0..draw_w {
                                        let src_row = row * pixmap.height / draw_h;
                                        let src_col = col * pixmap.width / draw_w;
                                        let idx = ((src_row * pixmap.width + src_col) * 4) as usize;
                                        if idx + 3 < pixmap.pixels.len() {
                                            let a = pixmap.pixels[idx] as f32 / 255.0;
                                            if a > 0.0 {
                                                let mut r = pixmap.pixels[idx + 1] as f32 / 255.0;
                                                let mut g = pixmap.pixels[idx + 2] as f32 / 255.0;
                                                let mut b = pixmap.pixels[idx + 3] as f32 / 255.0;
                                                
                                                if recolor_light {
                                                    let l = (r + g + b) / 3.0;
                                                    let new_l = 0.85 + (1.0 - 0.85) * l;
                                                    r = new_l;
                                                    g = new_l;
                                                    b = new_l;
                                                }
                                                
                                                self.rects.push(RectWidget {
                                                    x: x + col as f32 * pixel_w,
                                                    y: y + row as f32 * pixel_h,
                                                    w: pixel_w,
                                                    h: pixel_h,
                                                    color: [r, g, b, a],
                                                });
                                            }
                                        }
                                    }
                                }
                                drawn_pixmap = true;
                            }
                        }
                    }
                }

                if !drawn_pixmap {
                    let symbol = if let Some(ref name) = item.icon_name {
                        let name_lower = name.to_lowercase();
                        if name_lower.contains("volume") || name_lower.contains("sound") || name_lower.contains("audio") {
                            if name_lower.contains("mute") { "🔇" } else { "🔊" }
                        } else if name_lower.contains("wifi") || name_lower.contains("network") || name_lower.contains("ethernet") {
                            "📶"
                        } else if name_lower.contains("battery") {
                            "🔋"
                        } else if name_lower.contains("bluetooth") {
                            "ᛒ"
                        } else if name_lower.contains("mail") || name_lower.contains("envelope") {
                            "✉"
                        } else if name_lower.contains("chat") || name_lower.contains("messenger") || name_lower.contains("discord") || name_lower.contains("slack") || name_lower.contains("telegram") {
                            "💬"
                        } else if name_lower.contains("steam") || name_lower.contains("game") {
                            "🎮"
                        } else if name_lower.contains("dropbox") {
                            "📦"
                        } else {
                            "⚙"
                        }
                    } else {
                        "⚙"
                    };

                    let buf = make_text_buffer(&mut self.font_system, symbol, font_size, &font_family);
                    let tw = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
                    let tx = x + (icon_size - tw) / 2.0;
                    let ty = y + (icon_size - font_size * 1.4) / 2.0;
                    self.text_items.push(TextItem {
                        buffer: buf,
                        x: tx,
                        y: ty,
                        color: glyphon::Color::rgb(
                            (color::TEXT_ACCENT[0] * 255.0) as u8,
                            (color::TEXT_ACCENT[1] * 255.0) as u8,
                            (color::TEXT_ACCENT[2] * 255.0) as u8,
                        ),
                        bounds: None,
                    });
                }

                if idx < len - 1 {
                    right_x -= 8.0; // Gap between icons
                }
            }
            right_x -= 8.0; // Left margin for tray block to match visual padding of other modules
        }

        // 5c. Tooltip Rendering (if hovered)
        if let Some(ref hovered_id) = self.hovered_tray_item {
            if let Some(bound) = self.tray_item_bounds.iter().find(|b| &b.id == hovered_id) {
                let tooltip_text = bound.title.as_deref().unwrap_or(bound.id.as_str());
                let tooltip_font_size = font_size - 1.0;
                let buf = make_text_buffer(&mut self.font_system, tooltip_text, tooltip_font_size, &font_family);
                let text_w = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0);
                let padding = 6.0;

                let tooltip_w = text_w + padding * 2.0;
                let tooltip_w_h = tooltip_font_size * 1.4 + padding * 2.0;
                let tx = bound.x + (bound.w - tooltip_w) / 2.0;
                let ty = bar_h + 4.0;

                // Tooltip background
                self.rects.push(RectWidget {
                    x: tx,
                    y: ty,
                    w: tooltip_w,
                    h: tooltip_w_h,
                    color: [0.08, 0.08, 0.12, 0.95],
                });

                // Tooltip text
                self.text_items.push(TextItem {
                    buffer: buf,
                    x: tx + padding,
                    y: ty + padding,
                    color: glyphon::Color::rgb(
                        (color::TEXT_FG[0] * 255.0) as u8,
                        (color::TEXT_FG[1] * 255.0) as u8,
                        (color::TEXT_FG[2] * 255.0) as u8,
                    ),
                    bounds: None,
                });
            }
        }

        self.needs_rebuild = false;
    }
}

fn get_active_tag_index() -> u32 {
    let path = if let Ok(display) = std::env::var("WAYLAND_DISPLAY") {
        format!("/tmp/ccec-tags-{}", display)
    } else {
        "/tmp/ccec-tags".to_string()
    };
    if let Ok(content) = std::fs::read_to_string(path) {
        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.len() >= 2 {
            if let Ok(focused_tags) = parts[1].parse::<u32>() {
                if focused_tags > 0 {
                    return focused_tags.trailing_zeros() + 1;
                }
            }
        }
    }
    1
}

impl clear_ui::engine::Application for StatusApp {
    type Message = CustomEvent;

    fn new(_qh: &wayland_client::QueueHandle<clear_ui::engine::EngineState<Self>>, sender: calloop::channel::Sender<Self::Message>) -> Self {
        let sender_tags = sender.clone();
        let sender_layout = sender.clone();
        let sender_title = sender.clone();
        let sender_stats = sender.clone();
        let sender_tray = sender.clone();

        tokio::spawn(spawn_status_listener("tags", sender_tags));
        tokio::spawn(spawn_status_listener("layout", sender_layout));
        tokio::spawn(spawn_status_listener("title", sender_title));
        tokio::spawn(spawn_system_stats(sender_stats));
        tokio::spawn(spawn_status_tray(sender_tray));

        let font_system = FontSystem::new();

        let mut app = Self {
            tags: String::new(),
            layout: String::new(),
            title: String::new(),
            stats: None,
            tray_items: HashMap::new(),
            cursor_pos: (0.0, 0.0),
            hovered_tray_item: None,
            tray_item_bounds: Vec::new(),
            tag_bounds: Vec::new(),
            layout_bounds: None,
            layout_menu_pid: None,
            font_system,
            status_bar: clear_ui::widget::StatusBar::new(),
            rects: Vec::new(),
            overlay_rects: Vec::new(),
            separators: Vec::new(),
            text_items: Vec::new(),
            scale_factor: 1.0,
            width: 1920,
            height: read_status_height_from_config() as u32,
            needs_rebuild: true,
            current_bg_color: color::STATUS_BG,
            sender,
        };

        app.rebuild_layout();
        app
    }

    fn settings(&self) -> clear_ui::engine::WindowSettings {
        clear_ui::engine::WindowSettings {
            title: "Clear Status Interface".to_string(),
            app_id: "clear-status-interface".to_string(),
            width: 1920,
            height: read_status_height_from_config() as u32,
            fullscreen: true,
            min_size: None,
        }
    }

    fn update(&mut self, msg: Self::Message, needs_rebuild: &mut bool, _exit: &mut bool) {
        match msg {
            CustomEvent::TagsUpdated(t) => {
                self.tags = t;
            }
            CustomEvent::LayoutUpdated(l) => {
                self.layout = l;
            }
            CustomEvent::TitleUpdated(t) => {
                self.title = t;
            }
            CustomEvent::SystemStatsUpdated(s) => {
                self.stats = Some(s);
            }
            CustomEvent::TrayUpdated(item) => {
                self.tray_items.insert(item.id.clone(), item);
            }
            CustomEvent::TrayRemoved(id) => {
                self.tray_items.remove(&id);
            }
            CustomEvent::LayoutMenuClosed(pid) => {
                if self.layout_menu_pid == Some(pid) {
                    eprintln!("[layout-click] CustomEvent: clear-cloud (PID {}) closed, clearing tracking PID", pid);
                    self.layout_menu_pid = None;
                }
            }
        }
        self.needs_rebuild = true;
        *needs_rebuild = true;
    }

    fn tick(&mut self, _dt: f32, _needs_rebuild: &mut bool) {
        self.status_bar.prepare_text(&mut self.font_system);
    }

    fn view(&mut self, quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>, size: clear_ui::engine::LogicalSize, scale: f64) {
        if self.needs_rebuild || self.width != size.width as u32 || self.height != size.height as u32 || self.scale_factor != scale {
            self.width = size.width as u32;
            self.height = size.height as u32;
            self.scale_factor = scale;
            self.rebuild_layout();
        }
        let (sb_x, sb_y, sb_w, sb_h) = self.status_bar.rect();
        quads.push((sb_x, sb_y, sb_w, sb_h, self.status_bar.color()));
        for r in &self.rects {
            quads.push((r.x, r.y, r.w, r.h, r.color));
        }
        for sep in &self.separators {
            let (x, y, w, h) = sep.rect();
            quads.push((x, y, w, h, sep.color()));
        }
    }

    fn overlay_quads(&mut self, quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>, _size: clear_ui::engine::LogicalSize, _scale: f64) {
        for r in &self.overlay_rects {
            quads.push((r.x, r.y, r.w, r.h, r.color));
        }
    }

    fn text_items(&self) -> &[TextItem] {
        &self.text_items
    }

    fn text_areas(&self, scale_f32: f32, bounds: TextBounds) -> Vec<TextArea<'_>> {
        let mut areas = self.text_items().iter().map(|ti| TextArea {
            buffer: &ti.buffer,
            left: ti.x * scale_f32,
            top: ti.y * scale_f32,
            scale: scale_f32,
            bounds,
            default_color: ti.color,
            custom_glyphs: &[],
        }).collect::<Vec<_>>();

        for (buf, x, y, col) in self.status_bar.get_text_items() {
            areas.push(TextArea {
                buffer: buf,
                left: x * scale_f32,
                top: y * scale_f32,
                scale: scale_f32,
                bounds,
                default_color: col,
                custom_glyphs: &[],
            });
        }

        areas
    }

    fn clear_color(&self) -> [f32; 4] {
        let mut color = [
            self.current_bg_color[0].powf(1.0 / 2.2),
            self.current_bg_color[1].powf(1.0 / 2.2),
            self.current_bg_color[2].powf(1.0 / 2.2),
            self.current_bg_color[3],
        ];
        if let Some(opacity) = clear_ui::color::read_opacity_if_configured() {
            color[3] = opacity;
        }
        color
    }

    fn handle_pointer_move(&mut self, pos: clear_ui::engine::LogicalPosition, needs_rebuild: &mut bool) {
        let (lx, ly) = (pos.x, pos.y);
        self.cursor_pos = (lx as f64, ly as f64);
        let mut newly_hovered = None;
        for bound in &self.tray_item_bounds {
            if lx >= bound.x && lx <= (bound.x + bound.w)
                && ly >= bound.y && ly <= (bound.y + bound.h) {
                newly_hovered = Some(bound.id.clone());
                break;
            }
        }
        if self.hovered_tray_item != newly_hovered {
            self.hovered_tray_item = newly_hovered;
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }

    fn handle_mouse_input(&mut self, button: MouseButton, state: ElementState, pos: clear_ui::engine::LogicalPosition, _needs_rebuild: &mut bool) -> Option<Self::Message> {
        if state == ElementState::Pressed {
            let (lx, ly) = (pos.x, pos.y);
            let cx = lx as f64;
            let cy = ly as f64;

            // Check if tray icon was clicked
            let mut clicked_tray = None;
            for bound in &self.tray_item_bounds {
                if cx >= bound.x as f64 && cx <= (bound.x + bound.w) as f64
                    && cy >= bound.y as f64 && cy <= (bound.y + bound.h) as f64 {
                    clicked_tray = Some(bound.id.clone());
                    break;
                }
            }

            if let Some(id) = clicked_tray {
                let btn_code = match button {
                    MouseButton::Left => 272,
                    MouseButton::Right => 273,
                    _ => 0,
                };
                let cx_i = cx as i32;
                let cy_i = cy as i32;
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    rt.block_on(async move {
                        if let Some((destination, path_part)) = id.split_once('/') {
                            let path = format!("/{}", path_part);
                            if let Ok(conn) = zbus::Connection::session().await {
                                if let Ok(proxy) = StatusNotifierItemProxy::builder(&conn)
                                    .destination(destination.to_string())
                                    .unwrap()
                                    .path(path)
                                    .unwrap()
                                    .build()
                                    .await
                                {
                                    let is_menu = proxy.item_is_menu().await.unwrap_or(false);
                                    let menu_path = proxy.menu().await.ok();

                                    let should_show_menu = (btn_code == 273 && menu_path.is_some())
                                        || (btn_code == 272 && is_menu && menu_path.is_some());

                                    if should_show_menu {
                                        if let Some(menu_p) = menu_path {
                                            eprintln!("[tray-click] Displaying menu for {} at path {}", id, menu_p.as_str());
                                            if let Err(e) = show_clear_cloud_menu(&conn, destination, menu_p.as_str()).await {
                                                eprintln!("[tray-click] show_clear_cloud_menu failed: {:?}", e);
                                            }
                                        }
                                    } else if btn_code == 272 {
                                        eprintln!("[tray-click] Calling Activate on {} at ({}, {})", id, cx_i, cy_i);
                                        if let Err(e) = proxy.activate(cx_i, cy_i).await {
                                            eprintln!("[tray-click] Activate failed: {:?}", e);
                                            if let Some(menu_p) = menu_path {
                                                eprintln!("[tray-click] Fallback: Displaying menu for {}", id);
                                                if let Err(e) = show_clear_cloud_menu(&conn, destination, menu_p.as_str()).await {
                                                    eprintln!("[tray-click] Fallback show_clear_cloud_menu failed: {:?}", e);
                                                }
                                            }
                                        }
                                    } else if btn_code == 273 {
                                        eprintln!("[tray-click] Calling ContextMenu on {} at ({}, {})", id, cx_i, cy_i);
                                        let _ = proxy.context_menu(cx_i, cy_i).await;
                                    }
                                }
                            }
                        }
                    });
                });
                return None;
            }

            if button == MouseButton::Left {
                eprintln!("[tags-click] Mouse left click at logical: ({}, {})", cx, cy);
                
                // Check if layout mode was clicked
                let mut clicked_layout = false;
                if let Some(ref bounds) = self.layout_bounds {
                    if cx >= bounds.x as f64 && cx <= (bounds.x + bounds.w) as f64
                        && cy >= bounds.y as f64 && cy <= (bounds.y + bounds.h) as f64 {
                        clicked_layout = true;
                    }
                }

                if clicked_layout {
                    eprintln!("[layout-click] Layout mode clicked!");
                    
                    // Check if there is an existing, running clear-cloud instance
                    let mut menu_already_running = false;
                    if let Some(pid) = self.layout_menu_pid {
                        if std::path::Path::new(&format!("/proc/{}", pid)).exists() {
                            if let Ok(comm) = std::fs::read_to_string(format!("/proc/{}/comm", pid)) {
                                if comm.trim() == "clear-cloud" {
                                    menu_already_running = true;
                                }
                            }
                        }
                    }

                    if menu_already_running {
                        if let Some(pid) = self.layout_menu_pid {
                            eprintln!("[layout-click] clear-cloud (PID {}) is already running, killing it to close the menu", pid);
                            let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
                        }
                        self.layout_menu_pid = None;
                    } else {
                        let x_pos = self.layout_bounds.as_ref().map(|b| b.x as i32).unwrap_or(0);
                        let y_pos = self.layout_bounds.as_ref().map(|b| b.h as i32).unwrap_or_else(|| read_status_height_from_config() as i32);
                        
                        // Spawn the child on the main thread so we can capture its PID
                        let layout_json = serde_json::json!({
                            "width": 240,
                            "height": 320,
                            "widgets": [
                                { "type": "label", "text": "Window Mode" },
                                { "id": "apply_all", "type": "checkbox", "text": "Apply to all sharing mode", "checked": false },
                                { "id": "cascade", "type": "button", "text": "Cascade" },
                                { "id": "grid", "type": "button", "text": "Grid" },
                                { "id": "fullscreen", "type": "button", "text": "Fullscreen" },
                                { "id": "floating", "type": "button", "text": "Floating" },
                                { "id": "popup", "type": "button", "text": "Popup" }
                            ]
                        }).to_string();

                        if let Ok(mut child) = std::process::Command::new("clear-cloud")
                            .args([
                                "--json",
                                "-x",
                                &x_pos.to_string(),
                                "-y",
                                &y_pos.to_string(),
                            ])
                            .stdin(std::process::Stdio::piped())
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .spawn()
                        {
                            let pid = child.id();
                            self.layout_menu_pid = Some(pid);
                            eprintln!("[layout-click] Spawned clear-cloud with PID {}", pid);
                            
                            let thread_sender = self.sender.clone();
                            std::thread::spawn(move || {
                                let active_tag = get_active_tag_index();
                                eprintln!("[layout-click] Active tag is {}", active_tag);
                                if let Some(mut stdin) = child.stdin.take() {
                                    use std::io::Write;
                                    let _ = stdin.write_all(layout_json.as_bytes());
                                }
                                if let Ok(output) = child.wait_with_output() {
                                    let err_str = String::from_utf8_lossy(&output.stderr);
                                    if !err_str.is_empty() {
                                        eprintln!("[clear-cloud stderr] {}", err_str);
                                    }
                                    if output.status.success() {
                                        let out_str = String::from_utf8_lossy(&output.stdout);
                                        #[derive(serde::Deserialize)]
                                        struct LayoutMenuOutput {
                                            button: String,
                                            checkboxes: std::collections::HashMap<String, bool>,
                                        }
                                        if let Ok(val) = serde_json::from_str::<LayoutMenuOutput>(out_str.trim()) {
                                            let selected_mode = val.button.to_lowercase();
                                            let apply_all = val.checkboxes.get("apply_all").copied().unwrap_or(false);
                                            if apply_all {
                                                eprintln!("[layout-click] Selected mode: {}, applying to all windows sharing mode", selected_mode);
                                                let _ = std::process::Command::new("clearctl")
                                                    .args(["apply-mode-sharing", &selected_mode])
                                                    .spawn();
                                            } else {
                                                eprintln!("[layout-click] Selected mode: {}, setting for tag {}", selected_mode, active_tag);
                                                let _ = std::process::Command::new("clearctl")
                                                    .args(["tag-layout", &active_tag.to_string(), &selected_mode])
                                                    .spawn();
                                            }
                                        } else {
                                            // Fallback
                                            let selected = out_str.trim().to_string();
                                            if !selected.is_empty() {
                                                let selected_lower = selected.to_lowercase();
                                                let _ = std::process::Command::new("clearctl")
                                                    .args(["tag-layout", &active_tag.to_string(), &selected_lower])
                                                    .spawn();
                                            }
                                        }
                                    }
                                }
                                let _ = thread_sender.send(CustomEvent::LayoutMenuClosed(pid));
                            });
                        }
                    }
                } else {
                    for bound in &self.tag_bounds {
                        eprintln!("[tags-click] Checking Tag '{}' bounds: x=[{}..{}], y=[{}..{}]", 
                            bound.name, bound.x, bound.x + bound.w, bound.y, bound.y + bound.h);
                        if cx >= bound.x as f64 && cx <= (bound.x + bound.w) as f64
                            && cy >= bound.y as f64 && cy <= (bound.y + bound.h) as f64 {
                            eprintln!("[tags-click] Tag matched: {}", bound.name);
                            let name = bound.name.clone();
                            std::thread::spawn(move || {
                                let _ = std::process::Command::new("clearctl")
                                    .args(["view", &name])
                                    .spawn();
                            });
                            break;
                        }
                    }
                }
            }
        }
        None
    }

    fn handle_mouse_wheel(&mut self, _delta: &MouseScrollDelta, _pos: clear_ui::engine::LogicalPosition, _needs_rebuild: &mut bool) {}

    fn handle_key_input(&mut self, _event: &KeyEvent, _needs_rebuild: &mut bool) -> Option<Self::Message> { None }
}

async fn spawn_status_listener(sub: &'static str, sender: calloop::channel::Sender<CustomEvent>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    loop {
        let socket_path = match std::env::var("WAYLAND_DISPLAY") {
            Ok(display) => format!("/tmp/ccec-status-{}.sock", display),
            Err(_) => "/tmp/ccec-status.sock".to_string(),
        };
        if let Ok(mut stream) = UnixStream::connect(&socket_path).await {
            eprintln!("[status-listener] connected to {} for sub '{}'", socket_path, sub);
            if stream.write_all(format!("{}\n", sub).as_bytes()).await.is_ok() {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
                    let val = line.trim().to_string();
                    eprintln!("[status-listener] received '{}' update: '{}'", sub, val);
                    if !val.is_empty() {
                        let ev = match sub {
                            "tags" => CustomEvent::TagsUpdated(val.clone()),
                            "layout" => CustomEvent::LayoutUpdated(val.clone()),
                            "title" => CustomEvent::TitleUpdated(val.clone()),
                            _ => unreachable!(),
                        };
                        let _ = sender.send(ev);
                    }
                    line.clear();
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

async fn read_volume() -> Option<(String, bool)> {
    let vol_output = match tokio::process::Command::new("pactl")
        .args(["get-sink-volume", "@DEFAULT_SINK@"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[read_volume] failed to spawn pactl: {:?}", e);
            return None;
        }
    };
    if !vol_output.status.success() {
        eprintln!("[read_volume] pactl get-sink-volume exited with error: {:?}", String::from_utf8_lossy(&vol_output.stderr));
        return None;
    }
    let vol_str = String::from_utf8_lossy(&vol_output.stdout);
    
    let mute_output = match tokio::process::Command::new("pactl")
        .args(["get-sink-mute", "@DEFAULT_SINK@"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[read_volume] failed to spawn pactl mute: {:?}", e);
            return None;
        }
    };
    if !mute_output.status.success() {
        eprintln!("[read_volume] pactl get-sink-mute exited with error: {:?}", String::from_utf8_lossy(&mute_output.stderr));
        return None;
    }
    let mute_str = String::from_utf8_lossy(&mute_output.stdout);
    let muted = mute_str.contains("yes");

    let mut pct = None;
    if let Some(pos) = vol_str.find('%') {
        let start = vol_str[..pos].rfind(|c: char| !c.is_ascii_digit()).map(|i| i + 1).unwrap_or(0);
        if let Ok(num) = vol_str[start..pos].parse::<u32>() {
            pct = Some(num);
        }
    }

    match (muted, pct) {
        (true, Some(p)) => Some((format!("Vol {}%", p), true)),
        (true, None) => Some(("Vol Muted".to_string(), true)),
        (false, Some(p)) => Some((format!("Vol {}%", p), false)),
        (false, None) => Some(("Vol N/A".to_string(), false)),
    }
}

async fn spawn_system_stats(sender: calloop::channel::Sender<CustomEvent>) {
    eprintln!("[spawn_system_stats] Starting system stats loop!");
    let mut last_cpu = read_cpu_ticks().unwrap_or((0, 0));
    loop {
        eprintln!("[spawn_system_stats] loop iteration start");
        let clock = chrono::Local::now().format("%A, %B %d, %Y %I:%M %p").to_string();
        let memory = read_memory_usage().unwrap_or_else(|| "Mem N/A".to_string());
        
        let cpu_str = if let Some(current_cpu) = read_cpu_ticks() {
            let total_diff = current_cpu.0 - last_cpu.0;
            let idle_diff = current_cpu.1 - last_cpu.1;
            last_cpu = current_cpu;
            if total_diff > 0 {
                let usage = 100.0 - (idle_diff as f32 * 100.0 / total_diff as f32);
                format!("Cpu {:.1}%", usage)
            } else {
                "Cpu 0.0%".to_string()
            }
        } else {
            "Cpu N/A".to_string()
        };

        let (battery_str, battery_capacity, battery_charging) = if let Some((s, cap, chg)) = read_battery_details() {
            (s, cap, chg)
        } else {
            ("".to_string(), 0, false)
        };
        let (volume, volume_muted) = read_volume().await.unwrap_or_else(|| ("".to_string(), false));
        let brightness = read_brightness().unwrap_or_default();

        let stats = SystemStats {
            clock,
            memory,
            cpu: cpu_str,
            battery: battery_str,
            battery_capacity,
            battery_charging,
            volume,
            volume_muted,
            brightness,
        };
        eprintln!("[spawn_system_stats] stats: {:?}", stats);
        let _ = sender.send(CustomEvent::SystemStatsUpdated(stats));
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

#[derive(Debug, Clone)]
pub struct NotifierAddress {
    pub destination: String,
    pub path: String,
}

impl NotifierAddress {
    pub fn from_notifier_service(service: &str, sender: &str) -> Result<Self, String> {
        if service.starts_with('/') {
            Ok(NotifierAddress {
                destination: sender.to_string(),
                path: service.to_string(),
            })
        } else if let Some((destination, path)) = service.split_once('/') {
            Ok(NotifierAddress {
                destination: destination.to_string(),
                path: format!("/{}", path),
            })
        } else if service.contains(':') {
            let split = service.split(':').collect::<Vec<&str>>();
            Ok(NotifierAddress {
                destination: format!(":{}", split[1]),
                path: "/StatusNotifierItem".to_string(),
            })
        } else {
            Ok(NotifierAddress {
                destination: service.to_string(),
                path: "/StatusNotifierItem".to_string(),
            })
        }
    }
}

#[zbus::proxy(
    interface = "org.kde.StatusNotifierItem",
    default_path = "/StatusNotifierItem"
)]
trait StatusNotifierItem {
    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn category(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn title(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_name(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_theme_path(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn icon_pixmap(&self) -> zbus::Result<Vec<(i32, i32, Vec<u8>)>>;

    #[zbus(signal)]
    fn new_icon(&self) -> zbus::Result<()>;
    #[zbus(signal)]
    fn new_title(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn new_status(&self) -> zbus::Result<()>;

    fn activate(&self, x: i32, y: i32) -> zbus::Result<()>;
    fn context_menu(&self, x: i32, y: i32) -> zbus::Result<()>;

    #[zbus(property)]
    fn item_is_menu(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn menu(&self) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "com.canonical.dbusmenu",
    default_path = "/StatusNotifierItem/menu"
)]
trait DBusMenu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> zbus::Result<(u32, (i32, std::collections::HashMap<String, zbus::zvariant::OwnedValue>, Vec<zbus::zvariant::OwnedValue>))>;

    fn event(
        &self,
        id: i32,
        event_id: &str,
        data: &zbus::zvariant::Value<'_>,
        timestamp: u32,
    ) -> zbus::Result<()>;

    fn about_to_show(&self, id: i32) -> zbus::Result<bool>;
}

fn flatten_menu(
    id: i32,
    mut properties: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    children: Vec<zbus::zvariant::OwnedValue>,
    prefix: &str,
    out: &mut Vec<(i32, String)>
) {
    let label: String = properties.remove("label")
        .and_then(|v| {
            let s: Result<String, _> = v.try_into();
            s.ok()
        })
        .unwrap_or_default();
    let type_: String = properties.remove("type")
        .and_then(|v| {
            let s: Result<String, _> = v.try_into();
            s.ok()
        })
        .unwrap_or_default();
    let enabled: bool = properties.remove("enabled")
        .and_then(|v| {
            let b: Result<bool, _> = v.try_into();
            b.ok()
        })
        .unwrap_or(true);

    if type_ == "separator" || !enabled {
        // Skip
    } else {
        let current_path = if prefix.is_empty() {
            label.clone()
        } else if !label.is_empty() {
            format!("{} > {}", prefix, label)
        } else {
            prefix.to_string()
        };

        if !current_path.is_empty() && children.is_empty() {
            out.push((id, current_path.clone()));
        }

        for child_val in children {
            let child_val_inner = zbus::zvariant::Value::from(child_val);
            if let Ok(child) = <(i32, std::collections::HashMap<String, zbus::zvariant::OwnedValue>, Vec<zbus::zvariant::OwnedValue>)>::try_from(child_val_inner) {
                flatten_menu(child.0, child.1, child.2, &current_path, out);
            }
        }
    }
}

async fn show_clear_cloud_menu(conn: &zbus::Connection, destination: &str, menu_path: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let menu_proxy = DBusMenuProxy::builder(conn)
        .destination(destination)?
        .path(menu_path)?
        .build()
        .await?;

    let _ = menu_proxy.about_to_show(0).await;
    let (_, layout) = menu_proxy.get_layout(0, 3, vec![]).await?;
    eprintln!("[tray-click] show_clear_cloud_menu layout: root_id={}, properties={:?}, children_len={}", layout.0, layout.1, layout.2.len());

    let mut items = Vec::new();
    flatten_menu(layout.0, layout.1, layout.2, "", &mut items);
    eprintln!("[tray-click] show_clear_cloud_menu flattened items: {:?}", items);

    if items.is_empty() {
        eprintln!("[tray-click] show_clear_cloud_menu: items is empty, returning early");
        return Ok(());
    }

    let mut clear_cloud_input = String::new();
    for (_, label) in &items {
        clear_cloud_input.push_str(label);
        clear_cloud_input.push('\n');
    }

    let mut child = std::process::Command::new("clear-cloud")
        .args(["--dmenu", "-p", "Tray Menu:"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(clear_cloud_input.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if output.status.success() {
        let selected = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if let Some((id, _)) = items.iter().find(|(_, label)| label == &selected) {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as u32;
            let val = zbus::zvariant::Value::from("");
            let _ = menu_proxy.event(*id, "clicked", &val, timestamp).await;
        }
    } else {
        let stderr_str = String::from_utf8_lossy(&output.stderr).trim().to_string();
        eprintln!("[tray-click] clear-cloud failed with status: {:?}, stderr: {:?}", output.status, stderr_str);
    }
    Ok(())
}


fn find_icon_file(dir: &std::path::Path, icon_name: &str) -> Option<std::path::PathBuf> {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            if let Ok(file_type) = entry.file_type() {
                let path = entry.path();
                if file_type.is_dir() {
                    if !file_type.is_symlink() {
                        if let Some(found) = find_icon_file(&path, icon_name) {
                            return Some(found);
                        }
                    }
                } else if file_type.is_file() {
                    if let Some(file_name) = path.file_name().and_then(|f| f.to_str()) {
                        if file_name == format!("{}.png", icon_name) || file_name == format!("{}.svg", icon_name) {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }
    None
}

fn load_png_as_pixmap(path: &std::path::Path) -> Option<TrayPixmap> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    
    let width = info.width as i32;
    let height = info.height as i32;
    println!("[status-tray] PNG info: width={}, height={}, color_type={:?}, buffer_size={}", width, height, info.color_type, info.buffer_size());
    let mut argb_pixels = Vec::with_capacity((width * height * 4) as usize);
    
    let actual_bytes = &buf[..info.buffer_size()];
    let mut printed = 0;
    match info.color_type {
        png::ColorType::Rgba => {
            for (i, chunk) in actual_bytes.chunks_exact(4).enumerate() {
                if chunk[3] > 10 && printed < 5 {
                    println!("[status-tray] Pixel {}: original RGBA=[{}, {}, {}, {}]", i, chunk[0], chunk[1], chunk[2], chunk[3]);
                    printed += 1;
                }
                argb_pixels.push(chunk[3]); // A
                argb_pixels.push(chunk[0]); // R
                argb_pixels.push(chunk[1]); // G
                argb_pixels.push(chunk[2]); // B
            }
        }
        png::ColorType::Rgb => {
            for chunk in actual_bytes.chunks_exact(3) {
                argb_pixels.push(255);      // A
                argb_pixels.push(chunk[0]); // R
                argb_pixels.push(chunk[1]); // G
                argb_pixels.push(chunk[2]); // B
            }
        }
        png::ColorType::Grayscale => {
            for &g in actual_bytes {
                argb_pixels.push(255); // A
                argb_pixels.push(g);   // R
                argb_pixels.push(g);   // G
                argb_pixels.push(g);   // B
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for chunk in actual_bytes.chunks_exact(2) {
                argb_pixels.push(chunk[1]); // A
                argb_pixels.push(chunk[0]); // R
                argb_pixels.push(chunk[0]); // G
                argb_pixels.push(chunk[0]); // B
            }
        }
        _ => return None,
    }
    
    Some(TrayPixmap {
        width,
        height,
        pixels: argb_pixels,
    })
}

fn load_svg_as_pixmap(path: &std::path::Path) -> Option<TrayPixmap> {
    let svg_data = std::fs::read(path).ok()?;
    let opt = resvg::usvg::Options::default();
    let fontdb = resvg::usvg::fontdb::Database::new();
    let tree = resvg::usvg::Tree::from_data(&svg_data, &opt, &fontdb).ok()?;
    
    let target_w = 48;
    let target_h = 48;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(target_w, target_h)?;
    
    let orig_w = tree.size().width();
    let orig_h = tree.size().height();
    let sx = target_w as f32 / orig_w;
    let sy = target_h as f32 / orig_h;
    let transform = resvg::tiny_skia::Transform::from_scale(sx, sy);
    
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    
    let raw_pixels = pixmap.data();
    let mut argb_pixels = Vec::with_capacity((target_w * target_h * 4) as usize);
    for chunk in raw_pixels.chunks_exact(4) {
        argb_pixels.push(chunk[3]); // A
        argb_pixels.push(chunk[0]); // R
        argb_pixels.push(chunk[1]); // G
        argb_pixels.push(chunk[2]); // B
    }
    
    Some(TrayPixmap {
        width: target_w as i32,
        height: target_h as i32,
        pixels: argb_pixels,
    })
}

fn resolve_icon_path(theme_path: Option<&str>, icon_name: &str) -> Option<std::path::PathBuf> {
    if icon_name.is_empty() {
        return None;
    }

    let icon_name = if icon_name == "dropbox" { "dropboxstatus-idle" } else { icon_name };

    if let Some(path_str) = theme_path {
        if !path_str.is_empty() {
            let path = std::path::Path::new(path_str);
            if path.exists() {
                if let Some(found) = find_icon_file(path, icon_name) {
                    return Some(found);
                }
            }
        }
    }

    let mut search_dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        search_dirs.push(format!("{}/.local/share/icons", home));
        search_dirs.push(format!("{}/.icons", home));
    }
    search_dirs.push("/usr/share/icons".to_string());
    search_dirs.push("/usr/share/pixmaps".to_string());

    let sub_paths = [
        "hicolor/16x16/status",
        "hicolor/22x22/status",
        "hicolor/24x24/status",
        "hicolor/32x32/status",
        "hicolor/48x48/status",
        "hicolor/scalable/status",
        "hicolor/16x16/apps",
        "hicolor/22x22/apps",
        "hicolor/24x24/apps",
        "hicolor/32x32/apps",
        "hicolor/48x48/apps",
        "hicolor/scalable/apps",
        "gnome/16x16/status",
        "gnome/22x22/status",
        "gnome/24x24/status",
        "gnome/32x32/status",
        "gnome/48x48/status",
        "gnome/scalable/status",
        "gnome/16x16/apps",
        "gnome/22x22/apps",
        "gnome/24x24/apps",
        "gnome/32x32/apps",
        "gnome/48x48/apps",
        "gnome/scalable/apps",
    ];

    for base in &search_dirs {
        for sub in &sub_paths {
            let path_png = std::path::Path::new(base).join(sub).join(format!("{}.png", icon_name));
            if path_png.exists() && path_png.is_file() {
                return Some(path_png);
            }
            let path_svg = std::path::Path::new(base).join(sub).join(format!("{}.svg", icon_name));
            if path_svg.exists() && path_svg.is_file() {
                return Some(path_svg);
            }
        }
        let base_path = std::path::Path::new(base);
        if base_path.exists() {
            if let Some(found) = find_icon_file(base_path, icon_name) {
                return Some(found);
            }
        }
    }

    None
}

async fn fetch_tray_item(conn: &zbus::Connection, addr: &NotifierAddress) -> Result<TrayItem, zbus::Error> {
    let proxy = StatusNotifierItemProxy::builder(conn)
        .destination(addr.destination.clone())?
        .path(addr.path.clone())?
        .build()
        .await?;

    let id = format!("{}/{}", addr.destination, addr.path.trim_start_matches('/'));
    let icon_name = proxy.icon_name().await.ok();
    let icon_theme_path = proxy.icon_theme_path().await.ok();
    let title = proxy.title().await.ok();

    let mut pixmaps = proxy.icon_pixmap().await.ok().and_then(|v| {
        if v.is_empty() || (v.len() == 1 && v[0].0 == 0 && v[0].1 == 0) {
            None
        } else {
            Some(v.into_iter()
                .map(|(w, h, pixels)| TrayPixmap {
                    width: w,
                    height: h,
                    pixels,
                })
                .collect::<Vec<_>>())
        }
    });

    if pixmaps.is_none() {
        if let Some(ref name) = icon_name {
            println!("[status-tray] Trying to resolve theme icon for '{}' (theme path: {:?})", name, icon_theme_path);
            if let Some(icon_path) = resolve_icon_path(icon_theme_path.as_deref(), name) {
                println!("[status-tray] Found icon file at {:?}", icon_path);
                let ext = icon_path.extension().and_then(|e| e.to_str()).unwrap_or("");
                let pixmap = if ext.eq_ignore_ascii_case("svg") {
                    load_svg_as_pixmap(&icon_path)
                } else {
                    load_png_as_pixmap(&icon_path)
                };
                if let Some(pixmap) = pixmap {
                    println!("[status-tray] Successfully decoded icon file to pixmap (size: {}x{})", pixmap.width, pixmap.height);
                    pixmaps = Some(vec![pixmap]);
                } else {
                    println!("[status-tray] Failed to decode icon file");
                }
            } else {
                println!("[status-tray] Could not find icon file on system or theme path");
            }
        }
    } else {
        println!("[status-tray] Loaded raw D-Bus pixmap for '{}'", id);
    }

    Ok(TrayItem {
        id,
        icon_name,
        icon_theme_path,
        pixmaps,
        title,
    })
}

struct Watcher {
    registered_items: Arc<tokio::sync::Mutex<HashMap<String, NotifierAddress>>>,
    sender: calloop::channel::Sender<CustomEvent>,
    tokio_handle: tokio::runtime::Handle,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: zbus::MessageHeader<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) {
        let sender = header
            .sender()
            .map(|s| s.to_string())
            .unwrap_or_else(|| service.to_string());
        
        if let Ok(addr) = NotifierAddress::from_notifier_service(service, &sender) {
            let mut items = self.registered_items.lock().await;
            let full_address = format!("{}/{}", addr.destination, addr.path.trim_start_matches('/'));
            if !items.contains_key(&full_address) {
                items.insert(full_address.clone(), addr.clone());
                
                let conn = conn.clone();
                let addr_clone = addr.clone();
                let sender_clone = self.sender.clone();
                
                self.tokio_handle.spawn(async move {
                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                        let _ = sender_clone.send(CustomEvent::TrayUpdated(item));
                    }
                    
                    // Listen for updates
                    if let Ok(proxy) = StatusNotifierItemProxy::builder(&conn)
                        .destination(addr_clone.destination.clone())
                        .unwrap()
                        .path(addr_clone.path.clone())
                        .unwrap()
                        .build()
                        .await
                    {
                        let mut new_icon_stream = proxy.receive_new_icon().await.ok();
                        let mut new_title_stream = proxy.receive_new_title().await.ok();
                        let mut new_status_stream = proxy.receive_new_status().await.ok();
                        
                        use tokio_stream::StreamExt;
                        loop {
                            tokio::select! {
                                Some(_) = async {
                                    if let Some(ref mut s) = new_icon_stream {
                                        s.next().await
                                    } else {
                                        std::future::pending().await
                                    }
                                } => {
                                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                                        let _ = sender_clone.send(CustomEvent::TrayUpdated(item));
                                    }
                                }
                                Some(_) = async {
                                    if let Some(ref mut s) = new_title_stream {
                                        s.next().await
                                    } else {
                                        std::future::pending().await
                                    }
                                } => {
                                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                                        let _ = sender_clone.send(CustomEvent::TrayUpdated(item));
                                    }
                                }
                                Some(_) = async {
                                    if let Some(ref mut s) = new_status_stream {
                                        s.next().await
                                    } else {
                                        std::future::pending().await
                                    }
                                } => {
                                    if let Ok(item) = fetch_tray_item(&conn, &addr_clone).await {
                                        let _ = sender_clone.send(CustomEvent::TrayUpdated(item));
                                    }
                                }
                            }
                        }
                    }
                });
            }
        }
    }

    async fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    async fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(property)]
    async fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    async fn registered_status_notifier_items(&self) -> Vec<String> {
        let items = self.registered_items.lock().await;
        items.keys().cloned().collect()
    }
}

struct StatusInterface;

#[zbus::interface(name = "org.clear.StatusInterface")]
impl StatusInterface {
    async fn notify_attention(&self, app_id: String, title: String) {
        eprintln!("[status-interface] Received NotifyAttention: app_id={}, title={}", app_id, title);
        let title_escaped = title.replace('\'', "'\\''");
        let app_id_escaped = app_id.replace('\'', "'\\''");
        let cmd = format!(
            "notify-send -a '{}' '{} needs attention' 'This window has requested activation.'",
            app_id_escaped, title_escaped
        );
        std::process::Command::new("sh")
            .args(["-c", &cmd])
            .spawn()
            .ok();
    }
}

async fn spawn_status_tray(sender: calloop::channel::Sender<CustomEvent>) {
    let registered_items = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let tokio_handle = tokio::runtime::Handle::current();

    let watcher = Watcher {
        registered_items: registered_items.clone(),
        sender: sender.clone(),
        tokio_handle,
    };

    let conn = match zbus::ConnectionBuilder::session() {
        Ok(builder) => {
            match builder
                .name("org.kde.StatusNotifierWatcher")
                .unwrap()
                .serve_at("/StatusNotifierWatcher", watcher)
                .unwrap()
                .serve_at("/StatusInterface", StatusInterface)
                .unwrap()
                .build()
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to build D-Bus connection: {:?}", e);
                    return;
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to initialize D-Bus session: {:?}", e);
            return;
        }
    };

    println!("StatusNotifierWatcher running successfully on D-Bus!");

    // Start NameOwnerChanged listener to detect when tray apps disconnect
    let dbus_proxy = match zbus::fdo::DBusProxy::new(&conn).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to create DBusProxy: {:?}", e);
            return;
        }
    };
    let mut owner_changes = match dbus_proxy.receive_name_owner_changed().await {
        Ok(oc) => oc,
        Err(e) => {
            eprintln!("Failed to receive name owner changed: {:?}", e);
            return;
        }
    };

    let registered_items_clone = registered_items.clone();
    let sender_clone = sender.clone();
    
    tokio::spawn(async move {
        use tokio_stream::StreamExt;
        while let Some(signal) = owner_changes.next().await {
            if let Ok(args) = signal.args() {
                let old = args.old_owner;
                let new = args.new_owner;
                let old_opt: &Option<_> = &*old;
                if let Some(ref old_owner) = old_opt {
                    if new.is_none() {
                        let mut items = registered_items_clone.lock().await;
                        let mut to_remove = Vec::new();
                        for (key, addr) in items.iter() {
                            if addr.destination == old_owner.as_str() {
                                to_remove.push(key.clone());
                            }
                        }
                        for key in to_remove {
                            items.remove(&key);
                            let _ = sender_clone.send(CustomEvent::TrayRemoved(key));
                        }
                    }
                }
            }
        }
    });

    // Keep the task alive
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = rt.enter();

    clear_ui::engine::run::<StatusApp>();
}

fn read_normal_color_from_config() -> Option<[f32; 4]> {
    let content = std::fs::read_to_string("/home/lsgalante/.config/ccec/config.toml").ok()?;
    parse_srgb_color_from_key(&content, "status_normal_color")
}

fn read_disabled_color_from_config() -> Option<[f32; 4]> {
    let content = std::fs::read_to_string("/home/lsgalante/.config/ccec/config.toml").ok()?;
    parse_srgb_color_from_key(&content, "disabled_color")
}

fn parse_srgb_color_from_key(content: &str, key: &str) -> Option<[f32; 4]> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(key) {
            let rest = rest.trim_start_matches(|c: char| c == ' ' || c == '=' || c == '"');
            let hex = rest.trim_end_matches('"').trim();
            if let Some(rgb) = parse_hex(hex) {
                let r = rgb[0] as f32 / 255.0;
                let g = rgb[1] as f32 / 255.0;
                let b = rgb[2] as f32 / 255.0;
                return Some([r, g, b, 1.0]);
            }
        }
    }
    None
}

fn read_status_font_from_config() -> String {
    let font_conf_path = "/home/lsgalante/.config/fontconfig/fonts.conf";
    if let Ok(content) = std::fs::read_to_string(font_conf_path) {
        if let Some(font) = parse_font_for_alias(&content, "status-interface") {
            return font;
        }
    }
    "sans-serif".to_string()
}

fn read_status_height_from_config() -> f32 {
    let content = std::fs::read_to_string("/home/lsgalante/.config/ccec/config.toml").unwrap_or_default();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("bar_height") {
            let rest = rest.trim_start_matches(|c: char| c == ' ' || c == '=' || c == '"');
            if let Ok(val) = rest.trim_end_matches('"').trim().parse::<f32>() {
                return val;
            }
        }
    }
    28.0
}

fn read_status_font_size_from_config() -> f32 {
    let content = std::fs::read_to_string("/home/lsgalante/.config/ccec/config.toml").unwrap_or_default();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("status_font_size") {
            let rest = rest.trim_start_matches(|c: char| c == ' ' || c == '=' || c == '"');
            if let Ok(val) = rest.trim_end_matches('"').trim().parse::<f32>() {
                return val;
            }
        }
    }
    11.0
}

fn read_status_separators_from_config() -> bool {
    let content = std::fs::read_to_string("/home/lsgalante/.config/ccec/config.toml").unwrap_or_default();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("status_separators") {
            let rest = rest.trim_start_matches(|c: char| c == ' ' || c == '=' || c == '"');
            if let Ok(val) = rest.trim_end_matches('"').trim().parse::<bool>() {
                return val;
            }
        }
    }
    true
}

fn read_status_underline_from_config() -> bool {
    let content = std::fs::read_to_string("/home/lsgalante/.config/ccec/config.toml").unwrap_or_default();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("status_underline") {
            let rest = rest.trim_start_matches(|c: char| c == ' ' || c == '=' || c == '"');
            if let Ok(val) = rest.trim_end_matches('"').trim().parse::<bool>() {
                return val;
            }
        }
    }
    true
}

fn read_status_padding_from_config() -> f32 {
    let content = std::fs::read_to_string("/home/lsgalante/.config/ccec/config.toml").unwrap_or_default();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("status_padding") {
            let rest = rest.trim_start_matches(|c: char| c == ' ' || c == '=' || c == '"');
            if let Ok(val) = rest.trim_end_matches('"').trim().parse::<f32>() {
                return val;
            }
        }
    }
    8.0
}

fn read_separator_color_from_config() -> Option<[f32; 4]> {
    let content = std::fs::read_to_string("/home/lsgalante/.config/ccec/config.toml").ok()?;
    parse_color_from_key(&content, "status_separator_color")
}



fn parse_font_for_alias(content: &str, alias: &str) -> Option<String> {
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

fn read_bg_color_from_config() -> Option<[f32; 4]> {
    let content = std::fs::read_to_string("/home/lsgalante/.config/ccec/config.toml").ok()?;
    parse_color_from_key(&content, "low_color")
        .or_else(|| parse_color_from_key(&content, "background_color"))
}

fn parse_color_from_key(content: &str, key: &str) -> Option<[f32; 4]> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(key) {
            let rest = rest.trim_start_matches(|c: char| c == ' ' || c == '=' || c == '"');
            let hex = rest.trim_end_matches('"').trim();
            if let Some(rgb) = parse_hex(hex) {
                let r = (rgb[0] as f32 / 255.0).powf(2.2);
                let g = (rgb[1] as f32 / 255.0).powf(2.2);
                let b = (rgb[2] as f32 / 255.0).powf(2.2);
                return Some([r, g, b, 1.0]);
            }
        }
    }
    None
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim_start_matches('#');
    if s.len() >= 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some([r, g, b])
    } else {
        None
    }
}
