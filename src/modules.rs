use std::collections::HashMap;
use glyphon::FontSystem;
use cce_ui::color;
use cce_ui::widget::StyledLabel as Label;

use crate::{
    RectWidget, RoundedBox, ViewportBounds, LayoutBounds, SystemStats, TrayItem,
    TrayIconBounds, make_text_buffer, parse_viewport_text,
};

/// Vertical offset that centers a text run in a box `box_h` tall. The engine
/// shapes horizontal text with a line box of exactly `font_size` (cce-ui
/// window_runner uses line_height = physical_size * 1.0), so centering must
/// use that height — not a CSS-ish 1.4em line box.
pub(crate) fn centered_text_y(box_h: f32, font_size: f32) -> f32 {
    (box_h - font_size) / 2.0
}

pub trait StatusModule {
    fn name(&self) -> &'static str;
    
    fn has_custom_background(&self, _title: &str) -> bool { false }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        viewport: &str,
        layout: &str,
        title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32;

    fn render(
        &self,
        x: f32,
        w: f32,
        stats: &Option<SystemStats>,
        viewport: &str,
        layout: &str,
        title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        scale_factor: f64,
        text_prims: &mut Vec<crate::TextPrim>,
        rects: &mut Vec<RectWidget>,
        overlay_rects: &mut Vec<RectWidget>,
        viewport_bounds: &mut Vec<ViewportBounds>,
        layout_bounds: &mut Option<LayoutBounds>,
        tray_items: &HashMap<String, TrayItem>,
        tray_item_bounds: &mut Vec<TrayIconBounds>,
        box_bg_color: Option<[f32; 4]>,
        status_box_radius: f32,
        rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    );
}

pub struct WindowModule;

impl StatusModule for WindowModule {
    fn name(&self) -> &'static str { "window" }

    fn has_custom_background(&self, title: &str) -> bool {
        title.is_empty() || title == "(none)"
    }

