//! The volume/brightness slider: a transient on-screen display that appears
//! when either level moves and fades a moment after the last change.
//!
//! It is a layer-shell surface on the OVERLAY layer, which the compositor
//! stacks above `layers.fullscreen` (cce-compositor `scene.rs`) — so it shows
//! over a fullscreen game or video, where the bar's own readouts are hidden.
//! It takes no keyboard focus (a fullscreen window must never yield to it,
//! see the compositor's `fullscreen_yields`) and has an empty input region,
//! so a click on it lands on whatever is underneath.
//!
//! Two halves, both in this binary:
//!
//! - **The trigger** ([`spawn_trigger`]) runs in the launcher daemon. It
//!   drives the same fast-path watchers the bar's readouts use
//!   ([`crate::spawn_level_watchers`] — the backlight polled every 100ms, the
//!   sink followed through `pactl subscribe`), so the slider moves for a
//!   keypress, `brightnessctl` in a terminal or a mixer app alike. Each change
//!   is forwarded as one line to the running slider, or starts one.
//! - **The slider** (`--osd <line>`, [`main`]) is a single-instance `cce-ui`
//!   app on [`SOCKET_PREFIX`]'s socket. It EXITS when its timeout runs out
//!   rather than hiding: a surface that stays mapped, even fully
//!   transparent, keeps a fullscreen window off direct scanout for good.
//!   The compositor's close fade dissolves it on the way out.
//!
//! Config, in the app's own `config.kdl` beside `module { }`:
//! `osd { timeout_ms 1500 width 260 height <1.5 × bar> position "bottom" margin 96 }`
//! and `osd { enabled false }` to turn it off. The box, colors, font, glyphs
//! and droplet style are the bar's `module { }` keys, so the slider reads as
//! one of its readouts grown up.

use cce_ui::cosmic_text::FontSystem;
use cce_ui::engine::{
    EngineState, LayerAnchor, LayerKeyboardInteractivity, LayerKind, LayerSettings, LogicalPosition, LogicalSize,
    WindowSettings,
};
use cce_ui::scene::layout::Rect;
use cce_ui::widget::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use wayland_client::QueueHandle;

use crate::{LevelChange, StatusBoxBevel};

/// The slider's instance socket: `/tmp/cce-status-osd-<WAYLAND_DISPLAY>.sock`.
pub(crate) const SOCKET_PREFIX: &str = "cce-status-osd";

/// What the slider shows. The wire form is one line — see [`Level::line`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Level {
    Brightness(u32),
    /// `pct` is `None` when pactl answered without a percentage.
    Volume { pct: Option<u32>, muted: bool },
}

impl Level {
    /// The level a watcher change shows, or `None` when the source went away
    /// (no backlight, no pactl) — there is nothing to slide then.
    pub(crate) fn from_change(change: LevelChange) -> Option<Self> {
        match change {
            LevelChange::Brightness(b) => Some(Level::Brightness(b?.max(0) as u32)),
            LevelChange::Volume(v) => {
                let (pct, muted) = v?;
                Some(Level::Volume { pct, muted })
            }
        }
    }

    /// `brightness 40` / `volume 55 0` / `volume - 1` (muted, no level).
    pub(crate) fn line(self) -> String {
        match self {
            Level::Brightness(p) => format!("brightness {p}"),
            Level::Volume { pct, muted } => format!(
                "volume {} {}",
                pct.map_or("-".to_string(), |p| p.to_string()),
                u8::from(muted)
            ),
        }
    }

    pub(crate) fn parse(line: &str) -> Option<Self> {
        let mut words = line.split_whitespace();
        let level = match words.next()? {
            "brightness" => Level::Brightness(words.next()?.parse().ok()?),
            "volume" => {
                let pct = match words.next()? {
                    "-" => None,
                    p => Some(p.parse().ok()?),
                };
                let muted = match words.next()? {
                    "0" => false,
                    "1" => true,
                    _ => return None,
                };
                Level::Volume { pct, muted }
            }
            _ => return None,
        };
        words.next().is_none().then_some(level)
    }

    fn pct(self) -> Option<u32> {
        match self {
            Level::Brightness(p) => Some(p),
            Level::Volume { pct, .. } => pct,
        }
    }

