# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

mpvpaper-rs is a Rust port of mpvpaper - a video wallpaper player for wlroots-based Wayland compositors. It uses libmpv for video playback and integrates with Wayland through layer shells, rendering video frames via EGL/OpenGL.

## Build Commands

```bash
# Build all crates
cargo build --release

# Build specific crate
cargo build -p mpvpaper-rs
cargo build -p mpvpaper-rs-core
cargo build -p mpvpaper-rs-holder

# Run with arguments
cargo run --release -- -d                           # List outputs
cargo run --release -- DP-2 /path/to/video.mp4      # Play on output
cargo run --release -- -v DP-2 /path/to/video.mp4   # Verbose mode

# Check/lint
cargo check
cargo clippy

# Run tests
cargo test
cargo test -p mpvpaper-rs-core  # Test specific crate
```

## Workspace Structure

```
crates/
├── mpvpaper-rs/    # Main binary - video playback, event loop, rendering
├── core/           # Shared library (mpvpaper-rs-core)
└── holder/         # Lightweight process for auto-stop recovery
```

## Architecture

### Two-Binary Design with execv

The system uses two binaries that transfer control via `execv`:

1. **mpvpaper-rs**: Main process that renders video
2. **mpvpaper-rs-holder**: Maintains layer surfaces during auto-stop

When auto-stop triggers (fullscreen window detected or stoplist process running):
- Main process saves playback position and `execv`s to holder
- Holder maintains static layer surfaces with minimal resources
- When conditions clear, holder `execv`s back to main with restore info

### Initialization Order (Critical)

Must follow this sequence - deviating causes crashes or undefined behavior:
1. Wayland connection and output discovery
2. EGL initialization with Wayland display
3. Layer surface creation for selected outputs
4. EGL surface creation per output
5. Make EGL context current (required before step 6)
6. MPV render context initialization (needs active GL context)
7. Video file loading

Cleanup is reverse order: MPV → EGL → Wayland

### Core Library Modules (crates/core/src/)

- `cli.rs` - clap-based argument parsing with validation
- `error.rs` - AppError enum with thiserror
- `config.rs` - Config directory paths (~/.config/mpvpaper-rs/)
- `logging.rs` - Colored output helpers (cflp_success, cflp_error, etc.)
- `process.rs` - /proc-based process detection
- `wayland/` - Wayland state, output management, layer surface creation
- `render/` - EGL initialization, GL function loading, HiDPI support
- `mpv/` - libmpv context, options parsing, frame rendering
- `control/` - Thread management, pause/stop detection

### Key Patterns

**Event Loop (calloop)**: Main loop handles three sources:
1. WaylandSource - protocol events
2. render_wakeup channel - MPV frame notifications
3. Signal handlers - graceful shutdown

**Multi-Output Support**: Uses `HashMap<ObjectId, DisplayOutput>` for per-output state including separate EGL surfaces.

**Pause Detection (Deadman Switch)**: OutputFrameState tracks frame callbacks per output. If no callbacks received, output is considered hidden (fullscreen window covering it).

**Pause Counter**: `is_paused: AtomicI32` allows multiple sources (visibility, watchlist, signals) to independently pause playback.

**HiDPI Scaling**: Compositor reports scale factor; EGL surface dimensions must match logical size × scale. MPV FBO receives physical pixel dimensions.

### Thread Architecture

Worker threads spawned by `ThreadHandles::spawn_all()`:
- `mpv_event_handler` - Process MPV events, handle slideshow transitions
- `auto_pause_handler` - Monitor output visibility
- `auto_stop_handler` - Request holder execution when needed
- `pauselist_monitor_handler` - Watch pauselist processes
- `stoplist_monitor_handler` - Watch stoplist processes

### FFI/Unsafe Patterns

**libmpv2-sys constants**: bindgen generates `enum_name_CONSTANT_NAME` format (e.g., `mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE`), not `enum::CONSTANT`.

**Render callback memory**: The wakeup callback sender is intentionally leaked via `Box::into_raw` - freed when process exits. Attempting to reclaim causes use-after-free.

**Raw pointer functions**: Functions accepting `*mut c_void` (like `wl_display`) must be marked `unsafe fn` with safety documentation.

**SCTK trait imports**: `wayland_client::Proxy` for `id()`/`version()`, `smithay_client_toolkit::shell::WaylandSurface` for `wl_surface()`/`commit()`.

## Dependencies

Critical version constraints:
- `smithay-client-toolkit` 0.20 must match `calloop` 0.14 (and wayland-client 0.31)
- `wayland-protocols-wlr` 0.3 requires compositor wlr-layer-shell support
- `libmpv2` 5 requires libmpv development headers at build time
- EGL requires `libEGL.so.1` at runtime

## Runtime Requirements

- Wayland compositor with wlr-layer-shell (Hyprland, Sway, etc.)
- libmpv shared library
- EGL/OpenGL support
