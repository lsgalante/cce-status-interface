# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cce-status-interface` is the status bar of the `cce` Wayland desktop environment. It
is one crate in the multi-repo `cce` workspace — see `../cce-compositor/WORKSPACE.md` for
the workspace layout, the multi-repo git rules (commit here, never `git init` at the root),
and the `cce-ui` toolkit this app is built on. This crate is deliberately small:
`src/main.rs` (the `StatusApp` application, layout/input, launcher daemon),
`src/modules.rs` (the `StatusModule` trait and its eleven implementations),
`src/config.rs` (pointer-first config readers), `src/tray.rs` (SNI host),
`src/cloud.rs` (menu page building), `src/stats.rs` (system stat readers —
numbers, not strings; the modules do the formatting), `src/icons.rs` (tinted
cce-icons glyph textures), `src/listeners.rs` (status/switcher socket tasks),
`src/osd.rs` (the volume/brightness slider).

## Build, test, run

```sh
cargo build --release                 # standalone build (or `-p cce-status-interface` from the workspace root)
cargo test                            # 42 tests: main.rs (parsers, droplet geometry), config.rs, tray.rs, osd.rs, the tray bridge's x11.rs and title.rs
make install                          # release build, then `ccebuild install --no-build cce-status-interface`
```

Running it requires a live cce compositor session (`$WAYLAND_DISPLAY` plus the cce
sockets); there is no meaningful headless mode.

## `cce-xembed-tray` — legacy X11 tray icons in the bar

A second binary (`src/bin/cce-xembed-tray/`, run by `cce-xembed-tray.service`)
bridges the **XEmbed system tray** into the SNI tray `tray.rs` hosts. X11 apps
older than StatusNotifierItem — Wine and Proton programs above all — dock their
icons with whichever X client owns `_NET_SYSTEM_TRAY_S0`; with no owner, Wine
shows a fallback window of its own holding the icons, which is how a blank white
window came to sit beside Ubisoft Connect (2026-09-26). The bridge owns that
selection and, per docked icon:

- **reparents the icon window into a container** — an override-redirect window
  with `WM_CLASS` `cce-xembed-tray`, which the compositor never shows
  (`cce-compositor/src/server/xwayland_override_redirect.rs`,
  `is_xembed_tray_container`; keep the class in step). X needs it mapped or
  the icon never draws. It also gets an **empty input region**, so X never
  routes the pointer into it: a hidden container stacked over a real X window
  would otherwise swallow that window's clicks.
- **reads its pixels** with `GetImage` whenever X Damage reports a redraw, in
  the ARGB visual it advertises through `_NET_SYSTEM_TRAY_VISUAL` (so icons keep
  their transparency), and publishes them as `IconPixmap` — skipped while the
  icon is still fully transparent, so an undrawn icon is never an empty slot.
- **forwards clicks** as `SendEvent` button presses to the icon: `Activate` is
  button 1, `ContextMenu` button 3 (the app draws its own menu, which is why
  there is deliberately no `Menu` property — its presence makes the bar fetch a
  D-Bus menu instead), `Scroll` buttons 4-7. An app opens its menu at the
  root position the event reports, so the click is placed at the host's
  Activate/ContextMenu point — scaled into X pixels by `Xft.dpi`/96 — and the
  container is moved under it first. That point is only right because the
  bar sends SCREEN coordinates, as SNI asks: its segment's position from
  `ccectl windows --json` plus the pointer's x, at the bar's bottom edge
  (`tray_click_point`). It sent segment-local ones until 2026-09-26, and
  Ubisoft Connect's menu opened ~700 px from its icon. With no point (0,0)
  the container stays where it docked, at the top-right of the X screen.
