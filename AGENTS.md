# AGENTS.md — alttab

## Project Overview

**alttab** is a fast alt-tab window switcher for Wayland compositors (targeting Sway). It behaves like Windows/macOS alt-tab: a small overlay lists open windows sorted by most-recently-used order, the user holds Alt and presses Tab to cycle through them, and releasing Alt confirms and switches to the selected window.

Single Rust binary, ~2.8MB stripped. No tests.

## Target Environment

- **Compositor**: Sway 1.10.1 (wlroots-based)
- **OS**: Fedora 42 (Linux, Wayland-only)
- **No sudo access**: Dev headers for `libxkbcommon-devel` and `wayland-devel` were manually extracted from RPMs to `/tmp/devlibs/usr/`

## Build

```sh
PKG_CONFIG_PATH=/tmp/devlibs/usr/lib64/pkgconfig cargo build --release
```

Output binary: `target/release/alttab` (~2.8MB stripped)

## Architecture

### Daemon + Client Model

The process has two modes:

- **`alttab`** (no args) — Starts as a **long-lived daemon**. Connects to Wayland, binds the foreign-toplevel-manager protocol, and enters a calloop event loop. Continuously tracks window focus changes to build accurate MRU (most-recently-used) history. Listens on a Unix socket at `$XDG_RUNTIME_DIR/alttab.sock` for trigger commands.
- **`alttab --show`** — **Client mode**. Connects to the daemon's Unix socket and sends `"show"` to trigger the overlay. This is what the compositor calls on Alt+Tab.

### Sway Configuration

```
bindsym Alt+Tab exec /path/to/alttab --show
```

The compositor intercepts Alt+Tab before it reaches any application, so the keybinding never leaks to focused windows.

### Why a Daemon?

The `wlr-foreign-toplevel-management` protocol only reports current window state at connection time — it has no historical activation order. A fresh process on each Alt+Tab would see all windows but have no idea which was used most recently. The daemon stays alive and watches activation state transitions over time, building real MRU history.

## Source Files

### `src/main.rs`
Main application state (`AppState`) and all handler implementations:

- **AppState** — Holds all Wayland state (registry, seat, compositor, layer shell, SHM pool), the toplevel tracking state, overlay state (layer surface, selection index, dimensions), keyboard state (alt_held, serial), configuration, icon cache, and font renderer.
- **show_overlay()** — Creates a layer-shell surface on `Layer::Overlay` with `KeyboardInteractivity::Exclusive`, calculates dimensions based on window count, sets selected_index to 1 (the window to switch to). Passes the focused window's output to `create_layer_surface()` for multi-monitor awareness.
- **close_overlay()** — Destroys the layer surface, optionally activates the selected window. Does NOT exit the daemon.
- **draw()** — Renders overlay into wl_shm buffer via render module. Resolves icons for each window's app_id via `IconCache` and passes them to `render_overlay()`.
- **cycle_selection()** — Moves selected_index forward/backward with wrapping.
- **Handler impls** — CompositorHandler (frame callback redraws), SeatHandler (keyboard acquisition), KeyboardHandler (Tab/arrows cycle, Enter confirms, Escape cancels, Alt release confirms, modifier tracking), LayerShellHandler (configure triggers draw, closed cleans up without exiting), OutputHandler, ShmHandler.
- **Daemon setup** — Instance guard (checks socket liveness), calloop event loop with Wayland source, Unix socket listener thread (relays "show" messages via calloop channel), signal handler thread (SIGTERM/SIGINT via sigwait + calloop ping for graceful shutdown with socket cleanup).

### `src/toplevel.rs`
Window tracking and MRU ordering:

