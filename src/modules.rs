use std::collections::HashMap;
use glyphon::FontSystem;
use cce_ui::color;
use cce_ui::widget::{StyledLabel as Label, TextItem};

use crate::{
    RectWidget, RoundedBox, TagBounds, LayoutBounds, SystemStats, TrayItem,
    TrayIconBounds, make_text_buffer, parse_tags,
};

pub trait StatusModule {
    fn name(&self) -> &'static str;
    
    fn has_custom_background(&self) -> bool { false }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        tags: &str,
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
        tags: &str,
        layout: &str,
        title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        scale_factor: f64,
        text_items: &mut Vec<TextItem>,
        rects: &mut Vec<RectWidget>,
        overlay_rects: &mut Vec<RectWidget>,
        tag_bounds: &mut Vec<TagBounds>,
        layout_bounds: &mut Option<LayoutBounds>,
        tray_items: &HashMap<String, TrayItem>,
        tray_item_bounds: &mut Vec<TrayIconBounds>,
        box_bg_color: Option<[f32; 4]>,
        status_box_radius: f32,
        rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    );
}

pub struct TagsModule;

impl StatusModule for TagsModule {
    fn name(&self) -> &'static str { "tags" }
    
    fn has_custom_background(&self) -> bool { true }

    fn width(
        &self,
        _stats: &Option<SystemStats>,
        tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let tags_parsed = parse_tags(tags);
        if tags_parsed.is_empty() {
            0.0
        } else {
            let mut total_w = 0.0;
            for (col, text) in &tags_parsed {
                let label = Label::new_with_family(font_system, text, font_size, *col, font_family);
                total_w += label.w + 2.0 * padding + 4.0;
            }
            if total_w > 0.0 { total_w - 4.0 } else { 0.0 }
        }
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        _stats: &Option<SystemStats>,
        tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_items: &mut Vec<TextItem>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        tag_bounds: &mut Vec<TagBounds>,
        _layout_bounds: &mut Option<LayoutBounds>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        box_bg_color: Option<[f32; 4]>,
        status_box_radius: f32,
        rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        let tags_parsed = parse_tags(tags);
        let mut cur_x = x;
        for (col, text) in tags_parsed {
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
            label.draw(text_items, cur_x + padding, (bar_h - font_size * 1.4) / 2.0);
            tag_bounds.push(TagBounds {
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

pub struct LayoutModule;

impl StatusModule for LayoutModule {
    fn name(&self) -> &'static str { "layout" }

    fn width(
        &self,
        _stats: &Option<SystemStats>,
        _tags: &str,
        layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        if layout.is_empty() {
            0.0
        } else {
            let label = Label::new_with_family(font_system, layout, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
            label.w + 2.0 * padding
        }
    }

    fn render(
        &self,
        x: f32,
        w: f32,
        _stats: &Option<SystemStats>,
        _tags: &str,
        layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_items: &mut Vec<TextItem>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tag_bounds: &mut Vec<TagBounds>,
        layout_bounds: &mut Option<LayoutBounds>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        if !layout.is_empty() {
            let label = Label::new_with_family(font_system, layout, font_size, normal_color, font_family);
            label.draw(text_items, x + padding, (bar_h - font_size * 1.4) / 2.0);
            *layout_bounds = Some(LayoutBounds {
                x,
                y: 0.0,
                w,
                h: bar_h,
            });
        }
    }
}

pub struct TitleModule;

impl StatusModule for TitleModule {
    fn name(&self) -> &'static str { "title" }

    fn width(
        &self,
        _stats: &Option<SystemStats>,
        _tags: &str,
        _layout: &str,
        title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        if title.is_empty() || title == "(none)" {
            0.0
        } else {
            let mut display_title = title.to_string();
            if display_title.chars().count() > 40 {
                display_title = display_title.chars().take(37).collect::<String>() + "...";
            }
            let label = Label::new_with_family(font_system, &display_title, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
            label.w + 2.0 * padding
        }
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        _stats: &Option<SystemStats>,
        _tags: &str,
        _layout: &str,
        title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_items: &mut Vec<TextItem>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tag_bounds: &mut Vec<TagBounds>,
        _layout_bounds: &mut Option<LayoutBounds>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        if !title.is_empty() && title != "(none)" {
            let mut display_title = title.to_string();
            if display_title.chars().count() > 40 {
                display_title = display_title.chars().take(37).collect::<String>() + "...";
            }
            let label = Label::new_with_family(font_system, &display_title, font_size, normal_color, font_family);
            label.draw(text_items, x + padding, (bar_h - font_size * 1.4) / 2.0);
        }
    }
}

pub struct ClockModule;

impl StatusModule for ClockModule {
    fn name(&self) -> &'static str { "clock" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        if let Some(ref s) = stats {
            let label = Label::new_with_family(font_system, &s.clock, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
            label.w + 2.0 * padding
        } else {
            0.0
        }
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_items: &mut Vec<TextItem>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tag_bounds: &mut Vec<TagBounds>,
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
            label.draw(text_items, x + padding, (bar_h - font_size * 1.4) / 2.0);
        }
    }
}

pub struct BatteryModule;

impl StatusModule for BatteryModule {
    fn name(&self) -> &'static str { "battery" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        if let Some(ref s) = stats {
            if !s.battery.is_empty() {
                let label = Label::new_with_family(font_system, &s.battery, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
                label.w + 2.0 * padding
            } else {
                0.0
            }
        } else {
            0.0
        }
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_items: &mut Vec<TextItem>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tag_bounds: &mut Vec<TagBounds>,
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
                label.draw(text_items, x + padding, (bar_h - font_size * 1.4) / 2.0);
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
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        if let Some(ref s) = stats {
            if !s.volume.is_empty() {
                let label = Label::new_with_family(font_system, &s.volume, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
                label.w + 2.0 * padding
            } else {
                0.0
            }
        } else {
            0.0
        }
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        scale_factor: f64,
        text_items: &mut Vec<TextItem>,
        _rects: &mut Vec<RectWidget>,
        overlay_rects: &mut Vec<RectWidget>,
        _tag_bounds: &mut Vec<TagBounds>,
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
                let start_y = (bar_h - font_size * 1.4) / 2.0;
                if let Some((sx, sy, sw_rect, sh_rect, scol)) = label.strikethrough_rect(draw_x, start_y, scale_factor as f32) {
                    overlay_rects.push(RectWidget {
                        x: sx,
                        y: sy,
                        w: sw_rect,
                        h: sh_rect,
                        color: color::to_linear(scol),
                    });
                }
                label.draw(text_items, draw_x, start_y);
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
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        if let Some(ref s) = stats {
            if !s.brightness.is_empty() {
                let label = Label::new_with_family(font_system, &s.brightness, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
                label.w + 2.0 * padding
            } else {
                0.0
            }
        } else {
            0.0
        }
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_items: &mut Vec<TextItem>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tag_bounds: &mut Vec<TagBounds>,
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
                label.draw(text_items, x + padding, (bar_h - font_size * 1.4) / 2.0);
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
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        if let Some(ref s) = stats {
            let label = Label::new_with_family(font_system, &s.memory, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
            label.w + 2.0 * padding
        } else {
            0.0
        }
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_items: &mut Vec<TextItem>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tag_bounds: &mut Vec<TagBounds>,
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
            label.draw(text_items, x + padding, (bar_h - font_size * 1.4) / 2.0);
        }
    }
}

pub struct CpuModule;

impl StatusModule for CpuModule {
    fn name(&self) -> &'static str { "cpu" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        if let Some(ref s) = stats {
            let label = Label::new_with_family(font_system, &s.cpu, font_size, [0.0, 0.0, 0.0, 1.0], font_family);
            label.w + 2.0 * padding
        } else {
            0.0
        }
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        text_items: &mut Vec<TextItem>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tag_bounds: &mut Vec<TagBounds>,
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
            label.draw(text_items, x + padding, (bar_h - font_size * 1.4) / 2.0);
        }
    }
}

pub struct TrayModule;

impl StatusModule for TrayModule {
    fn name(&self) -> &'static str { "tray" }
    
    fn has_custom_background(&self) -> bool { true }

    fn width(
        &self,
        _stats: &Option<SystemStats>,
        _tags: &str,
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
        _tags: &str,
        _layout: &str,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _normal_color: [f32; 4],
        bar_h: f32,
        scale_factor: f64,
        text_items: &mut Vec<TextItem>,
        rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tag_bounds: &mut Vec<TagBounds>,
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
                                            
                                            rects.push(RectWidget {
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

                let buf = make_text_buffer(font_system, symbol, font_size, font_family);
                let scale = cce_ui::scale::scale_factor();
                let tw = buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0) / scale;
                let tx = icon_x + (icon_size - tw) / 2.0;
                let ty = icon_y + (icon_size - font_size * 1.4) / 2.0;
                text_items.push(TextItem {
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
        }
    }
}