    fn width(
        &self,
        _stats: &Option<SystemStats>,
        viewport: &str,
        layout: &str,
        title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let has_title = !title.is_empty() && title != "(none)";
        if has_title {
            let has_layout = !layout.is_empty();
            let mut total_w = 0.0;
            if has_title {
                let mut display_title = title.to_string();
                if display_title.chars().count() > 40 {
                    display_title = display_title.chars().take(37).collect::<String>() + "...";
                }
                let label = Label::new_with_family(font_system, &display_title, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
                total_w += label.w;
            }

            if has_title && has_layout {
                let sep_label = Label::new_with_family(font_system, " - ", font_size, [0.0, 0.0, 0.0, 1.0], font_family);
                total_w += sep_label.w;
            }

            if has_layout {
                let layout_label = Label::new_with_family(font_system, layout, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
                total_w += layout_label.w;
            }

            total_w + 2.0 * padding
        } else {
            let viewport_parsed = parse_viewport_text(viewport);
            if viewport_parsed.is_empty() {
                0.0
            } else {
                let mut total_w = 0.0;
                for (col, text) in &viewport_parsed {
                    let label = Label::new_with_family(font_system, text, font_size, *col, font_family);
                    total_w += label.w + 2.0 * padding + 4.0;
                }
                if total_w > 0.0 { total_w - 4.0 } else { 0.0 }
            }
        }
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        _stats: &Option<SystemStats>,
        viewport: &str,
        layout: &str,
        title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_prims: &mut Vec<crate::TextPrim>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        viewport_bounds: &mut Vec<ViewportBounds>,
        layout_bounds: &mut Option<LayoutBounds>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        box_bg_color: Option<[f32; 4]>,
        status_box_radius: f32,
        rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        let has_title = !title.is_empty() && title != "(none)";
        if has_title {
            let has_layout = !layout.is_empty();
            let mut cur_x = x + padding;
            let y_pos = centered_text_y(bar_h, font_size);

            if has_title {
                let mut display_title = title.to_string();
                if display_title.chars().count() > 40 {
                    display_title = display_title.chars().take(37).collect::<String>() + "...";
                }
                let label = Label::new_with_family(font_system, &display_title, font_size, normal_color, font_family);
                let w = label.w;
                crate::draw_label(text_prims, label, cur_x, y_pos);
                cur_x += w;
            }

            if has_title && has_layout {
                let sep_label = Label::new_with_family(font_system, " - ", font_size, normal_color, font_family);
                let w = sep_label.w;
                crate::draw_label(text_prims, sep_label, cur_x, y_pos);
                cur_x += w;
            }

            if has_layout {
                let layout_label = Label::new_with_family(font_system, layout, font_size, normal_color, font_family);
                let w = layout_label.w;
                crate::draw_label(text_prims, layout_label, cur_x, y_pos);
                *layout_bounds = Some(LayoutBounds {
                    x: cur_x - 2.0,
                    y: 0.0,
                    w: w + 4.0,
                    h: bar_h,
                });
            }
        } else {
            let viewport_parsed = parse_viewport_text(viewport);
            let mut cur_x = x;
            for (col, text) in viewport_parsed {
                let label = Label::new_with_family(font_system, &text, font_size, col, font_family);
                let box_w = label.w + 2.0 * padding;
                if let Some(color) = box_bg_color {
                    rounded_boxes.push(RoundedBox {
                        x: cur_x,
                        y: 0.0,
                        w: box_w,
                        h: bar_h,
                        radius: status_box_radius,
                        color,
                        corners: (false, false, true, true),
                    });
                }
                crate::draw_label(text_prims, label, cur_x + padding, centered_text_y(bar_h, font_size));
                viewport_bounds.push(ViewportBounds {
                    name: text.clone(),
                    x: cur_x,
                    y: 0.0,
                    w: box_w,
                    h: bar_h,
                });
                cur_x += box_w + 4.0;
            }
        }
    }
}

pub struct ClockModule;

impl StatusModule for ClockModule {
    fn name(&self) -> &'static str { "clock" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let text = if let Some(ref s) = stats {
            &s.clock
        } else {
            "Monday, January 01, 2000 00:00 AM"
        };
        let label = Label::new_with_family(font_system, text, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
        label.w + 2.0 * padding
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_prims: &mut Vec<crate::TextPrim>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _viewport_bounds: &mut Vec<ViewportBounds>,
        _layout_bounds: &mut Option<LayoutBounds>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        if let Some(ref s) = stats {
            let label = Label::new_with_family(font_system, &s.clock, font_size, normal_color, font_family);
            crate::draw_label(text_prims, label, x + padding, centered_text_y(bar_h, font_size));
        }
    }
}

pub struct BatteryModule;

impl StatusModule for BatteryModule {
    fn name(&self) -> &'static str { "battery" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let text = if let Some(ref s) = stats {
            if !s.battery.is_empty() { &s.battery } else { "Bat 100%" }
        } else {
            "Bat 100%"
        };
        let label = Label::new_with_family(font_system, text, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
        label.w + 2.0 * padding
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_prims: &mut Vec<crate::TextPrim>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _viewport_bounds: &mut Vec<ViewportBounds>,
        _layout_bounds: &mut Option<LayoutBounds>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        if let Some(ref s) = stats {
            if !s.battery.is_empty() {
                let bat_color = if !s.battery_charging && s.battery_capacity > 10 {
                    normal_color
                } else {
                    color::TEXT_ACCENT
                };
                let label = Label::new_with_family(font_system, &s.battery, font_size, bat_color, font_family);
                crate::draw_label(text_prims, label, x + padding, centered_text_y(bar_h, font_size));
            }
        }
    }
}

pub struct VolumeModule;

