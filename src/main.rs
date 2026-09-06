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
use cce_ui::cosmic_text::{
    Attrs, Buffer, FontSystem, Metrics,
};
use cce_ui::color;
use cce_ui::widget::{
    WidgetHost,
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
    LayoutUpdated(String),
    TitleUpdated(String),
    SystemStatsUpdated(SystemStats),
    TrayUpdated(TrayItem),
    TrayRemoved(String),
    /// A bar-built in-surface menu (window picker), fetched off-thread.
    MenuReady { title: String, pages: Vec<MenuPage>, min_w: f32 },
    SwitcherTriggered,
    /// Compositor click-away-close: a pointer press landed somewhere other
    /// than this expanded segment. Payload = the pressed segment's app_id
    /// ("-" for none); a segment ignores a dismiss naming itself.
    MenuDismiss(String),
    /// A tray icon's DBusMenu, fetched and flattened for the in-surface menu.
    TrayMenuFetched { destination: String, menu_path: String, pages: Vec<MenuPage> },
    /// What this segment is composited over, measured by the compositor:
    /// (luma, spread), both 0-100. The bar cannot see behind its own
    /// translucent box, so this is the only source of that fact — see
    /// `module { text_contrast }`.
    BackdropUpdated((u8, u8)),
    ToggleHideModules,
    ToggleAdjustPositionMode,
}

/// One sRGB channel to linear light (the WCAG transfer function).
fn to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// WCAG relative luminance of a raw-sRGB color, 0-1. Must agree with the
/// compositor's `backdrop::relative_luminance` — the two are the halves of
/// one contrast comparison, and weighting them differently would make the
/// ratio meaningless.
fn relative_luminance(rgba: [f32; 4]) -> f32 {
    0.2126 * to_linear(rgba[0]) + 0.7152 * to_linear(rgba[1]) + 0.0722 * to_linear(rgba[2])
}

