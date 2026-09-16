# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cce-status-interface` is the status bar of the `cce` Wayland desktop environment. It
is one crate in the multi-repo `cce` workspace — see `../cce-compositor/WORKSPACE.md` for
the workspace layout, the multi-repo git rules (commit here, never `git init` at the root),
and the `cce-ui` toolkit this app is built on. This crate is deliberately small:
`src/main.rs` (the `StatusApp` application, layout/input, launcher daemon),
`src/modules.rs` (the `StatusModule` trait and its ten implementations),
`src/config.rs` (pointer-first config readers), `src/tray.rs` (SNI host),
`src/cloud.rs` (menu page building), `src/stats.rs` (system stat readers —
numbers, not strings; the modules do the formatting), `src/icons.rs` (tinted
cce-icons glyph textures), `src/listeners.rs` (status/switcher socket tasks).

## Build, test, run

```sh
cargo build --release                 # standalone build (or `-p cce-status-interface` from the workspace root)
cargo test                            # 46 tests: main.rs (contrast, parsers), config.rs, tray.rs
make install                          # installs ../target/release/cce-status-interface to ~/.local/bin
```

Running it requires a live cce compositor session (`$WAYLAND_DISPLAY` plus the cce
sockets); there is no meaningful headless mode.

## Process model (the most important thing to know)

One binary, three modes, selected by CLI args in `main()`:

- **No args — launcher daemon.** Spawns one child process per module
  (`--module window`, `--module clock`, …), polls every 500ms and restarts crashed
  children with exponential backoff (500ms doubling to 30s; 30s of healthy uptime
  resets it). This is the normal production mode: each module is its own process and
  its own Wayland surface.
- **`--module <name>`** — a single-module bar segment. Valid names: `window`, `tray`,
  `stats`, `cpu`, `memory`, `brightness`, `volume`, `battery`, `clock`, `light_source`
  (the daemon launches `stats`, not the five it combines).
- **`--trigger-switcher`** — one-shot: writes `trigger` to the switcher socket of the
  running instance and exits (used as a keybinding target).

(The old `--monolithic` all-modules-in-one-window mode is gone, along with the
app-side super+drag module reordering that only made sense there.)

The compositor places each segment by its Wayland `app_id`, computed in
`StatusApp::get_app_id()`: `cce-status-{side}-{name}` (e.g. `cce-status-left-window`). If
`/tmp/cce-status-interface-{WAYLAND_DISPLAY}.sock` exists, the `cce-status-interface-`
prefix is used instead — keep both spellings in mind when matching app_ids. A module's
side comes from the config (`get_module_side`, which also maps snap positions like
`top-left`/`bottom-right` to left/right); default is `window` → left, everything else →
right. **`light_source` is the exception**: it short-circuits ahead of all of
that and takes its side from `/window_manager/light_source_position` — the
angle points at a side — so a `layout { status_bar light_source=… }` entry is
read and then ignored, which looks like the key not working.

## Rendering

The app implements `cce_ui::engine::Application` on the **`display_list()` paint path**
(Phase 6ak) — the legacy `view*()`/`text_items()` methods are gone. The flow:

1. `rebuild_layout()` runs the two-pass module layout — for each module first
   `StatusModule::width()`, then `StatusModule::render()` — filling retained buffers on
   `StatusApp`: `rects`, `rounded_boxes`, `text_prims`
   (the `TextPrim` tuple type; build them with `draw_label()` from a
   `cce_ui::widget::StyledLabel`), `icon_prims` (`IconPrim` — a tinted
   cce-icons glyph texture at a logical rect), plus `input_regions`,
   `module_bounds`, `tray_item_bounds`.
2. `display_list()` replays those buffers into a `PaintCtx` each frame (and triggers
   `rebuild_layout()` when size/scale changed or `needs_rebuild` is set).
   `overlay_quads()` remains a separate on-top pass (used for drag feedback).

**Module boxes hug their content.** `StatusModule::width()` is the STABLE slot
width — widest-plausible templates for the stat modules, 24px title buckets for
the window module — and it alone sizes the surface, which is what keeps the
compositor's configure-echo jitter out of the loop; `content_width()` (default:
`width()`) measures the live text, and the drawn bubble takes that width,
centered in the slot, so the padding on each side of the text is
`module { padding }` rather than padding-plus-template-surplus. The drawn width
is eased over ~120ms in `tick` (`bubble_w_now`/`bubble_w_target` — one pair of
fields, sound because a `StatusApp` hosts exactly one module), and the
in-surface menu expansion grows out of `collapsed_box` — the bubble actually
drawn — not out of the slot, so the box-grows-into-the-menu continuity holds.