impl StatusModule for VolumeModule {
    fn name(&self) -> &'static str { "volume" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let text = if let Some(ref s) = stats {
            if !s.volume.is_empty() { &s.volume } else { "Vol 100%" }
        } else {
            "Vol 100%"
        };
        let label = Label::new_with_family(font_system, text, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
        label.w + 2.0 * padding
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        scale_factor: f64,
        text_prims: &mut Vec<crate::TextPrim>,
        _rects: &mut Vec<RectWidget>,
        overlay_rects: &mut Vec<RectWidget>,
        _viewport_bounds: &mut Vec<ViewportBounds>,
        _layout_bounds: &mut Option<LayoutBounds>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        if let Some(ref s) = stats {
            if !s.volume.is_empty() {
                let is_muted = s.volume_muted;
                let color_val = if is_muted {
                    crate::read_disabled_color_from_config().unwrap_or(color::TEXT_DIM)
                } else {
                    normal_color
                };
                let label = Label::new_with_family(font_system, &s.volume, font_size, color_val, font_family)
                    .with_strikethrough(is_muted);
                let draw_x = x + padding;
                let start_y = centered_text_y(bar_h, font_size);
                if let Some((sx, sy, sw_rect, sh_rect, scol)) = label.strikethrough_rect(draw_x, start_y, scale_factor as f32) {
                    overlay_rects.push(RectWidget {
                        x: sx,
                        y: sy,
                        w: sw_rect,
                        h: sh_rect,
                        color: color::to_linear(scol),
                    });
                }
                crate::draw_label(text_prims, label, draw_x, start_y);
            }
        }
    }
}

pub struct BrightnessModule;

impl StatusModule for BrightnessModule {
    fn name(&self) -> &'static str { "brightness" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let text = if let Some(ref s) = stats {
            if !s.brightness.is_empty() { &s.brightness } else { "Bri 100%" }
        } else {
            "Bri 100%"
        };
        let label = Label::new_with_family(font_system, text, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
        label.w + 2.0 * padding
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_prims: &mut Vec<crate::TextPrim>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _viewport_bounds: &mut Vec<ViewportBounds>,
        _layout_bounds: &mut Option<LayoutBounds>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        if let Some(ref s) = stats {
            if !s.brightness.is_empty() {
                let label = Label::new_with_family(font_system, &s.brightness, font_size, normal_color, font_family);
                crate::draw_label(text_prims, label, x + padding, centered_text_y(bar_h, font_size));
            }
        }
    }
}

pub struct MemoryModule;

impl StatusModule for MemoryModule {
    fn name(&self) -> &'static str { "memory" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let text = if let Some(ref s) = stats {
            if !s.memory.is_empty() { &s.memory } else { "Mem 0.0/0.0G" }
        } else {
            "Mem 0.0/0.0G"
        };
        let label = Label::new_with_family(font_system, text, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
        label.w + 2.0 * padding
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_prims: &mut Vec<crate::TextPrim>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _viewport_bounds: &mut Vec<ViewportBounds>,
        _layout_bounds: &mut Option<LayoutBounds>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        if let Some(ref s) = stats {
            let label = Label::new_with_family(font_system, &s.memory, font_size, normal_color, font_family);
            crate::draw_label(text_prims, label, x + padding, centered_text_y(bar_h, font_size));
        }
    }
}

pub struct CpuModule;

impl StatusModule for CpuModule {
    fn name(&self) -> &'static str { "cpu" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let text = if let Some(ref s) = stats {
            if !s.cpu.is_empty() { &s.cpu } else { "Cpu 0.0%" }
        } else {
            "Cpu 0.0%"
        };
        let label = Label::new_with_family(font_system, text, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
        label.w + 2.0 * padding
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_prims: &mut Vec<crate::TextPrim>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _viewport_bounds: &mut Vec<ViewportBounds>,
        _layout_bounds: &mut Option<LayoutBounds>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        if let Some(ref s) = stats {
            let label = Label::new_with_family(font_system, &s.cpu, font_size, normal_color, font_family);
            crate::draw_label(text_prims, label, x + padding, centered_text_y(bar_h, font_size));
        }
    }
}

pub struct TrayModule;

impl StatusModule for TrayModule {
    fn name(&self) -> &'static str { "tray" }

