//! Main event loop and application lifecycle
//!
//! This module handles the calloop-based event loop integration with:
//! - Wayland event dispatching (via calloop-wayland-source)
//! - MPV render wakeup handling
//! - Signal handling (SIGINT, SIGTERM)

use std::ffi::{c_void, CString};
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use calloop::channel::{channel, Channel, Event as ChannelEvent, Sender};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use nix::libc;
use smithay_client_toolkit::shell::wlr_layer::Layer;
use wayland_client::{EventQueue, Proxy};

use mpvpaper_rs_core::cli::{Args, LayerArg};
use mpvpaper_rs_core::config::{get_pauselist_path, get_stoplist_path, load_list_file};
use mpvpaper_rs_core::control::{HaltInfo, ThreadHandles};
use mpvpaper_rs_core::error::{AppError, Result};
use mpvpaper_rs_core::logging::{cflp_error, cflp_info, cflp_success};
use mpvpaper_rs_core::mpv::{parse_user_options, render_frame, MpvState};
use mpvpaper_rs_core::render::{load_gl_functions, EglState};
use mpvpaper_rs_core::wayland::{create_layer_surface, select_outputs, AppState};

/// Run the main application
pub fn run_app(args: &Args) -> Result<()> {
    // Parse MPV options first (before any initialization)
    let parsed_mpv_options = if let Some(ref opts) = args.mpv_options {
        parse_user_options(opts)?
    } else {
        mpvpaper_rs_core::mpv::ParsedMpvOptions {
            init_options: Vec::new(),
            runtime_properties: Vec::new(),
        }
    };

    // Setup signal handlers early
    let halt_info = Arc::new(HaltInfo::new(args.auto_pause, args.auto_stop));

    // Load watchlists if they exist
    let halt_info = setup_watchlists(halt_info, args)?;

    // Connect to Wayland
    cflp_info(1, args.verbose, "Connecting to Wayland...");
    let (mut app_state, mut event_queue) = AppState::with_halt_info(Arc::clone(&halt_info))?;
    cflp_success("Connected to Wayland");

    // Select outputs
    let output_selector = args.output.as_deref().unwrap_or("*");
    let selected_outputs = select_outputs(&app_state.output_state, output_selector)?;
    cflp_info(
        1,
        args.verbose,
        &format!("Selected {} output(s)", selected_outputs.len()),
    );

    // Initialize EGL
    cflp_info(1, args.verbose, "Initializing EGL...");
    let egl_state = Arc::new(EglState::new(&app_state.conn)?);

    // Load GL functions
    load_gl_functions(&egl_state);

    // Create render wakeup channel
    let (render_wakeup_tx, render_wakeup_rx): (Sender<()>, Channel<()>) = channel();

    // Initialize MPV (without render context - needs active GL context)
    cflp_info(1, args.verbose, "Initializing MPV...");
    let wl_display = app_state.conn.backend().display_ptr() as *mut c_void;
    let mpv_state = Arc::new(MpvState::new(
        wl_display,
        render_wakeup_tx,
        !args.no_mpv_config,
        &parsed_mpv_options,
    )?);
    let mut mpv_state = mpv_state;

    // Create layer surfaces for each selected output
    let layer = match args.layer {
        LayerArg::Background => Layer::Background,
        LayerArg::Bottom => Layer::Bottom,
        LayerArg::Top => Layer::Top,
        LayerArg::Overlay => Layer::Overlay,
    };

    for wl_output in &selected_outputs {
        let qh = app_state.qh.clone();
        let layer_surface = create_layer_surface(&mut app_state, &qh, wl_output, layer);

        // Store layer surface in the corresponding DisplayOutput
        if let Some(display_output) = app_state.outputs.get_mut(&wl_output.id()) {
            display_output.layer_surface = Some(layer_surface);
        }
    }

    // Roundtrip to get configure events
    event_queue.roundtrip(&mut app_state)?;

    // Setup EGL surfaces for each output
    for display_output in app_state.outputs.values_mut() {
        if display_output.layer_surface.is_some() && display_output.width > 0 && display_output.height > 0 {
            display_output.setup_egl(&egl_state, display_output.width, display_output.height)?;
            cflp_info(
                2,
                args.verbose,
                &format!(
                    "EGL surface created for {} ({}x{})",
                    display_output.name, display_output.width, display_output.height
                ),
            );
        }
    }

    // Initialize MPV render context (requires current EGL context)
    // Find first valid EGL surface to make context current
    let first_surface = app_state
        .outputs
        .values()
        .find_map(|o| o.egl_surface.as_ref())
        .ok_or_else(|| AppError::EglInit("No EGL surface available for MPV init".into()))?;

    egl_state.make_current(first_surface)?;

    // Now initialize the MPV render context with active GL context
    let egl_for_mpv = Arc::clone(&egl_state);
    unsafe {
        Arc::get_mut(&mut mpv_state)
            .unwrap()
            .init_render_context(move |name| {
                egl_for_mpv
                    .instance
                    .get_proc_address(name.to_str().unwrap_or(""))
                    .map(|p| p as *mut c_void)
                    .unwrap_or(std::ptr::null_mut())
            })?;
    }

    // Load video file
    if let Some(ref video_path) = args.video_path {
        mpv_state.load_file(video_path)?;
    }

    // Restore position if provided
    if let Some(ref restore_info) = args.restore_info {
        mpv_state.restore_position(restore_info)?;
        cflp_info(1, args.verbose, "Restored playback position");
    }

    // Apply slideshow options if enabled
    if args.slideshow.is_some() {
        mpvpaper_rs_core::mpv::apply_slideshow_options(&mpv_state.mpv)?;
    }

    // Spawn worker threads
    let mut thread_handles = ThreadHandles::spawn_all(
        Arc::clone(&halt_info),
        Arc::clone(&mpv_state),
        args.slideshow,
        args.verbose,
    );

    // Setup signal handling with channel
    let (signal_tx, signal_rx): (Sender<()>, Channel<()>) = channel();
    setup_signal_handlers(Arc::clone(&halt_info), signal_tx)?;

    // Run main event loop
    cflp_success("Starting event loop");
    let result = main_loop(
        app_state,
        event_queue,
        Arc::clone(&egl_state),
        Arc::clone(&mpv_state),
        render_wakeup_rx,
        signal_rx,
        Arc::clone(&halt_info),
    );

    // Shutdown threads
    thread_handles.shutdown_all(&halt_info);

    // Check if we should exec holder
    if halt_info.should_exec_holder() {
        // Save playback position
        let save_info = mpv_state.get_save_info().ok();
        if let Some(ref info) = save_info {
            cflp_info(1, args.verbose, &format!("Saving position: {}", info));
        }

        // Exec holder to maintain layer surfaces
        if let Err(e) = exec_holder(args, save_info.as_deref()) {
            cflp_error(&format!("Failed to exec holder: {}", e));
            // Fall through to normal exit if exec fails
        }
        // exec_holder calls execv - if we reach here, it failed
    }

    result
}

