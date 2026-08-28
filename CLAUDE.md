# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cce-status-interface` is the status bar of the `cce` Wayland desktop environment. It
is one crate in the multi-repo `cce` workspace — see `../cce-compositor/WORKSPACE.md` for
the workspace layout, the multi-repo git rules (commit here, never `git init` at the root),
and the `cce-ui` toolkit this app is built on. This crate is deliberately small:
`src/main.rs` (the `StatusApp` application, layout/input, launcher daemon),
`src/modules.rs` (the `StatusModule` trait and its nine implementations),
`src/config.rs` (pointer-first config readers), `src/tray.rs` (SNI host),
`src/cloud.rs` (menu page building), `src/stats.rs` (system stat readers),
`src/listeners.rs` (status/switcher socket tasks).

## Build, test, run

```sh
cargo build --release                 # standalone build (or `-p cce-status-interface` from the workspace root)
cargo test                            # the tests live in main.rs (e.g. test_status_config)
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
  `cpu`, `memory`, `brightness`, `volume`, `battery`, `clock`, `light_source`.
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
right.

## Rendering

The app implements `cce_ui::engine::Application` on the **`display_list()` paint path**
(Phase 6ak) — the legacy `view*()`/`text_items()` methods are gone. The flow:

1. `rebuild_layout()` runs the two-pass module layout — for each module first
   `StatusModule::width()`, then `StatusModule::render()` — filling retained buffers on
   `StatusApp`: `rects`, `rounded_boxes`, `text_prims`
   (the `TextPrim` tuple type; build them with `draw_label()` from a
   `cce_ui::widget::StyledLabel`), plus `input_regions`, `module_bounds`,
   `tray_item_bounds`.
2. `display_list()` replays those buffers into a `PaintCtx` each frame (and triggers
   `rebuild_layout()` when size/scale changed or `needs_rebuild` is set).
   `overlay_quads()` remains a separate on-top pass (used for drag feedback).

Orientation is dynamic: `is_vertical()` compares the surface size against the
configured bar thickness; every module renders along one axis using `bar_h`/`coord`
accordingly.

## Events and IPC

`update()` consumes `CustomEvent`s sent over a calloop channel from tokio tasks spawned
in `new()` — which tasks run depends on the selected module, so a clock process doesn't
listen to tray D-Bus, etc.:

- **Compositor status feed** (`spawn_status_listener`): connects to
  `/tmp/cce-status[-interface]-{WAYLAND_DISPLAY}.sock`, subscribes to `layout`,
  `title`, `modifiers`, `dismiss` (line-oriented, auto-reconnects every 1s).
  (The old `viewport` topic is gone with the viewport-tag feature.)
- **System stats** (`spawn_system_stats`): `/proc/stat`, `/proc/meminfo`,
  `/sys/class/power_supply/BAT*`, `/sys/class/backlight`, and `pactl` for volume/mute.
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

**No cce-cloud popups remain in this app**: the window picker (window-module
click → `MenuReady` rows of `Ccectl(["focus-window", id])`) is an in-surface
menu too. Menu width sizes to
the longest row label. Expanded segments stack in the compositor's popups
layer (cce-fx@74a0f75) so click-away-close works across the whole surface,
including the strip band over neighboring segments.

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
each module box, bar-side only, falls back to the shared `status_padding`) and
`module { font_size }` (module text size, bar-side only; beats even the size
embedded in the shared font string, which remains the fallback) and
`module { font }` (module text family; an embedded size ranks below
module { font_size } in the size chain) and `module { background_color }` (the
module box fill, rgba; linearized like every quad color, and the
background_blur tint scaling still applies on top) and `module { text_color }`
(module text, raw-sRGB like every text color, falls back to the shared
`status_normal_color`) and `module { droplet }` (the water-droplet module
style — cce-ui's `Prim::Droplet`, shader mode 10; the key's PRESENCE enables
it, its value is whitespace-separated `k=v` pairs onto `DropletSpec` — sag,
belly, belly_w, blend, sheet_r, attach, clarity, dome, band, gleam, shine,
rim, bow, curve, core; defaults = the oval dewdrop (no belly; attach 0.42 +
sheet_r 0.58 fill the height so there is NO straight side; bow arcs the
bottom; curve 2.6 = superellipse joins, so everything but the flat top is one
continuous curve), belly>0 brings back the pendant-pool look —
warn-and-skip on unknown keys; the expanded menu box becomes the drop growing,
and drops inset 1px from the surface bottom for the silhouette's AA feather)
and `module { text_raise }` (lifts module text above vertical center, logical
px, bar-side only — every module funnels through `centered_text_y`) and
`module { text_relief }` (letterpress underlay strength 0-1: a translucent
white copy 0.75px down-right beneath each non-boxed text run — engraved text,
guaranteed contrast on dark backdrops) and `module { text_halo }` (full white
outline 0-1: FOUR diagonal white copies around each run; beats text_relief
when set) and `module { text_scrim }` (0-1 opacity of a
feathered pool hugging each text run; beats the halo when set) and
`module { text_scrim_feather }` (that pool's falloff in logical px, default a
quarter of the box height) and `module { text_contrast }` (adaptive contrast 0-1, default 0 =
off — when set it SUPERSEDES both fixed knobs above and drives the halo from
the compositor's `backdrop` measurement instead, so the outline appears only
over a backdrop the configured text color cannot carry; `text_relief`/
`text_halo` remain as the manual override for a session whose compositor
predates the topic). Everything is read through
`cce_ui::config::cached_config()`; KDL is converted to JSON
(`cce_ui::config::parse_kdl_to_json`) and looked up by **explicit JSON
pointer only**: every key names its canonical nesting
(`/style/status/background_color`, `/module/height`,
`/window_manager/light_source_position`, `/layout/status_bar/<module>` for
per-module sides, …), and a key parked anywhere else simply does not resolve.
(The legacy fuzzy `json_find_key` — snake_case split across nesting, then
depth-first search — was deleted 2026-08-18 after its fallback warnings went
quiet; don't reintroduce it.) Shared keys used here: `bar_height`, `status_font`
(also via fontconfig alias `status-interface`), `status_font_size`,
`status_padding`, `status_module_spacing`, `status_normal_color`,
`status_background_color`, `status_background_blur`, `status_box_bevel`(`_depth`),
`light_source_position`, and the per-module side entries. (The whole-bar
background chain is gone: a `StatusApp` is always a single `--module` segment,
so the surface bg is permanently transparent and only module boxes paint.)

Color space (one rule, enforced in `config.rs`): **text colors stay raw sRGB**
(`text_color_from` — cosmic-text consumes sRGB `[u8; 3]`), **quad/box colors
are linearized** (`quad_color_from` via `cce_ui::color::parse_hex_rgba_linear`,
for the Vulkan pipeline). No local gamma math — the old scattered `.powf(2.2)`
is gone; `test_text_colors_stay_srgb_and_quad_colors_are_linearized` is the
spec. Config changes are picked up by polling the file mtime in `tick()`, so
there is no reload event to wire up.

## Adaptive text contrast

The bar draws into its own buffer and can never see what it is composited
over, so a module box at `background_color` alpha `30` leaves its text at the
mercy of whatever the desktop shows through it. `module { text_contrast }`
closes that loop with the compositor, which CAN see:

1. `cce-fx` measures each segment's backdrop per frame (`backdrop.rs`) and
   pushes `<luma> <spread>` on the status socket's `backdrop` topic.
2. `halo_demand()` checks the configured text color's WCAG contrast against
   that backdrop at three points — the mean AND both ends of the spread — and
   takes the worst. Checking only the mean is the trap: a segment half on a
   black grid cell and half on a light gap averages to a comfortable mid-gray
   while the text is invisible over one half.
3. `tick` eases `halo_now` toward that demand over ~120ms. Stepping straight
   to it makes the outline strobe as the desktop pans under the segment.

`module { text_scrim }` is the alternative treatment, and supersedes the halo
when set: a feathered pool (`cce_ui`'s `Prim::Glow` — solid through a core
rect, falling off to nothing across `text_scrim_feather` px, tessellated as
per-vertex-alpha rings so there is no banding) drawn per TEXT RUN, hugging the
run rather than the module box. Where the halo rims every letterform, this
darkens the ground they sit on. It rests at the configured opacity and
`text_contrast` deepens it from there, so it is a constant when that knob is
off.

The run's measured width rides `TextPrim`'s last field, which is what makes
the hug possible; a run with no width (menu and tooltip text, already on an
opaque box) simply gets no pool. Each pool is clipped to its module box
(`clip_rounded`): a pool is wider than its run and, on a 27px bar with 14px
text, taller than the room above and below it, so without the clip the feather
would wash past the droplet's silhouette and hang in the air beside it.

Both treatments take their color from `treatment_rgb` — whichever of
black/white the run reads against, by contrast ratio (the WCAG crossover is
near 0.18, not 0.5). Applied PER RUN, not from the configured module color: a
module may paint a run in something else entirely, and the volume module's
muted state uses the shared `disabled_color`, which on this DE is black. A
white halo around white text, or a black pool behind black text, is not a
weaker treatment — it is an eraser. (`text_relief` is still white-only; it is
the manual knob, and it was written for dark text.)

A window covering part of a segment is measured too — the compositor reads
that window's own content over the overlapping strip and blends it with the
desktop reading for the rest. `unknown` is left for content it genuinely
cannot read (no committed buffer, an unsupported read format).

Failures resolve toward legible in every direction: an unparseable or absent
line reads as `(50, 100)` — mid luminance, full spread — which drives the
outline rather than switching it off.

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
