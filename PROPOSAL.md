# Proposal: cce-status-interface cleanup

Fixes for the issues identified in the 2026-07 review: `main.rs` carrying five jobs,
a fuzzy config-lookup layer that partially duplicates `cce-ui`, stringly-typed
coordination with the compositor, inconsistent color gamma handling, mixed
`eprintln!`/`log::` output, and near-zero test coverage.

Ordered so that each phase is independently commitable and the risky changes land on
top of a test safety net. Phases 1, 2, 5, and 6 touch only this repo; phase 3 also
touches `cce/`; phase 4 also touches `cce-ui/`. Per the multi-repo rules, each repo
gets its own commits and must keep building standalone.

---

## Phase 0 — Characterization tests (safety net)

Today there is one test (`test_status_config`). Before moving anything, pin down the
behavior the later phases will refactor:

- `parse_viewport_text`: pango spans, JSON-wrapped payloads, malformed/unterminated
  spans, plain-text fallback.
- `json_find_key`: exact match, snake_case split across nesting
  (`status_background_color` → `status { background_color }`), collision/traversal
  order, miss → `None`. These tests become the spec for phase 2's replacement.
- `get_module_side`: config side values, snap-position aliases
  (`top-left`/`bottom-right`/…), defaults (`window` → left, rest → right).