/// Execute holder process to maintain layer surfaces during auto-stop
///
/// This function replaces the current process with mpvpaper-rs-holder.
/// On success, this function never returns (execv replaces process).
fn exec_holder(args: &Args, save_info: Option<&str>) -> Result<()> {
    // Get path to holder binary
    let exe_path = std::fs::read_link("/proc/self/exe")
        .map_err(|e| AppError::Config(format!("Failed to read /proc/self/exe: {}", e)))?;

    // Derive holder path from mpvpaper-rs path
    // main:   /path/to/mpvpaper-rs
    // holder: /path/to/mpvpaper-rs-holder
    let parent = exe_path
        .parent()
        .ok_or_else(|| AppError::Config("Cannot determine parent directory".into()))?;

    let holder_path = parent.join("mpvpaper-rs-holder");

    if !holder_path.exists() {
        return Err(AppError::Config(format!(
            "mpvpaper-rs-holder not found at: {}",
            holder_path.display()
        )));
    }

    // Build argument list
    let mut c_args: Vec<CString> = Vec::new();

    // argv[0] = program name
    let prog_name = CString::new(holder_path.as_os_str().as_bytes())
        .map_err(|e| AppError::Config(format!("Invalid program path: {}", e)))?;
    c_args.push(prog_name);

    // Reconstruct arguments for holder
    // Verbose flags
    for _ in 0..args.verbose {
        c_args.push(CString::new("-v").unwrap());
    }

    // Auto-pause
    if args.auto_pause {
        c_args.push(CString::new("-p").unwrap());
    }

    // Auto-stop (required to be passed back when reviving)
    if args.auto_stop {
        c_args.push(CString::new("-s").unwrap());
    }

    // Slideshow
    if let Some(secs) = args.slideshow {
        c_args.push(CString::new("-n").unwrap());
        c_args.push(CString::new(secs.to_string()).unwrap());
    }

    // Layer
    let layer_str = match args.layer {
        LayerArg::Background => "background",
        LayerArg::Bottom => "bottom",
        LayerArg::Top => "top",
        LayerArg::Overlay => "overlay",
    };
    c_args.push(CString::new("-l").unwrap());
    c_args.push(CString::new(layer_str).unwrap());

    // Preserve --no-mpv-config across the main → holder transition so the
    // value is round-tripped if/when holder revives mpvpaper-rs.
    if args.no_mpv_config {
        c_args.push(CString::new("--no-mpv-config").unwrap());
    }

    // MPV options
    if let Some(ref opts) = args.mpv_options {
        c_args.push(CString::new("-o").unwrap());
        c_args.push(
            CString::new(opts.as_str())
                .map_err(|e| AppError::Config(format!("Invalid MPV options: {}", e)))?,
        );
    }

    // Save info (playback position) - passed to holder via -Z
    // Holder will pass this back when reviving mpvpaper-rs
    if let Some(info) = save_info {
        c_args.push(CString::new("-Z").unwrap());
        c_args.push(
            CString::new(info)
                .map_err(|e| AppError::Config(format!("Invalid save info: {}", e)))?,
        );
    }

    // Output (required)
    if let Some(ref output) = args.output {
        c_args.push(
            CString::new(output.as_str())
                .map_err(|e| AppError::Config(format!("Invalid output name: {}", e)))?,
        );
    }

    // Video path (required unless playlist)
    if let Some(ref video) = args.video_path {
        c_args.push(
            CString::new(video.as_str())
                .map_err(|e| AppError::Config(format!("Invalid video path: {}", e)))?,
        );
    }

    cflp_info(
        1,
        args.verbose,
        &format!(
            "Exec holder: {} with {} args",
            holder_path.display(),
            c_args.len()
        ),
    );

    // execv replaces this process
    let path_cstr = CString::new(holder_path.as_os_str().as_bytes())
        .map_err(|e| AppError::Config(format!("Invalid program path for execv: {}", e)))?;

    nix::unistd::execv(&path_cstr, &c_args)
        .map_err(|e| AppError::Config(format!("execv failed: {}", e)))?;

    // execv never returns on success
    unreachable!()
}