/// WCAG contrast ratio between two relative luminances, 1.0 (identical) to
/// 21.0 (black on white).
fn contrast_ratio(a: f32, b: f32) -> f32 {
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// How badly `text` fails to read against a backdrop of luminance `bg`:
/// 0 once the pair clears WCAG AA for normal text (4.5:1), rising to 1 as
/// the two converge on invisible.
fn contrast_deficit(text: f32, bg: f32) -> f32 {
    const AA: f32 = 4.5;
    ((AA - contrast_ratio(text, bg)) / (AA - 1.0)).clamp(0.0, 1.0)
}

/// How much help text of luminance `text_luma` needs over a backdrop
/// measured as `(luma, spread)`, 0 (none) to 1 (as much as the knob allows).
///
/// Contrast is checked at BOTH ends of the spread as well as at the mean, and
/// the worst answer wins: a segment lying half on a black cell and half on a
/// light gap averages to a perfectly comfortable mid-gray while the text is
/// unreadable over one of the two halves. Checking only the mean is the
/// mistake that would make an adaptive scheme look broken exactly where the
/// fixed one already worked.
fn contrast_demand(text_luma: f32, (luma, spread): (u8, u8)) -> f32 {
    let mid = luma as f32 / 100.0;
    let half = (spread as f32 / 100.0) / 2.0;
    let lo = (mid - half).clamp(0.0, 1.0);
    let hi = (mid + half).clamp(0.0, 1.0);
    contrast_deficit(text_luma, mid)
        .max(contrast_deficit(text_luma, lo))
        .max(contrast_deficit(text_luma, hi))
}

/// A droplet spec whose shape knobs resolve against `reference_h` instead of
/// the box they are given, for a box that is actually `box_h` tall.
///
/// `DropletSpec`'s shape knobs are fractions OF THE BOX HEIGHT, which is what
/// makes one spec survive a change to `module { height }` — the drop looks the
/// same on a 24px bar and a 40px one. The expanded context menu breaks that
/// assumption: it keeps the module's width and grows ten times taller, so the
/// same fractions resolve to a bottom radius that hits the half-width clamp
/// (a literal semicircle under a 10-row window picker) and a top taper eating
/// 145px of a 345px box, while the rows are laid out as a plain rectangle
/// inside it and overhang the silhouette at both ends.
///
/// Scaling every height-fraction knob by `reference_h / box_h` makes them
/// resolve to the SAME pixel values they would at `reference_h`, so the drop
/// keeps exactly the silhouette it has collapsed and the body extends straight
/// down. At the start of the expansion animation the factor is 1 and this is
/// the identity, so there is nothing to pop.
///
/// Only the knobs documented as fractions of height are touched. `belly_w` is
/// a fraction of the remaining half-width, and the rest (`clarity`, `dome`,
/// `gleam`, `shine`, `rim`, `curve`, `core`, `refr`, `ghost`, `shadow`) are
/// strengths or exponents with no length in them.
fn spec_at_reference_height(
    spec: cce_ui::scene::paint::DropletSpec,
    reference_h: f32,
    box_h: f32,
) -> cce_ui::scene::paint::DropletSpec {
    if box_h <= reference_h || reference_h <= 0.0 {
        return spec;
    }
    let k = reference_h / box_h;
    let mut out = spec;
    out.sag *= k;
    out.belly *= k;
    out.blend *= k;
    out.sheet_r *= k;
    out.attach *= k;
    out.bow *= k;
    out.band *= k;
    // The dome and its gleam FADE OUT as the box grows past the bar strip:
    // dome shading follows the rounded-rect SDF gradient, and on a box with
    // long straight sides that field creases along the corner diagonals —
    // full strength draws a blocky lit picture-frame (band pinned) or
    // envelope folds across the body (band grown); both were tried and read
    // as broken lighting rather than water. A tall panel is not a bead: the
    // expanded menu settles into a flat glass sheet that keeps the drop's
    // OTHER water terms — the thin-edge clarity falloff, the fresnel rim
    // crest along the lower arc, the core tint, the contact shadow — which
    // are all silhouette-hugging and crease-free. The ramp is continuous in
    // k — full lighting collapsed, gone once the box passes twice the bar
    // height — so the fade rides the expansion animation with nothing to
    // pop, and a real menu (k ≈ 0.1–0.2) lands at exactly zero.
    let lit = ((k - 0.5) * 2.0).clamp(0.0, 1.0);
    out.dome *= lit;
    out.gleam *= lit;
    out
}

/// The color a treatment behind or around `rgb` text should be drawn in:
/// whichever of black/white that text reads against.
///
/// Used by the scrim, and applied PER RUN rather than from the configured
/// module color, because a module may paint a run in something else entirely
/// — the volume module's muted state uses the shared `disabled_color`. A
/// black pool behind black text is not a weaker treatment, it is an eraser.
fn treatment_rgb(rgb: [u8; 3]) -> [f32; 3] {
    let luma = relative_luminance([rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0, 1.0]);
    if contrast_ratio(luma, 0.0) >= contrast_ratio(luma, 1.0) {
        [0.0, 0.0, 0.0]
    } else {
        [1.0, 1.0, 1.0]
    }
}

/// The scrim's opacity: it rests at the configured `base` and deepens toward
/// opaque as the measured backdrop demands more. `demand` is the eased
/// `contrast_now`, which is already zero when `module { text_contrast }` is
/// off — so without that knob the scrim is a constant, which is the point of
/// having it.
fn scrim_alpha(base: f32, demand: f32) -> f32 {
    (base + (1.0 - base) * demand.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

/// How far the pool fades out, logical px. Defaults to a quarter of the
/// bubble's height so the gradient scales with the bar, and is capped at half
/// of each axis: the feather is drawn OUTSIDE the solid core, so the core is
/// inset by this much, and a larger one would invert it and the pool would
/// vanish — exactly where a narrow module (a lone icon) lands.
fn scrim_feather(w: f32, h: f32, configured: Option<f32>) -> f32 {
    configured.unwrap_or(h * 0.25).max(0.0).min(w / 2.0).min(h / 2.0)
}

/// The color of the widest measured text run inside a box, which is the run a
/// box-sized pool is really there to protect. None when the box holds no
/// measured run at all.
///
/// Width is the tiebreak rather than, say, the first run, because a module
/// that mixes colors (a value in an accent beside its label) is led by its
/// longest label, and that is the one whose legibility carries the segment.
fn dominant_run_color(runs: &[TextPrim], bx: f32, by: f32, bw: f32, bh: f32) -> Option<[u8; 3]> {
    let mut best: Option<(f32, [u8; 3])> = None;
    for (_, tsize, x, y, color, _, _, _, run_w) in runs {
        let Some(rw) = *run_w else { continue };
        // Runs belong to the box they sit in; a segment with an expanded menu
        // has text in both.
        let (cx, cy) = (x + rw * 0.5, y + tsize * 0.5);
        if cx < bx || cx > bx + bw || cy < by || cy > by + bh {
            continue;
        }
        if best.map_or(true, |(w, _)| rw > w) {
            best = Some((rw, *color));
        }
    }
    best.map(|(_, c)| c)
}

/// The pool color for a box that holds tray icons rather than text. The
/// tray is the one module whose content is not a measured run, and the
/// text-keyed lookup finding nothing used to leave its bubble bare — the
/// only one in the strip painted without the pool, visibly lighter than its
/// neighbors and the only one that never answered the backdrop. The icons
/// read as light glyphs (a dark pixmap is recolored toward white in
/// `TrayModule::render`), so the box gets what a white run would get: a
/// black pool. None when no icon sits in the box.
fn dominant_icon_color(icons: &[TrayIconBounds], bx: f32, by: f32, bw: f32, bh: f32) -> Option<[u8; 3]> {
    icons
        .iter()
        .any(|b| {
            let (cx, cy) = (b.x + b.w * 0.5, b.y + b.h * 0.5);
            cx >= bx && cx <= bx + bw && cy >= by && cy <= by + bh
        })
        .then_some([255, 255, 255])
}

/// The in-surface right-click menu: instead of spawning a popup process, the
/// module's own surface EXPANDS below the bar strip to contain the menu. The
/// compositor treats a status segment thicker than the bar as expanded — it
/// keeps the segment's frozen slot, stops enforcing its size, and raises it
/// above the windows the menu overlaps. Pages support DBusMenu submenus:
/// tray icon menus navigate in place (`Submenu`/`Back` rows).
struct ModuleContextMenu {
    pages: Vec<MenuPage>,
    page: usize,
    /// (destination, menu_path) — the DBusMenu owner `Item` rows dispatch
    /// to; None for the bar's own module menu.
    tray_target: Option<(String, String)>,
    min_w: f32,
    hovered: Option<usize>,
    /// Menu box in surface-local logical coords, set by `rebuild_layout`.
    rect: (f32, f32, f32, f32),
    /// Per-row (y offset from the menu top, height), parallel to the current
    /// page's rows; rebuilt with the layout (rows have mixed heights).
    row_bounds: Vec<(f32, f32)>,
}

impl ModuleContextMenu {
    const PAD: f32 = 6.0;
    const HEADER_H: f32 = 26.0;
    const ITEM_H: f32 = 28.0;
    const SEP_H: f32 = 9.0;

    fn rows(&self) -> &[MenuRow] {
        self.pages.get(self.page).map(|p| p.rows.as_slice()).unwrap_or(&[])
    }

    fn title(&self) -> &str {
        self.pages.get(self.page).map(|p| p.title.as_str()).unwrap_or("")
    }

    fn height(&self) -> f32 {
        let rows: f32 = self
            .rows()
            .iter()
            .map(|r| if r.separator { Self::SEP_H } else { Self::ITEM_H })
            .sum();
        2.0 * Self::PAD + Self::HEADER_H + rows
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        let (mx, my, mw, mh) = self.rect;
        x >= mx && x <= mx + mw && y >= my && y <= my + mh
    }

    /// The interactive row under the pointer (separators, disabled and inert
    /// rows never match).
    fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.contains(x, y) {
            return None;
        }
        let rel = y - self.rect.1;
        self.row_bounds
            .iter()
            .position(|&(off, h)| rel >= off && rel < off + h)
            .filter(|&i| {
                self.rows().get(i).is_some_and(|r| {
                    !r.separator && r.enabled && !matches!(r.action, MenuRowAction::Inert)
                })
            })
    }
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
    // Same shaping rule as cce-ui's buffer path (ASCII in a mono face →
    // Basic, no ligatures), so this measurement agrees with what the engine
    // draws — an fi ligature applied on one side only would skew widths by a
    // full advance cell.
    let mut shaping = cce_ui::cosmic_text::Shaping::Advanced;
    if let Some(ref font_name) = family_name {
        let family = match font_name.as_str() {
            "monospace" => cce_ui::cosmic_text::Family::Name(cce_ui::layout::get_system_monospace_font()),
            "sans-serif" => cce_ui::cosmic_text::Family::SansSerif,
            "serif" => cce_ui::cosmic_text::Family::Serif,
            name => cce_ui::cosmic_text::Family::Name(name),
        };
        attrs = attrs.family(family);
        shaping = cce_ui::engine::shaping_for(fs, text, &family);
    }
    buf.set_text(fs, text, attrs, shaping);
    buf.shape_until_scroll(fs, true);
    buf
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
    /// Some((color, thickness)): draw as an outlined shape — `color` fills
    /// (pass transparent for an empty ring) and the stroke uses this color
    /// and thickness. Skips the box bevel treatment.
    pub border: Option<([f32; 4], f32)>,
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
    layout: String,
    title: String,
    stats: Option<SystemStats>,
    tray_items: HashMap<String, TrayItem>,
    cursor_pos: (f64, f64),
    hovered_tray_item: Option<String>,
    tray_item_bounds: Vec<TrayIconBounds>,

    font_system: FontSystem,
    status_bar: cce_ui::widget::Adapted<cce_ui::widget::StatusBar>,

    rects: Vec<RectWidget>,
    overlay_rects: Vec<RectWidget>,
    rounded_boxes: Vec<RoundedBox>,
    /// Module boxes drawn as water droplets instead of `rounded_boxes` entries
    /// when `module { droplet }` is configured — (x, y, w, h, color, spec),
    /// painted first so module content sits on the drop.
    ///
    /// The spec rides each box rather than being read from `self.droplet` at
    /// paint time because the expanded menu box needs a DIFFERENT one — see
    /// `spec_at_reference_height` — and the scrim has to be handed the same
    /// spec the drop was drawn with or the two silhouettes disagree.
    droplet_boxes: Vec<(f32, f32, f32, f32, [f32; 4], cce_ui::scene::paint::DropletSpec)>,
    droplet: Option<cce_ui::scene::paint::DropletSpec>,
    /// Adaptive-contrast strength (0 = off) — see
    /// `read_text_contrast_from_config`. Deepens the scrim as the measured
    /// backdrop demands more; on its own (no `text_scrim`) it makes the scrim
    /// appear only when it is needed.
    text_contrast: f32,
    /// The compositor's last `backdrop` push for this segment: (luma,
    /// spread), both 0-100. Starts at the worst case, so a segment that
    /// never hears from the compositor errs toward legible rather than
    /// toward bare.
    backdrop: (u8, u8),
    /// Relative luminance of the configured module text color, 0-1. Cached
    /// at config-reload time because the contrast decision needs it every
    /// frame and the color changes about never.
    text_luma: f32,
    /// Dark feathered pool behind each module's content (0 = off) — see
    /// `read_text_scrim_from_config`. The DE's one text-contrast treatment.
    text_scrim: f32,
    /// Feather distance for that pool, logical px; None derives it from the
    /// box height.
    text_scrim_feather: Option<f32>,
    /// The contrast demand actually in effect, eased toward the backdrop's
    /// in `tick`. Stepping straight to the target makes the scrim pulse as
    /// the desktop pans under a segment, which reads as a flicker rather than
    /// as an adaptation.
    contrast_now: f32,
    text_prims: Vec<TextPrim>,

    scale_factor: f64,
    width: u32,
    height: u32,
    needs_rebuild: bool,
    box_bevel: Option<StatusBoxBevel>,
    box_bevel_depth: f32,
    context_menu: Option<ModuleContextMenu>,
    /// Expansion progress of the in-surface menu, 0 (strip) → 1 (fully
    /// open). Advanced/reversed in `tick`; `rebuild_layout` eases it into
    /// the box size, and `desired_size` grows the surface with it.
    menu_anim: f32,
    /// True while the menu is animating shut; `context_menu` is dropped
    /// only when the contraction lands back at the strip.
    menu_closing: bool,
    /// The drawn bubble's width, eased in `tick` toward `bubble_w_target`
    /// (the module's live `content_width`). The slot and the surface hold the
    /// stable `width()` — templates and title quantization keep the
    /// compositor from ever seeing a resize — while the bubble inside hugs
    /// the content, centered on the difference, so side padding stays the
    /// configured padding. Easing is what keeps a flapping window title from
    /// snapping the bubble edge on every change. 0 = not yet measured (the
    /// first rebuild snaps straight to the target). Single-module by
    /// construction: a `StatusApp` always hosts exactly one module, so one
    /// pair of fields covers "the" bubble.
    bubble_w_now: f32,
    bubble_w_target: f32,
    /// The collapsed bubble actually drawn this rebuild (x, w), slot coords —
    /// what the in-surface menu expansion grows out of and contracts back to.
    collapsed_box: Option<(f32, f32)>,
    /// The hovered menu row's highlight pill (x, y, w, h), set by the menu
    /// branch of `rebuild_layout` and drawn in `display_list` AFTER the scrim
    /// (so the pool does not darken it) and before the text.
    menu_hover_rect: Option<(f32, f32, f32, f32)>,
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

/// The Wayland `app_id` a segment presents — the compositor places segments
/// by it, and it is also how this process names itself to the `backdrop`
/// subscription. A free function because `new()` must be able to spell it
/// before there is a `StatusApp` to ask, and the two spellings below are
/// exactly the kind of thing that drifts when copied.
fn status_app_id(selected: Option<(&str, Side)>) -> String {
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_string());
    let use_interface_prefix = std::path::Path::new(&format!("/tmp/cce-status-interface-{}.sock", display)).exists();
    let prefix = if use_interface_prefix { "cce-status-interface" } else { "cce-status" };
    match selected {
        Some((name, side)) => format!("{}-{:?}-{}", prefix, side, name).to_lowercase(),
        None => prefix.to_string(),
    }
}

impl StatusApp {
    fn get_app_id(&self) -> String {
        let selected = match (&self.selected_module_name, &self.selected_module_side) {
            (Some(name), Some(side)) => Some((name.as_str(), *side)),
            _ => None,
        };
        status_app_id(selected)
    }

    /// The contrast help this segment's measured backdrop calls for, 0-1.
    ///
    /// The compositor reports a mean luminance and a spread. Contrast is
    /// checked at BOTH ends of that spread as well as at the mean, and the
    /// worst answer wins: a segment lying half on a black cell and half on a
    /// light gap averages to a perfectly comfortable mid-gray, and the text
    /// is still unreadable over one of the two halves. Checking only the mean
    /// is the mistake that makes an adaptive scheme look broken exactly where
    /// a fixed one already worked.
    fn backdrop_contrast_demand(&self) -> f32 {
        if self.text_contrast <= 0.0 {
            return 0.0;
        }
        contrast_demand(self.text_luma, self.backdrop) * self.text_contrast
    }

    fn is_vertical(&self) -> bool {
        // An open in-surface menu makes the surface taller than wide; that
        // must not read as a vertical bar (menus only open on horizontal
        // segments).
        if self.context_menu.is_some() {
            return false;
        }
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
        let padding = read_status_padding_from_config();
        let spacing = read_status_module_spacing_from_config();
        let normal_color = read_normal_color_from_config().unwrap_or(color::TEXT_FG);
        self.text_luma = relative_luminance(normal_color);
        let sw_logical = if is_vertical { self.height as f32 } else { self.width as f32 };
        let bar_h = if is_vertical { self.width as f32 } else { read_status_height_from_config() };

        self.rects.clear();
        self.overlay_rects.clear();
        self.rounded_boxes.clear();
        self.text_prims.clear();
        self.input_regions.clear();
        self.module_bounds.clear();
        self.tray_item_bounds.clear();
        self.collapsed_box = None;
        self.menu_hover_rect = None;

        let left_modules = std::mem::take(&mut self.left_modules);
        let right_modules = std::mem::take(&mut self.right_modules);

        let box_bg_color = read_status_box_background_color_from_config();
        let status_box_radius = read_status_box_corner_radius_from_config();
        self.box_bevel = read_status_box_bevel_from_config();
        self.box_bevel_depth = read_status_box_bevel_depth_from_config();
        self.droplet = read_droplet_from_config();
        self.droplet_boxes.clear();
        self.text_contrast = read_text_contrast_from_config();
        self.text_scrim = read_text_scrim_from_config();
        self.text_scrim_feather = read_text_scrim_feather_from_config();

        self.status_bar.set_rect(0.0, 0.0, self.width as f32, self.height as f32);
        // The surface itself is transparent: every StatusApp is a single
        // `--module` segment (the no-arg form is the launcher daemon and
        // never creates a surface), so the only painted background is each
        // module's own rounded box.
        self.status_bar.set_bg_color([0.0, 0.0, 0.0, 0.0]);


        let is_single = self.selected_module_name.is_some();
        let margin_padding = if is_single { 6.0 } else { 12.0 };

        let mut left_x = margin_padding;
        let mut is_first_left = true;
        for module in &left_modules {
            let w = module.width(
                &self.stats,
                &self.title,
                &mut self.font_system,
                &font_family,
                font_size,
                &self.tray_items,
                padding,
            );
            if w > 0.0 {
                if !is_first_left {
                    left_x += spacing;
                }
                is_first_left = false;

                // The bubble hugs the LIVE content, centered in the stable
                // slot: `width()` keeps the surface from resizing (the
                // configure-echo jitter), while the drawn box shrinks so the
                // padding on each side of the text is the configured padding
                // rather than padding-plus-template-surplus. The drawn width
                // is `bubble_w_now`, eased toward the live measure in `tick`.
                let cw = module
                    .content_width(&self.stats, &self.title, &mut self.font_system, &font_family, font_size, &self.tray_items, padding)
                    .min(w);
                self.bubble_w_target = cw;
                if self.bubble_w_now <= 0.0 {
                    self.bubble_w_now = cw;
                }
                let bw = self.bubble_w_now.min(w);
                let bx = left_x + (w - bw) / 2.0;
                self.collapsed_box = Some((bx, bw));

                // With the in-surface menu open the module box is replaced by
                // the unified expanded box drawn in the menu branch below —
                // the module box GROWS into the menu, it doesn't sit atop it.
                if !module.has_custom_background(&self.title) && self.context_menu.is_none() {
                    if let Some(color) = box_bg_color {
                        if let Some(spec) = self.droplet {
                            // Inset from the surface bottom: 1px for the
                            // belly silhouette's AA feather, plus the
                            // contact shadow's reserved gap.
                            let inset = 1.0 + spec.shadow_gap(bar_h);
                            self.droplet_boxes.push((bx, 0.0, bw, bar_h - inset, color, spec));
                        } else {
                            self.rounded_boxes.push(RoundedBox {
                                x: bx,
                                y: 0.0,
                                w: bw,
                                h: bar_h,
                                radius: status_box_radius,
                                color,
                                corners: if self.selected_module_name.is_some() { (true, true, true, true) } else { (false, false, true, true) },
                                border: None,
                            });
                        }
                    }
                }

                module.render(
                    bx,
                    bw,
                    &self.stats,
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

                // See the left loop: the bubble hugs the eased live content
                // width, centered in the stable slot.
                let cw = module
                    .content_width(&self.stats, &self.title, &mut self.font_system, &font_family, font_size, &self.tray_items, padding)
                    .min(w);
                self.bubble_w_target = cw;
                if self.bubble_w_now <= 0.0 {
                    self.bubble_w_now = cw;
                }
                let bw = self.bubble_w_now.min(w);
                let bx = right_x + (w - bw) / 2.0;
                self.collapsed_box = Some((bx, bw));

                // See the left loop: an open in-surface menu swaps the module
                // box for the unified expanded box.
                if !module.has_custom_background(&self.title) && self.context_menu.is_none() {
                    if let Some(color) = box_bg_color {
                        if let Some(spec) = self.droplet {
                            // Same bottom inset as the left loop.
                            let inset = 1.0 + spec.shadow_gap(bar_h);
                            self.droplet_boxes.push((bx, 0.0, bw, bar_h - inset, color, spec));
                        } else {
                            self.rounded_boxes.push(RoundedBox {
                                x: bx,
                                y: 0.0,
                                w: bw,
                                h: bar_h,
                                radius: status_box_radius,
                                color,
                                corners: if self.selected_module_name.is_some() { (true, true, true, true) } else { (false, false, true, true) },
                                border: None,
                            });
                        }
                    }
                }

                module.render(
                    bx,
                    bw,
                    &self.stats,
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

        // 5c. Tooltip Rendering (if hovered) — suppressed while the
        // in-surface menu is open (the tooltip would overlap the menu header).
        if let Some(ref hovered_id) = self.hovered_tray_item.clone().filter(|_| self.context_menu.is_none()) {
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
                    // No scrim: the tooltip already sits on its own box.
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

                // No open menu: the surface is exactly the bar strip. This
                // reset is what COLLAPSES an expanded surface after the menu
                // closes — the compositor deliberately stops enforcing size
                // while we are thicker than the bar, so nobody else will.
                if self.context_menu.is_none() {
                    self.height = bar_h.round() as u32;
                }

                // In-surface context menu: grow the surface below the bar
                // strip and draw the current page into the retained buffers.
                // The panel reuses the module box pipeline (so box_bevel
                // applies) with rows, separators and a hover highlight on top.
                if let Some(menu) = &mut self.context_menu {
                    // The unified box keeps the module box's own margin so the
                    // strip band reads as the module box, grown.
                    let plate_x = margin_padding;
                    let module_box_w = self.width as f32 - 2.0 * plate_x;
                    // Wide enough for the longest row label — fixed minimums
                    // truncated window titles in the picker.
                    let tx_probe = ModuleContextMenu::PAD + 8.0;
                    let mut label_w: f32 = 0.0;
                    for page_row in menu.rows().to_vec() {
                        if !page_row.separator {
                            let l = cce_ui::widget::StyledLabel::new_with_family(&mut self.font_system, &page_row.label, font_size, [0.0, 0.0, 0.0, 1.0], &font_family);
                            label_w = label_w.max(l.w);
                        }
                    }
                    let menu_w = module_box_w.max(menu.min_w).max(label_w + 2.0 * tx_probe);
                    let menu_h = menu.height();
                    // Expansion animation: ease `menu_anim` (stepped in tick)
                    // into the box, growing width and revealing height from
                    // the collapsed module box. Rows keep their final
                    // positions and slide into view as the surface bottom
                    // edge (which clips them) travels down.
                    let t = {
                        let a = self.menu_anim.clamp(0.0, 1.0);
                        1.0 - (1.0 - a) * (1.0 - a) * (1.0 - a)
                    };
                    // The expansion grows out of the bubble actually drawn on
                    // the strip — which may sit inset in its slot, hugging
                    // the live content — not out of the slot itself, so the
                    // "module box grows into the menu" continuity holds with
                    // content-hugging bubbles. Contraction reverses back to
                    // the same collapsed box.
                    let (start_x, start_w) = self.collapsed_box.unwrap_or((plate_x, module_box_w));
                    let anim_w = start_w + (menu_w - start_w) * t;
                    let anim_x = start_x + (plate_x - start_x) * t;
                    let reveal_h = menu_h * t;
                    menu.rect = (anim_x, bar_h, anim_w, reveal_h);
                    self.width = self.width.max((anim_x + anim_w + plate_x).round() as u32);
                    self.height = (bar_h + reveal_h).round() as u32;

                    // ONE continuous box in the module's own fill, spanning
                    // the strip band and the menu — the module box literally
                    // grows into the menu. Inserted at the front so the
                    // module's strip content (tray icons, labels)
                    // renders on top of its band. In droplet style the drop
                    // grows DOWNWARD without growing its curvature: the knobs
                    // are re-resolved against the collapsed height, so the
                    // taper and the bottom corners stay the size they are on
                    // the bar and the sides run straight between them. Letting
                    // them scale with the box is what stopped the backdrop
                    // conforming to the rows it is behind.
                    let menu_color = box_bg_color.unwrap_or([0.055, 0.055, 0.075, 0.97]);
                    if let Some(spec) = self.droplet {
                        let inset = 1.0 + spec.shadow_gap(bar_h);
                        let collapsed_h = bar_h - inset;
                        let box_h = bar_h + reveal_h - inset;
                        let menu_spec = spec_at_reference_height(spec, collapsed_h, box_h);
                        self.droplet_boxes.push((anim_x, 0.0, anim_w, box_h, menu_color, menu_spec));
                    } else {
                        self.rounded_boxes.insert(0, RoundedBox {
                            x: anim_x,
                            y: 0.0,
                            w: anim_w,
                            h: bar_h + reveal_h,
                            radius: status_box_radius.max(4.0),
                            color: menu_color,
                            corners: (true, true, true, true),
                            border: None,
                        });
                    }

                    let text_u8 = [
                        (normal_color[0] * 255.0) as u8,
                        (normal_color[1] * 255.0) as u8,
                        (normal_color[2] * 255.0) as u8,
                    ];
                    let dim_u8 = [
                        (normal_color[0] * 150.0) as u8,
                        (normal_color[1] * 150.0) as u8,
                        (normal_color[2] * 150.0) as u8,
                    ];
                    let tx = plate_x + ModuleContextMenu::PAD + 8.0;
                    self.text_prims.push((
                        menu.title().to_string(),
                        font_size,
                        tx,
                        bar_h + ModuleContextMenu::PAD
                            + (ModuleContextMenu::HEADER_H - font_size) / 2.0,
                        dim_u8,
                        Some(font_family.clone()),
                        None,
                        None,
                        // Menu text sits on the expanded box; no scrim.
                        None,
                    ));

                    let rows = menu.rows().to_vec();
                    let hovered = menu.hovered;
                    let mut bounds = Vec::with_capacity(rows.len());
                    let mut off = ModuleContextMenu::PAD + ModuleContextMenu::HEADER_H;
                    for (i, row) in rows.iter().enumerate() {
                        let h = if row.separator {
                            ModuleContextMenu::SEP_H
                        } else {
                            ModuleContextMenu::ITEM_H
                        };
                        let iy = bar_h + off;
                        if row.separator {
                            self.rects.push(RectWidget {
                                x: tx,
                                y: iy + h / 2.0,
                                w: anim_w - 2.0 * (ModuleContextMenu::PAD + 8.0),
                                h: 1.0,
                                color: [0.35, 0.35, 0.42, 0.8],
                            });
                        } else {
                            if hovered == Some(i) && row.enabled {
                                // A rounded pill inset from the panel edge,
                                // drawn post-scrim in display_list — a
                                // square-cornered full-width rect butting
                                // into the rounded silhouette was the last
                                // blocky element of the expanded menu.
                                self.menu_hover_rect =
                                    Some((anim_x + 6.0, iy + 1.0, anim_w - 12.0, h - 2.0));
                            }
                            self.text_prims.push((
                                row.label.clone(),
                                font_size,
                                tx,
                                iy + (h - font_size) / 2.0,
                                if row.enabled { text_u8 } else { dim_u8 },
                                Some(font_family.clone()),
                                None,
                                None,
                                None,
                            ));
                        }
                        bounds.push((off, h));
                        off += h;
                    }
                    menu.row_bounds = bounds;

                    self.input_regions.clear();
                    self.input_regions.push((0, 0, self.width as i32, self.height as i32));
                }
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

        // Otherwise this is a click on the status-bar "window" module: open
        // the click-to-pick window list as an IN-SURFACE menu (the window
        // module's own surface expands below the strip).
        let thread_sender = self.sender.clone();
        std::thread::spawn(move || {
            let output = std::process::Command::new(get_ccectl_cmd())
                .args(["windows", "--json"])
                .output();
            let windows = if let Ok(out) = output {
                parse_ccectl_windows(&String::from_utf8_lossy(&out.stdout))
            } else {
                Vec::new()
            };
            if windows.is_empty() {
                return;
            }
            let rows = windows
                .into_iter()
                // The bar's own segments are noise in a window picker.
                .filter(|(_, app_id, _, _)| !app_id.starts_with("cce-status"))
                .map(|(id, app_id, title, _)| {
                    let display = if title.is_empty() {
                        app_id.clone()
                    } else {
                        format!("{} ({})", title, app_id)
                    };
                    MenuRow {
                        label: display,
                        enabled: true,
                        separator: false,
                        action: MenuRowAction::Ccectl(vec!["focus-window".to_string(), id]),
                    }
                })
                .collect();
            let _ = thread_sender.send(CustomEvent::MenuReady {
                title: "Windows".to_string(),
                pages: vec![MenuPage { title: "Windows".to_string(), rows }],
                min_w: 260.0,
            });
        });
    }
}


fn get_module_side(name: &str) -> Side {
    module_side_from_json(&get_cached_config(), name)
}

fn module_side_from_json(val: &serde_json::Value, name: &str) -> Side {
    if name == "light_source" {
        // The light module ignores any status_bar side entry: it sits on
        // whichever side the configured light angle points at.
        let light_pos = crate::config::light_source_position_from(val);

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

    // Canonical: `layout { status_bar <name>="top-left" }` — the same key the
    // compositor persists a super+drag snap into.
    let pointer = format!("/layout/status_bar/{}", name);
    if let Some(side_val) = val.pointer(&pointer) {
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
/// The last field is the run's MEASURED width in logical px, when the emitter
/// knew it — `draw_label` always does, since the label was built for its
/// width. It is what lets the text scrim hug the run instead of the whole
/// module box; `None` simply gets no scrim, which is right for the menu and
/// tooltip text that sits on an opaque box already.
pub(crate) type TextPrim = (String, f32, f32, f32, [u8; 3], Option<String>, Option<[f32; 4]>, Option<cce_ui::scene::paint::TextLayout>, Option<f32>);

/// Emit a measured `StyledLabel` as a text-prim tuple, returning its width (like the legacy
/// `StyledLabel::draw`). The label was built for its width; `into_prim` carries the source
/// text/size/family/box-layout so the engine reshapes it through the shared cache.
pub(crate) fn draw_label(prims: &mut Vec<TextPrim>, label: cce_ui::widget::StyledLabel, x: f32, y: f32) -> f32 {
    let w = label.w;
    let p = label.into_prim(x, y);
    prims.push((p.text, p.size, p.x, p.y, p.color, p.font, None, p.layout, Some(w)));
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
            tokio::spawn(spawn_status_listener("layout".to_string(), sender.clone()));
            tokio::spawn(spawn_status_listener("title".to_string(), sender.clone()));
        }
        // Every module can host an in-surface menu, so every process listens
        // for the compositor's click-away dismiss pushes.
        tokio::spawn(spawn_status_listener("dismiss".to_string(), sender.clone()));
        // ...and every module has text over a backdrop it cannot see, so
        // every one asks the compositor what it is sitting on. Subscribed
        // unconditionally rather than behind `module { text_contrast }`: the
        // knob is re-read live from the config file, and a task spawned once
        // in `new()` could not follow it being switched on.
        {
            let app_id = status_app_id(
                selected_module.as_ref().map(|(name, side)| (name.as_str(), *side)),
            );
            tokio::spawn(spawn_status_listener(format!("backdrop {}", app_id), sender.clone()));
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
            layout: String::new(),
            title: String::new(),
            stats: if has_stats { Some(get_initial_stats()) } else { None },
            tray_items: HashMap::new(),
            cursor_pos: (0.0, 0.0),
            hovered_tray_item: None,
            tray_item_bounds: Vec::new(),
            font_system,
            status_bar: cce_ui::widget::StatusBar::new(),
            rects: Vec::new(),
            overlay_rects: Vec::new(),
            rounded_boxes: Vec::new(),
            droplet_boxes: Vec::new(),
            droplet: None,
            text_contrast: 0.0,
            backdrop: (50, 100),
            text_luma: 0.0,
            text_scrim: 0.0,
            text_scrim_feather: None,
            contrast_now: 0.0,
            text_prims: Vec::new(),
            scale_factor: 1.0,
            width: if selected_module.is_some() { 120 } else { 1920 },
            height: read_status_height_from_config() as u32,
            needs_rebuild: true,
            box_bevel: None,
            box_bevel_depth: 3.0,
            context_menu: None,
            menu_anim: 0.0,
            menu_closing: false,
            menu_hover_rect: None,
            bubble_w_now: 0.0,
            bubble_w_target: 0.0,
            collapsed_box: None,
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
        // title push) no longer forces a redraw — and the compositor's
        // whole-backdrop blur re-bake — every time.
        let mut changed = true;
        match msg {
            CustomEvent::LayoutUpdated(l) => {
                // Nothing renders the mode in the strip anymore; it is read
                // at menu-open time for the window module's menu row.
                changed = false;
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
            CustomEvent::MenuReady { title, pages, min_w } => {
                if !pages.is_empty() && !self.is_vertical() {
                    let _ = title;
                    if self.context_menu.is_none() {
                        self.menu_anim = 0.0;
                    }
                    self.menu_closing = false;
                    self.context_menu = Some(ModuleContextMenu {
                        pages,
                        page: 0,
                        tray_target: None,
                        min_w,
                        hovered: None,
                        rect: (0.0, 0.0, 0.0, 0.0),
                        row_bounds: Vec::new(),
                    });
                } else {
                    changed = false;
                }
            }
            CustomEvent::TrayMenuFetched { destination, menu_path, pages } => {
                if !pages.is_empty() && !self.is_vertical() {
                    if self.context_menu.is_none() {
                        self.menu_anim = 0.0;
                    }
                    self.menu_closing = false;
                    self.context_menu = Some(ModuleContextMenu {
                        pages,
                        page: 0,
                        tray_target: Some((destination, menu_path)),
                        min_w: 260.0,
                        hovered: None,
                        rect: (0.0, 0.0, 0.0, 0.0),
                        row_bounds: Vec::new(),
                    });
                } else {
                    changed = false;
                }
            }
            CustomEvent::BackdropUpdated(sample) => {
                if self.backdrop != sample {
                    self.backdrop = sample;
                    // Only the paint changes, but something has to ask for a
                    // frame: `tick` eases toward the new target and nothing
                    // else on this segment is animating.
                    self.needs_rebuild = true;
                }
            }
            CustomEvent::SwitcherTriggered => {
                log::debug!("[switcher] SwitcherTriggered event received, calling trigger_switcher");
                self.trigger_switcher(true);
            }
            CustomEvent::MenuDismiss(pressed_app_id) => {
                // Close-on-click-away, unless the press was on THIS segment
                // (then handle_mouse_input already decided what to do).
                if self.context_menu.is_some()
                    && !self.menu_closing
                    && pressed_app_id != self.get_app_id()
                {
                    self.menu_closing = true;
                } else {
                    changed = false;
                }
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

    fn tick(&mut self, dt: f32, needs_rebuild: &mut bool) {
        // Ease toward what the backdrop currently demands. The measurement
        // itself is quantized and only pushed on change, so this is the only
        // thing standing between a camera pan and the scrim pulsing on the
        // cell edges it crosses.
        if self.text_contrast > 0.0 {
            const CONTRAST_EASE_S: f32 = 0.12;
            let target = self.backdrop_contrast_demand();
            if (target - self.contrast_now).abs() > 0.002 {
                let step = (dt / CONTRAST_EASE_S).clamp(0.0, 1.0);
                self.contrast_now += (target - self.contrast_now) * step;
                *needs_rebuild = true;
                self.needs_rebuild = true;
            } else if self.contrast_now != target {
                self.contrast_now = target;
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }

        // Ease the drawn bubble toward its live-content width — the same
        // treatment the scrim gets: stepping straight there would snap the
        // bubble edges on every stat update or title change, and a flapping
        // browser title would make the box twitch instead of breathe.
        {
            const BUBBLE_EASE_S: f32 = 0.12;
            let target = self.bubble_w_target;
            if (target - self.bubble_w_now).abs() > 0.5 {
                let step = (dt / BUBBLE_EASE_S).clamp(0.0, 1.0);
                self.bubble_w_now += (target - self.bubble_w_now) * step;
                *needs_rebuild = true;
                self.needs_rebuild = true;
            } else if self.bubble_w_now != target && target > 0.0 {
                self.bubble_w_now = target;
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }

        // In-surface menu expansion/contraction: the surface grows into the
        // menu and shrinks back over MENU_ANIM_S, one resize+repaint per
        // tick. The menu object is dropped only when the contraction lands.
        if self.context_menu.is_some() {
            const MENU_ANIM_S: f32 = 0.14;
            if self.menu_closing {
                self.menu_anim -= dt / MENU_ANIM_S;
                if self.menu_anim <= 0.0 {
                    self.menu_anim = 0.0;
                    self.menu_closing = false;
                    self.context_menu = None;
                }
                *needs_rebuild = true;
                self.needs_rebuild = true;
            } else if self.menu_anim < 1.0 {
                self.menu_anim = (self.menu_anim + dt / MENU_ANIM_S).min(1.0);
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }

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
        // Phase 6ak single paint path: the rounded boxes, the status-bar bg / module rects
        // (the legacy view_rounded_quads then view() bodies, in the wrapper's
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

        // Droplet-style module boxes paint first, so any remaining rounded
        // boxes (module-internal chips) and all content sit on the drops.
        for &(x, y, w, h, color, spec) in &self.droplet_boxes {
            pc.droplet(Rect { x, y, width: w, height: h }, color, spec);
        }

        for rb in &self.rounded_boxes {
            let rect = Rect { x: rb.x, y: rb.y, width: rb.w, height: rb.h };
            // Same positional corner→radius mapping the RoundedRect prim uses.
            let radii = (
                if rb.corners.0 { rb.radius } else { 0.0 },
                if rb.corners.1 { rb.radius } else { 0.0 },
                if rb.corners.2 { rb.radius } else { 0.0 },
                if rb.corners.3 { rb.radius } else { 0.0 },
            );
            // True-shape boxes (the light module's circle): no bevel
            // treatment. A full circle draws through the Circle/Arc prims —
            // the rounded-rect corner family is the squircle (corner_shape),
            // which reads as a rounded SQUARE at half-extent radius — with
            // the fill and stroke each optional. Anything else outlined goes
            // through the Border prim (fill + stroke).
            if let Some((border_color, thickness)) = rb.border {
                if (rb.w - rb.h).abs() < 0.5 && (rb.radius - rb.w / 2.0).abs() < 0.5 {
                    let r = rb.w / 2.0;
                    if rb.color[3] > 0.001 {
                        // With raised module boxes (lit plates), the circle
                        // takes the sphere-lit disc — the circular sibling of
                        // the plate treatment, same light and material.
                        if matches!(self.box_bevel, Some(StatusBoxBevel::Raised)) {
                            pc.sphere(rb.x + r, rb.y + r, r, rb.color);
                        } else {
                            pc.circle(rb.x + r, rb.y + r, r, rb.color);
                        }
                    }
                    if thickness > 0.05 {
                        pc.arc(rb.x + r, rb.y + r, r, thickness, 0.0, std::f32::consts::TAU, border_color);
                    }
                } else {
                    pc.border(rect, radii, rb.color, border_color, thickness);
                }
                continue;
            }
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

        // The pool, one per module box, filling the bubble rather than
        // hugging the run inside it, so a segment reads as one darkened
        // lozenge instead of a pill within a pill.
        //
        // Two shapes, because "the bubble" is two different things: a droplet
        // module gets a pool of the droplet's own silhouette (below), while a
        // plain rounded box gets `Prim::Glow` — a feathered aura, solid
        // through its core rect and falling off across `reach`, so insetting
        // the core by exactly the feather lands the gradient's outer edge on
        // the box's own edge.
        if self.text_scrim > 0.0 || self.text_contrast > 0.0 {
            // Rests at the configured opacity and deepens with the measured
            // demand. Either knob alone is meaningful: `text_scrim` with no
            // `text_contrast` is a constant ground (the demand stays zero),
            // and `text_contrast` with no `text_scrim` is a ground that
            // appears only when the backdrop earns it.
            let alpha = scrim_alpha(self.text_scrim, self.contrast_now);
            let feather_cfg = self.text_scrim_feather;
            let runs = &self.text_prims;
            let icons = &self.tray_item_bounds;
            let pool_in = |pc: &mut cce_ui::scene::paint::PaintCtx, bx: f32, by: f32, bw: f32, bh: f32, radius: f32| {
                // Colored for the text it is protecting — the widest run
                // inside this box, since a box with mixed colors is being
                // led by its longest label. A box holding no measured run
                // but tray icons is grounded for those (light glyphs, so a
                // black pool); a box holding neither gets no pool at all.
                let Some(color) = dominant_run_color(runs, bx, by, bw, bh)
                    .or_else(|| dominant_icon_color(icons, bx, by, bw, bh))
                else {
                    return;
                };
                let feather = scrim_feather(bw, bh, feather_cfg);
                let core = Rect {
                    x: bx + feather,
                    y: by + feather,
                    width: (bw - feather * 2.0).max(0.0),
                    height: (bh - feather * 2.0).max(0.0),
                };
                if core.width <= 0.0 || core.height <= 0.0 {
                    return;
                }
                let c = treatment_rgb(color);
                let box_rect = Rect { x: bx, y: by, width: bw, height: bh };
                pc.clip_rounded(box_rect, radius, |pc| {
                    pc.glow(core, (radius - feather).max(0.0), feather, [c[0], c[1], c[2], alpha]);
                });
            };
            // A droplet bubble gets a pool of its OWN silhouette, not a
            // rounded-rect stand-in: cce-ui's `Prim::DropletScrim` runs the
            // droplet's shader path with the same spec, filled flat and
            // feathered inward, so the vignette's edge is the drop's edge by
            // construction rather than by approximation.
            for &(x, y, w, h, _, spec) in &self.droplet_boxes {
                let rect = Rect { x, y, width: w, height: h };
                let Some(color) = dominant_run_color(runs, x, y, w, h)
                    .or_else(|| dominant_icon_color(icons, x, y, w, h))
                else {
                    continue;
                };
                let c = treatment_rgb(color);
                let feather = scrim_feather(w, h, feather_cfg);
                pc.droplet_scrim(rect, [c[0], c[1], c[2], alpha], spec, feather);
            }
            // Everything else is genuinely a rounded rect, so a rounded-rect
            // pool IS its exact shape.
            for rb in &self.rounded_boxes {
                pool_in(&mut pc, rb.x, rb.y, rb.w, rb.h, rb.radius);
            }
        }

        let (sb_x, sb_y, sb_w, sb_h) = self.status_bar.rect();
        pc.quad(Rect { x: sb_x, y: sb_y, width: sb_w, height: sb_h }, self.status_bar.color());
        for r in &self.rects {
            pc.quad(Rect { x: r.x, y: r.y, width: r.w, height: r.h }, r.color);
        }

        // The hovered menu row's pill: post-scrim so the pool cannot darken
        // it, pre-text so the label sits on it.
        if let Some((hx, hy, hw, hh)) = self.menu_hover_rect {
            pc.rounded_rect(
                Rect { x: hx, y: hy, width: hw, height: hh },
                (hh / 2.0).min(8.0),
                (true, true, true, true),
                [0.23, 0.35, 0.50, 0.55],
            );
        }

        for (text, tsize, x, y, color, font, bounds, layout, _run_w) in &self.text_prims {
            match layout {
                Some(l) => pc.text_boxed(text.clone(), *x, *y, *tsize, *color, font.clone(), *bounds, cce_ui::scene::paint::TextAttrs::default(), *l),
                // Glyphs are drawn plain. Contrast is the scrim's job now —
                // it darkens the ground rather than decorating the
                // letterforms, and the two together were always one treatment
                // too many.
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
        if let Some(menu) = &mut self.context_menu {
            let hovered = menu.item_at(pos.x, pos.y);
            if hovered != menu.hovered {
                menu.hovered = hovered;
                *needs_rebuild = true;
                self.needs_rebuild = true;
            }
        }
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

    fn handle_mouse_input(&mut self, button: MouseButton, state: ElementState, pos: cce_ui::engine::LogicalPosition, needs_rebuild: &mut bool) -> Option<Self::Message> {
        let (lx, ly) = (pos.x, pos.y);
        let is_vertical = self.is_vertical();
        let coord = if is_vertical { ly } else { lx };
        let cx = lx as f64;
        let cy = ly as f64;

        // An open in-surface menu owns every button event: row clicks run
        // their action (dispatch / DBusMenu event / page navigation); any
        // other press (bar strip, menu padding, right-click) closes.
        if self.context_menu.is_some() {
            if state != ElementState::Pressed {
                return None;
            }
            // A menu animating shut is already spoken for — its rows are
            // sliding away, so presses neither re-trigger nor re-open.
            if self.menu_closing {
                return None;
            }
            let hit = if button == MouseButton::Left {
                self.context_menu.as_ref().and_then(|m| m.item_at(lx, ly))
            } else {
                None
            };
            let mut result = None;
            match hit {
                Some(i) => {
                    let menu = self.context_menu.as_mut().unwrap();
                    let action = menu.rows().get(i).map(|r| r.action.clone());
                    match action {
                        Some(MenuRowAction::Dispatch(ev)) => {
                            self.menu_closing = true;
                            result = Some(ev);
                        }
                        Some(MenuRowAction::Item(id)) => {
                            if let Some((dest, path)) = menu.tray_target.clone() {
                                send_tray_menu_event(dest, path, id);
                            }
                            self.menu_closing = true;
                        }
                        Some(MenuRowAction::Submenu(p)) | Some(MenuRowAction::Back(p)) => {
                            menu.page = p;
                            menu.hovered = None;
                        }
                        Some(MenuRowAction::Ccectl(args)) => {
                            std::thread::spawn(move || {
                                let _ = std::process::Command::new(get_ccectl_cmd())
                                    .args(&args)
                                    .spawn();
                            });
                            self.menu_closing = true;
                        }
                        _ => {}
                    }
                }
                None => {
                    self.menu_closing = true;
                }
            }
            self.needs_rebuild = true;
            *needs_rebuild = true;
            return result;
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
                let btn_code = match button {
                    MouseButton::Left => 272,
                    MouseButton::Right => 273,
                    _ => 0,
                };
                let cx_i = cx as i32;
                let cy_i = cy as i32;
                let thread_sender = self.sender.clone();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    rt.block_on(async move {
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

                                            // In-surface menu: fetch the DBusMenu layout and hand
                                            // it to the module's update loop — the tray segment's
                                            // own surface expands to show it (no popup process).
                                            if should_show_menu {
                                                if let Some(menu_p) = menu_path {
                                                    match fetch_tray_menu_pages(&conn, destination, menu_p.as_str()).await {
                                                        Ok(pages) if !pages.is_empty() => {
                                                            let _ = thread_sender.send(CustomEvent::TrayMenuFetched {
                                                                destination: destination.to_string(),
                                                                menu_path: menu_p.as_str().to_string(),
                                                                pages,
                                                            });
                                                        }
                                                        Ok(_) => log::debug!("[tray-menu] empty menu for {}", destination),
                                                        Err(e) => log::warn!("[tray-menu] fetch failed: {:?}", e),
                                                    }
                                                }
                                            } else if btn_code == 272 {
                                                if let Err(e) = proxy.activate(cx_i, cy_i).await {
                                                    log::warn!("[tray-click] Activate failed: {:?}", e);
                                                    if let Some(menu_p) = menu_path {
                                                        if let Ok(pages) = fetch_tray_menu_pages(&conn, destination, menu_p.as_str()).await {
                                                            if !pages.is_empty() {
                                                                let _ = thread_sender.send(CustomEvent::TrayMenuFetched {
                                                                    destination: destination.to_string(),
                                                                    menu_path: menu_p.as_str().to_string(),
                                                                    pages,
                                                                });
                                                            }
                                                        }
                                                    }
                                                }
                                            } else if btn_code == 273 {
                                                let _ = proxy.context_menu(cx_i, cy_i).await;
                                            }
                                        }
                                        Err(e) => log::warn!("[tray-click] Failed to build proxy: {:?}", e),
                                    }
                                }
                                Err(e) => log::warn!("[tray-click] Failed to connect to session bus: {:?}", e),
                            }
                        } else {
                            log::warn!("[tray-click] Failed to split id: {}", id);
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
                    if is_vertical {
                        // v1: the in-surface menu only lays out on horizontal
                        // segments.
                        return None;
                    }
                    log::debug!("[module-right-click] opening in-surface menu for: {}", mb.name);
                    if self.context_menu.is_none() {
                        self.menu_anim = 0.0;
                    }
                    self.menu_closing = false;
                    let dispatch_row = |label: &str, ev: CustomEvent| MenuRow {
                        label: label.to_string(),
                        enabled: true,
                        separator: false,
                        action: MenuRowAction::Dispatch(ev),
                    };
                    let mut rows = if self.adjust_position_mode {
                        vec![dispatch_row("Done", CustomEvent::ToggleAdjustPositionMode)]
                    } else {
                        vec![
                            dispatch_row(
                                if self.status_hide_mode { "Show Modules" } else { "Hide Modules" },
                                CustomEvent::ToggleHideModules,
                            ),
                            dispatch_row("Adjust Positions", CustomEvent::ToggleAdjustPositionMode),
                        ]
                    };
                    // The light module's strip presence is just the empty
                    // circle; its value lives here in the menu.
                    if mb.name == "light_source" {
                        rows.insert(0, MenuRow {
                            label: format!("{:.2} rad", modules::get_light_source_pos_from_config()),
                            enabled: true,
                            separator: false,
                            action: MenuRowAction::Inert,
                        });
                    }
                    // The window module's strip shows only the title; the
                    // focused window's mode lives here in its menu — as a
                    // dropdown when the mode is one a user may set, opening
                    // a submenu page whose rows run `ccectl set-mode <mode>`
                    // on the focused window (status segments never take seat
                    // focus, so "focused" is still the real window).
                    let mut extra_pages: Vec<MenuPage> = Vec::new();
                    if mb.name == "window" && !self.layout.is_empty() {
                        const MODES: [&str; 3] = ["Floating", "Tiled", "Fullscreen"];
                        if MODES.contains(&self.layout.as_str()) {
                            // Page 0 is the root built below; the mode page is
                            // the only extra, so it is always page 1.
                            rows.insert(0, MenuRow {
                                label: format!("{} >", self.layout),
                                enabled: true,
                                separator: false,
                                action: MenuRowAction::Submenu(1),
                            });
                            let mut mode_rows = vec![MenuRow {
                                label: "< Back".to_string(),
                                enabled: true,
                                separator: false,
                                action: MenuRowAction::Back(0),
                            }];
                            for mode in MODES {
                                let current = mode == self.layout;
                                mode_rows.push(MenuRow {
                                    label: format!(
                                        "{} {}",
                                        if current { "[x]" } else { "[ ]" },
                                        mode
                                    ),
                                    // The current mode is a marker, not a
                                    // target — disabled rows never match a
                                    // click.
                                    enabled: !current,
                                    separator: false,
                                    action: MenuRowAction::Ccectl(vec![
                                        "set-mode".to_string(),
                                        mode.to_lowercase(),
                                    ]),
                                });
                            }
                            extra_pages.push(MenuPage { title: "Mode".to_string(), rows: mode_rows });
                        } else {
                            // Internal roles (Popup/Overlay/Status/Utility)
                            // and the no-focus "---" stay a plain readout.
                            rows.insert(0, MenuRow {
                                label: self.layout.clone(),
                                enabled: true,
                                separator: false,
                                action: MenuRowAction::Inert,
                            });
                        }
                    }
                    let mut pages = vec![MenuPage { title: mb.name.clone(), rows }];
                    pages.extend(extra_pages);
                    self.context_menu = Some(ModuleContextMenu {
                        pages,
                        page: 0,
                        tray_target: None,
                        min_w: 190.0,
                        hovered: None,
                        rect: (0.0, 0.0, 0.0, 0.0),
                        row_bounds: Vec::new(),
                    });
                    self.needs_rebuild = true;
                    *needs_rebuild = true;
                    return None;
                }
            }

            if button == MouseButton::Left {
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
                    log::debug!("[window-click] Window module clicked, opening window picker");
                    self.trigger_switcher(false);
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
    // Adaptive contrast: the bar cannot see its own backdrop, so these pin
    // down what it does with the compositor's measurement of it.
    // ------------------------------------------------------------------

    /// Luminance of the black text the droplet style is configured with.
    const BLACK_TEXT: f32 = 0.0;
    const WHITE_TEXT: f32 = 1.0;

    #[test]
    fn dark_text_on_a_light_uniform_backdrop_wants_no_scrim() {
        // The case that must stay untouched: the bar already reads fine, so
        // an adaptive scheme that decorates it anyway is worse than nothing.
        assert_eq!(contrast_demand(BLACK_TEXT, (100, 0)), 0.0);
    }

    #[test]
    fn dark_text_on_a_dark_uniform_backdrop_wants_a_full_scrim() {
        // Black text over a black grid cell — invisible, and the whole
        // reason for the feature.
        assert_eq!(contrast_demand(BLACK_TEXT, (0, 0)), 1.0);
    }

    #[test]
    fn light_text_reverses_the_verdict() {
        // The decision is about the CONFIGURED color, not a hardcoded
        // assumption that module text is dark.
        assert_eq!(contrast_demand(WHITE_TEXT, (0, 0)), 0.0);
        assert_eq!(contrast_demand(WHITE_TEXT, (100, 0)), 1.0);
    }

    #[test]
    fn a_comfortable_mean_over_a_split_backdrop_still_wants_a_scrim() {
        // Half black cell, half light gap: the mean alone says "mid-gray,
        // fine" while the text is invisible over one half. Checking the
        // spread's ends is what catches it.
        let mean_only = contrast_deficit(BLACK_TEXT, 0.5);
        let with_spread = contrast_demand(BLACK_TEXT, (50, 100));
        assert!(with_spread > mean_only, "{} !> {}", with_spread, mean_only);
        assert_eq!(with_spread, 1.0);
    }

    #[test]
    fn an_unknown_backdrop_is_treated_as_the_worst_case() {
        // What a segment reports before it has heard from the compositor,
        // and what an occluding window resolves to: assume unreadable.
        assert_eq!(contrast_demand(BLACK_TEXT, (50, 100)), 1.0);
    }

    const BLACK: [f32; 3] = [0.0, 0.0, 0.0];
    const WHITE: [f32; 3] = [1.0, 1.0, 1.0];

    #[test]
    fn a_treatment_contrasts_with_the_text_not_with_a_fixed_assumption() {
        // A white outline around white text is not a weaker treatment, it is
        // an eraser — which is exactly what a light backdrop got before this
        // followed the text color. The same holds for a black pool behind
        // black text.
        assert_eq!(treatment_rgb([255, 255, 255]), BLACK);
        assert_eq!(treatment_rgb([0, 0, 0]), WHITE);
    }

    #[test]
    fn the_treatment_is_chosen_per_run_so_an_odd_colored_module_is_safe() {
        // The volume module paints its muted state in the shared
        // disabled_color while every other run is the configured white, and
        // the two need not land on the same answer. They happen to today —
        // disabled_color is a light red, picked so the muted run keeps the
        // same dark pool as its neighbors — so the endpoints below stand in
        // for a palette that could part them again.
        assert_eq!(treatment_rgb([255, 255, 255]), BLACK);
        assert_eq!(treatment_rgb([0, 0, 0]), WHITE);
        // A mid accent color still resolves rather than landing in between.
        assert!(matches!(treatment_rgb([125, 222, 143]), BLACK | WHITE));
    }

    #[test]
    fn the_treatment_crossover_follows_the_wcag_curve_not_the_midpoint() {
        // Mid-gray (sRGB 128) is luminance ~0.22, which reads better against
        // black than white — so the crossover sits well below the halfway
        // byte, and picking it by midpoint would give a swathe of grays the
        // wrong treatment.
        assert_eq!(treatment_rgb([128, 128, 128]), BLACK);
        assert_eq!(treatment_rgb([80, 80, 80]), WHITE);
    }

    // ------------------------------------------------------------------
    // The expanded menu's drop: the module box grows downward, and the
    // silhouette must not grow with it.
    // ------------------------------------------------------------------

    /// The collapsed drop on a 27px bar, and a 10-row window picker.
    const COLLAPSED: (f32, f32) = (90.0, 21.0);
    const PICKER: (f32, f32) = (320.0, 339.0);

    #[test]
    fn an_expanded_drop_keeps_the_silhouette_it_had_collapsed() {
        // The whole point: same taper, same bottom corners, same bow, in
        // PIXELS, on a box sixteen times taller. Asserted through
        // resolve_silhouette rather than on the knobs, because that is the
        // function whose answer the shader actually draws.
        let spec = cce_ui::scene::paint::DropletSpec::default();
        let (sr0, ar0, bow0) = spec.resolve_silhouette(COLLAPSED.0, COLLAPSED.1);
        let grown = spec_at_reference_height(spec, COLLAPSED.1, PICKER.1);
        let (sr1, ar1, bow1) = grown.resolve_silhouette(PICKER.0, PICKER.1);
        for (a, b, what) in [(sr0, sr1, "sheet_r"), (ar0, ar1, "attach"), (bow0, bow1, "bow")] {
            assert!((a - b).abs() < 0.5, "{} drifted: {} vs {}", what, a, b);
        }
    }

    #[test]
    fn the_unscaled_spec_is_what_made_the_backdrop_miss_its_rows() {
        // The regression this guards. Left alone, the height fractions put
        // the picker's bottom radius on the half-width clamp — a literal
        // semicircle — with the rows laid out as a rectangle inside it.
        let spec = cce_ui::scene::paint::DropletSpec::default();
        let (sr, ar, _) = spec.resolve_silhouette(PICKER.0, PICKER.1);
        assert_eq!(sr, PICKER.0 / 2.0, "expected the half-width clamp");
        assert!(ar > PICKER.1 * 0.4, "expected the taper to eat the box");
    }

    #[test]
    fn a_box_no_taller_than_the_reference_is_left_exactly_alone() {
        // Every collapsed module box takes this path, and the expansion
        // animation starts here — so it has to be the identity, or the drop
        // pops on the first frame of opening.
        let spec = cce_ui::scene::paint::DropletSpec::default();
        assert_eq!(spec_at_reference_height(spec, COLLAPSED.1, COLLAPSED.1), spec);
        assert_eq!(spec_at_reference_height(spec, COLLAPSED.1, COLLAPSED.1 - 5.0), spec);
        assert_eq!(spec_at_reference_height(spec, 0.0, PICKER.1), spec);
    }

    #[test]
    fn only_the_knobs_measured_in_height_are_rescaled() {
        // The material and the exponents have no length in them: rescaling
        // `curve` would change the corner family, `shadow` the contact cue.
        let spec = cce_ui::scene::paint::DropletSpec::default();
        let grown = spec_at_reference_height(spec, COLLAPSED.1, PICKER.1);
        assert_eq!(grown.curve, spec.curve);
        assert_eq!(grown.core, spec.core);
        assert_eq!(grown.clarity, spec.clarity);
        assert_eq!(grown.shine, spec.shine);
        assert_eq!(grown.rim, spec.rim);
        assert_eq!(grown.shadow, spec.shadow);
        // belly_w is a fraction of the remaining half-WIDTH, not of height.
        assert_eq!(grown.belly_w, spec.belly_w);
        assert!(grown.sheet_r < spec.sheet_r);
    }

    #[test]
    fn the_expanded_panel_is_a_flat_sheet_with_the_water_terms_kept() {
        // The dome and its gleam fade OUT as the box grows: at full strength
        // the SDF-gradient dome draws a blocky lit frame (band pinned) or
        // envelope folds (band grown) on a long-sided panel — both tried,
        // both read as broken lighting. A real menu (k well under 0.5) lands
        // at exactly zero: flat glass, keeping clarity, rim, core, shadow.
        let spec = cce_ui::scene::paint::DropletSpec::default();
        let grown = spec_at_reference_height(spec, COLLAPSED.1, PICKER.1);
        assert_eq!(grown.dome, 0.0);
        assert_eq!(grown.gleam, 0.0);
        assert_eq!(grown.clarity, spec.clarity);
        assert_eq!(grown.rim, spec.rim);

        // The fade is CONTINUOUS in the growth factor — the expansion
        // animation passes through every k on its way down, so a step
        // anywhere would pop mid-flight. Just past the reference it is
        // near-identity; by twice the reference it has reached zero.
        let barely = spec_at_reference_height(spec, COLLAPSED.1, COLLAPSED.1 + 0.5);
        assert!((barely.dome - spec.dome).abs() < 0.1);
        let doubled = spec_at_reference_height(spec, COLLAPSED.1, COLLAPSED.1 * 2.0);
        assert_eq!(doubled.dome, 0.0);
    }

    #[test]
    fn the_scrim_is_constant_without_the_adaptive_knob() {
        // text_contrast off leaves `demand` at zero, and the scrim is then
        // exactly what was configured — a fixed dark ground, which is the
        // whole reason it can stand alone as a treatment.
        assert_eq!(scrim_alpha(0.55, 0.0), 0.55);
        assert_eq!(scrim_alpha(0.0, 0.0), 0.0);
    }

    #[test]
    fn the_scrim_deepens_with_demand_and_never_thins() {
        // Adaptive contrast can only ever darken the ground further; a
        // backdrop that needs help must not be able to lighten it.
        assert!(scrim_alpha(0.55, 0.5) > 0.55);
        assert_eq!(scrim_alpha(0.55, 1.0), 1.0);
        assert!(scrim_alpha(0.55, 1.0) >= scrim_alpha(0.55, 0.0));
    }

    #[test]
    fn the_feather_defaults_to_a_quarter_of_the_bubble_height() {
        assert_eq!(scrim_feather(200.0, 28.0, None), 7.0);
        assert_eq!(scrim_feather(200.0, 28.0, Some(9.0)), 9.0);
        assert_eq!(scrim_feather(200.0, 28.0, Some(-3.0)), 0.0);
    }

    #[test]
    fn the_feather_cannot_swallow_the_core_it_surrounds() {
        // Drawn OUTSIDE the core, so the core is inset by it; a feather past
        // half of either axis would invert the core and the pool would
        // disappear — exactly where a narrow module (a lone icon) lands.
        assert_eq!(scrim_feather(10.0, 28.0, Some(40.0)), 5.0);
        assert_eq!(scrim_feather(200.0, 28.0, Some(40.0)), 14.0);
    }

    fn run(x: f32, y: f32, w: f32, color: [u8; 3]) -> TextPrim {
        ("x".to_string(), 14.0, x, y, color, None, None, None, Some(w))
    }

    #[test]
    fn the_pool_takes_its_color_from_the_widest_run_it_covers() {
        let runs = vec![run(20.0, 7.0, 30.0, [255, 255, 255]), run(60.0, 7.0, 90.0, [0, 0, 0])];
        assert_eq!(dominant_run_color(&runs, 10.0, 0.0, 200.0, 27.0), Some([0, 0, 0]));
    }

    #[test]
    fn a_run_in_another_box_does_not_color_this_pool() {
        // An expanded segment has text in the strip AND in the menu below it;
        // the strip's pool must not be colored by a menu row.
        let runs = vec![run(20.0, 7.0, 30.0, [255, 255, 255]), run(20.0, 60.0, 90.0, [0, 0, 0])];
        assert_eq!(dominant_run_color(&runs, 10.0, 0.0, 200.0, 27.0), Some([255, 255, 255]));
    }

    #[test]
    fn a_box_with_no_measured_text_gets_no_pool() {
        // The tray is icons; there is no text to ground, and a pool there
        // would just be a smudge behind the icons.
        let runs: Vec<TextPrim> = vec![("i".to_string(), 14.0, 20.0, 7.0, [255, 255, 255], None, None, None, None)];
        assert_eq!(dominant_run_color(&runs, 10.0, 0.0, 200.0, 27.0), None);
        assert_eq!(dominant_run_color(&[], 10.0, 0.0, 200.0, 27.0), None);
    }

    #[test]
    fn a_box_of_tray_icons_gets_the_pool_a_white_run_would() {
        // The tray draws icons, not measured runs, so the text-keyed lookup
        // finds nothing; the icon fallback grounds the box as light glyphs.
        let icon = |x: f32| TrayIconBounds {
            id: "i".into(), x, y: 5.5, w: 16.0, h: 16.0, title: None, dbus_id: None,
        };
        let icons = [icon(18.0), icon(42.0)];
        assert_eq!(dominant_icon_color(&icons, 10.0, 0.0, 80.0, 27.0), Some([255, 255, 255]));
        assert_eq!(treatment_rgb([255, 255, 255]), [0.0, 0.0, 0.0]);
        // An icon whose center lies outside the box does not ground it —
        // the expanded menu box below the strip holds no icons.
        assert_eq!(dominant_icon_color(&icons, 10.0, 27.0, 80.0, 100.0), None);
        assert_eq!(dominant_icon_color(&[], 10.0, 0.0, 80.0, 27.0), None);
    }

    #[test]
    fn contrast_ratio_matches_the_wcag_endpoints() {
        assert!((contrast_ratio(0.0, 1.0) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(0.5, 0.5) - 1.0).abs() < 0.001);
    }

    #[test]
    fn parse_backdrop_reads_a_well_formed_line() {
        assert_eq!(crate::listeners::parse_backdrop("42 17"), (42, 17));
        assert_eq!(crate::listeners::parse_backdrop("  0 0  "), (0, 0));
    }

    #[test]
    fn parse_backdrop_falls_back_to_the_worst_case_not_the_best() {
        // Every unreadable form must fail toward "assume unreadable": a
        // fallback of (bright, uniform) would silently switch the treatment
        // off, and bare text over an unknown backdrop is the failure this
        // whole path exists to prevent.
        for line in ["unknown", "", "42", "nonsense here", "42 spread"] {
            assert_eq!(crate::listeners::parse_backdrop(line), (50, 100), "line {:?}", line);
        }
        assert_eq!(contrast_demand(BLACK_TEXT, crate::listeners::parse_backdrop("unknown")), 1.0);
    }

    #[test]
    fn parse_backdrop_rejects_out_of_range_but_tolerates_extra_fields() {
        // Out of protocol is unknown, not clamped — clamping a bad luma to
        // 100 would read as "bright and uniform" and switch the scrim off.
        assert_eq!(crate::listeners::parse_backdrop("200 200"), (50, 100));
        assert_eq!(crate::listeners::parse_backdrop("101 0"), (50, 100));
        // Room for the compositor to grow the line without the bar
        // misreading it as garbage.
        assert_eq!(crate::listeners::parse_backdrop("30 40 future"), (30, 40));
    }

    // ------------------------------------------------------------------
    // Characterization tests (phase 0): these pin down current behavior
    // before the refactors in PROPOSAL.md. Where the behavior is odd, the
    // test documents it rather than fixing it. The config-lookup and color
    // tests moved to config.rs with the phase-2 rewrite.
    // ------------------------------------------------------------------



    // --- module_side_from_json ---

    fn side_cfg(name: &str, value: &str) -> serde_json::Value {
        serde_json::json!({"layout": {"status_bar": {name: value}}})
    }

    #[test]
    fn module_side_explicit_values() {
        assert_eq!(module_side_from_json(&side_cfg("clock", "left"), "clock"), Side::Left);
        assert_eq!(module_side_from_json(&side_cfg("window", "right"), "window"), Side::Right);
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
            assert_eq!(module_side_from_json(&side_cfg("cpu", snap), "cpu"), side, "snap {}", snap);
        }
    }

    #[test]
    fn module_side_only_canonical_location_resolves() {
        // Only `layout { status_bar <name>=... }` counts; a same-named key
        // anywhere else is ignored (the fuzzy search is gone).
        let val = serde_json::json!({"stray": {"clock": "left"}});
        assert_eq!(module_side_from_json(&val, "clock"), Side::Right);
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
        assert_eq!(module_side_from_json(&side_cfg("window", "sideways"), "window"), Side::Left);
        assert_eq!(module_side_from_json(&side_cfg("clock", "sideways"), "clock"), Side::Right);
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
        // Values > 2π are degrees (int or float), otherwise radians.
        assert_eq!(module_side_from_json(&mk(serde_json::json!(180)), "light_source"), Side::Left);
        assert_eq!(module_side_from_json(&mk(serde_json::json!(135.0)), "light_source"), Side::Left);
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