- **ToplevelInfo** — Title, app_id, is_activated, is_minimized, is_fullscreen, is_maximized.
- **ToplevelState** — Stores `Vec<(handle, pending_info, committed_info)>` with double-buffered updates (pending accumulates events, committed on `Done`). MRU tracked via `Vec<u32>` of assigned IDs. Each handle gets a monotonic ID via `assign_id()`. `touch_mru_by_id()` moves an ID to the front. The `commit()` method detects activated state transitions (false→true) and updates MRU. Tracks `OutputEnter`/`OutputLeave` per handle for multi-monitor support.
- **window_list()** — Returns `Vec<(&handle, &info)>` sorted by MRU order.
- **focused_output()** — Returns the `WlOutput` for the currently-activated window (used for multi-monitor overlay placement).
- **Dispatch impls** — `Dispatch<ZwlrForeignToplevelManagerV1>` (receives new toplevel handles, uses `event_created_child!` macro), `Dispatch<ZwlrForeignToplevelHandleV1>` (title/app_id/state/output_enter/output_leave/done/closed events). On window close while overlay is visible, adjusts selection or closes overlay if <2 windows remain.

### `src/render.rs`
Software rendering with tiny-skia:

- **render_overlay()** — Renders into ARGB8888 pixel buffer. Colors and layout are config-driven with defaults (dark semi-transparent background, blue highlight for selected item). Accepts `icons: &[Option<&IconData>]` to render window icons alongside text. Uses `FontRenderer` for proper TTF text rendering.
- **draw_icon()** — Scales source RGBA icon data to target size using nearest-neighbor sampling and alpha-blends into the BGRA (little-endian ARGB8888) pixel buffer.
- **draw_rounded_rect()** — Rounded rectangle via quadratic bezier corners.
- **truncate_to_width()** — Truncates text with "..." to fit within available pixel width, using actual font metrics.
- **Layout** — Config-driven: width (default 500px), height adapts to window count. Padding, item height, item spacing, corner radii all configurable. Icons are 32x32, left-aligned within items with text offset when present.

### `src/font.rs`
Font rendering with fontdue:

- **FontRenderer** — Wraps fontdue's `Font` with a glyph rasterization cache. Embeds DroidSans.ttf (Apache 2.0, 187KB) at compile time via `include_bytes!`.
- **draw_text()** — Rasterizes text into the ARGB8888 pixel buffer with proper alpha blending (font color alpha * glyph coverage). Uses fontdue's horizontal line metrics for baseline positioning.
- **measure_text()** — Returns the pixel width of a string at a given font size. Used for text truncation.
- **Glyph cache** — HashMap keyed by `(char, size_in_tenths_of_px)` to avoid re-rasterizing the same glyphs. Stores bitmap, dimensions, offsets, and advance width.

### `src/config.rs`
Configuration loading:

- **Config** — Loaded from `$XDG_CONFIG_HOME/alttab/config.toml` (falls back to defaults).
- **[colors]** section — `background`, `item`, `selected`, `title`, `app_id` as hex color strings (e.g., `"#1E1E1EE6"`). Supports RGB (`#RRGGBB`) and RGBA (`#RRGGBBAA`) formats.
- **[layout]** section — `width`, `max_height`, `item_height`, `item_spacing`, `padding`, `corner_radius`, `item_corner_radius` as integers.
- **parse_hex_color()** — Parses hex color strings into `(r, g, b, a)` tuples.

### `src/icons.rs`
Window icon loading and caching:

- **IconData** — Struct holding `width`, `height`, and RGBA `pixels` vector.
- **IconCache** — HashMap-based cache keyed by `app_id`. On first lookup for an app_id:
  1. Searches `.desktop` files for the `Icon=` field (direct name match, case-insensitive, StartupWMClass matching).
  2. If the icon name is an absolute path, loads it directly.
  3. Otherwise searches icon theme directories: `hicolor/{48x48,64x64,32x32,128x128,256x256,24x24}/apps/`, then `/usr/share/pixmaps/`.
  4. Loads PNG files and converts to RGBA (handles RGB, Grayscale, GrayscaleAlpha color types).