/// Main event loop using calloop
///
/// CRITICAL: This function takes ownership of all state and ensures correct drop order:
/// MPV render context → EGL surfaces/context → Wayland connection
///
/// The mpv_state and egl_state Arc clones passed to the render callback are moved
/// into the closure, but we keep the original Arcs to drop them explicitly at the end.
fn main_loop(
    mut app_state: AppState,
    event_queue: EventQueue<AppState>,
    egl_state: Arc<EglState>,
    mpv_state: Arc<MpvState>,
    render_wakeup_rx: Channel<()>,
    signal_rx: Channel<()>,
    halt_info: Arc<HaltInfo>,
) -> Result<()> {
    let mut event_loop: EventLoop<AppState> =
        EventLoop::try_new().map_err(|e| AppError::Config(format!("Failed to create event loop: {}", e)))?;
    let loop_signal = event_loop.get_signal();
    let handle = event_loop.handle();

    // Register Wayland source
    // WaylandSource handles dispatching the Wayland event queue automatically
    let wayland_source = WaylandSource::new(app_state.conn.clone(), event_queue);
    wayland_source
        .insert(handle.clone())
        .map_err(|e| AppError::Config(format!("Failed to insert Wayland source: {}", e)))?;

    // Register render wakeup channel
    let mpv_for_render = Arc::clone(&mpv_state);
    let egl_for_render = Arc::clone(&egl_state);
    let halt_for_render = Arc::clone(&halt_info);
    let signal_for_render = loop_signal.clone();

    handle
        .insert_source(render_wakeup_rx, move |event, _, state: &mut AppState| {
            if let ChannelEvent::Msg(()) = event {
                // CRITICAL: Always call poll_render_update() on every wakeup
                // MPV_RENDER_PARAM_ADVANCED_CONTROL requires this:
                // - Wakeup callback may be called even when no new frame is ready
                // - If mpv_render_context_update() is not called, mpv core may block
                let needs_render = mpv_for_render.poll_render_update();

                if !needs_render {
                    return; // No new frame - just state update
                }

                // MPV_RENDER_UPDATE_FRAME flag is set - render to all outputs
                for output in state.outputs.values_mut() {
                    if output.egl_surface.is_some() {
                        if let Err(e) = render_frame(&mpv_for_render, &egl_for_render, output) {
                            cflp_error(&format!("Render error: {}", e));
                            halt_for_render.stop_render_loop.store(true, Ordering::SeqCst);
                            signal_for_render.stop();
                            return;
                        }

                        // Request frame callback for next frame
                        // This ensures CompositorHandler::frame() is called,
                        // which is required for the deadman switch (auto_pause/auto_stop)
                        if let Some(layer_surface) = &output.layer_surface {
                            use smithay_client_toolkit::shell::WaylandSurface;
                            let wl_surface = layer_surface.wl_surface();
                            wl_surface.frame(&state.qh, wl_surface.clone());
                            wl_surface.commit();
                        }
                    }
                }
            }
        })
        .map_err(|e| AppError::Config(format!("Failed to insert render channel: {}", e)))?;

    // Register signal channel
    let halt_for_signal = Arc::clone(&halt_info);
    let signal_for_signal = loop_signal.clone();
    handle
        .insert_source(signal_rx, move |event, _, _state: &mut AppState| {
            if let ChannelEvent::Msg(()) = event {
                // Signal received - stop the render loop and mark as signal exit
                // This prevents transitioning to holder on SIGINT/SIGTERM
                halt_for_signal.signal_exit.store(true, Ordering::SeqCst);
                halt_for_signal.stop_render_loop.store(true, Ordering::SeqCst);
                signal_for_signal.stop();
            }
        })
        .map_err(|e| AppError::Config(format!("Failed to insert signal channel: {}", e)))?;

    // Main loop. The dispatch timeout only governs how often we re-check
    // `stop_render_loop` (set by auto_stop / stoplist worker threads). All
    // active event sources — Wayland fd, render wakeup channel, signal channel —
    // wake the dispatch immediately on activity, so a long timeout doesn't
    // affect rendering responsiveness. SIGINT/SIGTERM go through signal_rx
    // which wakes dispatch instantly; only auto_stop transition latency is
    // bounded by this timeout (1s is acceptable for a holder switch).
    while !halt_info.stop_render_loop.load(Ordering::SeqCst) {
        event_loop
            .dispatch(Duration::from_secs(1), &mut app_state)
            .map_err(|e| AppError::Config(format!("Event loop dispatch error: {}", e)))?;
    }

    // CRITICAL: Explicit drop order to prevent use-after-free
    // Drop order MUST be: MPV → EGL surfaces (in outputs) → EGL state → Wayland
    //
    // 1. Drop MPV render context first (uses EGL/GL context)
    drop(mpv_state);

    // 2. Drop EGL surfaces in outputs (each output has egl_surface, egl_window)
    //    This is handled by DisplayOutput::drop() which clears surfaces first
    for output in app_state.outputs.values_mut() {
        output.egl_surface = None;
        output.egl_window = None;
    }

    // 3. Drop EGL state (display, context)
    drop(egl_state);

    // 4. app_state (Wayland connection) drops automatically when function returns

    Ok(())
}