- **looks after the popup the click opens** (`PopupWatch`): the next
  override-redirect window to map within 1.5 s of a forwarded click is that
  click's popup. Windows tray apps open their menu UPWARD from the pointer
  (the taskbar is at the bottom there) and clamp it to the screen top, so on
  a top bar it lands over the icon — a popup reaching above the bar's bottom
  edge is moved down to it. The app keeps working in the moved window, since
  X reports pointer positions relative to the window. Wine's `_NET_WORKAREA`
  does not help: Ubisoft Connect places its own menu, and ignored a work area
  that excluded the bar. The popup is also closed on the compositor's
  `clickaway` status topic (a press on no X11 surface): the bridge addresses
  it a press just outside itself, which the app — holding the mouse capture
  while its menu is up — reads as a click outside. Xwayland never delivers a
  press on a Wayland window, so before this only a click on one of the app's
  own X windows closed the menu.
  **Tooltips are not the popup** (`is_tooltip`). A forwarded click makes
  Wine's `explorer.exe` — the prefix's tray host — show the icon's tooltip,
  and Wine gives it the same window type (`DIALOG`) and Win32 styles as the
  app's menu; only its owner tells them apart. So a window from a Wine
  plumbing process (`title::is_wine_plumbing`: a `WINEPREFIX` process that
  `is_wine_program` does not count as an app) is skipped, as is one typed
  `_NET_WM_WINDOW_TYPE_TOOLTIP`. Until 2026-09-28 the watch took whichever
  mapped LAST, so a tooltip mapping over an open menu replaced it and the
  click-away closed the tooltip, leaving the menu up. A headless shadow never
  shows the tooltip (Wine's tooltip checks the real X pointer); reproduce it
  by mapping explorer's `DIALOG` window override-redirect yourself while a
  test app's menu is open.
- **names it after its app** (`title.rs`), since icon windows are untitled
  and their WM_CLASS names the toolkit (`steam_proton` for every Proton
  program). A Wine icon is not even the app's window: Wine's tray lives in the
  prefix's `explorer.exe`, which creates every program's icon windows. So the
  name is the most common title among the top-level windows of the programs
  sharing the icon's `WINEPREFIX` — drive-letter exes outside `C:\windows\`,
  which leaves out explorer, Proton's `steam.exe` shim and xalia — or of the
  icon's own process for a native app. Ubisoft Connect titles its windows
  "Ubisoft Connect" even while hidden in the tray. With no titled window yet,
  the exe's stem stands in and the lookup is retried at 2, 5, 10 and 30 s.

Each item is its own session-bus connection registering by object **path**, so
the watcher records its unique name — the only kind of name whose disappearance
`spawn_status_tray` notices. Closing the connection is the whole of
unregistering. On SIGTERM the bridge hands every icon back to the root window,
unmapped, which XEmbed clients read as "the tray is gone". Another tray already
owning the selection is waited out, not displaced.

Verify it in a shadow with `cce-shadow start --xwayland` and
`cce-compositor/verify/clients`' `xembed-icon`, under `dbus-run-session` so the
test bar and bridge never reach the live session's bus.

## Process model (the most important thing to know)

One binary, four modes, selected by CLI args in `main()`:

- **No args — launcher daemon.** Spawns one child process per module
  (`--module window`, `--module clock`, …), polls every 500ms and restarts crashed
  children with exponential backoff (500ms doubling to 30s; 30s of healthy uptime
  resets it). This is the normal production mode: each module is its own process and
  its own Wayland surface.
- **`--module <name>`** — a single-module bar segment. Valid names: `window`, `tray`,
  `stats`, `cpu`, `memory`, `brightness`, `volume`, `wifi`, `battery`, `clock`,
  `light_source` (the daemon launches `stats`, not the six it combines).
- **`--trigger-switcher`** — one-shot: writes `trigger` to the switcher socket of the
  running instance and exits (used as a keybinding target).
- **`--osd <level>`** — the volume/brightness slider (below). Started by the
  launcher daemon, never supervised: it exits on its own.

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

## The volume/brightness slider (`osd.rs`)

A transient bubble — glyph, track, number — that appears when either level
moves and leaves `osd { timeout_ms }` (1500) after the last change. It is a
layer-shell surface on the **OVERLAY** layer, which the compositor stacks
above `layers.fullscreen`, so it shows over fullscreen games and video where
the bar is hidden. Keyboard interactivity is `None` (a fullscreen window
must never yield focus to it) and the input region is empty (clicks pass
through).

- **Trigger**: the launcher daemon runs `spawn_level_watchers` — the same
  fast path the readouts use, now taking a callback (`LevelChange`) instead
  of a calloop sender — so a key, `brightnessctl` in a terminal or a mixer
  all show it. Each change is forwarded as one line (`brightness 40`,
  `volume 55 0`, `volume - 1`) to the slider's instance socket
  (`/tmp/cce-status-osd-<display>.sock`, `cce_ui::ipc::instance`), or, with
  nothing listening, `--osd <line>` is spawned. Queued changes collapse to
  the newest.
- **It exits rather than hides**: a mapped surface, even fully transparent,
  keeps a fullscreen window off direct scanout. It gives up its socket
  BEFORE the close fade, so a change during the fade starts a fresh slider
  instead of being answered and dropped.
- Looks: the bar's `module { }` box (droplet, bevel or plain), colors, font
  and glyphs, scaled by `osd { height }` (default 1.5 × bar height) over
  the bar height. `osd { width position margin }` place it (`"bottom"`
  default, `"top"`, `"center"`; margin from that edge, default 96);
  `osd { enabled false }` turns it off. Muted reads in `disabled_color`.
- Verify in a shadow by running `--osd volume 55 0` directly (export
  `CCE_ICONS_DIR`), or the launcher plus a real level change — the watchers
  read the machine's real backlight and sink, which the shadow shares.

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
label.** `cpu`, `memory`, `brightness`, `volume`, `wifi` and `battery` are
`IconStat` implementations: each names a cce-icons glyph, the bare number
and a color, and `IconReadout` draws the glyph (tinted that color, at
`module { icon_alpha }`) with the number `module { icon_gap }` to its right
— no unit symbol, since the glyph IS the unit ("87" beside the battery, not
"Bat 87%"). **The launcher runs them as ONE segment**, `stats`
(`StatsModule`): every readout in a single bubble, `module { icon_spacing }`
apart, in the order cpu, memory, brightness, volume, wifi, battery — the
order the compositor's `RIGHT_ORDER` gave the five separate segments, with
wifi (added 2026-10-02, after that order) beside volume, and `stats` has
its own slot there between `tray` and `clock` (cce-window-manager
2026-09-16; `wifi` joined it between `volume` and `battery` in
cce-window-manager@1279fc5). The single names stay valid `--module` values for a bar
that wants them apart; a blanket `impl<T: IconStat> StatusModule for T`
lays a lone readout out through the same `readouts_width` /
`render_readouts` the combined segment uses. (Superimposing the number on a
ghosted glyph, with a bold weight and a dark pocket under the digits, was
tried first on 2026-09-16 and replaced the same day by the side-by-side
form; `icon_weight` survives as an opt-in, the pocket is gone.) The muted
sink swaps to `volume-muted`, the charging battery to `battery-charging`;
the battery also keeps its accent color while charging or under 10%. Wifi
is the link's signal strength, and a disconnected adapter reads like a
muted sink: `wifi-off`, dimmed, no number. A
reader with nothing (no battery, no backlight, no pactl, no wireless
adapter) returns `None`
and drops out of the row — a lone module with nothing has width 0, i.e. it
is hidden rather than an empty bubble; a reader that answers without a
number (cpu with no /proc/stat, a sink with no level) draws the glyph
alone.

The glyphs come from the **cce-icons** crate via `cce_ui::icons_dir()`
(`$CCE_ICONS_DIR`, else `~/projects/cce/cce-icons/svg`) — but NOT through
`cce_ui::upload_icon`: a `Prim::Image` has alpha and no color, and the
artwork is white, so `icons.rs::tinted_icon` rasterizes the SVG itself
(`cce_ui::rasterize_svg`), multiplies it by the readout's raw-sRGB color and
uploads it, cached per `(name, px, color)` — for the life of the RENDERER,
not the process: the cache holds renderer image ids, and a reconnect
(cce-ui repairs a lost transport by opening a new session around the same
`Application`) rebuilds the renderer and its image table, leaving every
cached id naming nothing. A draw for an unknown id is skipped rather than
reported, so a reconnected bar came back with its numbers and no glyphs at
all; `renderer_init` now calls `icons::drop_textures()` on every renderer
after the first, and the rebuild it forces re-uploads them. A
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
  the window module, `dismiss` in every one. Reconnects back off
  1s doubling to 30s, reset the moment a connection delivers a line: a
  compositor that does not know a topic drops the subscription on sight, so a
  flat retry made a bar running ahead of its compositor reconnect once a second
  from every module process, forever. The compositor also offers `modifiers`
  (`status_server.rs`), but nothing here subscribes to it and the match over
  topics ends in `unreachable!()` — adding a subscription means adding its arm
  first. (The old `viewport` topic is gone with the viewport-tag feature.)
- **System stats** (`spawn_system_stats`): `/proc/stat`, `/proc/meminfo`,
  `/sys/class/power_supply/BAT*`, `/sys/class/backlight`, `pactl` for volume/mute,
  and `/sys/class/net/*/wireless` + `/proc/net/wireless` for wifi (`read_wifi`:
  the link is connected when its `operstate` is `up`, and the quality column is
  on cfg80211's 0..=70 scale, so 70 is 100%).
  `SystemStats` carries numbers (`cpu_pct`, `memory`, `battery: (capacity,
  charging)`, `volume: (level, muted)`, `brightness`, `wifi: (signal,
  connected)`), each `Option` where
  the source can be absent; only the clock arrives pre-formatted. The loop is
  once a second, which is fine for a clock or a load average and far too slow
  for the two values a KEYPRESS moves — so the backlight and the sink have a
  fast path beside it (`spawn_level_watchers`), each pushing its own
  one-field event (`BrightnessUpdated` / `VolumeUpdated`) that patches
  `stats` in place. `watch_brightness` polls `/sys/class/backlight` every
  100ms — `brightnessctl` writes the attribute directly, so there is nothing
  to subscribe to, and two small sysfs reads are cheap enough that the
  interval is not worth tuning; `watch_volume` follows `pactl subscribe` and
  re-reads only on a `sink`/`server` event (NOT `sink-input`, which fires
  throughout playback, and NOT `client`, which the bar's own `pactl` runs
  generate — matching either would put the reader in a loop with itself).
  Both send only a CHANGED value, so an idle desktop never wakes the event
  loop, and `update()` asks `paints_stat` whether this module shows the field
  before redrawing — the one-field counterpart to `stats_signature`, and a
  test holds the two in agreement. The subscription burst is coalesced for
  30ms before the read (a held volume key emits a stream of events, and one
  `pactl` spawn per event would fall behind); the child carries
  `PR_SET_PDEATHSIG` as well as `kill_on_drop`, because a subscription whose
  reader was killed outright is reparented to init and sits there rather than
  noticing. The one-second loop still reads both values, so it remains the
  safety net when `pactl subscribe` cannot run at all. Measured in a shadow:
  ~45ms for the backlight, ~55ms for the sink, against a second before.
- **Tray** (`spawn_status_tray`): a full StatusNotifierItem/Watcher host over `zbus`,
  including DBusMenu fetching. Icons arrive as pixmaps or theme names (rendered via
  `resvg`/`png`).
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
panel edge (`menu_hover_rect`, drawn before the text), not a full-width rect.

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
default 16 = the tray's fixed icon size, so the two read as one set) and `module { icon_font_size }` (the number beside
the glyph, default `module { font_size }`) and `module { icon_gap }` (glyph
to number, logical px, default 4) and `module { icon_spacing }` (between
readouts in the `stats` bubble, default `module { spacing }`) and
`module { icon_alpha }` (glyph opacity 0-1, default 1) and
`module { icon_weight }` (OpenType weight of the number, unset = regular)
and `module { text_raise }` (lifts module text above vertical center, logical
px, bar-side only — every module funnels through `centered_text_y`) and
`module { backdrop_compress }` (the minimum WCAG contrast ratio the text must
hold against any backdrop pixel, e.g. 10; unset = off — read by the
COMPOSITOR only, see "Text contrast" below). Everything is read through
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

## Text contrast: backdrop compression

The bar draws into its own buffer and can never see what it is composited
over, so a module box at `background_color` alpha `30` leaves its text at the
mercy of whatever the desktop shows through it. The fix lives in the
compositor, which CAN see: `module { backdrop_compress }` makes `cce-fx`
compress the luminance of each segment's backdrop — the blurred one, or the
refracted one when the droplet lens is live — so the module text keeps that
contrast ratio over every pixel of it. For light text the backdrop's bright
parts are pulled down under a ceiling; for dark text its shadows are lifted
instead. Below a knee (half the ceiling) nothing moves, so a backdrop that is
already dark enough is left exactly as it is, and the curve approaches the
ceiling asymptotically with no seam. The color is scaled, not desaturated, so
a bright backdrop keeps its hue. The text color is `module { text_color }`
(else the shared `/style/status/normal_color`), read compositor-side;
`backdrop_compress_params` in cce-compositor's `config.rs` turns the pair into
the shader's ceiling, and scenefx's `tex.frag`/`droplet.frag`
(`compress_backdrop`) apply it. It rides the backdrop blur, so a bar with
`/style/status/background_blur` at 0 has nothing to compress. The bubble's own
translucent fill and the droplet's lighting composite on top of the compressed
backdrop and lift it, so the ratio reached is well under the one asked for:
measured in a shadow on a pure-white desktop with this repo's droplet style,
`backdrop_compress 4.5` reached 2.7:1 and `10` reached 4.7:1 (`15`: 6.4:1).
Over a backdrop already dark enough, on and off are pixel-identical.

This replaced the bar-side scrim on 2026-10-01: a dark feathered pool
(`text_scrim`, deepened by `text_contrast` from a per-segment `backdrop`
luminance measurement the compositor pushed on the status socket) that
darkened the whole bubble even over a backdrop that needed no help. Compression
is per pixel, so it needs neither the pool nor the measurement loop. (Before
the scrim, `text_relief`'s letterpress underlay and `text_halo`'s four-copy
outline decorated the letterforms; they went 2026-08-28. Don't reintroduce a
per-letterform treatment without a reason the backdrop cannot serve.)

## Interactions worth knowing before touching input code

- **Super + left-drag on a segment is handled by the compositor**, not this app: it
  starts the same segment drag as adjust-position mode (snap to an edge on release,
  persisted to `layout.status_bar.<module>` in config.kdl). This app never sees those
  presses and no longer tracks the super key — with two exceptions since
  2026-09-16 (cce-fx `cursor.rs`): a press that travels under 6px is a CLICK,
  replayed to the segment as press+release instead of snapped (a still click on
  a top-edge segment used to re-home it to top-center), and an EXPANDED segment
  (menu open) is never grabbed at all, so the "Done" row can end adjust mode.
- Tray icons left-click activate / right-click open their DBusMenu. (The old
  layout-mode menu and viewport tabs are gone with the viewport-tag feature.)
- `ToggleHideModules` / `ToggleAdjustPositionMode` mirror their state to the compositor
  via `ccectl status-hide-mode|adjust-position-mode true|false`; the adjust-mode state
  is read back with `ccectl adjust-position-mode query` (the compositor is the single
  source of truth — the old `/tmp/cce-status-interface-adjust-mode` sentinel file is
  no longer consulted).
