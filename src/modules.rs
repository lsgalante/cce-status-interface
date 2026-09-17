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
        icon_prims: &mut Vec<crate::IconPrim>,
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

    // No has_custom_background override: the "(none)" chip is an ordinary
    // bubble (it used to draw its own square-topped box in render, which
    // ignored the droplet style), and the empty-title state has width 0, so
    // no box is ever drawn for it anyway.

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
        _icon_prims: &mut Vec<crate::IconPrim>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        let has_title = !title.is_empty() && title != "(none)";
        if has_title {
            let label = Label::new_with_family(font_system, &display_title(title), font_size, normal_color, font_family);
            crate::draw_label(text_prims, label, x + padding, centered_text_y(bar_h, font_size));
        } else if title == "(none)" {
            // Dim chip signalling that no window has keyboard focus — the
            // state where typing goes nowhere. Half-alpha text, not
            // clickable; the bubble behind it is the standard one drawn by
            // rebuild_layout, same as every module.
            let mut dim = normal_color;
            dim[3] *= 0.5;
            let label = Label::new_with_family(font_system, NO_FOCUS_TEXT, font_size, dim, font_family);
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
        _icon_prims: &mut Vec<crate::IconPrim>,
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

/// A stat module's readout: a cce-icons glyph with its value beside it —
/// the glyph IS the unit, so the number is bare ("87" next to the battery,
/// not "Bat 87%"). The glyph is tinted the readout's color; see `icons.rs`
/// for why the tint is done here rather than through `cce_ui::upload_icon`.
///
/// The pre-glyph text form rides along as the FALLBACK: `tinted_icon` returns
/// `None` when the icon set is missing or unparsable, and a readout that
/// silently loses its glyph would be a bare number nobody can attribute — so
/// it degrades to the old "Cpu 45%" instead.
pub(crate) struct IconReadout {
    pub icon: &'static str,
    /// The number drawn beside the glyph; `None` draws the glyph alone (a
    /// value the reader could not produce — cpu with no /proc/stat, a
    /// sink without a level).
    pub number: Option<String>,
    pub color: [f32; 4],
    pub fallback: String,
    /// Widest-plausible fallback text, for the stable slot width.
    pub fallback_template: &'static str,
}

/// The widest number a percentage readout shows.
const NUMBER_TEMPLATE: &str = "100";

impl IconReadout {
    /// The glyph as an uploaded texture plus its LOGICAL size, at
    /// `module { icon_size }` on the longer side. `None` = no glyph, text
    /// fallback.
    fn glyph(&self) -> Option<(u32, f32, f32)> {
        let scale = cce_ui::scale::scale_factor();
        let px = (crate::read_icon_size_from_config() * scale).round().max(1.0) as u32;
        let (image, w, h) = crate::icons::tinted_icon(self.icon, px, crate::icons::tint_of(self.color))?;
        Some((image, w as f32 / scale, h as f32 / scale))
    }

    /// The number's font size — `module { icon_font_size }`, else the
    /// module font.
    fn number_size(font_size: f32) -> f32 {
        crate::read_icon_font_size_from_config(font_size)
    }

    /// A number's run width at the readout weight, logical px.
    fn number_width(font_system: &mut FontSystem, text: &str, size: f32, font_family: &str) -> f32 {
        let buf = crate::make_text_buffer_weighted(font_system, text, size, font_family, crate::read_icon_weight_from_config());
        buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0) / cce_ui::scale::scale_factor()
    }

    /// This item's width — glyph, gap and number — with the number as the
    /// live value (`template` false) or the widest plausible one (`template`
    /// true, for the stable slot). In text fallback, the fallback string or
    /// its template.
    fn item_width(&self, font_system: &mut FontSystem, font_family: &str, font_size: f32, template: bool) -> f32 {
        match self.glyph() {
            Some((_, gw, _)) => {
                let ns = Self::number_size(font_size);
                let nw = if template {
                    Self::number_width(font_system, NUMBER_TEMPLATE, ns, font_family)
                } else {
                    self.number.as_deref().map_or(0.0, |n| Self::number_width(font_system, n, ns, font_family))
                };
                if nw > 0.0 { gw + crate::read_icon_gap_from_config() + nw } else { gw }
            }
            None => {
                let text = if template { self.fallback_template } else { self.fallback.as_str() };
                Label::new_with_family(font_system, text, font_size, [0.0, 0.0, 0.0, 1.0], font_family).w
            }
        }
    }

    /// Draw the item with its left edge at `x`; returns the width drawn.
    fn render(
        &self,
        x: f32,
        font_system: &mut FontSystem,
        font_family: &str,
        font_size: f32,
        bar_h: f32,
        text_prims: &mut Vec<crate::TextPrim>,
        icon_prims: &mut Vec<crate::IconPrim>,
    ) -> f32 {
        match self.glyph() {
            Some((image, gw, gh)) => {
                // The same `module { text_raise }` lift every text run gets
                // via `centered_text_y` is applied to the glyph too, so
                // glyph and number stay level with each other and with the
                // neighboring modules' text.
                let gy = (bar_h - gh) / 2.0 - crate::config::read_text_raise_from_config();
                icon_prims.push(crate::IconPrim {
                    image,
                    x,
                    y: gy,
                    w: gw,
                    h: gh,
                    alpha: crate::read_icon_alpha_from_config(),
                });
                let mut w = gw;
                if let Some(n) = self.number.as_deref() {
                    let ns = Self::number_size(font_size);
                    let nw = Self::number_width(font_system, n, ns, font_family);
                    let nx = x + gw + crate::read_icon_gap_from_config();
                    text_prims.push((
                        n.to_string(),
                        ns,
                        nx,
                        centered_text_y(bar_h, ns),
                        crate::icons::tint_of(self.color),
                        Some(font_family.to_string()),
                        None,
                        None,
                        Some(nw),
                        crate::read_icon_weight_from_config(),
                    ));
                    w = nx + nw - x;
                }
                w
            }
            None => {
                let label = Label::new_with_family(font_system, &self.fallback, font_size, self.color, font_family);
                crate::draw_label(text_prims, label, x, centered_text_y(bar_h, font_size))
            }
        }
    }
}