/// Setup signal handlers for graceful shutdown
///
/// Handles SIGINT (Ctrl+C) and SIGTERM (systemd stop)
fn setup_signal_handlers(halt_info: Arc<HaltInfo>, signal_tx: Sender<()>) -> Result<()> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;

    // Handle SIGINT and SIGTERM with signal-hook iterator
    let tx_clone = signal_tx.clone();
    std::thread::spawn(move || {
        let mut signals = Signals::new([SIGINT, SIGTERM])
            .expect("Failed to create signal iterator");

        // signals is an iterator - use &mut for iteration
        for signal in &mut signals {
            match signal as libc::c_int {
                SIGINT | SIGTERM => {
                    // Send to channel to notify main loop
                    let _ = tx_clone.send(());
                    break;
                }
                _ => {}
            }
        }
    });

    // Also handle SIGINT with ctrlc for backup (terminal Ctrl+C)
    let halt_for_ctrlc = Arc::clone(&halt_info);
    ctrlc::set_handler(move || {
        // Mark as signal exit to prevent holder transition
        halt_for_ctrlc.signal_exit.store(true, Ordering::SeqCst);
        halt_for_ctrlc.stop_render_loop.store(true, Ordering::SeqCst);
        // Also try to send via channel
        let _ = signal_tx.send(());
    })
    .map_err(|e| AppError::Config(format!("Failed to set SIGINT handler: {}", e)))?;

    Ok(())
}

/// Setup watchlists from config files
fn setup_watchlists(halt_info: Arc<HaltInfo>, args: &Args) -> Result<Arc<HaltInfo>> {
    let mut halt = Arc::try_unwrap(halt_info).unwrap_or_else(|arc| (*arc).clone());

    // Load pauselist
    let pauselist_path = get_pauselist_path();
    if pauselist_path.exists() {
        if let Ok(list) = load_list_file(&pauselist_path) {
            if !list.is_empty() {
                cflp_info(
                    1,
                    args.verbose,
                    &format!("Loaded {} entries from pauselist", list.len()),
                );
                halt.pauselist = Some(list);
            }
        }
    }

    // Load stoplist
    let stoplist_path = get_stoplist_path();
    if stoplist_path.exists() {
        if let Ok(list) = load_list_file(&stoplist_path) {
            if !list.is_empty() {
                cflp_info(
                    1,
                    args.verbose,
                    &format!("Loaded {} entries from stoplist", list.len()),
                );
                halt.stoplist = Some(list);
            }
        }
    }

    Ok(Arc::new(halt))
}
