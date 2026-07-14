# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cce-status-interface` is the status bar of the `cce` Wayland desktop environment. It
is one crate in the multi-repo `cce` workspace — see `../CLAUDE.md` for the workspace
layout, the multi-repo git rules (commit here, never `git init` at the root), and the
`cce-ui` toolkit this app is built on. This crate is deliberately small: `src/main.rs`
(the `StatusApp` application + IPC + config parsing) and `src/modules.rs` (the
`StatusModule` trait and its nine implementations).

## Build, test, run

```sh
cargo build --release                 # standalone build (or `-p cce-status-interface` from the workspace root)
cargo test                            # the tests live in main.rs (e.g. test_status_config)
make install                          # installs ../target/release/cce-status-interface to ~/.local/bin
```

Running it requires a live cce compositor session (`$WAYLAND_DISPLAY` plus the cce
sockets); there is no meaningful headless mode.

## Process model (the most important thing to know)

One binary, four modes, selected by CLI args in `main()`:

- **No args — launcher daemon.** Spawns one child process per module
  (`--module window`, `--module clock`, …), polls every 500ms and restarts crashed
  children. This is the normal production mode: each module is its own process and its
  own Wayland surface.
- **`--module <name>`** — a single-module bar segment. Valid names: `window`, `tray`,
  `cpu`, `memory`, `brightness`, `volume`, `battery`, `clock`, `light_source`.
- **`--monolithic`** — all modules in one window (window on the left, the rest on the
  right). Useful for debugging layout without nine processes.
- **`--trigger-switcher`** — one-shot: writes `trigger` to the switcher socket of the
  running instance and exits (used as a keybinding target).

The compositor places each segment by its Wayland `app_id`, computed in
`StatusApp::get_app_id()`: `cce-status-{side}-{name}` (e.g. `cce-status-left-window`),
or plain `cce-status` for the monolithic bar. If
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
   `StatusApp`: `rects`, `rounded_boxes`, `separators`, `text_prims`
   (the `TextPrim` tuple type; build them with `draw_label()` from a
   `cce_ui::widget::StyledLabel`), plus `input_regions`, `module_bounds`,
   `tray_item_bounds`, `viewport_bounds`.
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
  `/tmp/cce-status[-interface]-{WAYLAND_DISPLAY}.sock`, subscribes to `viewport`,
  `layout`, `title`, `modifiers` (line-oriented, auto-reconnects every 1s). The
  `viewport` payload is Pango-ish markup parsed by `parse_viewport_text()`.
- **System stats** (`spawn_system_stats`): `/proc/stat`, `/proc/meminfo`,
  `/sys/class/power_supply/BAT*`, `/sys/class/backlight`, and `pactl` for volume/mute.
- **Tray** (`spawn_status_tray`): a full StatusNotifierItem/Watcher host over `zbus`,
  including DBusMenu fetching. Icons arrive as pixmaps or theme names (rendered via
  `resvg`/`png`).
- **Switcher** (`spawn_switcher_listener`): binds
  `/tmp/cce-status-interface-switcher-{WAYLAND_DISPLAY}.sock`; a line on it fires
  `SwitcherTriggered`.

Outbound actions shell out to `ccectl` (`view <viewport>`, `windows --json`,
`focus-window`, `viewport-layout`, `window-switcher`, `status-hide-mode`,
`adjust-position-mode`), resolved from `~/.local/bin` first (`get_ccectl_cmd`).
`ccectl windows --json` returns one JSON object per line; the text format is kept only
as a parse fallback for older compositors (`parse_ccectl_window_any_line` handles
both). Keyboard alt-tab switching is delegated to the compositor
(`ccectl window-switcher`) — don't reimplement it here.

**Popups are `cce-cloud` processes**, not surfaces of this app: the window picker, tray
context menus, and the layout-mode menu each run a `cce_ui::process::CloudPopup`
(`run_json`/`run_dmenu`) on a worker thread, and the single-popup toggle state lives in
`cce_ui::process::CloudPopupTracker` (`StatusApp.cloud_popups`) — the thread reports
back via the `CloudSpawned`/`CloudClosed` events, which feed
`tracker.on_spawned`/`on_closed`. Clicking a trigger again toggles its popup off
(`tracker.click`); closing restores focus with `ccectl focus-window`. Follow this pattern
for any new popup.

## Config

Everything reads the shared `~/.config/cce/config.kdl` through
`cce_ui::config::cached_config()`; KDL is converted to JSON
(`cce_ui::config::parse_kdl_to_json`) and looked up with the local `json_find_key`,
which splits snake_case keys across nesting — `status_background_color` matches
`style { status background_color=... }`. Keys used here: `bar_height`, `status_font`
(also via fontconfig alias `status-interface`), `status_font_size`, `status_padding`,
`status_module_spacing`, `status_normal_color`, `status_separator_color`,
`status_background_color`, `status_background_blur`, `status_box_corner_radius`,
`background_color`/`low_color`/`desktop_gap_color` (bar bg fallback chain),
`light_source_position`, and per-module-name position/side entries.

Two gotchas: background/box colors are gamma-corrected (`.powf(2.2)`) while
`status_normal_color` is plain sRGB — match the existing `parse_*_color_from_key`
helper for the kind of color you add. Config changes are picked up by polling the file
mtime in `tick()`, so there is no reload event to wire up.

## Interactions worth knowing before touching input code

- **Super + left-drag** moves a module along the bar (`dragged_module`,
  `ModifiersUpdated` tracks the super key from the compositor feed).
- Viewport tabs in the window module are clickable (`viewport_bounds` → `ccectl view`);
  the layout indicator opens the layout-mode menu; tray icons left-click activate /
  right-click open their DBusMenu.
- `ToggleHideModules` / `ToggleAdjustPositionMode` mirror their state to the compositor
  via `ccectl status-hide-mode|adjust-position-mode true|false`; the adjust-mode state
  is read back with `ccectl adjust-position-mode query` (the compositor is the single
  source of truth — the old `/tmp/cce-status-interface-adjust-mode` sentinel file is
  no longer consulted).