/// Width of a row of readouts in one bubble: `padding` inside each end,
/// `module { icon_spacing }` between items. 0 for an empty row (the module
/// hides). `template` sizes every number at its widest, for the stable
/// slot; the live row is what the bubble hugs.
fn readouts_width(
    items: &[IconReadout],
    font_system: &mut FontSystem,
    font_family: &str,
    font_size: f32,
    padding: f32,
    template: bool,
) -> f32 {
    if items.is_empty() {
        return 0.0;
    }
    let spacing = crate::read_icon_spacing_from_config();
    let sum: f32 = items.iter().map(|r| r.item_width(font_system, font_family, font_size, template)).sum();
    sum + spacing * (items.len() as f32 - 1.0) + 2.0 * padding
}

/// Draw a row of readouts starting at the bubble's left edge `x`.
fn render_readouts(
    items: &[IconReadout],
    x: f32,
    font_system: &mut FontSystem,
    font_family: &str,
    font_size: f32,
    bar_h: f32,
    text_prims: &mut Vec<crate::TextPrim>,
    icon_prims: &mut Vec<crate::IconPrim>,
    padding: f32,
) {
    let spacing = crate::read_icon_spacing_from_config();
    let mut cx = x + padding;
    for (i, r) in items.iter().enumerate() {
        if i > 0 {
            cx += spacing;
        }
        cx += r.render(cx, font_system, font_family, font_size, bar_h, text_prims, icon_prims);
    }
}

/// A module that reads out as one [`IconReadout`]: it only has to say which
/// glyph, which number and which color, and the blanket `StatusModule` impl
/// below does the shared layout. `None` hides the module (width 0) — a
/// machine with no battery or backlight has nothing to read out.
pub(crate) trait IconStat {
    const NAME: &'static str;
    fn readout(stats: &Option<SystemStats>, normal_color: [f32; 4]) -> Option<IconReadout>;
}

impl<T: IconStat> StatusModule for T {
    fn name(&self) -> &'static str { T::NAME }

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
        // The color only tints the glyph, and the width is the same in any
        // tint; the readout's own color is applied at render.
        let items: Vec<_> = T::readout(stats, color::TEXT_FG).into_iter().collect();
        readouts_width(&items, font_system, font_family, font_size, padding, true)
            .max(readouts_width(&items, font_system, font_family, font_size, padding, false))
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
        let items: Vec<_> = T::readout(stats, color::TEXT_FG).into_iter().collect();
        readouts_width(&items, font_system, font_family, font_size, padding, false)
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
        icon_prims: &mut Vec<crate::IconPrim>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        let items: Vec<_> = T::readout(stats, normal_color).into_iter().collect();
        render_readouts(&items, x, font_system, font_family, font_size, bar_h, text_prims, icon_prims, padding);
    }
}

/// The combined readout segment: every `IconStat` module's readout in one
/// bubble, in the order the compositor used to lay the five separate
/// segments out (cpu, memory, brightness, volume, battery). A reader with
/// nothing (no battery, no backlight) simply drops out of the row. This is
/// what the launcher daemon runs; the five single names stay valid for a
/// bar configured to run them separately.
pub struct StatsModule;