Orientation is dynamic: `is_vertical()` compares the surface size against the
configured bar thickness; every module renders along one axis using `bar_h`/`coord`
accordingly.

**The stat modules read out as a glyph with the number beside it, not a
label.** `cpu`, `memory`, `brightness`, `volume` and `battery` are
`IconStat` implementations: each names a cce-icons glyph, the bare number
and a color, and `IconReadout` draws the glyph (tinted that color, at
`module { icon_alpha }`) with the number `module { icon_gap }` to its right
— no unit symbol, since the glyph IS the unit ("87" beside the battery, not
"Bat 87%"). **The launcher runs them as ONE segment**, `stats`
(`StatsModule`): every readout in a single bubble, `module { icon_spacing }`
apart, in the order cpu, memory, brightness, volume, battery — the order the
compositor's `RIGHT_ORDER` gave the five separate segments, and `stats` has
its own slot there between `tray` and `clock` (cce-window-manager
2026-09-16). The five single names stay valid `--module` values for a bar
that wants them apart; a blanket `impl<T: IconStat> StatusModule for T`
lays a lone readout out through the same `readouts_width` /
`render_readouts` the combined segment uses. (Superimposing the number on a
ghosted glyph, with a bold weight and a dark pocket under the digits, was
tried first on 2026-09-16 and replaced the same day by the side-by-side
form; `icon_weight` survives as an opt-in, the pocket is gone.) The muted
sink swaps to `volume-muted`, the charging battery to `battery-charging`;
the battery also keeps its accent color while charging or under 10%. A
reader with nothing (no battery, no backlight, no pactl) returns `None`
and drops out of the row — a lone module with nothing has width 0, i.e. it
is hidden rather than an empty bubble; a reader that answers without a
number (cpu with no /proc/stat, a sink with no level) draws the glyph
alone.

The glyphs come from the **cce-icons** crate via `cce_ui::icons_dir()`
(`$CCE_ICONS_DIR`, else `~/projects/cce/cce-icons/svg`) — but NOT through
`cce_ui::upload_icon`: a `Prim::Image` has alpha and no color, and the
artwork is white, so `icons.rs::tinted_icon` rasterizes the SVG itself
(`cce_ui::rasterize_svg`), multiplies it by the readout's raw-sRGB color and
uploads it, cached per `(name, px, color)` for the life of the process. A
glyph that fails to load falls back to the old text readout ("Cpu 45%"), so
a bar started without the icon set is still attributable; **a shadow session
needs `CCE_ICONS_DIR` exported into the spawn**, its HOME being elsewhere,
exactly as it needs `CCE_FONTS_DIR`. Slot stability holds as before: the
stable width sizes every number at the "100" template, so a value crossing
a digit boundary never resizes the surface, and the bubble eases to the
live row. `memory` reads as a percentage of the total in use (used = total
less free, buffers and page cache) since 2026-09-16 — the "Mem 10/62G"
gigabyte form went with the label.

## Events and IPC

`update()` consumes `CustomEvent`s sent over a calloop channel from tokio tasks spawned
in `new()` — which tasks run depends on the selected module, so a clock process doesn't
listen to tray D-Bus, etc.:

- **Compositor status feed** (`spawn_status_listener`): connects to
  `/tmp/cce-status[-interface]-{WAYLAND_DISPLAY}.sock` and subscribes, one task
  per topic, line-oriented — `layout` and `title` only in the process that owns
  the window module, `dismiss` and `backdrop` in every one. Reconnects back off
  1s doubling to 30s, reset the moment a connection delivers a line: a
  compositor that does not know a topic drops the subscription on sight, so a
  flat retry made a bar running ahead of its compositor reconnect once a second
  from every module process, forever. The compositor also offers `modifiers`
  (`status_server.rs`), but nothing here subscribes to it and the match over
  topics ends in `unreachable!()` — adding a subscription means adding its arm
  first. (The old `viewport` topic is gone with the viewport-tag feature.)
- **System stats** (`spawn_system_stats`): `/proc/stat`, `/proc/meminfo`,
  `/sys/class/power_supply/BAT*`, `/sys/class/backlight`, and `pactl` for volume/mute.
  `SystemStats` carries numbers (`cpu_pct`, `memory`, `battery: (capacity,
  charging)`, `volume: (level, muted)`, `brightness`), each `Option` where
  the source can be absent; only the clock arrives pre-formatted.