- **Desktop dirs** — `$XDG_DATA_HOME/applications`, then `$XDG_DATA_DIRS/*/applications`.
- **get()** — Mutable lookup that triggers loading if not cached.
- **peek()** — Immutable lookup of already-cached icons (used in render path to avoid borrow conflicts).

## Wayland Protocols Used

| Protocol | Purpose |
|---|---|
| `wlr-foreign-toplevel-management-unstable-v1` | Window listing, activation state tracking, activating/unminimizing windows, output tracking per window |
| `wlr-layer-shell-unstable-v1` | Overlay surface on `Layer::Overlay` with exclusive keyboard grab |
| `wl_shm` | Shared memory buffers for software rendering |
| `wl_compositor` | Surface creation |
| `wl_seat` / `wl_keyboard` | Keyboard input handling |
| `wl_output` | Output (monitor) tracking for multi-monitor support |

## Key Dependencies

| Crate | Version | Purpose |
|---|---|---|
| `wayland-client` | 0.31 | Wayland client protocol |
| `wayland-protocols-wlr` | 0.3 | wlr-specific protocol extensions |
| `smithay-client-toolkit` | 0.20 | High-level SCTK helpers (layer shell, seat, SHM, keyboard with xkbcommon) |
| `calloop` | 0.14 | Event loop (Wayland source + channel + ping) |
| `calloop-wayland-source` | 0.4 | Wayland event source adapter for calloop |
| `tiny-skia` | 0.12 | 2D software rendering (paths, fills) |
| `nix` | 0.29 | Unix signal handling (sigwait) |
| `toml` | 0.8 | TOML config file parsing |
| `serde` | 1 | Deserialization for config structs |
| `png` | 0.17 | PNG icon loading |
| `fontdue` | 0.9 | TTF font rasterization |
| `log` | 0.4 | Logging facade |
| `env_logger` | 0.11 | Log output to stderr |

SCTK 0.20 does NOT have built-in support for the `wlr` foreign-toplevel protocol (only the read-only `ext` variant). The `Dispatch` impls for `ZwlrForeignToplevelManagerV1` and `ZwlrForeignToplevelHandleV1` are written manually, using the `event_created_child!` macro for the Toplevel event.

## Keyboard Navigation

| Key | Action |
|---|---|
| Tab / Down / Right | Select next window |
| Shift+Tab / Up / Left | Select previous window |
| Enter / KP_Enter | Confirm and switch to selected window |
| Escape | Cancel (close overlay, don't switch) |
| Release Alt | Confirm and switch (standard alt-tab behavior) |

All cycling keys support key repeat.

## IPC / Daemon Communication

- **Socket**: `$XDG_RUNTIME_DIR/alttab.sock` (Unix domain socket)
- **Protocol**: Client connects, sends `"show"`, disconnects. Daemon's listener thread relays via calloop channel to the event loop.
- **Instance guard**: On startup, daemon checks if socket exists and is connectable. If yes, another daemon is running — exits with error. If socket exists but is dead, removes the stale file.
- **Graceful shutdown**: SIGTERM/SIGINT caught by a dedicated thread via `sigwait()`, triggers a calloop ping that cleans up the socket and stops the event loop.

## Configuration

Config file: `$XDG_CONFIG_HOME/alttab/config.toml` (typically `~/.config/alttab/config.toml`)

Example:
```toml
[colors]
background = "#1E1E1EE6"
item = "#323232FF"
selected = "#3C78D8FF"
title = "#FFFFFFFF"
app_id = "#AAAAAAFF"

[layout]
width = 500
max_height = 600
item_height = 48
item_spacing = 8
padding = 16
corner_radius = 12.0
item_corner_radius = 8.0
```

All fields are optional — missing fields use defaults.

## Known Constraints

1. `tiny_skia::Color::from_rgba8` is not const — color constants use functions instead of `const` declarations.
2. Only PNG icons are supported (SVG/scalable icons are skipped).
3. Icon scaling uses nearest-neighbor sampling (fast but not the highest quality).