impl StatsModule {
    fn readouts(stats: &Option<SystemStats>, normal_color: [f32; 4]) -> Vec<IconReadout> {
        [
            CpuModule::readout(stats, normal_color),
            MemoryModule::readout(stats, normal_color),
            BrightnessModule::readout(stats, normal_color),
            VolumeModule::readout(stats, normal_color),
            BatteryModule::readout(stats, normal_color),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

impl StatusModule for StatsModule {
    fn name(&self) -> &'static str { "stats" }

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
        let items = Self::readouts(stats, color::TEXT_FG);
        readouts_width(&items, font_system, font_family, font_size, padding, true)
            .max(readouts_width(&items, font_system, font_family, font_size, padding, false))
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
        let items = Self::readouts(stats, color::TEXT_FG);
        readouts_width(&items, font_system, font_family, font_size, padding, false)
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
        icon_prims: &mut Vec<crate::IconPrim>,
        _rects: &mut Vec<RectWidget>,
        _overlay_rects: &mut Vec<RectWidget>,
        _tray_items: &HashMap<String, TrayItem>,
        _tray_item_bounds: &mut Vec<TrayIconBounds>,
        _box_bg_color: Option<[f32; 4]>,
        _status_box_radius: f32,
        _rounded_boxes: &mut Vec<RoundedBox>,
        padding: f32,
    ) {
        let items = Self::readouts(stats, normal_color);
        render_readouts(&items, x, font_system, font_family, font_size, bar_h, text_prims, icon_prims, padding);
    }
}

pub struct BatteryModule;

impl IconStat for BatteryModule {
    const NAME: &'static str = "battery";

    fn readout(stats: &Option<SystemStats>, normal_color: [f32; 4]) -> Option<IconReadout> {
        let (cap, charging) = match stats {
            Some(s) => s.battery?,
            None => (100, false),
        };
        // Accent while charging or nearly flat; charging also swaps in the
        // bolt glyph, the icon form of the text readout's "⚡" prefix.
        let color = if !charging && cap > 10 { normal_color } else { color::TEXT_ACCENT };
        Some(IconReadout {
            icon: if charging { "battery-charging" } else { "battery" },
            number: Some(cap.to_string()),
            color,
            fallback: format!("{} {cap}%", if charging { "⚡" } else { "Bat" }),
            fallback_template: "Bat 100%",
        })
    }
}

pub struct VolumeModule;

impl IconStat for VolumeModule {
    const NAME: &'static str = "volume";

    fn readout(stats: &Option<SystemStats>, normal_color: [f32; 4]) -> Option<IconReadout> {
        let (pct, muted) = match stats {
            Some(s) => s.volume?,
            None => (Some(100), false),
        };
        let color = if muted {
            crate::read_disabled_color_from_config().unwrap_or(color::TEXT_DIM)
        } else {
            normal_color
        };
        let fallback = match (muted, pct) {
            (_, Some(p)) => format!("Vol {p}%"),
            (true, None) => "Vol Muted".to_string(),
            (false, None) => "Vol N/A".to_string(),
        };
        Some(IconReadout {
            icon: if muted { "volume-muted" } else { "volume" },
            number: pct.map(|p| p.to_string()),
            color,
            fallback,
            fallback_template: "Vol 100%",
        })
    }
}

pub struct BrightnessModule;

impl IconStat for BrightnessModule {
    const NAME: &'static str = "brightness";

    fn readout(stats: &Option<SystemStats>, normal_color: [f32; 4]) -> Option<IconReadout> {
        let pct = match stats {
            Some(s) => s.brightness?,
            None => 100,
        };
        Some(IconReadout {
            icon: "brightness",
            number: Some(pct.to_string()),
            color: normal_color,
            fallback: format!("Bri {pct}%"),
            fallback_template: "Bri 100%",
        })
    }
}

pub struct MemoryModule;

impl IconStat for MemoryModule {
    const NAME: &'static str = "memory";

    fn readout(stats: &Option<SystemStats>, normal_color: [f32; 4]) -> Option<IconReadout> {
        let pct = match stats {
            Some(s) => s.memory,
            None => Some(0),
        };
        Some(IconReadout {
            icon: "memory",
            number: pct.map(|p| p.to_string()),
            color: normal_color,
            fallback: pct.map_or("Mem N/A".to_string(), |p| format!("Mem {p}%")),
            fallback_template: "Mem 100%",
        })
    }
}

pub struct CpuModule;

impl IconStat for CpuModule {
    const NAME: &'static str = "cpu";

    fn readout(stats: &Option<SystemStats>, normal_color: [f32; 4]) -> Option<IconReadout> {
        let pct = match stats {
            Some(s) => s.cpu_pct,
            None => Some(0),
        };
        Some(IconReadout {
            icon: "cpu",
            number: pct.map(|p| p.to_string()),
            color: normal_color,
            fallback: pct.map_or("Cpu N/A".to_string(), |p| format!("Cpu {p}%")),
            fallback_template: "Cpu 100%",
        })
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
        _icon_prims: &mut Vec<crate::IconPrim>,
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
            let icon_size = crate::config::TRAY_ICON_SIZE;
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
                    None,
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
        _icon_prims: &mut Vec<crate::IconPrim>,
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
