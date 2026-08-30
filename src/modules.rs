use std::collections::HashMap;
use cce_ui::cosmic_text::FontSystem;
use cce_ui::color;
use cce_ui::widget::StyledLabel as Label;

use crate::{
    RectWidget, RoundedBox, SystemStats, TrayItem,
    TrayIconBounds, make_text_buffer,
};

/// Vertical offset that centers a text run in a box `box_h` tall. The engine
/// shapes horizontal text with a line box of exactly `font_size` (cce-ui
/// window_runner uses line_height = physical_size * 1.0), so centering must
/// use that height — not a CSS-ish 1.4em line box.
pub(crate) fn centered_text_y(box_h: f32, font_size: f32) -> f32 {
    (box_h - font_size) / 2.0 - crate::config::read_text_raise_from_config()
}

pub trait StatusModule {
    fn name(&self) -> &'static str;
    
    fn has_custom_background(&self, _title: &str) -> bool { false }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32;

    /// Width of the module's LIVE content plus padding — what the drawn
    /// bubble hugs. `width()` stays the stable LAYOUT width (widest-plausible
    /// templates, title quantization) that sizes the slot and the surface, so
    /// the compositor never sees a resize; this one may be narrower, and the
    /// bubble is centered in the slot on the difference so the padding on
    /// each side of the content is the configured padding rather than
    /// padding-plus-template-surplus. Defaults to `width()` for modules whose
    /// slot already is their content (clock, tray, light_source).
    fn content_width(
        &self,
        stats: &Option<SystemStats>,
        title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        self.width(stats, title, font_system, font_family, font_size, tray_items, padding)
    }

    fn render(
        &self,
        x: f32,
        w: f32,
        stats: &Option<SystemStats>,
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
        tray_items: &HashMap<String, TrayItem>,
        tray_item_bounds: &mut Vec<TrayIconBounds>,
        box_bg_color: Option<[f32; 4]>,
        status_box_radius: f32,
        rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    );
}

/// A stat module's width from the WIDER of its live text and a
/// widest-plausible template ("Cpu 100%"), plus padding. Sizing to the live
/// text alone made the surface resize whenever the value crossed a digit
/// boundary ("Cpu 9.9%" ↔ "Cpu 10.2%"), which re-arranged the whole status
/// strip and — through the compositor's configure echo — ping-ponged the
/// module and its neighbors at frame rate (the tray/cpu jitter).
fn stable_text_width(
    font_system: &mut FontSystem,
    text: &str,
    template: &str,
    font_size: f32,
    font_family: &str,
    padding: f32,
) -> f32 {
    let live = Label::new_with_family(font_system, text, font_size, [0.0, 0.0, 0.0, 1.0], font_family).w;
    let tmpl = Label::new_with_family(font_system, template, font_size, [0.0, 0.0, 0.0, 1.0], font_family).w;
    live.max(tmpl) + 2.0 * padding
}

/// The live half of `stable_text_width`: the text as it is right now, plus
/// padding — the content measure `content_width` implementations return.
fn live_text_width(
    font_system: &mut FontSystem,
    text: &str,
    font_size: f32,
    font_family: &str,
    padding: f32,
) -> f32 {
    Label::new_with_family(font_system, text, font_size, [0.0, 0.0, 0.0, 1.0], font_family).w
        + 2.0 * padding
}

/// Chip text shown by the window module while nothing holds keyboard focus.
const NO_FOCUS_TEXT: &str = "no focus";