- **Tray** (`spawn_status_tray`): a full StatusNotifierItem/Watcher host over `zbus`,
  including DBusMenu fetching. Icons arrive as pixmaps or theme names (rendered via
  `resvg`/`png`).
- **Backdrop** (`spawn_status_listener("backdrop <app_id>")`): what THIS segment
  is composited over, measured compositor-side and pushed as `<luma> <spread>`
  (0-100 each) or `unknown`. Every module process subscribes, naming itself with
  `status_app_id()`. A Wayland client cannot see behind its own surface, so this
  is the only source of the fact — see `module { text_contrast }` below.
- **Switcher** (`spawn_switcher_listener`): binds
  `/tmp/cce-status-interface-switcher-{WAYLAND_DISPLAY}.sock`; a line on it fires
  `SwitcherTriggered`.

Outbound actions shell out to `ccectl` (`windows --json`, `focus-window`,
`window-switcher`, `status-hide-mode`, `adjust-position-mode`), resolved from `~/.local/bin` first (`get_ccectl_cmd`).
`ccectl windows --json` returns one JSON object per line; the text format is kept only
as a parse fallback for older compositors (`parse_ccectl_window_any_line` handles
both). Keyboard alt-tab switching is delegated to the compositor
(`ccectl window-switcher`) — don't reimplement it here.

**Right-click menus are IN-SURFACE** (`ModuleContextMenu`): the module's own
surface expands below the bar strip to contain the menu — the module box
literally grows into the menu (one continuous rounded box; the expansion and
contraction are ANIMATED over ~140ms, `menu_anim`/`menu_closing` stepped in
`tick`, eased in `rebuild_layout`, surface resized per-frame via
`desired_size`; the menu object drops only when the contraction lands) —
module context menus and tray icon DBusMenus alike (fetched/flattened by `cloud.rs::
fetch_tray_menu_pages` into `MenuPage`/`MenuRow` pages riding a
`CustomEvent::TrayMenuFetched`; submenus paginate in place; row clicks send the
DBusMenu "clicked" via `send_tray_menu_event`). The compositor treats a status
segment thicker than the bar as expanded: frozen slot, no size enforcement,
raised above overlapped windows; the bar must reset its own height on close.
In droplet style the expanded panel is a FLAT glass sheet: `spec_at_reference_height`
fades `dome` and `gleam` to zero (continuously in the growth factor, gone by
twice the bar height) because the SDF-gradient dome creases on a long-sided
box — full strength drew a blocky lit picture-frame with the band pinned, and
envelope folds across the body with the band grown; both were tried. The panel
keeps the silhouette-hugging water terms (clarity, rim crest, core, contact
shadow), and the hovered row's highlight is a rounded pill inset from the
panel edge (`menu_hover_rect`, drawn post-scrim), not a full-width rect.

**No cce-cloud popups remain in this app**: the window picker (window-module
click → `MenuReady` rows of `Ccectl(["focus-window", id])`) is an in-surface
menu too. Menu width sizes to
the longest row label. Expanded segments stack in the compositor's popups
layer (cce-fx@74a0f75) so click-away-close works across the whole surface,
including the strip band over neighboring segments. Plain Escape closes open
menus too — compositor-side like click-away (cce-fx@7db8c03), arriving here as
the same `dismiss` push; this app never sees the key itself, since status
segments hold no keyboard focus.

## Config

