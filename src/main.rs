mod cloud;
mod config;
mod listeners;
mod modules;
mod stats;
mod tray;

// Re-export at the crate root so call sites (here and in modules.rs) keep
// their pre-split names.
pub(crate) use cloud::*;
pub(crate) use config::*;
pub(crate) use listeners::*;
pub(crate) use stats::*;
pub(crate) use tray::*;

use modules::{StatusModule, WindowModule, ClockModule, BatteryModule, VolumeModule, BrightnessModule, MemoryModule, CpuModule, TrayModule, LightSourceModule};

use std::collections::HashMap;
use glyphon::{
    Attrs, Buffer, FontSystem, Metrics,
};
use cce_ui::color;
use cce_ui::widget::{
    Adapted, Separator, WidgetHost,
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

/// The subset of [`SystemStats`] a given module actually paints, as a comparable
/// signature. The stats poller pushes a full `SystemStatsUpdated` every second
/// regardless of change; deduping on this signature lets a module skip the redraw
/// (and the compositor's whole-backdrop blur re-bake it triggers) when its own value
/// is unchanged — e.g. the clock shows HH:MM and only changes once a minute.
/// `None` means the module ignores stats entirely (window/tray/light_source), so a
/// stats push never redraws it. Unknown / combined-bar modules compare everything.
fn stats_signature(module: Option<&str>, s: &SystemStats) -> Option<String> {
    match module {
        Some("clock") => Some(s.clock.clone()),
        Some("cpu") => Some(s.cpu.clone()),
        Some("memory") => Some(s.memory.clone()),
        Some("battery") => Some(format!("{}|{}|{}", s.battery, s.battery_capacity, s.battery_charging)),
        Some("volume") => Some(format!("{}|{}", s.volume, s.volume_muted)),
        Some("brightness") => Some(s.brightness.clone()),
        Some("window") | Some("tray") | Some("light_source") => None,
        _ => Some(format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            s.clock, s.memory, s.cpu, s.battery, s.battery_capacity,
            s.battery_charging, s.volume, s.volume_muted, s.brightness
        )),
    }
}

#[derive(Debug, Clone)]
pub(crate) enum CustomEvent {
    ViewportUpdated(String),
    LayoutUpdated(String),
    TitleUpdated(String),
    SystemStatsUpdated(SystemStats),
    TrayUpdated(TrayItem),
    TrayRemoved(String),
    CloudSpawned { pid: u32, source: String },
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
    cloud_popups: cce_ui::process::CloudPopupTracker,
    previously_focused_window: Option<String>,

    font_system: FontSystem,
    status_bar: cce_ui::widget::Adapted<cce_ui::widget::StatusBar>,

    rects: Vec<RectWidget>,
    overlay_rects: Vec<RectWidget>,
    rounded_boxes: Vec<RoundedBox>,
    separators: Vec<Adapted<Separator>>,
    text_prims: Vec<TextPrim>,

    scale_factor: f64,
    width: u32,
    height: u32,
    needs_rebuild: bool,
    current_bg_color: [f32; 4],
    box_bevel: Option<StatusBoxBevel>,
    box_bevel_depth: f32,
    input_regions: Vec<(i32, i32, i32, i32)>,
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

        // Family WITHOUT the embedded size: the toolkit's text pipeline lets
        // a size inside the font string ("Chivo Mono 14") override the
        // explicit font-size parameter, which would dead-end
        // `module { font_size }`. Size is resolved separately below.
        let (font_family, _) =
            cce_ui::layout::parse_font_string(&read_status_font_from_config());
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
        self.text_prims.clear();
        self.input_regions.clear();
        self.module_bounds.clear();
        self.tray_item_bounds.clear();

        let left_modules = std::mem::take(&mut self.left_modules);
        let right_modules = std::mem::take(&mut self.right_modules);