/// The window module's title as displayed: ellipsized past 40 chars. One
/// place, because `width`, `content_width` and `render` must all measure the
/// same string.
fn display_title(title: &str) -> String {
    if title.chars().count() > 40 {
        title.chars().take(37).collect::<String>() + "..."
    } else {
        title.to_string()
    }
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
        title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let has_title = !title.is_empty() && title != "(none)";
        if has_title {
            let total_w = live_text_width(font_system, &display_title(title), font_size, font_family, padding);

            // Title-mode width quantized UP to a coarse step: titles change
            // constantly (dirty markers, browser tabs, terminal cwd), and
            // sizing to the exact text resized this surface on every change —
            // shoving the neighboring module sideways each time (the
            // light_source flicker) and, at 372↔456px alternation rates,
            // feeding the compositor's configure echo loop. Within a bucket a
            // title change costs nothing. (The drawn bubble hugs the exact
            // title via `content_width`; the bucket sizes only the surface.)
            const TITLE_WIDTH_STEP: f32 = 24.0;
            (total_w / TITLE_WIDTH_STEP).ceil() * TITLE_WIDTH_STEP
        } else {
            // "(none)" is the compositor explicitly reporting Focus::None
            // (keystrokes go nowhere); an empty title is just the feed not
            // having connected yet, which must not flash the indicator.
            if title == "(none)" {
                live_text_width(font_system, NO_FOCUS_TEXT, font_size, font_family, padding)
            } else {
                0.0
            }
        }
    }

    fn content_width(
        &self,
        _stats: &Option<SystemStats>,
        title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let has_title = !title.is_empty() && title != "(none)";
        if has_title {
            live_text_width(font_system, &display_title(title), font_size, font_family, padding)
        } else if title == "(none)" {
            live_text_width(font_system, NO_FOCUS_TEXT, font_size, font_family, padding)
        } else {
            0.0
        }
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        _stats: &Option<SystemStats>,
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
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        box_bg_color: Option<[f32; 4]>,
        status_box_radius: f32,
        rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        let has_title = !title.is_empty() && title != "(none)";
        if has_title {
            let label = Label::new_with_family(font_system, &display_title(title), font_size, normal_color, font_family);
            crate::draw_label(text_prims, label, x + padding, centered_text_y(bar_h, font_size));
        } else if title == "(none)" {
            // Dim chip signalling that no window has keyboard focus — the
            // state where typing goes nowhere. Half-alpha text, not
            // clickable.
            let mut dim = normal_color;
            dim[3] *= 0.5;
            let label = Label::new_with_family(font_system, NO_FOCUS_TEXT, font_size, dim, font_family);
            let box_w = label.w + 2.0 * padding;
            if let Some(color) = box_bg_color {
                rounded_boxes.push(RoundedBox {
                    x,
                    y: 0.0,
                    w: box_w,
                    h: bar_h,
                    radius: status_box_radius,
                    color,
                    corners: (false, false, true, true),
                    border: None,
                });
            }
            crate::draw_label(text_prims, label, x + padding, centered_text_y(bar_h, font_size));
        }
    }
}

pub struct ClockModule;

impl StatusModule for ClockModule {
    fn name(&self) -> &'static str { "clock" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
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

impl BatteryModule {
    fn live_text<'a>(stats: &'a Option<SystemStats>) -> &'a str {
        match stats {
            Some(s) if !s.battery.is_empty() => &s.battery,
            _ => "Bat 100%",
        }
    }
}

impl StatusModule for BatteryModule {
    fn name(&self) -> &'static str { "battery" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        stable_text_width(font_system, Self::live_text(stats), "Bat 100%", font_size, font_family, padding)
    }

    fn content_width(
        &self,
        stats: &Option<SystemStats>,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        live_text_width(font_system, Self::live_text(stats), font_size, font_family, padding)
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
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

impl VolumeModule {
    fn live_text<'a>(stats: &'a Option<SystemStats>) -> &'a str {
        match stats {
            Some(s) if !s.volume.is_empty() => &s.volume,
            _ => "Vol 100%",
        }
    }
}

impl StatusModule for VolumeModule {
    fn name(&self) -> &'static str { "volume" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        stable_text_width(font_system, Self::live_text(stats), "Vol 100%", font_size, font_family, padding)
    }

    fn content_width(
        &self,
        stats: &Option<SystemStats>,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        live_text_width(font_system, Self::live_text(stats), font_size, font_family, padding)
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
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
                let label = Label::new_with_family(font_system, &s.volume, font_size, color_val, font_family);
                crate::draw_label(text_prims, label, x + padding, centered_text_y(bar_h, font_size));
            }
        }
    }
}

pub struct BrightnessModule;

impl BrightnessModule {
    fn live_text<'a>(stats: &'a Option<SystemStats>) -> &'a str {
        match stats {
            Some(s) if !s.brightness.is_empty() => &s.brightness,
            _ => "Bri 100%",
        }
    }
}

impl StatusModule for BrightnessModule {
    fn name(&self) -> &'static str { "brightness" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        stable_text_width(font_system, Self::live_text(stats), "Bri 100%", font_size, font_family, padding)
    }

    fn content_width(
        &self,
        stats: &Option<SystemStats>,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        live_text_width(font_system, Self::live_text(stats), font_size, font_family, padding)
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
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

impl MemoryModule {
    fn live_text<'a>(stats: &'a Option<SystemStats>) -> &'a str {
        match stats {
            Some(s) if !s.memory.is_empty() => &s.memory,
            _ => "Mem 0/0G",
        }
    }
}