    fn width(
        &self,
        _stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        _font_system: &mut FontSystem,
        _font_family: &str,
        _font_size: f32,
        tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        if tray_items.is_empty() {
            0.0
        } else {
            let len = tray_items.len() as f32;
            (len * 16.0) + ((len - 1.0) * 8.0) + 2.0 * padding
        }
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        _stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _normal_color: [f32; 4],
        bar_h: f32,
        scale_factor: f64,
        text_prims: &mut Vec<crate::TextPrim>,
        _rects: &mut Vec<RectWidget>,
        overlay_rects: &mut Vec<RectWidget>,
        _viewport_bounds: &mut Vec<ViewportBounds>,
        _layout_bounds: &mut Option<LayoutBounds>,
        tray_items: &HashMap<String, TrayItem>,
        tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        if tray_items.is_empty() {
            return;
        }
        let mut sorted_tray: Vec<&TrayItem> = tray_items.values().collect();
        sorted_tray.sort_by_key(|item| &item.id);

        for (i, item) in sorted_tray.iter().enumerate() {
            let icon_size = 16.0;
            let icon_x = x + padding + (i as f32) * (icon_size + 8.0);
            let icon_y = (bar_h - icon_size) / 2.0;

            tray_item_bounds.push(TrayIconBounds {
                id: item.id.clone(),
                x: icon_x,
                y: icon_y,
                w: icon_size,
                h: icon_size,
                title: item.title.clone(),
                dbus_id: item.dbus_id.clone(),
            });

            let mut drawn_pixmap = false;
            if let Some(ref pixmaps) = item.pixmaps {
                if !pixmaps.is_empty() {
                    let target_pixel_width = (icon_size * scale_factor as f32) as i32;
                    if let Some(pixmap) = pixmaps.iter().min_by_key(|p| (p.width - target_pixel_width).abs()) {
                        if pixmap.width > 0 && pixmap.height > 0 {
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
                                            
                                            overlay_rects.push(RectWidget {
                                                x: icon_x + col as f32 * pixel_w,
                                                y: icon_y + row as f32 * pixel_h,
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

                // Measure the throwaway buffer for centering, then emit a text prim.
                let buf = make_text_buffer(font_system, symbol, font_size, font_family);
                let scale = cce_ui::scale::scale_factor();
                let tw = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0) / scale;
                let tx = icon_x + (icon_size - tw) / 2.0;
                let ty = icon_y + centered_text_y(icon_size, font_size);
                text_prims.push((
                    symbol.to_string(),
                    font_size,
                    tx,
                    ty,
                    [
                        (color::TEXT_ACCENT[0] * 255.0) as u8,
                        (color::TEXT_ACCENT[1] * 255.0) as u8,
                        (color::TEXT_ACCENT[2] * 255.0) as u8,
                    ],
                    Some(font_family.to_string()),
                    None,
                    None,
                ));
            }
        }
    }
}

pub struct LightSourceModule;

fn get_light_source_pos_from_config() -> f32 {
    let val = crate::get_cached_config();
    if let Some(pos_val) = crate::json_find_key(&val, "light_source_position") {
        if let Some(f) = pos_val.as_f64() {
            let val = f as f32;
            if val > 2.0 * std::f32::consts::PI {
                return val.to_radians();
            } else {
                return val;
            }
        }
    }
    2.35619
}

impl StatusModule for LightSourceModule {
    fn name(&self) -> &'static str { "light_source" }

    fn width(
        &self,
        _stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let pos = get_light_source_pos_from_config();
        let text = format!("Light: {:.2} rad", pos);
        let label = Label::new_with_family(font_system, &text, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
        label.w + 2.0 * padding
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        _stats: &Option<SystemStats>,
        _viewport: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_prims: &mut Vec<crate::TextPrim>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _viewport_bounds: &mut Vec<ViewportBounds>,
        _layout_bounds: &mut Option<LayoutBounds>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        let pos = get_light_source_pos_from_config();
        let text = format!("Light: {:.2} rad", pos);
        let label = Label::new_with_family(font_system, &text, font_size, normal_color, font_family);
        crate::draw_label(text_prims, label, x + padding, centered_text_y(bar_h, font_size));
    }
}