        let box_bg_color = read_status_box_background_color_from_config();
        let status_box_radius = read_status_box_corner_radius_from_config();
        self.box_bevel = read_status_box_bevel_from_config();
        self.box_bevel_depth = read_status_box_bevel_depth_from_config();

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
                    &mut self.text_prims,
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
                    &mut self.text_prims,
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
                let _ = buf; // shaped only to measure `text_w` above
                self.text_prims.push((
                    tooltip_text.clone(),
                    tooltip_font_size,
                    tx + padding,
                    ty + padding,
                    [
                        (color::TEXT_FG[0] * 255.0) as u8,
                        (color::TEXT_FG[1] * 255.0) as u8,
                        (color::TEXT_FG[2] * 255.0) as u8,
                    ],
                    Some(font_family.clone()),
                    None,
                    None,
                ));
            }
        }

        if self.selected_module_name.is_some() {
            if is_vertical {
                let old_h = self.height;
                self.height = (left_x + margin_padding).round() as u32;
                log::debug!("[module-{}] rebuild_layout vertical: height calculated as {} (was {})", self.selected_module_name.as_deref().unwrap_or("none"), self.height, old_h);
                self.input_regions.clear();
                self.input_regions.push((0, 0, self.width as i32, self.height as i32));
            } else {
                let old_w = self.width;
                self.width = (left_x + margin_padding).round() as u32;
                log::debug!("[module-{}] rebuild_layout horizontal: width calculated as {} (was {})", self.selected_module_name.as_deref().unwrap_or("none"), self.width, old_w);
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
            // Rotate separators (rect lives on the Adapted base since the Phase 5 migration)
            for sep in &mut self.separators {
                let (old_x, old_y, old_w, old_h) = sep.rect();
                sep.set_rect(old_y, old_x, old_h, old_w);
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
            // Rotate text prims (swap x/y — tuple fields .2 and .3)
            for tp in &mut self.text_prims {
                let old_x = tp.2;
                let old_y = tp.3;
                tp.2 = old_y;
                tp.3 = old_x;
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

    fn trigger_switcher(&mut self, is_switcher_mode: bool) {
        // Keyboard alt-tab switching is implemented natively by the compositor
        // (bound to super+tab via the window_manager.window_switcher config key).
        // Delegate to it so there is a single window-switcher implementation.
        if is_switcher_mode {
            std::thread::spawn(|| {
                let _ = std::process::Command::new(get_ccectl_cmd())
                    .arg("window-switcher")
                    .spawn();
            });
            return;
        }

        // Otherwise this is a click on the status-bar "window" module: show a
        // click-to-pick list of the current windows.
        let switcher_source = "window".to_string();

        if self.cloud_popups.click(&switcher_source) == cce_ui::process::CloudPopupClick::ToggledOff {
            return;
        }

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

        cloud::spawn_window_picker(x_pos, y_pos, self.sender.clone(), switcher_source);
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
    let val = cce_ui::config::parse_kdl_to_json(&content);
    module_side_from_json(&val, name)
}

fn module_side_from_json(val: &serde_json::Value, name: &str) -> Side {
    if name == "light_source" {
        let mut light_pos = 2.356194490192345_f32; // Default 135 deg in rad
        if let Some(wm_obj) = json_find_key(val, "window_manager") {
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

    if let Some(side_val) = json_find_key(val, name) {
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

/// Ask the compositor for the current adjust-position-mode state
/// (`ccectl adjust-position-mode query` → `ok true|false`). `None` when the
/// query fails or the reply is unrecognized. Note: a pre-query compositor
/// treats the `query` argument as a toggle — the two repos ship together.
pub(crate) fn query_adjust_position_mode() -> Option<bool> {
    let out = std::process::Command::new(get_ccectl_cmd())
        .args(["adjust-position-mode", "query"])
        .output()
        .ok()?;
    match String::from_utf8_lossy(&out.stdout).trim() {
        "ok true" => Some(true),
        "ok false" => Some(false),
        _ => None,
    }
}

/// One window from `ccectl windows` output: (id, app_id, title, focused).
pub(crate) type CcectlWindow = (String, String, String, bool);

/// Parse one line of `ccectl windows` output. `window id=` and `app_id=` are
/// required; `title="…"` (truncated at the first inner quote — the wire format
/// does not escape) and `focused=` are optional.
pub(crate) fn parse_ccectl_window_line(line: &str) -> Option<CcectlWindow> {
    let app_id = {
        let idx = line.find("app_id=")?;
        let rest = &line[idx + 7..];
        let end = rest.find(' ').unwrap_or(rest.len());
        rest[..end].to_string()
    };

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

    let id = {
        let idx = line.find("window id=")?;
        let rest = &line[idx + 10..];
        let end = rest.find(' ').unwrap_or(rest.len());
        rest[..end].to_string()
    };

    Some((id, app_id, title, focused))
}

/// Parse one line of `ccectl windows --json` output.
pub(crate) fn parse_ccectl_window_json_line(line: &str) -> Option<CcectlWindow> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let id = v.get("id")?.as_u64()?.to_string();
    let app_id = v.get("app_id")?.as_str()?.to_string();
    let title = v.get("title").and_then(|t| t.as_str()).unwrap_or("").to_string();
    let focused = v.get("focused").and_then(|f| f.as_bool()).unwrap_or(false);
    Some((id, app_id, title, focused))
}

/// Parse one `ccectl windows` line in either format — JSON (`--json`) when the
/// compositor supports it, otherwise the legacy text format (an older
/// compositor ignores the `--json` flag and answers in text; titles containing
/// `"` are then truncated at the quote).
pub(crate) fn parse_ccectl_window_any_line(line: &str) -> Option<CcectlWindow> {
    if line.trim_start().starts_with('{') {
        parse_ccectl_window_json_line(line)
    } else {
        parse_ccectl_window_line(line)
    }
}

/// Parse `ccectl windows [--json]` output, dropping this app's own surfaces and
/// cce-cloud popups (they should never appear in the window picker).
pub(crate) fn parse_ccectl_windows(output: &str) -> Vec<CcectlWindow> {
    output
        .lines()
        .filter_map(parse_ccectl_window_any_line)
        .filter(|(_, app_id, _, _)| {
            app_id != "cce-status" && app_id != "cce-status-interface" && app_id != "cce-cloud"
        })
        .collect()
}

/// The frame's text as prim data: (text, size, x, y, color_u8, font, bounds, box-layout).
pub(crate) type TextPrim = (String, f32, f32, f32, [u8; 3], Option<String>, Option<[f32; 4]>, Option<cce_ui::scene::paint::TextLayout>);

/// Emit a measured `StyledLabel` as a text-prim tuple, returning its width (like the legacy
/// `StyledLabel::draw`). The label was built for its width; `into_prim` carries the source
/// text/size/family/box-layout so the engine reshapes it through the shared cache.
pub(crate) fn draw_label(prims: &mut Vec<TextPrim>, label: cce_ui::widget::StyledLabel, x: f32, y: f32) -> f32 {
    let w = label.w;
    let p = label.into_prim(x, y);
    prims.push((p.text, p.size, p.x, p.y, p.color, p.font, None, p.layout));
    w
}

impl cce_ui::engine::Application for StatusApp {
    type Message = CustomEvent;

    fn new(_qh: &wayland_client::QueueHandle<cce_ui::engine::EngineState<Self>>, sender: calloop::channel::Sender<Self::Message>) -> Self {
        let selected_module = parse_selected_module_from_args();

        let mut left_modules: Vec<Box<dyn StatusModule>> = Vec::new();
        let right_modules: Vec<Box<dyn StatusModule>> = Vec::new();

        let mut has_window = false;

        let (ref name, _) = selected_module
            .as_ref()
            .expect("StatusApp requires --module <name>; the no-arg form runs the launcher daemon");
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

        if has_window {
            tokio::spawn(spawn_status_listener("viewport", sender.clone()));
            tokio::spawn(spawn_status_listener("layout", sender.clone()));
            tokio::spawn(spawn_status_listener("title", sender.clone()));
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
            cloud_popups: cce_ui::process::CloudPopupTracker::new(),
            previously_focused_window: None,
            font_system,
            status_bar: cce_ui::widget::StatusBar::new(),
            rects: Vec::new(),
            overlay_rects: Vec::new(),
            rounded_boxes: Vec::new(),
            separators: Vec::new(),
            text_prims: Vec::new(),
            scale_factor: 1.0,
            width: if selected_module.is_some() { 120 } else { 1920 },
            height: read_status_height_from_config() as u32,
            needs_rebuild: true,
            current_bg_color: color::STATUS_BG,
            box_bevel: None,
            box_bevel_depth: 3.0,
            input_regions: Vec::new(),
            module_bounds: Vec::new(),
            left_modules,
            right_modules,
            sender,
            last_config_modified: cce_ui::config::config_files_modified(),
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
        // Default to redrawing; the high-frequency push events below clear this when
        // their value is unchanged, so a once-a-second stats poll (or a repeated
        // viewport/title push) no longer forces a redraw — and the compositor's
        // whole-backdrop blur re-bake — every time.
        let mut changed = true;
        match msg {
            CustomEvent::ViewportUpdated(t) => {
                changed = self.viewport != t;
                self.viewport = t;
            }
            CustomEvent::LayoutUpdated(l) => {
                changed = self.layout != l;
                self.layout = l;
            }
            CustomEvent::TitleUpdated(t) => {
                changed = self.title != t;
                self.title = t;
            }
            CustomEvent::SystemStatsUpdated(s) => {
                log::debug!("[module-{}] stats updated, current width={}", self.selected_module_name.as_deref().unwrap_or("none"), self.width);
                let module = self.selected_module_name.as_deref();
                match stats_signature(module, &s) {
                    // Module ignores stats (window/tray/light_source): never redraw here.
                    None => changed = false,
                    Some(new_sig) => {
                        let old_sig = self.stats.as_ref().and_then(|o| stats_signature(module, o));
                        changed = old_sig.as_deref() != Some(new_sig.as_str());
                    }
                }
                self.stats = Some(s);
            }
            CustomEvent::TrayUpdated(item) => {
                self.tray_items.insert(item.id.clone(), item);
            }
            CustomEvent::TrayRemoved(id) => {
                self.tray_items.remove(&id);
            }
            CustomEvent::CloudSpawned { pid, source } => {
                self.cloud_popups.on_spawned(pid, &source);
            }
            CustomEvent::CloudClosed { pid, source } => {
                if self.cloud_popups.on_closed(pid, &source) {
                    log::debug!("[cloud-event] CloudClosed: pid {} for source {} closed, clearing tracking", pid, source);
                    if source == "window" {
                        self.previously_focused_window = None;
                    } else if source == "layout" || source.starts_with("context_menu:") || source.starts_with("tray:") {
                        if let Some(ref focus_query) = self.previously_focused_window {
                            log::debug!("[cloud-event] Restoring focus to: {}", focus_query);
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
                log::debug!("[switcher] SwitcherTriggered event received, calling trigger_switcher");
                self.trigger_switcher(true);
            }
            CustomEvent::ToggleHideModules => {
                self.status_hide_mode = !self.status_hide_mode;
                let cmd = if self.status_hide_mode { "true" } else { "false" };
                if let Err(e) = std::process::Command::new(get_ccectl_cmd())
                    .args(["status-hide-mode", cmd])
                    .status()
                {
                    log::warn!("[hide-mode] ccectl status-hide-mode failed: {:?}", e);
                }
                *needs_rebuild = true;
            }
            CustomEvent::ToggleAdjustPositionMode => {
                // The compositor is the source of truth: sync to its state,
                // then send the flipped value.
                self.adjust_position_mode =
                    !query_adjust_position_mode().unwrap_or(self.adjust_position_mode);
                let cmd = if self.adjust_position_mode { "true" } else { "false" };
                if let Err(e) = std::process::Command::new(get_ccectl_cmd())
                    .args(["adjust-position-mode", cmd])
                    .status()
                {
                    log::warn!("[adjust-mode] ccectl adjust-position-mode failed: {:?}", e);
                }
                *needs_rebuild = true;
            }
        }
        if changed {
            self.needs_rebuild = true;
            *needs_rebuild = true;
        }
    }

    fn tick(&mut self, _dt: f32, needs_rebuild: &mut bool) {
        // Watches the shared config AND the app's own override file (the
        // newest mtime of the pair) — same key cce-ui's config cache uses.
        let modified = cce_ui::config::config_files_modified();
        if modified.is_some() && modified != self.last_config_modified {
            self.last_config_modified = modified;
            *needs_rebuild = true;
            self.needs_rebuild = true;
        }
    }

    fn display_list(&mut self, size: cce_ui::engine::LogicalSize, scale: f64) -> Option<cce_ui::scene::paint::DisplayList> {
        // Phase 6ak single paint path: the rounded boxes, the status-bar bg / module rects /
        // separators (the legacy view_rounded_quads then view() bodies, in the wrapper's
        // order), and the module text (prims, reshaped by the engine cache). overlay_quads
        // stays a separate on-top pass. The status bar's own text is never set in this app,
        // so it contributes only its background quad.
        use cce_ui::scene::layout::Rect;
        if self.needs_rebuild || self.width != size.width as u32 || self.height != size.height as u32 || self.scale_factor != scale {
            self.width = size.width as u32;
            self.height = size.height as u32;
            self.scale_factor = scale;
            cce_ui::scale::set_scale_factor(scale as f32);
            self.rebuild_layout();
        }
        let mut pc = cce_ui::scene::paint::PaintCtx::new();

        for rb in &self.rounded_boxes {
            let rect = Rect { x: rb.x, y: rb.y, width: rb.w, height: rb.h };
            // Same positional corner→radius mapping the RoundedRect prim uses.
            let radii = (
                if rb.corners.0 { rb.radius } else { 0.0 },
                if rb.corners.1 { rb.radius } else { 0.0 },
                if rb.corners.2 { rb.radius } else { 0.0 },
                if rb.corners.3 { rb.radius } else { 0.0 },
            );
            match self.box_bevel {
                Some(StatusBoxBevel::Raised) => {
                    // A lit plate: fill + rolled lip in one prim.
                    pc.bevel(rect, radii, rb.color, self.box_bevel_depth);
                }
                Some(StatusBoxBevel::Inset) => {
                    // Recess shades only the rim, so keep the flat fill under it.
                    if rb.radius > 0.1 {
                        pc.rounded_rect(rect, rb.radius, rb.corners, rb.color);
                    } else {
                        pc.quad(rect, rb.color);
                    }
                    pc.recess(rect, radii, self.box_bevel_depth);
                }
                None => {
                    if rb.radius > 0.1 {
                        pc.rounded_rect(rect, rb.radius, rb.corners, rb.color);
                    } else {
                        pc.quad(rect, rb.color);
                    }
                }
            }
        }

        let (sb_x, sb_y, sb_w, sb_h) = self.status_bar.rect();
        pc.quad(Rect { x: sb_x, y: sb_y, width: sb_w, height: sb_h }, self.status_bar.color());
        for r in &self.rects {
            pc.quad(Rect { x: r.x, y: r.y, width: r.w, height: r.h }, r.color);
        }
        for sep in &self.separators {
            let (x, y, w, h) = sep.rect();
            pc.quad(Rect { x, y, width: w, height: h }, sep.color());
        }

        for (text, tsize, x, y, color, font, bounds, layout) in &self.text_prims {
            match layout {
                Some(l) => pc.text_boxed(text.clone(), *x, *y, *tsize, *color, font.clone(), *bounds, cce_ui::scene::paint::TextAttrs::default(), *l),
                None => pc.text_with(text.clone(), *x, *y, *tsize, *color, font.clone(), *bounds),
            }
        }

        Some(pc.finish())
    }

    fn display_list_text(&self) -> bool {
        true
    }

    fn overlay_quads(&mut self, quads: &mut Vec<(f32, f32, f32, f32, [f32; 4])>, _size: cce_ui::engine::LogicalSize, _scale: f64) {
        for r in &self.overlay_rects {
            quads.push((r.x, r.y, r.w, r.h, r.color));
        }
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

    fn handle_mouse_input(&mut self, button: MouseButton, state: ElementState, pos: cce_ui::engine::LogicalPosition, _needs_rebuild: &mut bool) -> Option<Self::Message> {
        let (lx, ly) = (pos.x, pos.y);
        let is_vertical = self.is_vertical();
        let coord = if is_vertical { ly } else { lx };
        let cx = lx as f64;
        let cy = ly as f64;

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

                if self.cloud_popups.click(&tray_source) == cce_ui::process::CloudPopupClick::ToggledOff {
                    return None;
                }
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
                                            log::debug!("[tray-click] Clicked tray item at bound_x={}, bound_w={}, screen_width={}, calculated x_pos={}, y_pos={}", bound_x, bound_w, screen_width, x_pos, y_pos);

                                            if should_show_menu {
                                                 if let Some(menu_p) = menu_path {
                                                     menu_shown = true;
                                                     if let Err(e) = show_cce_cloud_menu(&conn, destination, menu_p.as_str(), x_pos, y_pos, true, thread_sender.clone(), tray_source_clone.clone(), parent_app_id.clone()).await {
                                                         log::warn!("[tray-click] show_cce_cloud_menu failed: {:?}", e);
                                                     }
                                                 }
                                             } else if btn_code == 272 {
                                                 if let Err(e) = proxy.activate(cx_i, cy_i).await {
                                                     log::warn!("[tray-click] Activate failed: {:?}", e);
                                                     if let Some(menu_p) = menu_path {
                                                         menu_shown = true;
                                                         if let Err(e) = show_cce_cloud_menu(&conn, destination, menu_p.as_str(), x_pos, y_pos, true, thread_sender.clone(), tray_source_clone.clone(), parent_app_id.clone()).await {
                                                             log::warn!("[tray-click] Fallback show_cce_cloud_menu failed: {:?}", e);
                                                         }
                                                     }
                                                 }
                                             } else if btn_code == 273 {
                                                 let _ = proxy.context_menu(cx_i, cy_i).await;
                                             }
                                        }
                                        Err(e) => {
                                            log::warn!("[tray-click] Failed to build proxy: {:?}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::warn!("[tray-click] Failed to connect to session bus: {:?}", e);
                                }
                            }
                        } else {
                            log::warn!("[tray-click] Failed to split id: {}", id);
                        }
                        if !menu_shown {
                            let _ = thread_sender.send(CustomEvent::CloudClosed { pid: 0, source: tray_source_clone });
                        }
                    });
                });
                return None;
            }

            if button == MouseButton::Right {
                self.adjust_position_mode =
                    query_adjust_position_mode().unwrap_or(self.adjust_position_mode);
                // Find which module was right-clicked
                let mut clicked_module = None;
                for mb in &self.module_bounds {
                    if coord >= mb.x && coord <= (mb.x + mb.w) {
                        clicked_module = Some(mb.clone());
                        break;
                    }
                }

                if let Some(mb) = clicked_module {
                    log::debug!("[module-right-click] Right-clicked module: {}", mb.name);
                    let context_source = format!("context_menu:{}", mb.name);

                    if self.cloud_popups.click(&context_source) == cce_ui::process::CloudPopupClick::ToggledOff {
                        return None;
                    }

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
                    let thread_sender = self.sender.clone();
                    std::thread::spawn(move || {
                        let popup = cce_ui::process::CloudPopup::at(x_pos, y_pos)
                            .parent_app_id(parent_app_id);
                        let mut spawned_pid = 0;
                        let result = popup.run_json(&context_json, |pid| {
                            spawned_pid = pid;
                            log::debug!("[module-right-click] Spawned cce-cloud with PID {}", pid);
                            let _ = thread_sender.send(CustomEvent::CloudSpawned { pid, source: context_source.clone() });
                        });
                        if let Ok(Some(out_str)) = &result {
                            if let Ok(parsed_json) = serde_json::from_str::<serde_json::Value>(out_str) {
                                if let Some(btn_id) = parsed_json.get("button").and_then(|v| v.as_str()) {
                                    if btn_id == "toggle_hide" {
                                        let _ = thread_sender.send(CustomEvent::ToggleHideModules);
                                    } else if btn_id == "toggle_adjust" {
                                        let _ = thread_sender.send(CustomEvent::ToggleAdjustPositionMode);
                                    }
                                }
                            }
                        }
                        let _ = thread_sender.send(CustomEvent::CloudClosed { pid: spawned_pid, source: context_source });
                    });

                    return None;
                }
            }

            if button == MouseButton::Left {
                log::debug!("[viewport-click] Mouse left click at logical: ({}, {})", cx, cy);
                
                // Check if layout mode was clicked
                let mut clicked_layout = false;
                if let Some(ref bounds) = self.layout_bounds {
                    if cx >= bounds.x as f64 && cx <= (bounds.x + bounds.w) as f64
                        && cy >= bounds.y as f64 && cy <= (bounds.y + bounds.h) as f64 {
                        clicked_layout = true;
                    }
                }

                if clicked_layout {
                    log::debug!("[layout-click] Layout mode clicked!");
                    let layout_source = "layout".to_string();

                    if self.cloud_popups.click(&layout_source) == cce_ui::process::CloudPopupClick::ToggledOff {
                        return None;
                    }

                    if self.previously_focused_window.is_none() {
                        self.previously_focused_window = get_currently_focused_window();
                    }

                    let x_pos = self.layout_bounds.as_ref().map(|b| b.x as i32).unwrap_or(0);
                    let y_pos = self.layout_bounds.as_ref().map(|b| b.h as i32).unwrap_or_else(|| read_status_height_from_config() as i32);
                    
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

                    let active_viewport = get_active_viewport_from_camera(&self.viewport);
                    let thread_sender = self.sender.clone();
                    std::thread::spawn(move || {
                        log::debug!("[layout-click] Active tag is {}", active_viewport);
                        let popup = cce_ui::process::CloudPopup::at(x_pos, y_pos);
                        let mut spawned_pid = 0;
                        let result = popup.run_json(&layout_json, |pid| {
                            spawned_pid = pid;
                            log::debug!("[layout-click] Spawned cce-cloud with PID {}", pid);
                            let _ = thread_sender.send(CustomEvent::CloudSpawned { pid, source: layout_source.clone() });
                        });
                        if let Ok(Some(out_str)) = &result {
                            #[derive(serde::Deserialize)]
                            struct LayoutMenuOutput {
                                button: String,
                                checkboxes: std::collections::HashMap<String, bool>,
                            }
                            if let Ok(val) = serde_json::from_str::<LayoutMenuOutput>(out_str) {
                                let selected_mode = val.button.to_lowercase();
                                let apply_all = val.checkboxes.get("apply_all").copied().unwrap_or(false);
                                if apply_all {
                                    log::debug!("[layout-click] Selected mode: {}, applying to all windows sharing mode", selected_mode);
                                    let _ = std::process::Command::new(get_ccectl_cmd())
                                        .args(["apply-mode-sharing", &selected_mode])
                                        .spawn();
                                } else {
                                    log::debug!("[layout-click] Selected mode: {}, setting for tag {}", selected_mode, active_viewport);
                                    let _ = std::process::Command::new(get_ccectl_cmd())
                                        .args(["viewport-layout", &active_viewport.to_string(), &selected_mode])
                                        .spawn();
                                }
                            } else {
                                // Fallback
                                let selected_lower = out_str.to_lowercase();
                                let _ = std::process::Command::new(get_ccectl_cmd())
                                    .args(["viewport-layout", &active_viewport.to_string(), &selected_lower])
                                    .spawn();
                            }
                        }
                        let _ = thread_sender.send(CustomEvent::CloudClosed { pid: spawned_pid, source: layout_source });
                    });
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
                                    log::debug!("[viewport-click-via-window] Viewport matched: {}", bound.name);
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
                                log::debug!("[window-click] Window module clicked (no window focused, fallback to switcher)!");
                                self.trigger_switcher(false);
                            }
                        } else {
                            log::debug!("[window-click] Window module clicked (window focused)!");
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








fn main() {
    // Info by default; RUST_LOG (e.g. =debug) takes precedence when set.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    log::info!("cce-status-interface started with args = {:?}", args);
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

    let has_module = args.iter().any(|arg| arg == "--module");

    if !has_module {
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

            // Restart crashed modules with exponential backoff: a module
            // that keeps dying quickly waits longer each time (up to
            // RESTART_MAX) instead of respawning twice a second; a run of
            // HEALTHY_UPTIME resets its backoff.
            const RESTART_BASE: std::time::Duration = std::time::Duration::from_millis(500);
            const RESTART_MAX: std::time::Duration = std::time::Duration::from_secs(30);
            const HEALTHY_UPTIME: std::time::Duration = std::time::Duration::from_secs(30);

            struct Supervised {
                child: Option<std::process::Child>,
                spawned_at: std::time::Instant,
                backoff: std::time::Duration,
                restart_at: std::time::Instant,
            }

            let spawn_module = |module: &str| {
                std::process::Command::new(&current_exe)
                    .arg("--module")
                    .arg(module)
                    .spawn()
            };

            let mut supervised: std::collections::HashMap<String, Supervised> = std::collections::HashMap::new();

            for module in &modules {
                let now = std::time::Instant::now();
                let child = match spawn_module(module) {
                    Ok(c) => {
                        log::info!("Spawned module process for: {}", module);
                        Some(c)
                    }
                    Err(e) => {
                        log::error!("Failed to spawn module process for {}: {:?}", module, e);
                        None
                    }
                };
                supervised.insert(module.to_string(), Supervised {
                    child,
                    spawned_at: now,
                    backoff: RESTART_BASE,
                    restart_at: now + RESTART_BASE,
                });
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
                        let now = std::time::Instant::now();
                        for module in &modules {
                            let Some(entry) = supervised.get_mut(*module) else { continue };

                            if let Some(child) = entry.child.as_mut() {
                                match child.try_wait() {
                                    Ok(None) => continue,
                                    Ok(Some(status)) => {
                                        entry.child = None;
                                        if entry.spawned_at.elapsed() >= HEALTHY_UPTIME {
                                            entry.backoff = RESTART_BASE;
                                        } else {
                                            entry.backoff = (entry.backoff * 2).min(RESTART_MAX);
                                        }
                                        entry.restart_at = now + entry.backoff;
                                        log::warn!(
                                            "Module process '{}' exited with status: {:?}. Restarting in {:?}...",
                                            module, status, entry.backoff
                                        );
                                    }
                                    Err(e) => {
                                        log::error!("Error checking status for module '{}': {:?}", module, e);
                                        continue;
                                    }
                                }
                            }

                            if now >= entry.restart_at {
                                match spawn_module(module) {
                                    Ok(c) => {
                                        log::info!("Restarted module process for: {}", module);
                                        entry.child = Some(c);
                                        entry.spawned_at = now;
                                    }
                                    Err(e) => {
                                        entry.backoff = (entry.backoff * 2).min(RESTART_MAX);
                                        entry.restart_at = now + entry.backoff;
                                        log::error!(
                                            "Failed to restart module process for {}: {:?}. Retrying in {:?}...",
                                            module, e, entry.backoff
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }

            for (module, entry) in supervised {
                if let Some(mut child) = entry.child {
                    log::info!("Killing module process: {}", module);
                    let _ = child.kill();
                }
            }
        });
        return;
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let _guard = rt.enter();

    cce_ui::engine::run::<StatusApp>();
}


#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Characterization tests (phase 0): these pin down current behavior
    // before the refactors in PROPOSAL.md. Where the behavior is odd, the
    // test documents it rather than fixing it. The config-lookup and color
    // tests moved to config.rs with the phase-2 rewrite.
    // ------------------------------------------------------------------

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

    // --- parse_viewport_text ---

    #[test]
    fn viewport_text_single_span() {
        let out = parse_viewport_text("<span color='#ff0000'>1</span>");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, "1");
        assert_rgba_close(out[0].0, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn viewport_text_multiple_spans() {
        let out = parse_viewport_text(
            "<span color='#ff0000'>1</span><span color='#00ff00'>2</span>",
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].1, "1");
        assert_eq!(out[1].1, "2");
        assert_rgba_close(out[1].0, [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn viewport_text_json_wrapped() {
        let out = parse_viewport_text(r##"{"text": "<span color='#0000ff'>3</span>"}"##);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, "3");
        assert_rgba_close(out[0].0, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn viewport_text_plain_text_falls_back_to_default_color() {
        let out = parse_viewport_text("hello");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, "hello");
        assert_rgba_close(out[0].0, [0.8, 0.8, 0.8, 1.0]);
    }

    #[test]
    fn viewport_text_unterminated_span_falls_back_to_raw_input() {
        // A span with no closing tag aborts markup parsing; the whole raw
        // input (markup included) is emitted with the default color.
        let input = "<span color='#ff0000'>abc";
        let out = parse_viewport_text(input);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, input);
        assert_rgba_close(out[0].0, [0.8, 0.8, 0.8, 1.0]);
    }

    #[test]
    fn viewport_text_bad_hex_gets_default_color() {
        let out = parse_viewport_text("<span color='zzz'>x</span>");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, "x");
        assert_rgba_close(out[0].0, [0.8, 0.8, 0.8, 1.0]);
    }

    #[test]
    fn viewport_text_empty_input_is_empty() {
        assert!(parse_viewport_text("").is_empty());
    }

    // --- module_side_from_json ---

    #[test]
    fn module_side_explicit_values() {
        let val = serde_json::json!({"clock": "left", "window": "right"});
        assert_eq!(module_side_from_json(&val, "clock"), Side::Left);
        assert_eq!(module_side_from_json(&val, "window"), Side::Right);
    }

    #[test]
    fn module_side_snap_aliases() {
        for (snap, side) in [
            ("top-left", Side::Left),
            ("bottom-left", Side::Left),
            ("top-center", Side::Left),
            ("bottom-center", Side::Left),
            ("top-right", Side::Right),
            ("bottom-right", Side::Right),
            ("TOP-LEFT", Side::Left), // case-insensitive
        ] {
            let val = serde_json::json!({"cpu": snap});
            assert_eq!(module_side_from_json(&val, "cpu"), side, "snap {}", snap);
        }
    }

    #[test]
    fn module_side_defaults() {
        let val = serde_json::json!({});
        assert_eq!(module_side_from_json(&val, "window"), Side::Left);
        assert_eq!(module_side_from_json(&val, "clock"), Side::Right);
        assert_eq!(module_side_from_json(&val, "tray"), Side::Right);
    }

    #[test]
    fn module_side_unknown_value_falls_through_to_default() {
        let val = serde_json::json!({"window": "sideways", "clock": "sideways"});
        assert_eq!(module_side_from_json(&val, "window"), Side::Left);
        assert_eq!(module_side_from_json(&val, "clock"), Side::Right);
    }

    #[test]
    fn module_side_light_source_from_angle() {
        // Left iff angle (rad) in [5π/8, 11π/8); default 135° is Left.
        let mk = |v: serde_json::Value| serde_json::json!({"window_manager": {"light_source_position": v}});
        assert_eq!(module_side_from_json(&serde_json::json!({}), "light_source"), Side::Left);
        // Float values are radians.
        assert_eq!(
            module_side_from_json(&mk(serde_json::json!(std::f64::consts::PI)), "light_source"),
            Side::Left
        );
        assert_eq!(module_side_from_json(&mk(serde_json::json!(0.0)), "light_source"), Side::Right);
        // Integers > 2π are degrees, otherwise radians.
        assert_eq!(module_side_from_json(&mk(serde_json::json!(180)), "light_source"), Side::Left);
        assert_eq!(module_side_from_json(&mk(serde_json::json!(3)), "light_source"), Side::Left);
        assert_eq!(module_side_from_json(&mk(serde_json::json!(0)), "light_source"), Side::Right);
    }

    // --- parse_ccectl_windows ---

    #[test]
    fn ccectl_windows_full_line() {
        let out = parse_ccectl_windows(
            "window id=3 app_id=firefox title=\"Mozilla Firefox\" focused=true\n\
             window id=7 app_id=kitty title=\"~\" focused=false",
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], ("3".into(), "firefox".into(), "Mozilla Firefox".into(), true));
        assert_eq!(out[1], ("7".into(), "kitty".into(), "~".into(), false));
    }

    #[test]
    fn ccectl_windows_missing_required_fields_skips_line() {
        // No app_id → skipped; no window id → skipped.
        assert!(parse_ccectl_windows("window id=3 title=\"x\"").is_empty());
        assert!(parse_ccectl_windows("app_id=firefox title=\"x\"").is_empty());
    }

    #[test]
    fn ccectl_windows_optional_fields_default() {
        let out = parse_ccectl_windows("window id=3 app_id=firefox");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], ("3".into(), "firefox".into(), "".into(), false));
    }

    #[test]
    fn ccectl_windows_filters_own_surfaces() {
        let out = parse_ccectl_windows(
            "window id=1 app_id=cce-status\n\
             window id=2 app_id=cce-status-interface\n\
             window id=3 app_id=cce-cloud\n\
             window id=4 app_id=firefox",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, "firefox");
    }

    #[test]
    fn ccectl_windows_title_truncates_at_inner_quote() {
        // Known limitation of the legacy text format kept as the fallback for
        // pre---json compositors: titles are not escaped, so an inner quote
        // truncates the title. The JSON path below handles this correctly.
        let out = parse_ccectl_windows("window id=3 app_id=x title=\"say \"hi\"\" focused=false");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].2, "say ");
    }

    // --- parse_ccectl_windows, JSON format (`windows --json`) ---

    #[test]
    fn ccectl_windows_json_full_line() {
        let out = parse_ccectl_windows(
            r#"{"id":3,"app_id":"firefox","title":"hello","mode":"grid","x":0,"y":0,"w":800,"h":600,"vx":0.0,"vy":0.0,"minimized":false,"has_parent":false,"focused":true,"ssd":false}"#,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], ("3".to_string(), "firefox".to_string(), "hello".to_string(), true));
    }

    #[test]
    fn ccectl_windows_json_title_with_quotes_and_spaces() {
        // The reason --json exists: titles survive quoting untouched.
        let out = parse_ccectl_windows(
            r#"{"id":3,"app_id":"x","title":"say \"hi\" title=fake","focused":false}"#,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].2, "say \"hi\" title=fake");
    }

    #[test]
    fn ccectl_windows_json_filters_own_surfaces() {
        let out = parse_ccectl_windows(
            "{\"id\":1,\"app_id\":\"cce-status\",\"title\":\"\",\"focused\":false}\n\
             {\"id\":2,\"app_id\":\"cce-cloud\",\"title\":\"\",\"focused\":false}\n\
             {\"id\":3,\"app_id\":\"firefox\",\"title\":\"\",\"focused\":false}",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, "firefox");
    }

    #[test]
    fn ccectl_windows_json_missing_required_fields_skips_line() {
        assert!(parse_ccectl_windows(r#"{"app_id":"x","title":"no id"}"#).is_empty());
        assert!(parse_ccectl_windows(r#"{"id":3,"title":"no app_id"}"#).is_empty());
        assert!(parse_ccectl_windows("{not json").is_empty());
    }
}