impl StatusModule for MemoryModule {
    fn name(&self) -> &'static str { "memory" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        let text = Self::live_text(stats);
        // "Mem 17/62G" → "Mem 62/62G": used pinned to the total, the
        // widest this machine's readout gets.
        let template = text
            .rsplit('/')
            .next()
            .and_then(|total| total.strip_suffix('G'))
            .map(|total| format!("Mem {total}/{total}G"))
            .unwrap_or_else(|| text.to_string());
        stable_text_width(font_system, text, &template, font_size, font_family, padding)
    }

    fn content_width(
        &self,
        stats: &Option<SystemStats>,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        live_text_width(font_system, Self::live_text(stats), font_size, font_family, padding)
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
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

impl CpuModule {
    fn live_text<'a>(stats: &'a Option<SystemStats>) -> &'a str {
        match stats {
            Some(s) if !s.cpu.is_empty() => &s.cpu,
            _ => "Cpu 0%",
        }
    }
}

impl StatusModule for CpuModule {
    fn name(&self) -> &'static str { "cpu" }

    fn width(
        &self,
        stats: &Option<SystemStats>,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        stable_text_width(font_system, Self::live_text(stats), "Cpu 100%", font_size, font_family, padding)
    }

    fn content_width(
        &self,
        stats: &Option<SystemStats>,
        _title: &str,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        padding: f32,
    ) -> f32 {
        live_text_width(font_system, Self::live_text(stats), font_size, font_family, padding)
    }

    fn render(
        &self,
        x: f32,
        _w: f32,
        stats: &Option<SystemStats>,
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
            // The same `module { text_raise }` lift every text run gets via
            // `centered_text_y` — without it the icons sit at geometric
            // center while neighboring modules' text rides `text_raise`
            // higher, and the tray reads as low.
            let icon_y = (bar_h - icon_size) / 2.0 - crate::config::read_text_raise_from_config();

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
                // Plain centering within the icon box: the box itself already
                // carries the `text_raise` lift, and `centered_text_y` here
                // would apply it a second time.
                let ty = icon_y + (icon_size - font_size) / 2.0;
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
                    Some(tw),
                ));
            }
        }
    }
}

pub struct LightSourceModule;

pub(crate) fn get_light_source_pos_from_config() -> f32 {
    crate::config::read_light_source_position_from_config()
}

impl StatusModule for LightSourceModule {
    fn name(&self) -> &'static str { "light_source" }

    // The module is just the empty circle — no module box behind it.
    fn has_custom_background(&self, _title: &str) -> bool { true }

    fn width(
        &self,
        _stats: &Option<SystemStats>,
        _title: &str,
        _font_system: &mut FontSystem,
        _font_family: &str,
        _font_size: f32,
        _tray_items: &HashMap<String, TrayItem>,
        _padding: f32,
    ) -> f32 {
        // An empty circle with the bar's own thickness as its diameter; the
        // radians value lives in the module's menu, not the strip.
        crate::read_status_height_from_config()
    }

    fn render(
        &self,
        x: f32,
        w: f32,
        _stats: &Option<SystemStats>,
        _title: &str,
        _font_system: &mut FontSystem,
        _font_family: &str,
        _font_size: f32,
        _normal_color: [f32; 4],
        bar_h: f32,
        _scale_factor: f64,
        _text_prims: &mut Vec<crate::TextPrim>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        rounded_boxes: &mut Vec<RoundedBox>,
        _padding: f32,
    ) {
        let d = w.min(bar_h);
        // The circle wears the module-box fill: same color, opacity and
        // (per-pixel, compositor-side) backdrop blur as every other module's
        // box — just circle-shaped and empty of content.
        rounded_boxes.push(RoundedBox {
            x: x + (w - d) / 2.0,
            y: (bar_h - d) / 2.0,
            w: d,
            h: d,
            radius: d / 2.0,
            color: box_bg_color.unwrap_or([0.0, 0.0, 0.0, 0.0]),
            corners: (true, true, true, true),
            // Circle marker; zero thickness = fill only, no stroke.
            border: Some(([0.0; 4], 0.0)),
        });
    }
}
