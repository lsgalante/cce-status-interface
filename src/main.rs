mod modules;
use modules::{StatusModule, WindowModule, ClockModule, BatteryModule, VolumeModule, BrightnessModule, MemoryModule, CpuModule, TrayModule, LightSourceModule};

use std::collections::HashMap;
use std::sync::Arc;
use glyphon::{
    Attrs, Buffer, FontSystem, Metrics, TextArea, TextBounds,
};
use cce_ui::color;
use cce_ui::widget::{
    TextItem, Separator, Element,
    MouseButton, ElementState, MouseScrollDelta, KeyEvent,
};

#[derive(Debug, Clone)]
pub struct TrayPixmap {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TrayItem {
    pub id: String,
    pub icon_name: Option<String>,
    pub icon_theme_path: Option<String>,
    pub pixmaps: Option<Vec<TrayPixmap>>,
    pub title: Option<String>,
    pub dbus_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TrayIconBounds {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub title: Option<String>,
    pub dbus_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ViewportBounds {
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone)]
pub struct LayoutBounds {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone)]
pub struct SystemStats {
    pub clock: String,
    pub memory: String,
    pub cpu: String,
    pub battery: String,
    pub battery_capacity: i32,
    pub battery_charging: bool,
    pub volume: String,
    pub volume_muted: bool,
    pub brightness: String,
}

#[derive(Clone)]
struct StdinWriter(std::sync::mpsc::Sender<String>);

impl std::fmt::Debug for StdinWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StdinWriter")
    }
}

#[derive(Debug, Clone)]
enum CustomEvent {
    ViewportUpdated(String),
    LayoutUpdated(String),
    TitleUpdated(String),
    ModifiersUpdated(String),
    SystemStatsUpdated(SystemStats),
    TrayUpdated(TrayItem),
    TrayRemoved(String),
    CloudSpawned { pid: u32, source: String, switcher_stdin: Option<StdinWriter> },
    CloudClosed { pid: u32, source: String },
    SwitcherTriggered,
    ToggleHideModules,
    ToggleAdjustPositionMode,
}

pub(crate) fn make_text_buffer(fs: &mut FontSystem, text: &str, size: f32, font_family: &str) -> Buffer {
    let scale = cce_ui::scale::scale_factor();
    let mut font_size = size;

    let (parsed_family, parsed_size) = cce_ui::layout::parse_font_string(font_family);
    if let Some(ps) = parsed_size {
        font_size = ps;
    }
    let family_name = Some(parsed_family);

    let physical_size = font_size * scale;
    let metrics = Metrics::new(physical_size, physical_size * 1.4);
    let mut buf = Buffer::new(fs, metrics);
    let mut attrs = Attrs::new();
    if let Some(ref font_name) = family_name {
        let family = match font_name.as_str() {
            "monospace" => glyphon::Family::Name(cce_ui::layout::get_system_monospace_font()),
            "sans-serif" => glyphon::Family::SansSerif,
            "serif" => glyphon::Family::Serif,
            name => glyphon::Family::Name(name),
        };
        attrs = attrs.family(family);
    }
    buf.set_text(fs, text, attrs, glyphon::Shaping::Advanced);
    buf.shape_until_scroll(fs, true);
    buf
}

pub(crate) fn parse_hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
    cce_ui::color::parse_hex_rgba(hex)
}