Config comes from the shared `~/.config/cce/config.kdl` with the app's own
`~/.config/cce/cce-status-interface/config.kdl` merged over it (cce-ui does the
merge by executable name; both files' mtimes drive the live-reload poll via
`config_files_modified`). App-native keys live in the app file — currently
`module { corner_radius }` (overall module box radius; deliberately NO shared
fallback — the old `status_box_corner_radius` rung was removed) and `module { spacing }` (the gap between segments;
the COMPOSITOR reads this one for its arrange pass — bar-side it only affects a
multi-module surface — applied on `ccectl reload`) and `module { height }` (the
bar height; read by BOTH sides — bar surfaces live via the mtime poll, the
compositor's segment height + reserved strip on `ccectl reload` — falls back to
the shared `layout { bar_height }`) and `module { padding }` (text inset inside
each module box, bar-side only, falls back to the shared
`/style/status/padding`) and
`module { font_size }` (module text size, bar-side only; beats even the size
embedded in the shared font string, which remains the fallback) and
`module { font }` (module text family; an embedded size ranks below
module { font_size } in the size chain) and `module { background_color }` (the
module box fill, rgba; linearized like every quad color, and the
background_blur tint scaling still applies on top) and `module { text_color }`
(module text, raw-sRGB like every text color, falls back to the shared
`/style/status/normal_color`) and `module { droplet }` (the water-droplet module
style — cce-ui's `Prim::Droplet`, shader mode 10; the key's PRESENCE enables
it, its value is whitespace-separated `k=v` pairs onto `DropletSpec` — sag,
belly, belly_w, blend, sheet_r, attach, clarity, dome, band, gleam, shine,
rim, bow, curve, core, refr, ghost, shadow; defaults = the oval dewdrop (no
belly; attach 0.42 + sheet_r 0.58 fill the height so there is NO straight
side; bow arcs the bottom; curve 2.6 = superellipse joins, so everything but
the flat top is one continuous curve), belly>0 brings back the pendant-pool
look — warn-and-skip on unknown keys. Three of those knobs are not this
side's: `refr` (rim refraction, logical px) and `ghost` (the inverted lens
image in the belly) are read by the COMPOSITOR, whose scenefx droplet node
bends the backdrop behind the drop — a Wayland client cannot see behind its
own surface, so this side parses them and draws nothing. `shadow` (0-1, the
contact shadow under the drop's lower arc) IS drawn here, and it is why the
drop box does not fill the surface: the box is inset by 1px for the
silhouette's AA feather plus `DropletSpec::shadow_gap()` for the shadow's
falloff (`main.rs`, three call sites — left, right, and the expanded menu
box, which becomes the drop growing))
and `module { icon_size }` (glyph height for the icon readouts, logical px,
default bar height − 6) and `module { icon_font_size }` (the number beside
the glyph, default `module { font_size }`) and `module { icon_gap }` (glyph
to number, logical px, default 4) and `module { icon_spacing }` (between
readouts in the `stats` bubble, default `module { spacing }`) and
`module { icon_alpha }` (glyph opacity 0-1, default 1) and
`module { icon_weight }` (OpenType weight of the number, unset = regular)
and `module { text_raise }` (lifts module text above vertical center, logical
px, bar-side only — every module funnels through `centered_text_y`) and
`module { text_scrim }` (0-1 resting opacity of a
feathered pool filling each module box, the DE's one text-contrast treatment)
and `module { text_scrim_feather }` (that pool's falloff in logical px,
default a quarter of the box height) and `module { text_contrast }` (adaptive
contrast 0-1, default 0 = off — the compositor's `backdrop` measurement
deepens the pool through it, so the ground darkens only as far as a backdrop
the configured text color cannot carry demands; on its own, with no
`text_scrim`, it makes the pool appear ONLY when the backdrop earns it).
(The glyph-decorating treatments this replaced — `text_relief`'s letterpress
underlay and `text_halo`'s four-copy outline — were deleted 2026-08-28 once
the scrim superseded both; don't reintroduce a per-letterform treatment
without a reason the ground cannot serve.) Everything is read through
`cce_ui::config::cached_config()`; KDL is converted to JSON
(`cce_ui::config::parse_kdl_to_json`) and looked up by **explicit JSON
pointer only**: every key names its canonical nesting
(`/style/status/background_color`, `/module/height`,
`/window_manager/light_source_position`, `/layout/status_bar/<module>` for
per-module sides, …), and a key parked anywhere else simply does not resolve.
(The legacy fuzzy `json_find_key` — snake_case split across nesting, then
depth-first search — was deleted 2026-08-18 after its fallback warnings went
quiet; don't reintroduce it.) Shared keys used here, written as the pointers
they are actually looked up by — the flat snake_case spellings this list used
to carry (`status_padding`, `status_font`, …) appear nowhere in the config or
the code: `/layout/bar_height`, `/style/status/font` (also via fontconfig alias
`status-interface`), `/style/status/font_size`, `/style/status/padding`,
`/style/status/module_spacing`, `/style/status/normal_color`,
`/style/status/disabled_color`, `/style/status/background_color`,
`/style/status/background_blur`, `/style/status/box_bevel`(`_depth`),
`/window_manager/light_source_position`, and `/layout/status_bar/<module>` for
the per-module sides. (The whole-bar
background chain is gone: a `StatusApp` is always a single `--module` segment,
so the surface bg is permanently transparent and only module boxes paint.)

Color space (one rule, enforced in `config.rs`): **text colors stay raw sRGB**
(`text_color_from` — cosmic-text consumes sRGB `[u8; 3]`), **quad/box colors
are linearized** (`quad_color_from` via `cce_ui::color::parse_hex_rgba_linear`,
for the Vulkan pipeline). No local gamma math — the old scattered `.powf(2.2)`
is gone; `text_colors_stay_srgb_and_quad_colors_are_linearized`, in
`config.rs`, is the spec. Config changes are picked up by polling the file
mtime in `tick()`, so there is no reload event to wire up.

## Adaptive text contrast

The bar draws into its own buffer and can never see what it is composited
over, so a module box at `background_color` alpha `30` leaves its text at the
mercy of whatever the desktop shows through it. `module { text_contrast }`
closes that loop with the compositor, which CAN see:

1. `cce-fx` measures each segment's backdrop per frame (`backdrop.rs`) and
   pushes `<luma> <spread>` on the status socket's `backdrop` topic.
2. `contrast_demand()` checks the configured text color's WCAG contrast
   against that backdrop at three points — the mean AND both ends of the
   spread — and takes the worst. Checking only the mean is the trap: a segment
   half on a black grid cell and half on a light gap averages to a comfortable
   mid-gray while the text is invisible over one half.
3. `tick` eases `contrast_now` toward that demand over ~120ms. Stepping
   straight to it makes the scrim pulse as the desktop pans under the segment.

`module { text_scrim }` is the treatment itself: a feathered pool (`cce_ui`'s
`Prim::Glow` — solid through a core rect, falling off to nothing across
`text_scrim_feather` px, tessellated as per-vertex-alpha rings so there is no
banding) filling each module box. It darkens the ground the glyphs sit on
rather than decorating the letterforms. It rests at the configured opacity and
`text_contrast` deepens it from there, so it is a constant when that knob is
off — and either knob alone is meaningful.

One pool per bubble, not per text run, so a segment reads as one darkened
lozenge rather than a pill inside a pill. Its shape is the bubble's ACTUAL
shape, which means two paths: a droplet module gets `Prim::DropletScrim`
(cce-ui shader mode 12 — the droplet's own SDF under the same `DropletSpec`,
filled flat and feathered inward, so the vignette's edge is the drop's edge by
construction), while a plain rounded box gets `Prim::Glow` with its core inset
by exactly the feather, which lands the gradient's outer edge on the box edge.
The rounded-rect pool is clipped to its box; the droplet one needs no clip,
since the shader cannot draw outside the silhouette it is evaluating.

The pool's color comes from `dominant_run_color` — the widest run inside the
box, using the width that rides `TextPrim`'s last field — with
`dominant_icon_color` as the fallback for a box that holds tray icons and no
run: the icons read as light glyphs (dark pixmaps are recolored toward white),
so the tray is grounded as a white run would be, with a black pool. Before
2026-09-05 a box with no measured run got no pool, which left the tray the
one bare bubble in the strip — visibly lighter than its neighbors, and the
one segment the backdrop feed could not deepen. A box holding neither text
nor icons still gets no pool. Width is the tiebreak because a module mixing
colors is led by its longest label.

The pool takes its color from `treatment_rgb` — whichever of black/white the
run reads against, by contrast ratio (the WCAG crossover is near 0.18, not
0.5). Chosen from the run's OWN color, not the configured module color: a
module may paint a run in something else entirely, and the volume module's
muted state uses the shared `disabled_color`. A black pool behind black text
is not a weaker treatment — it is an eraser. (That `disabled_color` is a light
red as of 2026-08-28, chosen so the muted run keeps the same dark pool as
every other bubble instead of inverting to a light one.)

A window covering part of a segment is measured too — the compositor reads
that window's own content over the overlapping strip and blends it with the
desktop reading for the rest. `unknown` is left for content it genuinely
cannot read (no committed buffer, an unsupported read format).

Failures resolve toward legible in every direction: an unparseable or absent
line reads as `(50, 100)` — mid luminance, full spread — which drives the
scrim rather than switching it off.

## Interactions worth knowing before touching input code

- **Super + left-drag on a segment is handled by the compositor**, not this app: it
  starts the same segment drag as adjust-position mode (snap to an edge on release,
  persisted to `layout.status_bar.<module>` in config.kdl). This app never sees those
  clicks and no longer tracks the super key.
- Tray icons left-click activate / right-click open their DBusMenu. (The old
  layout-mode menu and viewport tabs are gone with the viewport-tag feature.)
- `ToggleHideModules` / `ToggleAdjustPositionMode` mirror their state to the compositor
  via `ccectl status-hide-mode|adjust-position-mode true|false`; the adjust-mode state
  is read back with `ccectl adjust-position-mode query` (the compositor is the single
  source of truth — the old `/tmp/cce-status-interface-adjust-mode` sentinel file is
  no longer consulted).