- The `ccectl windows` line parser in `trigger_switcher` (extract the per-line parse
  into a free function first so it's testable): `app_id=`/`title="…"`/`focused=`/
  `window id=` extraction, filtered app_ids. These tests get retired in phase 3 when
  the parser is replaced by JSON, but until then they document the wire format.
- Color parsing: one test asserting which keys are gamma-corrected and which are raw
  sRGB, so phase 2's fix is a deliberate, visible change rather than an accident.

**Effort:** small. **Risk:** none (test-only).

## Phase 1 — Split `main.rs` (mechanical, no behavior change)

`main.rs` is ~3,400 lines. Split by existing seams, keeping `modules.rs` as-is:

| New file | Contents (moved, not rewritten) | ~lines |
|---|---|---|
| `src/tray.rs` | zbus proxies/traits, `StatusNotifierWatcher`/host impl, `spawn_status_tray`, `fetch_tray_item`, icon decode | ~850 |
| `src/cloud.rs` | `show_cce_cloud_menu`, `MenuItem` parsing/paging, window-picker spawn body from `trigger_switcher` | ~500 |
| `src/stats.rs` | `spawn_system_stats`, `read_cpu_ticks`, `read_memory_usage`, `read_battery_details`, `read_volume`, brightness | ~300 |
| `src/config.rs` | all `read_*_from_config`, `parse_*_color_from_key`, `json_find_key`, `parse_font_for_alias`, `get_*_cmd` | ~350 |
| `src/listeners.rs` | `spawn_status_listener`, `spawn_switcher_listener` | ~100 |
| `main.rs` (remains) | `StatusApp`, `Application` impl, layout/input, `CustomEvent`, launcher-daemon `main()` | ~1,300 |

Rule for the phase: `git diff` should show only moves, `use` changes, and visibility
bumps (`fn` → `pub(crate) fn`). No logic edits — those come later, reviewable on their
own.

**Effort:** medium (mostly mechanical). **Risk:** low with phase 0 in place.

## Phase 2 — Config: replace the local layer with `cce-ui` accessors

`cce_ui::config` already provides `cached_config()`, JSON-pointer accessors
(`get_f32`, `get_bool`, `get_string`, `get_color`) and a recursive `find_key`. The
local layer in this crate re-implements the lookup with an extra behavior — splitting
snake_case keys across nesting — and hand-rolls gamma with `.powf(2.2)`.

1. **Make each config key an explicit pointer.** Replace
   `json_find_key(&val, "status_background_color")` with
   `cce_ui::config::get_color("/style/status/background_color")` (etc.), encoding the
   real nesting once instead of discovering it by recursive search. Keys whose actual
   KDL location is unclear get resolved by looking at a real `config.kdl` and the
   compositor's reader — that's the point: today nobody can grep where a key lives.
2. **Keep a thin fallback during migration.** A local
   `get_color_fuzzy(key)` that first tries the pointer, then falls back to the old
   `json_find_key`, with a `log::warn!` when only the fallback hits. After one release
   of quiet logs, delete the fallback and `json_find_key` entirely.
3. **Fix gamma in one place.** `get_color` returns raw sRGB by its own doc; apply
   `cce_ui::color::srgb_to_linear` at the single point where colors enter the render
   state (`rebuild_layout`), replacing the scattered `.powf(2.2)` and the
   `status_normal_color`-is-linear-but-background-isn't inconsistency. Verify visually
   against the current bar before/after (screenshot compare) since this may shift
   perceived colors that users have tuned; if `status_normal_color` was correct as-is,
   document that in the code rather than leaving it implicit.
4. **Delete local duplicates:** `parse_hex`, `parse_hex_rgba` wrappers,
   `parse_srgb_color_from_key`, `parse_rgba_color_from_key`, `parse_json` — all become
   calls into `cce_ui::config`/`cce_ui::color`.

**Effort:** medium. **Risk:** medium (visible color shifts possible — mitigated by the
phase-0 gamma test and a manual screenshot check). Cross-crate impact: none; other
clients using the fuzzy pattern can migrate later on their own schedule.

## Phase 3 — Structured `ccectl` output (cross-repo: `cce/`)

The window picker parses `ccectl windows` free text with `find("app_id=")` and
friends; titles containing `"` or spaces in unexpected places break it silently.

1. **In `cce/` (owns `run_cce_ctl` and the control socket):** add `--json` to the
   read commands this crate consumes — `windows` first; `viewports`/others as needed.
   Output: one JSON object per line or a single array —
   `{"id": …, "app_id": "…", "title": "…", "focused": bool}`. Text output stays the
   default so nothing else breaks.
2. **In this crate:** replace the line parser with `serde_json` deserialization into a
   small `WindowEntry` struct; fall back to the text parser if `--json` is rejected
   (running against an older compositor), so the two repos can ship independently.
3. **Same pass, smaller items:**
   - Replace `Command::new("kill").arg(pid)` with a direct `SIGTERM` via `libc::kill`
     (or the `nix` crate) — no shell-out, and an error result instead of a silent
     failure.
   - Replace the `/tmp/cce-status-interface-adjust-mode` sentinel-file read in
     `ToggleAdjustPositionMode` with a `ccectl` query, so mode state has one source of
     truth (the compositor).

**Effort:** medium, split across two repos. **Risk:** low (fallback keeps old/new
combinations working).

## Phase 4 — A shared popup helper (cross-repo: `cce-ui/`)

The `cce-cloud` popup pattern (spawn, write JSON pages to stdin, track pid via
`CloudSpawned`/`CloudClosed`, toggle-off by kill, restore focus on close) is
hand-rolled here in three places (window picker, tray menus, layout menu) and will be
wanted by other clients.

- Add `cce_ui::process::CloudPopup` (the crate already has `process.rs`):
  `spawn(pages, position, source) -> CloudPopup`, `toggle_off()`, `is_running()`,
  completion callback for focus-restore. Internals lifted from this crate's working
  implementation — it's extraction, not redesign.
- Port the three call sites here; `active_cloud_pid`/`active_cloud_source` and the
  `/proc/<pid>/comm` checks move behind the helper.

Deliberately **not** proposing a socket protocol between bar and popups: the
pid-plus-stdin model is working, and the goal is to stop *re-implementing* it, not to
replace it. Revisit only if popups need richer two-way communication.

**Effort:** medium. **Risk:** low-medium (behavior-preserving extraction, three call
sites to verify by hand: window picker, a tray menu, layout menu).

## Phase 5 — Logging unification

51 `eprintln!` vs 12 `log::` calls today, and `env_logger` is already initialized.

- Convert `eprintln!` → `log::debug!` (chatty per-event traces: `[status-listener]`,
  `[cloud-event]`, stats updates) or `log::info!`/`log::warn!` (lifecycle, failures).
- Keep the existing bracketed subsystem tags as message prefixes; they're useful.
- Default filter stays `Info`, so the net effect is a much quieter stderr with
  `RUST_LOG=debug` restoring today's firehose.

**Effort:** small, mechanical. **Risk:** none.

## Phase 6 — Follow-ups (explicitly out of scope for now)

- **Tray as its own crate** (`cce-tray`): worth doing only when a second client needs
  SNI. Phase 1's `tray.rs` makes the later extraction cheap.
- **Upstreaming the fuzzy key-split into `cce_ui::config::find_key`**: rejected —
  phase 2 goes the other way (explicit pointers), and two lookup semantics in the
  toolkit is worse than one.
- **Launcher-daemon supervision polish** (backoff on crash-looping modules instead of
  unconditional 500ms restarts): cheap, but wait until after phase 1 so it lands in a
  small `main()`.

---

## Sequencing and verification

```
0 tests ─→ 1 split ─→ 2 config/gamma ─→ 5 logging
                └────→ 3 ccectl --json (needs a cce/ commit first)
                └────→ 4 CloudPopup    (needs a cce-ui/ commit first)
```

Phases 3 and 4 are independent of 2 and of each other; 5 can land any time after 1.

Each phase ends with: `cargo test` green, `cargo build --release` standalone, and a
manual smoke test in a live session — bar renders in both orientations, viewport tabs
switch, a tray menu opens and closes, the window picker toggles, super+drag moves a
module, and a config edit hot-reloads.