    fn muted(self) -> bool {
        matches!(self, Level::Volume { muted: true, .. })
    }

    fn icon(self) -> &'static str {
        match self {
            Level::Brightness(_) => "brightness",
            Level::Volume { muted: true, .. } => "volume-muted",
            Level::Volume { .. } => "volume",
        }
    }

    /// The share of the track filled, 0..=1 (a sink boosted past 100% fills it).
    fn fraction(self) -> f32 {
        self.pct().map_or(0.0, |p| (p as f32 / 100.0).clamp(0.0, 1.0))
    }
}

// ---------------------------------------------------------------- config

fn cfg_f32(pointer: &str) -> Option<f32> {
    crate::get_cached_config().pointer(pointer).and_then(|v| v.as_f64()).map(|n| n as f32)
}

/// `osd { enabled false }` turns the slider off; read per change, so it
/// takes effect without restarting the bar.
fn enabled() -> bool {
    crate::get_cached_config().pointer("/osd/enabled").and_then(|v| v.as_bool()).unwrap_or(true)
}

/// `osd { timeout_ms }` — how long the slider stays after the LAST change.
fn timeout() -> std::time::Duration {
    std::time::Duration::from_millis(cfg_f32("/osd/timeout_ms").unwrap_or(1500.0).clamp(200.0, 10_000.0) as u64)
}

/// `osd { width height }`, logical px. The height defaults to one and a half
/// bar heights, and the glyph and number scale with it.
fn size() -> (u32, u32) {
    let bar_h = crate::read_status_height_from_config();
    let h = cfg_f32("/osd/height").unwrap_or((bar_h * 1.5).round()).max(16.0);
    let w = cfg_f32("/osd/width").unwrap_or(260.0).max(h * 3.0);
    (w.round() as u32, h.round() as u32)
}

/// `osd { position "bottom"|"top"|"center" margin 96 }` — horizontally
/// centered either way; `margin` is the gap from the chosen screen edge.
fn placement() -> (LayerAnchor, (i32, i32, i32, i32)) {
    let margin = cfg_f32("/osd/margin").unwrap_or(96.0).max(0.0) as i32;
    let position = crate::get_cached_config()
        .pointer("/osd/position")
        .and_then(|v| v.as_str())
        .unwrap_or("bottom")
        .to_string();
    match position.as_str() {
        "top" => (LayerAnchor::TOP, (margin, 0, 0, 0)),
        "center" => (LayerAnchor::empty(), (0, 0, 0, 0)),
        _ => (LayerAnchor::BOTTOM, (0, 0, margin, 0)),
    }
}

// ---------------------------------------------------------------- trigger

/// Watch both levels and show the slider for every change. Runs for the life
/// of the launcher daemon; `exe` is this binary, re-run as `--osd`.
pub(crate) async fn spawn_trigger(exe: std::path::PathBuf) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    crate::spawn_level_watchers(move |change| {
        let _ = tx.send(change);
    })
    .await;
    tokio::spawn(async move {
        while let Some(mut change) = rx.recv().await {
            // Forwarding blocks until the slider answers, which a slider
            // still starting up does only once its loop is running; a held
            // key queues changes meanwhile, and only the newest matters.
            while let Ok(newer) = rx.try_recv() {
                change = newer;
            }
            let Some(level) = Level::from_change(change) else { continue };
            if !enabled() {
                continue;
            }
            show(&exe, level).await;
        }
    });
}