pub(crate) fn parse_viewport_text(input: &str) -> Vec<([f32; 4], String)> {
    let mut pango = input.to_string();
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(input) {
        if let Some(t) = val.get("text").and_then(|v| v.as_str()) {
            pango = t.to_string();
        }
    }
    let mut result = Vec::new();
    let mut remaining = pango.as_str();
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
        Some(format!("Mem {:.1}/{:.1}G", used, total))
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

pub struct RectWidget {
    pub x: f32, pub y: f32, pub w: f32, pub h: f32,
    pub color: [f32; 4],
}

pub struct RoundedBox {
    pub x: f32, pub y: f32, pub w: f32, pub h: f32,
    pub radius: f32,
    pub color: [f32; 4],
    pub corners: (bool, bool, bool, bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub struct ModuleBounds {
    pub name: String,
    pub side: Side,
    pub x: f32,
    pub w: f32,
}


struct StatusApp {
    // Status State
    viewport: String,
    layout: String,
    title: String,
    stats: Option<SystemStats>,
    tray_items: HashMap<String, TrayItem>,
    cursor_pos: (f64, f64),
    hovered_tray_item: Option<String>,
    tray_item_bounds: Vec<TrayIconBounds>,
    viewport_bounds: Vec<ViewportBounds>,
    layout_bounds: Option<LayoutBounds>,
    active_cloud_pid: Option<u32>,
    active_cloud_source: Option<String>,
    active_switcher_stdin: Option<StdinWriter>,
    previously_focused_window: Option<String>,

    font_system: FontSystem,
    status_bar: cce_ui::widget::StatusBar,

    rects: Vec<RectWidget>,
    overlay_rects: Vec<RectWidget>,
    rounded_boxes: Vec<RoundedBox>,
    separators: Vec<Separator>,
    text_items: Vec<TextItem>,

    scale_factor: f64,
    width: u32,
    height: u32,
    needs_rebuild: bool,
    current_bg_color: [f32; 4],
    input_regions: Vec<(i32, i32, i32, i32)>,
    super_pressed: bool,
    dragged_module: Option<(String, Side, f32)>,
    module_bounds: Vec<ModuleBounds>,
    left_modules: Vec<Box<dyn StatusModule>>,
    right_modules: Vec<Box<dyn StatusModule>>,
    sender: calloop::channel::Sender<CustomEvent>,
    last_config_modified: Option<std::time::SystemTime>,
    selected_module_name: Option<String>,
    selected_module_side: Option<Side>,
    status_hide_mode: bool,
    adjust_position_mode: bool,
}

impl StatusApp {
    fn get_app_id(&self) -> String {
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string());
        let use_interface_prefix = std::path::Path::new(&format!("/tmp/cce-status-interface-{}.sock", display)).exists();

        if let (Some(ref name), Some(ref side)) = (&self.selected_module_name, &self.selected_module_side) {
            if use_interface_prefix {
                format!("cce-status-interface-{:?}-{}", side, name).to_lowercase()
            } else {
                format!("cce-status-{:?}-{}", side, name).to_lowercase()
            }
        } else {
            if use_interface_prefix {
                "cce-status-interface".to_string()
            } else {
                "cce-status".to_string()
            }
        }
    }

    fn is_vertical(&self) -> bool {
        let bar_thickness = read_status_height_from_config() as u32;
        if self.width == bar_thickness && self.height != bar_thickness {
            true
        } else if self.height == bar_thickness && self.width != bar_thickness {
            false
        } else if self.width < self.height {
            true
        } else if self.width > self.height {
            false
        } else {
            cce_ui::IS_VERTICAL.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[allow(unused_assignments)]
    fn rebuild_layout(&mut self) {
        log::info!("[cce-status-interface] rebuild_layout module={:?} size={}x{}", self.selected_module_name, self.width, self.height);
        let is_vertical = self.is_vertical();
        cce_ui::IS_VERTICAL.store(is_vertical, std::sync::atomic::Ordering::Relaxed);
        let bar_thickness = read_status_height_from_config() as u32;
        cce_ui::BAR_THICKNESS.store(bar_thickness, std::sync::atomic::Ordering::Relaxed);

        let font_family = read_status_font_from_config();
        let font_size = read_status_font_size_from_config();
        let show_separators = false;
        let padding = read_status_padding_from_config();
        let spacing = read_status_module_spacing_from_config();
        let separator_color = read_separator_color_from_config().unwrap_or(color::STATUS_ACCENT);
        let normal_color = read_normal_color_from_config().unwrap_or(color::TEXT_FG);
        let sw_logical = if is_vertical { self.height as f32 } else { self.width as f32 };
        let bar_h = if is_vertical { self.width as f32 } else { read_status_height_from_config() };

        self.current_bg_color = read_bg_color_from_config().unwrap_or(color::STATUS_BG);
        if let Some(opacity) = cce_ui::color::read_opacity_if_configured() {
            self.current_bg_color[3] = opacity;
        }

        self.rects.clear();
        self.overlay_rects.clear();
        self.rounded_boxes.clear();
        self.separators.clear();
        self.text_items.clear();
        self.input_regions.clear();
        self.module_bounds.clear();
        self.tray_item_bounds.clear();

        let left_modules = std::mem::take(&mut self.left_modules);
        let right_modules = std::mem::take(&mut self.right_modules);

        let box_bg_color = read_status_box_background_color_from_config();
        let status_box_radius = read_status_box_corner_radius_from_config();

        self.status_bar.set_rect(0.0, 0.0, self.width as f32, self.height as f32);
        if self.selected_module_name.is_some() {
            self.status_bar.set_bg_color([0.0, 0.0, 0.0, 0.0]);
        } else {
            self.status_bar.set_bg_color(self.current_bg_color);
        }

        self.viewport_bounds.clear();
        self.layout_bounds = None;

        let is_single = self.selected_module_name.is_some();
        let margin_padding = if is_single { 6.0 } else { 12.0 };

        let mut left_x = margin_padding;
        let mut is_first_left = true;
        for module in &left_modules {
            let w = module.width(
                &self.stats,
                &self.viewport,
                &self.layout,
                &self.title,
                &mut self.font_system,
                &font_family,
                font_size,
                &self.tray_items,
                padding,
            );
            if w > 0.0 {
                if !is_first_left {
                    if show_separators {
                        self.separators.push(Separator::new(
                            left_x + spacing / 2.0,
                            0.0,
                            1.0,
                            bar_h,
                            separator_color,
                        ));
                    }
                    left_x += spacing;
                }
                is_first_left = false;

                if !module.has_custom_background(&self.title) {
                    if let Some(color) = box_bg_color {
                        self.rounded_boxes.push(RoundedBox {
                            x: left_x,
                            y: 0.0,
                            w,
                            h: bar_h,
                            radius: status_box_radius,
                            color,
                            corners: if self.selected_module_name.is_some() { (true, true, true, true) } else { (false, false, true, true) },
                        });
                    }
                }

                module.render(
                    left_x,
                    w,
                    &self.stats,
                    &self.viewport,
                    &self.layout,
                    &self.title,
                    &mut self.font_system,
                    &font_family,
                    font_size,
                    normal_color,
                    bar_h,
                    self.scale_factor,
                    &mut self.text_items,
                    &mut self.rects,
                    &mut self.overlay_rects,
                    &mut self.viewport_bounds,
                    &mut self.layout_bounds,
                    &self.tray_items,
                    &mut self.tray_item_bounds,
                    box_bg_color,
                    status_box_radius,
                    &mut self.rounded_boxes,
                    padding,
                );

                self.module_bounds.push(ModuleBounds {
                    name: module.name().to_string(),
                    side: Side::Left,
                    x: left_x,
                    w,
                });
                self.input_regions.push((left_x.round() as i32, 0, w.round() as i32, bar_h.round() as i32));
                left_x += w;
            }
        }

        let mut right_x = sw_logical - 12.0;
        let mut is_first_right = true;
        
        for module in right_modules.iter().rev() {
            let w = module.width(
                &self.stats,
                &self.viewport,
                &self.layout,
                &self.title,
                &mut self.font_system,
                &font_family,
                font_size,
                &self.tray_items,
                padding,
            );
            if w > 0.0 {
                if !is_first_right {
                    right_x -= spacing;
                    if show_separators {
                        self.separators.push(Separator::new(
                            right_x + spacing / 2.0,
                            0.0,
                            1.0,
                            bar_h,
                            separator_color,
                        ));
                    }
                }
                is_first_right = false;

                right_x -= w;
                self.module_bounds.push(ModuleBounds {
                    name: module.name().to_string(),
                    side: Side::Right,
                    x: right_x,
                    w,
                });
                self.input_regions.push((right_x.round() as i32, 0, w.round() as i32, bar_h.round() as i32));

                if !module.has_custom_background(&self.title) {
                    if let Some(color) = box_bg_color {
                        self.rounded_boxes.push(RoundedBox {
                            x: right_x,
                            y: 0.0,
                            w,
                            h: bar_h,
                            radius: status_box_radius,
                            color,
                            corners: if self.selected_module_name.is_some() { (true, true, true, true) } else { (false, false, true, true) },
                        });
                    }
                }

                module.render(
                    right_x,
                    w,
                    &self.stats,
                    &self.viewport,
                    &self.layout,
                    &self.title,
                    &mut self.font_system,
                    &font_family,
                    font_size,
                    normal_color,
                    bar_h,
                    self.scale_factor,
                    &mut self.text_items,
                    &mut self.rects,
                    &mut self.overlay_rects,
                    &mut self.viewport_bounds,
                    &mut self.layout_bounds,
                    &self.tray_items,
                    &mut self.tray_item_bounds,
                    box_bg_color,
                    status_box_radius,
                    &mut self.rounded_boxes,
                    padding,
                );

                if module.name() == "tray" {
                    right_x -= padding;
                }
            }
        }

        // 5c. Tooltip Rendering (if hovered)
        if let Some(ref hovered_id) = self.hovered_tray_item {
            if let Some(bound) = self.tray_item_bounds.iter().find(|b| &b.id == hovered_id) {
                let clean_tooltip = |title: Option<&str>, dbus_id: Option<&str>, fallback_id: &str| -> String {
                    if let Some(t) = title {
                        let trimmed = t.trim();
                        if !trimmed.is_empty() {
                            return trimmed.to_string();
                        }
                    }
                    if let Some(id) = dbus_id {
                        let trimmed = id.trim();
                        if !trimmed.is_empty() {
                            let cleaned = trimmed
                                .split('_')
                                .next()
                                .unwrap_or(trimmed)
                                .split('-')
                                .next()
                                .unwrap_or(trimmed);
                            if !cleaned.is_empty() && cleaned.chars().any(|c| c.is_alphabetic()) {
                                let mut chars = cleaned.chars();
                                if let Some(first) = chars.next() {
                                    return first.to_uppercase().collect::<String>() + chars.as_str();
                                }
                                return cleaned.to_string();
                            }
                            return trimmed.to_string();
                        }
                    }
                    let last_segment = fallback_id.split('/').last().unwrap_or(fallback_id);
                    let cleaned = last_segment
                        .split('_')
                        .next()
                        .unwrap_or(last_segment)
                        .split('-')
                        .next()
                        .unwrap_or(last_segment);
                    if !cleaned.is_empty() && cleaned.chars().any(|c| c.is_alphabetic()) {
                        let mut chars = cleaned.chars();
                        if let Some(first) = chars.next() {
                            return first.to_uppercase().collect::<String>() + chars.as_str();
                        }
                        return cleaned.to_string();
                    }
                    last_segment.to_string()
                };

                let tooltip_text = clean_tooltip(
                    bound.title.as_deref(),
                    bound.dbus_id.as_deref(),
                    bound.id.as_str(),
                );
                let tooltip_font_size = font_size - 1.0;
                let buf = make_text_buffer(&mut self.font_system, &tooltip_text, tooltip_font_size, &font_family);
                let scale = cce_ui::scale::scale_factor();
                let text_w = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0) / scale;
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

        if self.selected_module_name.is_some() {
            if is_vertical {
                let old_h = self.height;
                self.height = (left_x + margin_padding).round() as u32;
                eprintln!("[module-{}] rebuild_layout vertical: height calculated as {} (was {})", self.selected_module_name.as_deref().unwrap_or("none"), self.height, old_h);
                self.input_regions.clear();
                self.input_regions.push((0, 0, self.width as i32, self.height as i32));
            } else {
                let old_w = self.width;
                self.width = (left_x + margin_padding).round() as u32;
                eprintln!("[module-{}] rebuild_layout horizontal: width calculated as {} (was {})", self.selected_module_name.as_deref().unwrap_or("none"), self.width, old_w);
                self.input_regions.clear();
                self.input_regions.push((0, 0, self.width as i32, bar_h.round() as i32));
            }
        }

        if is_vertical {
            // Rotate rounded_boxes
            for rb in &mut self.rounded_boxes {
                let old_x = rb.x;
                let old_y = rb.y;
                let old_w = rb.w;
                let old_h = rb.h;
                rb.x = old_y;
                rb.y = old_x;
                rb.w = old_h;
                rb.h = old_w;
            }
            // Rotate separators
            for sep in &mut self.separators {
                let old_x = sep.x;
                let old_y = sep.y;
                let old_w = sep.w;
                let old_h = sep.h;
                sep.x = old_y;
                sep.y = old_x;
                sep.w = old_h;
                sep.h = old_w;
            }
            // Rotate rects
            for r in &mut self.rects {
                let old_x = r.x;
                let old_y = r.y;
                let old_w = r.w;
                let old_h = r.h;
                r.x = old_y;
                r.y = old_x;
                r.w = old_h;
                r.h = old_w;
            }
            // Rotate text_items
            for ti in &mut self.text_items {
                let old_x = ti.x;
                let old_y = ti.y;
                ti.x = old_y;
                ti.y = old_x;
            }
            // Rotate tray_item_bounds
            for tib in &mut self.tray_item_bounds {
                let old_x = tib.x;
                let old_y = tib.y;
                let old_w = tib.w;
                let old_h = tib.h;
                tib.x = old_y;
                tib.y = old_x;
                tib.w = old_h;
                tib.h = old_w;
            }
        }

        self.left_modules = left_modules;
        self.right_modules = right_modules;
        self.needs_rebuild = false;
    }

    fn check_drag_swap(&mut self, mouse_x: f32) -> bool {
        if let Some((ref dragged_name, side, _)) = self.dragged_module {
            match side {
                Side::Left => {
                    if let Some(curr_idx) = self.left_modules.iter().position(|m| m.name() == dragged_name) {
                        let curr_bounds = self.module_bounds.iter().find(|mb| mb.name == *dragged_name && mb.side == Side::Left);
                        if curr_bounds.is_some() {
                            // Check left neighbor
                            if curr_idx > 0 {
                                let prev_name = self.left_modules[curr_idx - 1].name();
                                if let Some(prev) = self.module_bounds.iter().find(|mb| mb.name == prev_name && mb.side == Side::Left) {
                                    let prev_center = prev.x + prev.w / 2.0;
                                    if mouse_x < prev_center {
                                        self.left_modules.swap(curr_idx, curr_idx - 1);
                                        return true;
                                    }
                                }
                            }
                            // Check right neighbor
                            if curr_idx < self.left_modules.len() - 1 {
                                let next_name = self.left_modules[curr_idx + 1].name();
                                if let Some(next) = self.module_bounds.iter().find(|mb| mb.name == next_name && mb.side == Side::Left) {
                                    let next_center = next.x + next.w / 2.0;
                                    if mouse_x > next_center {
                                        self.left_modules.swap(curr_idx, curr_idx + 1);
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
                Side::Right => {
                    if let Some(curr_idx) = self.right_modules.iter().position(|m| m.name() == dragged_name) {
                        let curr_bounds = self.module_bounds.iter().find(|mb| mb.name == *dragged_name && mb.side == Side::Right);
                        if curr_bounds.is_some() {
                            // Check left neighbor
                            if curr_idx > 0 {
                                let prev_name = self.right_modules[curr_idx - 1].name();
                                if let Some(prev) = self.module_bounds.iter().find(|mb| mb.name == prev_name && mb.side == Side::Right) {
                                    let prev_center = prev.x + prev.w / 2.0;
                                    if mouse_x < prev_center {
                                        self.right_modules.swap(curr_idx, curr_idx - 1);
                                        return true;
                                    }
                                }
                            }
                            // Check right neighbor
                            if curr_idx < self.right_modules.len() - 1 {
                                let next_name = self.right_modules[curr_idx + 1].name();
                                if let Some(next) = self.module_bounds.iter().find(|mb| mb.name == next_name && mb.side == Side::Right) {
                                    let next_center = next.x + next.w / 2.0;
                                    if mouse_x > next_center {
                                        self.right_modules.swap(curr_idx, curr_idx + 1);
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        false
    }

    fn trigger_switcher(&mut self, is_switcher_mode: bool) {
        let switcher_source = "window".to_string();

        // 1. Check if any cce-cloud instance is already running
        let mut running_cloud_pid = None;
        if let Some(pid) = self.active_cloud_pid {
            if std::path::Path::new(&format!("/proc/{}", pid)).exists() {
                if let Ok(comm) = std::fs::read_to_string(format!("/proc/{}/comm", pid)) {
                    if comm.trim() == "cce-cloud" {
                        running_cloud_pid = Some(pid);
                    }
                }
            }
        }

        if let Some(pid) = running_cloud_pid {
            if is_switcher_mode && self.active_cloud_source.as_ref() == Some(&switcher_source) {
                if let Some(ref writer) = self.active_switcher_stdin {
                    eprintln!("[switcher] Already open, sending cycle command to cce-cloud stdin");
                    let _ = writer.0.send("__cce_switcher_next__\n".to_string());
                    return;
                }
            } else {
                // Kill it to switch focus
                eprintln!("[switcher] Killing existing cce-cloud PID {}", pid);
                let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
                self.active_cloud_pid = None;
                
                // If it was a normal click on the same thing, toggle it off
                if !is_switcher_mode && self.active_cloud_source.as_ref() == Some(&switcher_source) {
                    self.active_cloud_source = None;
                    return;
                }
            }
        } else {
            // No active dialog is running, check if pending for same source
            if !is_switcher_mode && self.active_cloud_source.as_ref() == Some(&switcher_source) {
                self.active_cloud_source = None;
                return;
            }
        }

        // Set active cloud source
        self.active_cloud_source = Some(switcher_source.clone());

        // Get placement coords: align just below Window module if we can find it
        let mut target_x = 0.0;
        for mb in &self.module_bounds {
            if mb.name == "window" {
                target_x = mb.x;
                break;
            }
        }

        let bar_height = read_status_height_from_config() as i32;

        // Position it under Window module
        let x_pos = target_x as i32;
        let y_pos = bar_height;

        let thread_sender = self.sender.clone();
        let switcher_source_clone = switcher_source.clone();

        std::thread::spawn(move || {
            // Run "ccectl windows" to fetch the windows list
            let output = std::process::Command::new(get_ccectl_cmd())
                .arg("windows")
                .output();

            let mut windows = Vec::new();
            if let Ok(out) = output {
                let stdout_str = String::from_utf8_lossy(&out.stdout);
                for line in stdout_str.lines() {
                    let app_id = if let Some(idx) = line.find("app_id=") {
                        let rest = &line[idx + 7..];
                        let end = rest.find(' ').unwrap_or(rest.len());
                        rest[..end].to_string()
                    } else {
                        continue;
                    };

                    if app_id == "cce-status" || app_id == "cce-status-interface" || app_id == "cce-cloud" {
                        continue;
                    }

                    let title = if let Some(idx) = line.find("title=\"") {
                        let rest = &line[idx + 7..];
                        let end = rest.find('"').unwrap_or(rest.len());
                        rest[..end].to_string()
                    } else {
                        "".to_string()
                    };

                    let focused = if let Some(idx) = line.find("focused=") {
                        let rest = &line[idx + 8..];
                        let end = rest.find(' ').unwrap_or(rest.len());
                        rest[..end].trim() == "true"
                    } else {
                        false
                    };

                    let id = if let Some(idx) = line.find("window id=") {
                        let rest = &line[idx + 10..];
                        let end = rest.find(' ').unwrap_or(rest.len());
                        rest[..end].to_string()
                    } else {
                        continue;
                    };

                    windows.push((id, app_id, title, focused));
                }
            }

            if windows.is_empty() {
                // If there are no windows, don't open a switcher and clear state
                let _ = thread_sender.send(CustomEvent::CloudClosed { pid: 0, source: switcher_source_clone });
                return;
            }

            // Format items for dmenu, keeping the stable order returned by clearctl
            let mut input_str = String::new();
            let mut display_items = Vec::new();
            for (_, app_id, title, _) in &windows {
                let display = if title.is_empty() {
                    app_id.clone()
                } else {
                    format!("{} ({})", title, app_id)
                };
                input_str.push_str(&display);
                input_str.push('\n');
                display_items.push(display);
            }

            let mut cmd_args = vec![
                "--dmenu".to_string(),
                "-p".to_string(),
                "Windows:".to_string(),
                "-x".to_string(),
                x_pos.to_string(),
                "-y".to_string(),
                y_pos.to_string(),
            ];
            if is_switcher_mode {
                cmd_args.push("--switcher".to_string());
                if let Some(focused_idx) = windows.iter().position(|w| w.3) {
                    let target_idx = (focused_idx + 1) % windows.len();
                    let target_display = &display_items[target_idx];
                    cmd_args.push("-s".to_string());
                    cmd_args.push(target_display.clone());
                }
            }

            let mut child = match std::process::Command::new(get_cce_cloud_cmd())
                .args(&cmd_args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[switcher] Failed to spawn cce-cloud: {:?}", e);
                    let _ = thread_sender.send(CustomEvent::CloudClosed { pid: 0, source: switcher_source_clone });
                    return;
                }
            };

            let pid = child.id();
            let mut stdin = child.stdin.take().unwrap();
            let mut stdout = child.stdout.take().unwrap();

            // Create channels for stdin writing and spawn forwarder
            let (stdin_tx, stdin_rx) = std::sync::mpsc::channel::<String>();
            use std::io::Write;
            let _ = stdin.write_all(input_str.as_bytes());
            let _ = stdin.flush();

            std::thread::spawn(move || {
                while let Ok(msg) = stdin_rx.recv() {
                    if stdin.write_all(msg.as_bytes()).is_err() {
                        break;
                    }
                    let _ = stdin.flush();
                }
            });

            // Spawn stdout reader
            let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut out_str = String::new();
                use std::io::Read;
                let _ = stdout.read_to_string(&mut out_str);
                let _ = stdout_tx.send(out_str);
            });

            let _ = thread_sender.send(CustomEvent::CloudSpawned { pid, source: switcher_source_clone.clone(), switcher_stdin: Some(StdinWriter(stdin_tx)) });

            let _ = child.wait();
            let stdout_str = stdout_rx.recv().unwrap_or_default();

            let selected = stdout_str.trim().to_string();
            if !selected.is_empty() {
                // Find the matched window
                for (id, app_id, title, _) in windows {
                    let display = if title.is_empty() {
                        app_id.clone()
                    } else {
                        format!("{} ({})", title, app_id)
                    };
                    if display == selected {
                        eprintln!("[switcher] Selecting window title: {}, app_id: {}, id: {}", title, app_id, id);
                        let _ = std::process::Command::new(get_ccectl_cmd())
                            .args(["focus-window", &id])
                            .spawn();
                        break;
                    }
                }
            }

            let _ = thread_sender.send(CustomEvent::CloudClosed { pid, source: switcher_source_clone });
        });
    }
}

fn get_closest_viewport(x: f64, y: f64) -> i32 {
    let centers = [(0.0, 0.0), (2000.0, 0.0), (0.0, 2000.0), (2000.0, 2000.0)];
    let mut min_dist = f64::MAX;
    let mut best_tag = 1;
    for (i, &(cx, cy)) in centers.iter().enumerate() {
        let dx = x - cx;
        let dy = y - cy;
        let dist = dx * dx + dy * dy;
        if dist < min_dist {
            min_dist = dist;
            best_tag = (i + 1) as i32;
        }
    }
    best_tag
}

fn get_active_viewport_from_camera(viewport_json: &str) -> u32 {
    let mut text = viewport_json.to_string();
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(viewport_json) {
        if let Some(t) = val.get("text").and_then(|v| v.as_str()) {
            text = t.to_string();
        }
    }
    if let Some(pan_idx) = text.find("Pan: (") {
        let coords_str = &text[pan_idx + "Pan: (".len()..];
        if let Some(end_idx) = coords_str.find(")") {
            let parts: Vec<&str> = coords_str[..end_idx].split(',').collect();
            if parts.len() == 2 {
                let pan_x = parts[0].trim().parse::<f64>().unwrap_or(0.0);
                let pan_y = parts[1].trim().parse::<f64>().unwrap_or(0.0);
                return get_closest_viewport(pan_x, pan_y) as u32;
            }
        }
    }
    1
}

fn get_module_side(name: &str) -> Side {
    let content = std::fs::read_to_string(cce_ui::config::get_config_path()).unwrap_or_default();
    let val = parse_json(&content);
    
    if name == "light_source" {
        let mut light_pos = 2.356194490192345_f32; // Default 135 deg in rad
        if let Some(wm_obj) = json_find_key(&val, "window_manager") {
            if let Some(pos_val) = json_find_key(&wm_obj, "light_source_position") {
                if let Some(f) = pos_val.as_f64() {
                    light_pos = f as f32;
                } else if let Some(i) = pos_val.as_i64() {
                    let deg = i as f32;
                    if deg > 2.0 * std::f32::consts::PI {
                        light_pos = deg.to_radians();
                    } else {
                        light_pos = deg;
                    }
                }
            }
        }
        
        let two_pi = 2.0 * std::f32::consts::PI;
        let mut angle = light_pos % two_pi;
        if angle < 0.0 {
            angle += two_pi;
        }
        
        let pi = std::f32::consts::PI;
        // Side mapping: Left side is roughly [5pi/8, 11pi/8)
        if angle >= 5.0 * pi / 8.0 && angle < 11.0 * pi / 8.0 {
            return Side::Left;
        } else {
            return Side::Right;
        }
    }

    if let Some(side_val) = json_find_key(&val, name) {
        if let Some(side_str) = side_val.as_str() {
            match side_str.to_lowercase().as_str() {
                "left" | "top-left" | "bottom-left" | "top-center" | "bottom-center" => return Side::Left,
                "right" | "top-right" | "bottom-right" => return Side::Right,
                _ => {}
            }
        }
    }
    match name {
        "window" => Side::Left,
        _ => Side::Right,
    }
}

fn parse_selected_module_from_args() -> Option<(String, Side)> {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--module" && i + 1 < args.len() {
            let name = args[i + 1].clone();
            let side = get_module_side(&name);
            return Some((name, side));
        }
    }
    None
}

impl cce_ui::engine::Application for StatusApp {
    type Message = CustomEvent;

    fn new(_qh: &wayland_client::QueueHandle<cce_ui::engine::EngineState<Self>>, sender: calloop::channel::Sender<Self::Message>) -> Self {
        let selected_module = parse_selected_module_from_args();

        let mut left_modules: Vec<Box<dyn StatusModule>> = Vec::new();
        let mut right_modules: Vec<Box<dyn StatusModule>> = Vec::new();

        let mut has_window = false;

        if let Some((ref name, _)) = selected_module {
            let module: Box<dyn StatusModule> = match name.as_str() {
                "window" => {
                    has_window = true;
                    Box::new(WindowModule)
                }
                "tray" => Box::new(TrayModule),
                "cpu" => Box::new(CpuModule),
                "memory" => Box::new(MemoryModule),
                "brightness" => Box::new(BrightnessModule),
                "volume" => Box::new(VolumeModule),
                "battery" => Box::new(BatteryModule),
                "clock" => Box::new(ClockModule),
                "light_source" => Box::new(LightSourceModule),
                _ => panic!("Unknown module: {}", name),
            };
            left_modules.push(module);
        } else {
            left_modules.push(Box::new(WindowModule));
            right_modules.push(Box::new(TrayModule));
            right_modules.push(Box::new(CpuModule));
            right_modules.push(Box::new(MemoryModule));
            right_modules.push(Box::new(BrightnessModule));
            right_modules.push(Box::new(VolumeModule));
            right_modules.push(Box::new(BatteryModule));
            right_modules.push(Box::new(ClockModule));
            right_modules.push(Box::new(LightSourceModule));
            has_window = true;
        }

        if has_window {
            tokio::spawn(spawn_status_listener("viewport", sender.clone()));
            tokio::spawn(spawn_status_listener("layout", sender.clone()));
            tokio::spawn(spawn_status_listener("title", sender.clone()));
        }
        if selected_module.is_none() {
            tokio::spawn(spawn_status_listener("modifiers", sender.clone()));
        }
        let is_primary_for_switcher = selected_module.as_ref().map_or(true, |(name, _)| name == "window");
        if is_primary_for_switcher {
            tokio::spawn(spawn_switcher_listener(sender.clone()));
        }

        let has_tray = selected_module.as_ref().map_or(true, |(name, _)| name == "tray");
        if has_tray {
            tokio::spawn(spawn_status_tray(sender.clone()));
        }
        let has_stats = selected_module.as_ref().map_or(true, |(name, _)| {
            name == "cpu" || name == "memory" || name == "brightness" || name == "volume" || name == "battery" || name == "clock"
        });
        if has_stats {
            tokio::spawn(spawn_system_stats(sender.clone()));
        }

        let font_system = cce_ui::create_font_system();

        let mut app = Self {
            viewport: String::new(),
            layout: String::new(),
            title: String::new(),
            stats: if has_stats { Some(get_initial_stats()) } else { None },
            tray_items: HashMap::new(),
            cursor_pos: (0.0, 0.0),
            hovered_tray_item: None,
            tray_item_bounds: Vec::new(),
            viewport_bounds: Vec::new(),
            layout_bounds: None,
            active_cloud_pid: None,
            active_cloud_source: None,
            active_switcher_stdin: None,
            previously_focused_window: None,
            font_system,
            status_bar: cce_ui::widget::StatusBar::new(),
            rects: Vec::new(),
            overlay_rects: Vec::new(),
            rounded_boxes: Vec::new(),
            separators: Vec::new(),
            text_items: Vec::new(),
            scale_factor: 1.0,
            width: if selected_module.is_some() { 120 } else { 1920 },
            height: read_status_height_from_config() as u32,
            needs_rebuild: true,
            current_bg_color: color::STATUS_BG,
            input_regions: Vec::new(),
            super_pressed: false,
            dragged_module: None,
            module_bounds: Vec::new(),
            left_modules,
            right_modules,
            sender,
            last_config_modified: std::fs::metadata(cce_ui::config::get_config_path()).ok().and_then(|m| m.modified().ok()),
            selected_module_name: selected_module.as_ref().map(|(n, _)| n.clone()),
            selected_module_side: selected_module.as_ref().map(|(_, s)| s.clone()),
            status_hide_mode: false,
            adjust_position_mode: false,
        };

        app.rebuild_layout();
        app
    }



    fn settings(&self) -> cce_ui::engine::WindowSettings {
        let app_id = self.get_app_id();
        cce_ui::engine::WindowSettings {
            title: "Status Interface".to_string(),
            app_id,
            width: self.width,
            height: self.height,
            fullscreen: false,
            min_size: None,
        }
    }

    fn desired_size(&self) -> Option<(u32, u32)> {
        if self.selected_module_name.is_some() {
            Some((self.width, self.height))
        } else {
            None
        }
    }

    fn update(&mut self, msg: Self::Message, needs_rebuild: &mut bool, _exit: &mut bool) {
        match msg {
            CustomEvent::ViewportUpdated(t) => {
                self.viewport = t;
            }
            CustomEvent::LayoutUpdated(l) => {
                self.layout = l;
            }
            CustomEvent::TitleUpdated(t) => {
                self.title = t;
            }
            CustomEvent::ModifiersUpdated(m) => {
                let was_super = self.super_pressed;
                self.super_pressed = m.contains("super");

                if was_super && !self.super_pressed {
                    if let Some(ref writer) = self.active_switcher_stdin {
                        eprintln!("[switcher] Super modifier released (via status updates), triggering select and close");
                        let _ = writer.0.send("__cce_switcher_select_and_close__\n".to_string());
                    }
                }
            }
            CustomEvent::SystemStatsUpdated(s) => {
                eprintln!("[module-{}] stats updated, current width={}", self.selected_module_name.as_deref().unwrap_or("none"), self.width);
                self.stats = Some(s);
            }
            CustomEvent::TrayUpdated(item) => {
                self.tray_items.insert(item.id.clone(), item);
            }
            CustomEvent::TrayRemoved(id) => {
                self.tray_items.remove(&id);
            }
            CustomEvent::CloudSpawned { pid, source, switcher_stdin } => {
                if self.active_cloud_source.as_ref() == Some(&source) {
                    eprintln!("[cloud-event] CloudSpawned: pid {} for source {} matches expected, tracking", pid, source);
                    self.active_cloud_pid = Some(pid);
                    if source == "window" {
                        self.active_switcher_stdin = switcher_stdin;
                    }
                } else {
                    eprintln!("[cloud-event] CloudSpawned: pid {} for source {} is obsolete/canceled, killing", pid, source);
                    let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
                }
            }
            CustomEvent::CloudClosed { pid, source } => {
                if self.active_cloud_pid == Some(pid) || (pid == 0 && self.active_cloud_source.as_ref() == Some(&source)) {
                    eprintln!("[cloud-event] CloudClosed: pid {} for source {} closed, clearing tracking", pid, source);
                    self.active_cloud_pid = None;
                    self.active_cloud_source = None;
                    if source == "window" {
                        self.active_switcher_stdin = None;
                        self.previously_focused_window = None;
                    } else if source == "layout" || source.starts_with("context_menu:") || source.starts_with("tray:") {
                        if let Some(ref focus_query) = self.previously_focused_window {
                            eprintln!("[cloud-event] Restoring focus to: {}", focus_query);
                            let focus_query_clone = focus_query.clone();
                            std::thread::spawn(move || {
                                let _ = std::process::Command::new(get_ccectl_cmd())
                                    .args(["focus-window", &focus_query_clone])
                                    .status();
                            });
                        }
                        self.previously_focused_window = None;
                    }
                }
            }
            CustomEvent::SwitcherTriggered => {
                eprintln!("[switcher] SwitcherTriggered event received, calling trigger_switcher");
                self.trigger_switcher(true);
            }
            CustomEvent::ToggleHideModules => {
                self.status_hide_mode = !self.status_hide_mode;
                let cmd = if self.status_hide_mode { "true" } else { "false" };
                let _ = std::process::Command::new(get_cce_cmd())
                    .args(["control", "status-hide-mode", cmd])
                    .status();
                *needs_rebuild = true;
            }
            CustomEvent::ToggleAdjustPositionMode => {
                self.adjust_position_mode = std::path::Path::new("/tmp/cce-status-interface-adjust-mode").exists();
                self.adjust_position_mode = !self.adjust_position_mode;
                let cmd = if self.adjust_position_mode { "true" } else { "false" };
                let _ = std::process::Command::new(get_cce_cmd())
                    .args(["control", "adjust-position-mode", cmd])
                    .status();
                *needs_rebuild = true;
            }
        }
        self.needs_rebuild = true;
        *needs_rebuild = true;
    }

    fn tick(&mut self, _dt: f32, needs_rebuild: &mut bool) {
        if let Ok(metadata) = std::fs::metadata(cce_ui::config::get_config_path()) {
            if let Ok(modified) = metadata.modified() {
                if Some(modified) != self.last_config_modified {
                    self.last_config_modified = Some(modified);
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                }
            }
        }
        self.status_bar.prepare_text(&mut self.font_system);
    }

    fn view(&mut self, quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>, size: cce_ui::engine::LogicalSize, scale: f64) {
        if self.needs_rebuild || self.width != size.width as u32 || self.height != size.height as u32 || self.scale_factor != scale {
            self.width = size.width as u32;
            self.height = size.height as u32;
            self.scale_factor = scale;
            cce_ui::scale::set_scale_factor(scale as f32);
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

    fn view_rounded_quads(&mut self, quads: &mut Vec<(f32, f32, f32, f32, f32, [f32; 4], (bool, bool, bool, bool))>, _size: cce_ui::engine::LogicalSize, _scale: f64) {
        for rb in &self.rounded_boxes {
            quads.push((rb.x, rb.y, rb.w, rb.h, rb.radius, rb.color, rb.corners));
        }
    }

    fn overlay_quads(&mut self, quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>, _size: cce_ui::engine::LogicalSize, _scale: f64) {
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
            left: (ti.x * scale_f32).round(),
            top: (ti.y * scale_f32).round(),
            scale: 1.0,
            bounds,
            default_color: ti.color,
            custom_glyphs: &[],
        }).collect::<Vec<_>>();

        for (buf, x, y, col) in self.status_bar.get_text_items() {
            areas.push(TextArea {
                buffer: buf,
                left: (x * scale_f32).round(),
                top: (y * scale_f32).round(),
                scale: 1.0,
                bounds,
                default_color: col,
                custom_glyphs: &[],
            });
        }

        areas
    }

    fn clear_color(&self) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn input_regions(&self) -> Option<Vec<(i32, i32, i32, i32)>> {
        Some(self.input_regions.clone())
    }

    fn handle_pointer_move(&mut self, pos: cce_ui::engine::LogicalPosition, needs_rebuild: &mut bool) {
        let (lx, ly) = (pos.x, pos.y);
        self.cursor_pos = (lx as f64, ly as f64);
        let is_vertical = self.is_vertical();
        let coord = if is_vertical { ly } else { lx };

        if self.dragged_module.is_some() {
            if self.check_drag_swap(coord) {
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
            return;
        }

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

    fn handle_mouse_input(&mut self, button: MouseButton, state: ElementState, pos: cce_ui::engine::LogicalPosition, needs_rebuild: &mut bool) -> Option<Self::Message> {
        let (lx, ly) = (pos.x, pos.y);
        let is_vertical = self.is_vertical();
        let coord = if is_vertical { ly } else { lx };
        let cx = lx as f64;
        let cy = ly as f64;

        if button == MouseButton::Left {
            if state == ElementState::Pressed {
                if self.super_pressed {
                    let mut clicked_module = None;
                    for mb in &self.module_bounds {
                        if lx >= mb.x && lx <= (mb.x + mb.w) {
                            clicked_module = Some((mb.name.clone(), mb.side));
                            break;
                        }
                    }
                    if let Some((name, side)) = clicked_module {
                        self.dragged_module = Some((name, side, lx));
                        *needs_rebuild = true;
                        self.needs_rebuild = true;
                        return None;
                    }
                }
            } else {
                if self.dragged_module.is_some() {
                    self.dragged_module = None;
                    *needs_rebuild = true;
                    self.needs_rebuild = true;
                    return None;
                }
            }
        }

        if state == ElementState::Pressed {
            // Check if tray icon was clicked
            let mut clicked_tray = None;
            for bound in &self.tray_item_bounds {
                if cx >= bound.x as f64 && cx <= (bound.x + bound.w) as f64
                    && cy >= bound.y as f64 && cy <= (bound.y + bound.h) as f64 {
                    clicked_tray = Some(bound.clone());
                    break;
                }
            }

            if let Some(bound) = clicked_tray {
                let id = bound.id.clone();
                let tray_source = format!("tray:{}", id);

                // Check if any cce-cloud instance is already running
                let mut running_cloud_pid = None;
                if let Some(pid) = self.active_cloud_pid {
                    if std::path::Path::new(&format!("/proc/{}", pid)).exists() {
                        if let Ok(comm) = std::fs::read_to_string(format!("/proc/{}/comm", pid)) {
                            if comm.trim() == "cce-cloud" {
                                running_cloud_pid = Some(pid);
                            }
                        }
                    }
                }

                if let Some(pid) = running_cloud_pid {
                    // There is an active dialog open.
                    // Kill it regardless of which one it is.
                    eprintln!("[tray-click] cce-cloud (PID {}) is running, killing it", pid);
                    let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
                    self.active_cloud_pid = None;

                    // If it was clicked for the SAME tray icon, this is a toggle-off.
                    if self.active_cloud_source.as_ref() == Some(&tray_source) {
                        self.active_cloud_source = None;
                        return None;
                    }
                } else {
                    // No active dialog is running, but check if there is a pending one for the same source
                    if self.active_cloud_source.as_ref() == Some(&tray_source) {
                        // User clicked same icon again while it was pending. Cancel it!
                        self.active_cloud_source = None;
                        return None;
                    }
                }

                // Now set the active cloud source to this one
                self.active_cloud_source = Some(tray_source.clone());
                if self.previously_focused_window.is_none() {
                    self.previously_focused_window = get_currently_focused_window();
                }

                let btn_code = match button {
                    MouseButton::Left => 272,
                    MouseButton::Right => 273,
                    _ => 0,
                };
                let cx_i = cx as i32;
                let cy_i = cy as i32;
                let screen_width = self.width as i32;
                let bar_height = read_status_height_from_config() as i32;
                let bound_x = bound.x;
                let bound_w = bound.w;
                let parent_app_id = self.get_app_id();
                let thread_sender = self.sender.clone();
                let tray_source_clone = tray_source.clone();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    rt.block_on(async move {
                        let mut menu_shown = false;
                        if let Some((destination, path_part)) = id.split_once('/') {
                            let path = format!("/{}", path_part);
                            match zbus::Connection::session().await {
                                Ok(conn) => {
                                    match StatusNotifierItemProxy::builder(&conn)
                                        .destination(destination.to_string())
                                        .unwrap()
                                        .path(path)
                                        .unwrap()
                                        .build()
                                        .await
                                    {
                                        Ok(proxy) => {
                                            let is_menu = proxy.item_is_menu().await.unwrap_or(false);
                                            let menu_path = proxy.menu().await.ok();

                                            let should_show_menu = (btn_code == 273 && menu_path.is_some())
                                                || (btn_code == 272 && is_menu && menu_path.is_some());

                                            let x_pos = screen_width - (bound_x + bound_w) as i32;
                                            let y_pos = bar_height;
                                            eprintln!("[tray-click] Clicked tray item at bound_x={}, bound_w={}, screen_width={}, calculated x_pos={}, y_pos={}", bound_x, bound_w, screen_width, x_pos, y_pos);

                                            if should_show_menu {
                                                 if let Some(menu_p) = menu_path {
                                                     menu_shown = true;
                                                     if let Err(e) = show_cce_cloud_menu(&conn, destination, menu_p.as_str(), x_pos, y_pos, true, thread_sender.clone(), tray_source_clone.clone(), parent_app_id.clone()).await {
                                                         eprintln!("[tray-click] show_cce_cloud_menu failed: {:?}", e);
                                                     }
                                                 }
                                             } else if btn_code == 272 {
                                                 if let Err(e) = proxy.activate(cx_i, cy_i).await {
                                                     eprintln!("[tray-click] Activate failed: {:?}", e);
                                                     if let Some(menu_p) = menu_path {
                                                         menu_shown = true;
                                                         if let Err(e) = show_cce_cloud_menu(&conn, destination, menu_p.as_str(), x_pos, y_pos, true, thread_sender.clone(), tray_source_clone.clone(), parent_app_id.clone()).await {
                                                             eprintln!("[tray-click] Fallback show_cce_cloud_menu failed: {:?}", e);
                                                         }
                                                     }
                                                 }
                                             } else if btn_code == 273 {
                                                 let _ = proxy.context_menu(cx_i, cy_i).await;
                                             }
                                        }
                                        Err(e) => {
                                            eprintln!("[tray-click] Failed to build proxy: {:?}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!("[tray-click] Failed to connect to session bus: {:?}", e);
                                }
                            }
                        } else {
                            eprintln!("[tray-click] Failed to split id: {}", id);
                        }
                        if !menu_shown {
                            let _ = thread_sender.send(CustomEvent::CloudClosed { pid: 0, source: tray_source_clone });
                        }
                    });
                });
                return None;
            }

            if button == MouseButton::Right {
                self.adjust_position_mode = std::path::Path::new("/tmp/cce-status-interface-adjust-mode").exists();
                // Find which module was right-clicked
                let mut clicked_module = None;
                for mb in &self.module_bounds {
                    if coord >= mb.x && coord <= (mb.x + mb.w) {
                        clicked_module = Some(mb.clone());
                        break;
                    }
                }

                if let Some(mb) = clicked_module {
                    eprintln!("[module-right-click] Right-clicked module: {}", mb.name);
                    let context_source = format!("context_menu:{}", mb.name);

                    // Check if any cce-cloud instance is already running
                    let mut running_cloud_pid = None;
                    if let Some(pid) = self.active_cloud_pid {
                        if std::path::Path::new(&format!("/proc/{}", pid)).exists() {
                            if let Ok(comm) = std::fs::read_to_string(format!("/proc/{}/comm", pid)) {
                                if comm.trim() == "cce-cloud" {
                                    running_cloud_pid = Some(pid);
                                }
                            }
                        }
                    }

                    if let Some(pid) = running_cloud_pid {
                        eprintln!("[module-right-click] cce-cloud (PID {}) is running, killing it", pid);
                        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
                        self.active_cloud_pid = None;

                        // If it was clicked for the same context menu, this is a toggle-off
                        if self.active_cloud_source.as_ref() == Some(&context_source) {
                            self.active_cloud_source = None;
                            return None;
                        }
                    } else {
                        if self.active_cloud_source.as_ref() == Some(&context_source) {
                            self.active_cloud_source = None;
                            return None;
                        }
                    }

                    self.active_cloud_source = Some(context_source.clone());

                    if self.previously_focused_window.is_none() {
                        self.previously_focused_window = get_currently_focused_window();
                    }

                    let x_pos = mb.x as i32;
                    let y_pos = read_status_height_from_config() as i32;
                    let context_json = if self.adjust_position_mode {
                        serde_json::json!({
                            "width": 180,
                            "height": 80,
                            "widgets": [
                                { "type": "label", "text": mb.name },
                                { "type": "button", "text": "Done", "id": "toggle_adjust" }
                            ]
                        }).to_string()
                    } else {
                        let menu_text = if self.status_hide_mode {
                            "Show Modules"
                        } else {
                            "Hide Modules"
                        };
                        serde_json::json!({
                            "width": 180,
                            "height": 110,
                            "widgets": [
                                { "type": "label", "text": mb.name },
                                { "type": "button", "text": menu_text, "id": "toggle_hide" },
                                { "type": "button", "text": "Adjust Positions", "id": "toggle_adjust" }
                            ]
                        }).to_string()
                    };

                    let parent_app_id = self.get_app_id();

                    if let Ok(mut child) = std::process::Command::new(get_cce_cloud_cmd())
                        .args([
                            "--json",
                            "-x",
                            &x_pos.to_string(),
                            "-y",
                            &y_pos.to_string(),
                            "--parent-app-id",
                            &parent_app_id,
                        ])
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn()
                    {
                        let pid = child.id();
                        self.active_cloud_pid = Some(pid);
                        eprintln!("[module-right-click] Spawned cce-cloud with PID {}", pid);

                        let thread_sender = self.sender.clone();
                        let context_source_clone = context_source.clone();
                        std::thread::spawn(move || {
                            if let Some(mut stdin) = child.stdin.take() {
                                use std::io::Write;
                                let _ = stdin.write_all(context_json.as_bytes());
                            }
                            if let Ok(output) = child.wait_with_output() {
                                let err_str = String::from_utf8_lossy(&output.stderr);
                                if !err_str.is_empty() {
                                    eprintln!("[cce-cloud context stderr] {}", err_str);
                                }
                                if output.status.success() {
                                    let stdout_str = String::from_utf8_lossy(&output.stdout);
                                    if let Ok(parsed_json) = serde_json::from_str::<serde_json::Value>(stdout_str.trim()) {
                                        if let Some(btn_id) = parsed_json.get("button").and_then(|v| v.as_str()) {
                                            if btn_id == "toggle_hide" {
                                                let _ = thread_sender.send(CustomEvent::ToggleHideModules);
                                            } else if btn_id == "toggle_adjust" {
                                                let _ = thread_sender.send(CustomEvent::ToggleAdjustPositionMode);
                                            }
                                        }
                                    }
                                }
                            }
                            let _ = thread_sender.send(CustomEvent::CloudClosed { pid, source: context_source_clone });
                        });
                    }

                    return None;
                }
            }

            if button == MouseButton::Left {
                eprintln!("[viewport-click] Mouse left click at logical: ({}, {})", cx, cy);
                
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
                    let layout_source = "layout".to_string();

                    // Check if any cce-cloud instance is already running
                    let mut running_cloud_pid = None;
                    if let Some(pid) = self.active_cloud_pid {
                        if std::path::Path::new(&format!("/proc/{}", pid)).exists() {
                            if let Ok(comm) = std::fs::read_to_string(format!("/proc/{}/comm", pid)) {
                                if comm.trim() == "cce-cloud" {
                                    running_cloud_pid = Some(pid);
                                }
                            }
                        }
                    }

                    if let Some(pid) = running_cloud_pid {
                        // There is an active dialog open.
                        // Kill it regardless of which one it is.
                        eprintln!("[layout-click] cce-cloud (PID {}) is running, killing it", pid);
                        let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
                        self.active_cloud_pid = None;

                        // If it was clicked for the layout menu, this is a toggle-off.
                        if self.active_cloud_source.as_ref() == Some(&layout_source) {
                            self.active_cloud_source = None;
                            return None;
                        }
                    } else {
                        // No active dialog is running, but check if there is a pending one for the same source
                        if self.active_cloud_source.as_ref() == Some(&layout_source) {
                            self.active_cloud_source = None;
                            return None;
                        }
                    }

                    // Now set the active cloud source to this one
                    self.active_cloud_source = Some(layout_source);

                    if self.previously_focused_window.is_none() {
                        self.previously_focused_window = get_currently_focused_window();
                    }

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

                    if let Ok(mut child) = std::process::Command::new(get_cce_cloud_cmd())
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
                        self.active_cloud_pid = Some(pid);
                        eprintln!("[layout-click] Spawned cce-cloud with PID {}", pid);
                        
                        let active_viewport = get_active_viewport_from_camera(&self.viewport);
                        let thread_sender = self.sender.clone();
                        std::thread::spawn(move || {
                            eprintln!("[layout-click] Active tag is {}", active_viewport);
                            if let Some(mut stdin) = child.stdin.take() {
                                use std::io::Write;
                                let _ = stdin.write_all(layout_json.as_bytes());
                            }
                            if let Ok(output) = child.wait_with_output() {
                                let err_str = String::from_utf8_lossy(&output.stderr);
                                if !err_str.is_empty() {
                                    eprintln!("[cce-cloud stderr] {}", err_str);
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
                                            let _ = std::process::Command::new(get_ccectl_cmd())
                                                .args(["apply-mode-sharing", &selected_mode])
                                                .spawn();
                                        } else {
                                            eprintln!("[layout-click] Selected mode: {}, setting for tag {}", selected_mode, active_viewport);
                                            let _ = std::process::Command::new(get_ccectl_cmd())
                                                .args(["viewport-layout", &active_viewport.to_string(), &selected_mode])
                                                .spawn();
                                        }
                                    } else {
                                        // Fallback
                                        let selected = out_str.trim().to_string();
                                        if !selected.is_empty() {
                                            let selected_lower = selected.to_lowercase();
                                            let _ = std::process::Command::new(get_ccectl_cmd())
                                                .args(["viewport-layout", &active_viewport.to_string(), &selected_lower])
                                                .spawn();
                                        }
                                    }
                                }
                            }
                            let _ = thread_sender.send(CustomEvent::CloudClosed { pid, source: "layout".to_string() });
                        });
                    }
                } else {
                    let mut clicked_window = false;
                    for mb in &self.module_bounds {
                        if mb.name == "window" {
                            if coord >= mb.x && coord <= (mb.x + mb.w) {
                                clicked_window = true;
                                break;
                            }
                        }
                    }

                    if clicked_window {
                        let has_focus = !self.title.is_empty() && self.title != "(none)";
                        if !has_focus {
                            let mut clicked_viewport = false;
                            for bound in &self.viewport_bounds {
                                if cx >= bound.x as f64 && cx <= (bound.x + bound.w) as f64
                                    && cy >= bound.y as f64 && cy <= (bound.y + bound.h) as f64 {
                                    eprintln!("[viewport-click-via-window] Viewport matched: {}", bound.name);
                                    let name = bound.name.clone();
                                    std::thread::spawn(move || {
                                        let _ = std::process::Command::new(get_ccectl_cmd())
                                            .args(["view", &name])
                                            .spawn();
                                    });
                                    clicked_viewport = true;
                                    break;
                                }
                            }
                            if !clicked_viewport {
                                eprintln!("[window-click] Window module clicked (no window focused, fallback to switcher)!");
                                self.trigger_switcher(false);
                            }
                        } else {
                            eprintln!("[window-click] Window module clicked (window focused)!");
                            self.trigger_switcher(false);
                        }
                    }
                }
            }
        }
        None
    }

    fn handle_mouse_wheel(&mut self, _delta: &MouseScrollDelta, _pos: cce_ui::engine::LogicalPosition, _needs_rebuild: &mut bool) {}

    fn handle_key_input(&mut self, _event: &KeyEvent, _needs_rebuild: &mut bool) -> Option<Self::Message> { None }
}

async fn spawn_status_listener(sub: &'static str, sender: calloop::channel::Sender<CustomEvent>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    loop {
        let socket_path = match std::env::var("WAYLAND_DISPLAY") {
            Ok(display) => {
                let primary = format!("/tmp/cce-status-interface-{}.sock", display);
                if std::path::Path::new(&primary).exists() {
                    primary
                } else {
                    format!("/tmp/cce-status-{}.sock", display)
                }
            }
            Err(_) => {
                let primary = "/tmp/cce-status-interface.sock".to_string();
                if std::path::Path::new(&primary).exists() {
                    primary
                } else {
                    "/tmp/cce-status.sock".to_string()
                }
            }
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
                            "viewport" => CustomEvent::ViewportUpdated(val.clone()),
                            "layout" => CustomEvent::LayoutUpdated(val.clone()),
                            "title" => CustomEvent::TitleUpdated(val.clone()),
                            "modifiers" => CustomEvent::ModifiersUpdated(val.clone()),
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

async fn spawn_switcher_listener(sender: calloop::channel::Sender<CustomEvent>) {
    use tokio::io::AsyncBufReadExt;
    use tokio::net::UnixListener;
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string());
    let socket_path = format!("/tmp/cce-status-interface-switcher-{}.sock", display);
    let _ = std::fs::remove_file(&socket_path);

    if let Ok(listener) = UnixListener::bind(&socket_path) {
        eprintln!("[switcher-listener] Listening on {}", socket_path);
        loop {
            if let Ok((stream, _)) = listener.accept().await {
                let mut reader = tokio::io::BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).await.is_ok() {
                    let _ = sender.send(CustomEvent::SwitcherTriggered);
                }
            }
        }
    } else {
        eprintln!("[switcher-listener] Failed to bind to {}", socket_path);
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

fn get_initial_stats() -> SystemStats {
    let clock = chrono::Local::now().format("%A, %B %d, %Y %I:%M %p").to_string();
    let memory = read_memory_usage().unwrap_or_else(|| "Mem N/A".to_string());

    let (battery_str, battery_capacity, battery_charging) = if let Some((s, cap, chg)) = read_battery_details() {
        (s, cap, chg)
    } else {
        ("".to_string(), 0, false)
    };
    let (volume, volume_muted) = pollster::block_on(read_volume()).unwrap_or_else(|| ("".to_string(), false));
    let brightness = read_brightness().unwrap_or_default();

    SystemStats {
        clock,
        memory,
        cpu: "Cpu 0.0%".to_string(),
        battery: battery_str,
        battery_capacity,
        battery_charging,
        volume,
        volume_muted,
        brightness,
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

struct MenuItem {
    id: i32,
    label: String,
    enabled: bool,
    is_separator: bool,
    toggle_state: i32, // -1 if not toggleable, 0 if unchecked, 1 if checked
    children: Vec<MenuItem>,
}

fn parse_menu_item(
    id: i32,
    mut properties: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    children_vals: Vec<zbus::zvariant::OwnedValue>,
) -> Option<MenuItem> {
    let type_: String = properties.remove("type")
        .and_then(|v| {
            let s: Result<String, _> = v.try_into();
            s.ok()
        })
        .unwrap_or_default();
    let is_separator = type_ == "separator";

    let label: String = properties.remove("label")
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

    let toggle_state: i32 = properties.remove("toggle-state")
        .and_then(|v| {
            let i: Result<i32, _> = v.try_into();
            i.ok()
        })
        .unwrap_or(-1);

    let mut children = Vec::new();
    for child_val in children_vals {
        let child_val_inner = zbus::zvariant::Value::from(child_val);
        if let Ok(child) = <(i32, std::collections::HashMap<String, zbus::zvariant::OwnedValue>, Vec<zbus::zvariant::OwnedValue>)>::try_from(child_val_inner) {
            if let Some(parsed) = parse_menu_item(child.0, child.1, child.2) {
                children.push(parsed);
            }
        }
    }

    Some(MenuItem {
        id,
        label,
        enabled,
        is_separator,
        toggle_state,
        children,
    })
}

fn get_cce_cloud_cmd() -> String {
    if let Ok(home) = std::env::var("HOME") {
        let path = format!("{}/.local/bin/cce-cloud", home);
        if std::path::Path::new(&path).exists() {
            return path;
        }
    }
    "cce-cloud".to_string()
}

fn get_currently_focused_window() -> Option<String> {
    let output = std::process::Command::new(get_ccectl_cmd())
        .arg("windows")
        .output();
    if let Ok(out) = output {
        let stdout_str = String::from_utf8_lossy(&out.stdout);
        for line in stdout_str.lines() {
            let focused = if let Some(idx) = line.find("focused=") {
                let rest = &line[idx + 8..];
                let end = rest.find(' ').unwrap_or(rest.len());
                rest[..end].trim() == "true"
            } else {
                false
            };

            if focused {
                let app_id = if let Some(idx) = line.find("app_id=") {
                    let rest = &line[idx + 7..];
                    let end = rest.find(' ').unwrap_or(rest.len());
                    rest[..end].to_string()
                } else {
                    continue;
                };
                if app_id == "cce-status" || app_id == "cce-cloud" {
                    continue;
                }
                
                // Return the unique window ID if present, otherwise fall back to app_id
                let id = if let Some(idx) = line.find("window id=") {
                    let rest = &line[idx + 10..];
                    let end = rest.find(' ').unwrap_or(rest.len());
                    rest[..end].to_string()
                } else {
                    app_id
                };
                return Some(id);
            }
        }
    }
    None
}

async fn show_cce_cloud_menu(
    conn: &zbus::Connection,
    destination: &str,
    menu_path: &str,
    x_pos: i32,
    y_pos: i32,
    align_right: bool,
    thread_sender: calloop::channel::Sender<CustomEvent>,
    source: String,
    parent_app_id: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut last_spawned_pid = 0;

    let res = async {
        let menu_proxy = DBusMenuProxy::builder(conn)
            .destination(destination)?
            .path(menu_path)?
            .build()
            .await?;

        let _ = menu_proxy.about_to_show(0).await;
        let (_, layout) = menu_proxy.get_layout(0, 5, vec![]).await?;

        let root_item = match parse_menu_item(layout.0, layout.1, layout.2) {
            Some(item) => item,
            None => return Ok(()),
        };

        // Assign page indices to submenus.
        let mut page_indices = std::collections::HashMap::new();
        page_indices.insert(root_item.id, 0);
        let mut parent_pages = std::collections::HashMap::new();
        let mut next_page = 1;

        fn assign_pages(
            item: &MenuItem,
            current_page: usize,
            page_indices: &mut std::collections::HashMap<i32, usize>,
            parent_pages: &mut std::collections::HashMap<usize, usize>,
            next_page: &mut usize,
        ) {
            for child in &item.children {
                if child.is_separator || !child.enabled {
                    continue;
                }
                if !child.children.is_empty() && *next_page < 16 {
                    let child_page = *next_page;
                    page_indices.insert(child.id, child_page);
                    parent_pages.insert(child_page, current_page);
                    *next_page += 1;
                    assign_pages(child, child_page, page_indices, parent_pages, next_page);
                }
            }
        }

        assign_pages(&root_item, 0, &mut page_indices, &mut parent_pages, &mut next_page);

        #[derive(Debug, Clone)]
        struct LocalWidget {
            widget_type: String,
            text: String,
            id: Option<String>,
            target_page: Option<usize>,
        }

        #[derive(Debug, Clone)]
        struct LocalPage {
            title: String,
            widgets: Vec<LocalWidget>,
        }

        let mut pages = vec![LocalPage {
            title: "".to_string(),
            widgets: Vec::new(),
        }; next_page];

        fn build_pages(
            item: &MenuItem,
            current_page: usize,
            page_indices: &std::collections::HashMap<i32, usize>,
            parent_pages: &std::collections::HashMap<usize, usize>,
            pages: &mut [LocalPage],
        ) {
            let mut widgets = Vec::new();

            if current_page > 0 {
                if let Some(&parent_page) = parent_pages.get(&current_page) {
                    widgets.push(LocalWidget {
                        widget_type: "button".to_string(),
                        text: "< Back".to_string(),
                        id: Some(format!("back_to_{}", parent_page)),
                        target_page: Some(parent_page),
                    });
                }
            }

            for child in &item.children {
                if child.is_separator || !child.enabled {
                    continue;
                }

                let mut display_label = if child.toggle_state == 1 {
                    format!("[x] {}", child.label)
                } else if child.toggle_state == 0 {
                    format!("[ ] {}", child.label)
                } else {
                    child.label.clone()
                };

                if !child.children.is_empty() {
                    if let Some(&target_page) = page_indices.get(&child.id) {
                        display_label = format!("{} >", display_label);

                        widgets.push(LocalWidget {
                            widget_type: "button".to_string(),
                            text: display_label,
                            id: Some(format!("submenu_{}", child.id)),
                            target_page: Some(target_page),
                        });

                        build_pages(child, target_page, page_indices, parent_pages, pages);
                    } else {
                        widgets.push(LocalWidget {
                            widget_type: "button".to_string(),
                            text: display_label,
                            id: Some(format!("item_{}", child.id)),
                            target_page: None,
                        });
                    }
                } else {
                    widgets.push(LocalWidget {
                        widget_type: "button".to_string(),
                        text: display_label,
                        id: Some(format!("item_{}", child.id)),
                        target_page: None,
                    });
                }
            }

            let title = if item.label.is_empty() {
                if current_page == 0 {
                    "Tray Menu".to_string()
                } else {
                    "".to_string()
                }
            } else {
                item.label.clone()
            };

            pages[current_page] = LocalPage {
                title,
                widgets,
            };
        }

        build_pages(&root_item, 0, &page_indices, &parent_pages, &mut pages);

        // Serialize to JSON value
        let mut pages_json = Vec::new();
        for page in pages {
            let mut widgets_json = Vec::new();
            for w in page.widgets {
                let mut w_val = serde_json::json!({
                    "type": w.widget_type,
                    "text": w.text,
                });
                if let Some(id) = w.id {
                    w_val["id"] = serde_json::Value::String(id);
                }
                if let Some(tp) = w.target_page {
                    w_val["target_page"] = serde_json::Value::Number(tp.into());
                }
                widgets_json.push(w_val);
            }
            pages_json.push(serde_json::json!({
                "title": page.title,
                "widgets": widgets_json,
            }));
        }

        let layout_json = serde_json::json!({
            "width": 260,
            "pages": pages_json,
        });
        let layout_str = layout_json.to_string();

        let mut cmd_args = vec![
            "--json".to_string(),
            "-x".to_string(),
            x_pos.to_string(),
            "-y".to_string(),
            y_pos.to_string(),
            "--parent-app-id".to_string(),
            parent_app_id,
        ];
        if align_right {
            cmd_args.push("--align-right".to_string());
        }

        let mut child = std::process::Command::new(get_cce_cloud_cmd())
            .args(&cmd_args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;

        let pid = child.id();
        last_spawned_pid = pid;
        let _ = thread_sender.send(CustomEvent::CloudSpawned { pid, source: source.clone(), switcher_stdin: None });

        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(layout_str.as_bytes())?;
        }

        let output = child.wait_with_output()?;
        if output.status.success() {
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            if let Ok(parsed_json) = serde_json::from_str::<serde_json::Value>(stdout_str.trim()) {
                if let Some(btn_id) = parsed_json.get("button").and_then(|v| v.as_str()) {
                    if btn_id.starts_with("item_") {
                        if let Ok(item_id) = btn_id["item_".len()..].parse::<i32>() {
                            let timestamp = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as u32;
                            let val = zbus::zvariant::Value::from("");
                            let _ = menu_proxy.event(item_id, "clicked", &val, timestamp).await;
                        }
                    }
                }
            }
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    }.await;

    let _ = thread_sender.send(CustomEvent::CloudClosed { pid: last_spawned_pid, source });
    res
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
    let mut argb_pixels = Vec::with_capacity((width * height * 4) as usize);
    
    let actual_bytes = &buf[..info.buffer_size()];
    match info.color_type {
        png::ColorType::Rgba => {
            for chunk in actual_bytes.chunks_exact(4) {
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
    let dbus_id = proxy.id().await.ok();

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
            if let Some(icon_path) = resolve_icon_path(icon_theme_path.as_deref(), name) {
                let ext = icon_path.extension().and_then(|e| e.to_str()).unwrap_or("");
                let pixmap = if ext.eq_ignore_ascii_case("svg") {
                    load_svg_as_pixmap(&icon_path)
                } else {
                    load_png_as_pixmap(&icon_path)
                };
                if let Some(pixmap) = pixmap {
                    pixmaps = Some(vec![pixmap]);
                }
            }
        }
    }

    Ok(TrayItem {
        id,
        icon_name,
        icon_theme_path,
        pixmaps,
        title,
        dbus_id,
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
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    let args: Vec<String> = std::env::args().collect();
    eprintln!("cce-status-interface started with args = {:?}", args);
    if args.len() > 1 && args[1] == "--trigger-switcher" {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string());
            let socket_path = format!("/tmp/cce-status-interface-switcher-{}.sock", display);
            use tokio::io::AsyncWriteExt;
            if let Ok(mut stream) = tokio::net::UnixStream::connect(&socket_path).await {
                let _ = stream.write_all(b"trigger\n").await;
            }
        });
        return;
    }

    let mut has_module = false;
    let mut monolithic = false;
    for i in 0..args.len() {
        if args[i] == "--module" {
            has_module = true;
        }
        if args[i] == "--monolithic" {
            monolithic = true;
        }
    }

    if !has_module && !monolithic {
        log::info!("Starting cce-status-interface launcher daemon...");
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigint = signal(SignalKind::interrupt()).expect("SIGINT");
            let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM");
            
            let modules = vec![
                "window", "tray", "cpu", "memory", "brightness",
                "volume", "battery", "clock", "light_source"
            ];
            let current_exe = std::env::current_exe().unwrap_or_else(|_| {
                std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
                    .join(".local/bin/cce-status-interface")
            });

            let mut active_children: std::collections::HashMap<String, std::process::Child> = std::collections::HashMap::new();

            for module in &modules {
                let child = std::process::Command::new(&current_exe)
                    .arg("--module")
                    .arg(module)
                    .spawn();
                match child {
                    Ok(c) => {
                        log::info!("Spawned module process for: {}", module);
                        active_children.insert(module.to_string(), c);
                    }
                    Err(e) => {
                        log::error!("Failed to spawn module process for {}: {:?}", module, e);
                    }
                }
            }

            loop {
                tokio::select! {
                    _ = sigint.recv() => {
                        log::info!("Received SIGINT, shutting down...");
                        break;
                    }
                    _ = sigterm.recv() => {
                        log::info!("Received SIGTERM, shutting down...");
                        break;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                        for module in &modules {
                            let mut restart = false;
                            if let Some(child) = active_children.get_mut(*module) {
                                match child.try_wait() {
                                    Ok(Some(status)) => {
                                        log::warn!("Module process '{}' exited with status: {:?}. Restarting...", module, status);
                                        restart = true;
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        log::error!("Error checking status for module '{}': {:?}. Restarting...", module, e);
                                        restart = true;
                                    }
                                }
                            } else {
                                restart = true;
                            }

                            if restart {
                                let child = std::process::Command::new(&current_exe)
                                    .arg("--module")
                                    .arg(module)
                                    .spawn();
                                match child {
                                    Ok(c) => {
                                        log::info!("Restarted module process for: {}", module);
                                        active_children.insert(module.to_string(), c);
                                    }
                                    Err(e) => {
                                        log::error!("Failed to restart module process for {}: {:?}", module, e);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            for (module, mut child) in active_children {
                log::info!("Killing module process: {}", module);
                let _ = child.kill();
            }
        });
        return;
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = rt.enter();

    cce_ui::engine::run::<StatusApp>();
}

fn parse_json(content: &str) -> serde_json::Value {
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

fn read_normal_color_from_config() -> Option<[f32; 4]> {
    let content = get_cached_config_content();
    parse_srgb_color_from_key(&content, "status_normal_color")
}

pub(crate) fn read_disabled_color_from_config() -> Option<[f32; 4]> {
    let content = get_cached_config_content();
    parse_srgb_color_from_key(&content, "disabled_color")
}

fn parse_srgb_color_from_key(content: &str, key: &str) -> Option<[f32; 4]> {
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

fn read_status_font_from_config() -> String {
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

fn read_status_height_from_config() -> f32 {
    let val = get_cached_config();
    json_find_key(&val, "bar_height").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(28.0)
}

fn read_status_font_size_from_config() -> f32 {
    let val = get_cached_config();
    
    if let Some(font_str) = json_find_key(&val, "status_font").and_then(|v| v.as_str()) {
        let (_, parsed_size) = cce_ui::layout::parse_font_string(font_str);
        if let Some(size) = parsed_size {
            return size;
        }
    }
    
    json_find_key(&val, "status_font_size").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(11.0)
}

fn read_status_padding_from_config() -> f32 {
    let val = get_cached_config();
    json_find_key(&val, "status_padding").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(8.0)
}

fn read_status_module_spacing_from_config() -> f32 {
    let val = get_cached_config();
    json_find_key(&val, "status_module_spacing").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(8.0)
}

fn read_separator_color_from_config() -> Option<[f32; 4]> {
    let content = get_cached_config_content();
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
    let content = get_cached_config_content();
    parse_color_from_key(&content, "background_color")
        .or_else(|| parse_color_from_key(&content, "low_color"))
        .or_else(|| parse_color_from_key(&content, "desktop_gap_color"))
}

fn parse_color_from_key(content: &str, key: &str) -> Option<[f32; 4]> {
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

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    cce_ui::color::parse_hex_bytes(s).map(|[r, g, b, _]| [r, g, b])
}
fn parse_hex_rgba(s: &str) -> Option<[u8; 4]> {
    cce_ui::color::parse_hex_bytes(s)
}

fn parse_rgba_color_from_key(content: &str, key: &str) -> Option<[f32; 4]> {
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

fn read_status_background_blur_from_config() -> f32 {
    let val = get_cached_config();
    json_find_key(&val, "status_background_blur").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(0.0)
}

fn read_status_box_background_color_from_config() -> Option<[f32; 4]> {
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

fn read_status_box_corner_radius_from_config() -> f32 {
    let val = get_cached_config();
    json_find_key(&val, "status_box_corner_radius").and_then(|v| v.as_f64()).map(|n| n as f32).unwrap_or(4.0)
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn test_status_config() {
        let content = r##"
style {
    status normal_color=(color)"#ccccd8" background_color=(color)"#151520e6" background_blur=(f64)0.8 font="Berkeley Mono 14"
}
"##;
        let val = parse_json(content);
        println!("Parsed KDL to JSON: {:#?}", val);

        let bg_color_val = json_find_key(&val, "status_background_color");
        println!("bg_color_val = {:?}", bg_color_val);
        assert!(bg_color_val.is_some());
        assert_eq!(bg_color_val.unwrap().as_str().unwrap(), "#151520e6");

        let blur_val = json_find_key(&val, "status_background_blur");
        println!("blur_val = {:?}", blur_val);
        assert!(blur_val.is_some());
        assert_eq!(blur_val.unwrap().as_f64().unwrap(), 0.8);

        let font_val = json_find_key(&val, "status_font");
        println!("font_val = {:?}", font_val);
        assert!(font_val.is_some());
        assert_eq!(font_val.unwrap().as_str().unwrap(), "Berkeley Mono 14");
    }
}

fn get_cce_cmd() -> String {
    if let Ok(home) = std::env::var("HOME") {
        let path = format!("{}/.local/bin/cce", home);
        if std::path::Path::new(&path).exists() {
            return path;
        }
    }
    "cce".to_string()
}

fn get_ccectl_cmd() -> String {
    if let Ok(home) = std::env::var("HOME") {
        let path = format!("{}/.local/bin/ccectl", home);
        if std::path::Path::new(&path).exists() {
            return path;
        }
    }
    "ccectl".to_string()
}