/// Hand `level` to the running slider, or start one showing it.
async fn show(exe: &std::path::Path, level: Level) {
    let line = level.line();
    let forwarded = {
        let line = line.clone();
        tokio::task::spawn_blocking(move || cce_ui::ipc::instance::forward(SOCKET_PREFIX, &line).is_some())
            .await
            .unwrap_or(false)
    };
    if forwarded {
        return;
    }
    // Two of these racing (a second change before the first slider has bound
    // its socket) is settled by `forward_or_claim` in the children.
    match tokio::process::Command::new(exe).arg("--osd").args(line.split_whitespace()).spawn() {
        Ok(mut child) => {
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        Err(e) => log::error!("[osd] could not start the slider: {e}"),
    }
}

// ---------------------------------------------------------------- slider

/// The level the slider was started with, parked by [`main`] for
/// `OsdApp::new` (`engine::run` takes no arguments).
static INITIAL: std::sync::OnceLock<Level> = std::sync::OnceLock::new();

/// `cce-status-interface --osd <line>`: forward to a running slider, or
/// become it.
pub(crate) fn main(args: &[String]) {
    let line = args.join(" ");
    let Some(level) = Level::parse(&line) else {
        log::error!("[osd] bad level {line:?} — expected `brightness <pct>` or `volume <pct|-> <0|1>`");
        return;
    };
    if cce_ui::ipc::instance::forward_or_claim(SOCKET_PREFIX, &line) {
        return;
    }
    let _ = INITIAL.set(level);
    cce_ui::engine::run::<OsdApp>();
    cce_ui::ipc::instance::cleanup();
}

#[derive(Debug, Clone)]
pub(crate) enum OsdEvent {
    Show(Level),
    /// The timeout armed by the `Show` with this generation ran out.
    Hide(u64),
}

pub(crate) struct OsdApp {
    level: Level,
    /// The track fill drawn, eased toward `level.fraction()`.
    fill_now: f32,
    /// Bumped by every `Show`, so only the newest timeout hides the slider.
    generation: u64,
    sender: calloop::channel::Sender<OsdEvent>,
    font_system: FontSystem,
    /// See `StatusApp::seen_renderer`: a later renderer is a reconnect, and
    /// the cached glyph ids died with the old one.
    seen_renderer: bool,
    width: u32,
    height: u32,
}

impl OsdApp {
    /// (Re)start the countdown to hiding.
    fn arm_hide(&mut self) {
        self.generation += 1;
        let generation = self.generation;
        let sender = self.sender.clone();
        let wait = timeout();
        std::thread::spawn(move || {
            std::thread::sleep(wait);
            let _ = sender.send(OsdEvent::Hide(generation));
        });
    }
}

/// A raw-sRGB text color as a linear quad color with alpha `a`.
fn quad_of(text: [f32; 4], a: f32) -> [f32; 4] {
    let l = cce_ui::color::srgb_to_linear;
    [l(text[0]), l(text[1]), l(text[2]), a]
}

impl cce_ui::engine::Application for OsdApp {
    type Message = OsdEvent;

    fn new(_qh: &QueueHandle<EngineState<Self>>, sender: calloop::channel::Sender<Self::Message>) -> Self {
        let level = INITIAL.get().copied().unwrap_or(Level::Brightness(0));
        let forward = sender.clone();
        cce_ui::ipc::instance::serve(move |line| {
            let level = Level::parse(line)?;
            forward.send(OsdEvent::Show(level)).ok()?;
            Some("ok".into())
        });
        let (width, height) = size();
        let mut app = Self {
            level,
            // Opens already at the level: the slider is news only once it is
            // up, so the first value does not sweep in from empty.
            fill_now: level.fraction(),
            generation: 0,
            sender,
            font_system: cce_ui::create_font_system(),
            seen_renderer: false,
            width,
            height,
        };
        app.arm_hide();
        app
    }

    fn settings(&self) -> WindowSettings {
        WindowSettings {
            title: "Level".to_string(),
            app_id: SOCKET_PREFIX.to_string(),
            width: self.width,
            height: self.height,
            fullscreen: false,
            min_size: None,
        }
    }

    fn layer(&self) -> Option<LayerSettings> {
        let (anchor, margin) = placement();
        Some(LayerSettings {
            layer: LayerKind::Overlay,
            anchor,
            exclusive_zone: 0,
            keyboard_interactivity: LayerKeyboardInteractivity::None,
            margin,
            namespace: SOCKET_PREFIX.to_string(),
        })
    }

    fn update(&mut self, msg: Self::Message, needs_rebuild: &mut bool, exit: &mut bool) {
        match msg {
            OsdEvent::Show(level) => {
                self.level = level;
                self.arm_hide();
                *needs_rebuild = true;
            }
            OsdEvent::Hide(generation) if generation == self.generation => {
                // Give up the socket BEFORE the close fade: a change during
                // the fade must start a fresh slider, not be answered by this
                // one and then dropped with it.
                cce_ui::ipc::instance::cleanup();
                *exit = true;
            }
            OsdEvent::Hide(_) => {}
        }
    }

    fn tick(&mut self, dt: f32, needs_rebuild: &mut bool) {
        const FILL_EASE_S: f32 = 0.08;
        let target = self.level.fraction();
        if self.fill_now == target {
            return;
        }
        if !cce_ui::motion::enabled() || (target - self.fill_now).abs() < 0.002 {
            self.fill_now = target;
        } else {
            self.fill_now += (target - self.fill_now) * (dt / FILL_EASE_S).clamp(0.0, 1.0);
        }
        *needs_rebuild = true;
    }

    // style-audit: opt-out a transparent surface; the slider's box is the bar's module plate
    fn display_list(&mut self, size: LogicalSize, scale: f64) -> Option<cce_ui::scene::paint::DisplayList> {
        cce_ui::scale::set_scale_factor(scale as f32);
        let (w, h) = (size.width, size.height);
        let bar_h = crate::read_status_height_from_config();
        let k = h / bar_h.max(1.0);
        let padding = crate::read_status_padding_from_config() * k;
        let (font_family, _) = cce_ui::layout::parse_font_string(&crate::read_status_font_from_config());
        let font_size = crate::read_icon_font_size_from_config(crate::read_status_font_size_from_config()) * k;
        let weight = crate::read_icon_weight_from_config();
        let gap = crate::read_icon_gap_from_config().max(4.0) * k;

        let normal = crate::read_normal_color_from_config().unwrap_or(cce_ui::color::TEXT_FG);
        let color = if self.level.muted() {
            crate::read_disabled_color_from_config().unwrap_or(cce_ui::color::TEXT_DIM)
        } else {
            normal
        };

        let mut pc = cce_ui::scene::paint::PaintCtx::new();

        // The box: the bar's module plate, in whichever style the bar wears.
        if let Some(bg) = crate::read_status_box_background_color_from_config() {
            let radius = crate::read_status_box_corner_radius_from_config().min(h / 2.0);
            let radii = (radius, radius, radius, radius);
            let rect = Rect { x: 0.0, y: 0.0, width: w, height: h };
            let material = cce_ui::scene::Material::from_fill(bg);
            if let Some(spec) = crate::read_droplet_from_config() {
                // Same inset the bar's drops take: the AA feather plus the
                // contact shadow's gap.
                let inset = 1.0 + spec.shadow_gap(h);
                pc.droplet(Rect { height: h - inset, ..rect }, &material.with_finish(spec.finish()), spec);
            } else {
                match crate::read_status_box_bevel_from_config() {
                    Some(StatusBoxBevel::Raised) => {
                        pc.bevel(rect, radii, &material, crate::read_status_box_bevel_depth_from_config());
                    }
                    Some(StatusBoxBevel::Inset) => {
                        pc.rounded_rect(rect, radius, (true, true, true, true), bg);
                        pc.recess(rect, radii, crate::read_status_box_bevel_depth_from_config());
                    }
                    None => pc.rounded_rect(rect, radius, (true, true, true, true), bg),
                }
            }
        }

        // The glyph at the left, as in the bar's readouts but scaled with
        // the box. Without the icon set it is simply left out: the slider
        // still says which level it is by moving when that key is pressed.
        let raise = crate::read_text_raise_from_config() * k;
        let mut x = padding;
        let glyph_px = (crate::read_icon_size_from_config() * k * scale as f32).round().max(1.0) as u32;
        if let Some((image, gw, gh)) = crate::icons::tinted_icon(self.level.icon(), glyph_px, crate::icons::tint_of(color)) {
            let (gw, gh) = (gw as f32 / scale as f32, gh as f32 / scale as f32);
            pc.image(
                image,
                Rect { x, y: (h - gh) / 2.0 - raise, width: gw, height: gh },
                crate::read_icon_alpha_from_config(),
            );
            x += gw + gap;
        }

        // The number at the right, right-aligned in a "100"-wide column so
        // the track keeps its length as the value changes digit count.
        let measure = |fs: &mut FontSystem, text: &str| {
            let buf = crate::make_text_buffer_weighted(fs, text, font_size, &font_family, weight);
            buf.layout_runs().next().map(|r| r.line_w).unwrap_or(0.0) / scale as f32
        };
        let column = measure(&mut self.font_system, "100");
        let right = w - padding;
        if let Some(pct) = self.level.pct() {
            let text = pct.to_string();
            let tw = measure(&mut self.font_system, &text);
            pc.text_attrs(
                text,
                right - tw,
                (h - font_size) / 2.0 - raise,
                font_size,
                crate::icons::tint_of(color),
                Some(font_family.clone()),
                None,
                cce_ui::scene::paint::TextAttrs { italic: false, weight },
            );
        }

        // The track between them, and its fill.
        let track_w = (right - column - gap - x).max(0.0);
        let thickness = (h * 0.16).clamp(3.0, 10.0);
        let track = Rect { x, y: (h - thickness) / 2.0 - raise, width: track_w, height: thickness };
        let ends = (true, true, true, true);
        pc.rounded_rect(track, thickness / 2.0, ends, quad_of(normal, 0.22));
        if self.fill_now > 0.0 {
            // Never shorter than its own rounded ends, so 1% is a dot, not a sliver.
            let fill_w = (track_w * self.fill_now).max(thickness).min(track_w);
            pc.rounded_rect(Rect { width: fill_w, ..track }, thickness / 2.0, ends, quad_of(color, 1.0));
        }

        Some(pc.finish())
    }

    fn renderer_init(&mut self, _renderer: &mut cce_ui::vk::VkRenderer) {
        if std::mem::replace(&mut self.seen_renderer, true) {
            crate::icons::drop_textures();
        }
    }

    fn display_list_text(&self) -> bool {
        true
    }

    fn clear_color(&self) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    /// Empty: the slider is never in the way of a click.
    fn input_regions(&self) -> Option<Vec<(i32, i32, i32, i32)>> {
        Some(Vec::new())
    }

    fn handle_pointer_move(&mut self, _pos: LogicalPosition, _needs_rebuild: &mut bool) {}
    fn handle_mouse_input(
        &mut self,
        _button: MouseButton,
        _state: ElementState,
        _pos: LogicalPosition,
        _needs_rebuild: &mut bool,
    ) -> Option<Self::Message> {
        None
    }
    fn handle_mouse_wheel(&mut self, _delta: &MouseScrollDelta, _pos: LogicalPosition, _needs_rebuild: &mut bool) {}
    fn handle_key_input(&mut self, _event: &KeyEvent, _needs_rebuild: &mut bool) -> Option<Self::Message> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_survive_the_wire() {
        for level in [
            Level::Brightness(0),
            Level::Brightness(100),
            Level::Volume { pct: Some(55), muted: false },
            Level::Volume { pct: Some(130), muted: true },
            Level::Volume { pct: None, muted: true },
        ] {
            assert_eq!(Level::parse(&level.line()), Some(level), "{}", level.line());
        }
    }

    #[test]
    fn malformed_lines_are_refused() {
        for line in ["", "volume", "volume 50", "volume 50 2", "volume x 0", "brightness", "brightness -3", "brightness 5 6", "mic 50"] {
            assert_eq!(Level::parse(line), None, "{line:?}");
        }
    }

    #[test]
    fn a_vanished_source_shows_nothing() {
        assert_eq!(Level::from_change(LevelChange::Brightness(None)), None);
        assert_eq!(Level::from_change(LevelChange::Volume(None)), None);
        assert_eq!(Level::from_change(LevelChange::Brightness(Some(40))), Some(Level::Brightness(40)));
    }

    #[test]
    fn a_boosted_sink_fills_the_track_and_no_more() {
        assert_eq!(Level::Volume { pct: Some(150), muted: false }.fraction(), 1.0);
        assert_eq!(Level::Volume { pct: None, muted: false }.fraction(), 0.0);
        assert_eq!(Level::Brightness(25).fraction(), 0.25);
    }
}
