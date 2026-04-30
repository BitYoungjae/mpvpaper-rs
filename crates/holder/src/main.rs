//! mpvpaper-rs-holder: Lightweight holder process
//!
//! Maintains transparent layer surfaces while mpvpaper-rs is stopped (stoplist).
//! When stoplist processes exit, revives mpvpaper-rs with saved playback position.

use std::collections::HashSet;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::time::Duration;

use anyhow::{Context, Result};
use mpvpaper_rs_core::cli::Args;
use mpvpaper_rs_core::config::{get_stoplist_path, load_list_file};
use mpvpaper_rs_core::logging::{cflp_error, cflp_info, cflp_success};
use mpvpaper_rs_core::process::check_watch_list;
use clap::Parser;
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    registry_handlers,
};
use wayland_client::backend::ObjectId;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_buffer, wl_output, wl_shm};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

/// HolderState - minimal Wayland state for holding layer surfaces
struct HolderState {
    registry_state: RegistryState,
    compositor_state: CompositorState,
    output_state: OutputState,
    layer_shell: LayerShell,
    shm: Shm,

    /// Track configure-completed surfaces (HashSet for deduplication)
    /// Wayland configure events can arrive multiple times for the same surface
    configured_surfaces: HashSet<ObjectId>,

    /// CRITICAL: SlotPool owns the backing memory for buffers.
    /// If pool is dropped while buffers are still attached, the compositor
    /// will read from unmapped memory -> undefined behavior / crash.
    /// Keep pool alive in state until holder exits.
    slot_pool: Option<SlotPool>,

    /// CRITICAL: wl_buffer must be kept alive until compositor sends release event.
    /// Dropping buffer before release causes undefined behavior.
    active_buffers: Vec<wl_buffer::WlBuffer>,
}

// SCTK delegate macros
delegate_registry!(HolderState);
delegate_compositor!(HolderState);
delegate_output!(HolderState);
delegate_layer!(HolderState);
delegate_shm!(HolderState);

impl ProvidesRegistryState for HolderState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

// CRITICAL: OutputHandler must be implemented for OutputState to populate outputs
impl OutputHandler for HolderState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        // Holder only maintains surfaces - minimal output tracking
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        // Ignore updates
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        // Could trigger holder exit/restart - for now, ignore
    }
}

impl LayerShellHandler for HolderState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        // Layer closed by compositor
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        _configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // Note: SCTK 0.20 automatically calls ack_configure in dispatch layer

        // Track configured surface (HashSet ensures each surface counted once)
        // Even if multiple configure events arrive for the same surface (resize, etc.),
        // HashSet::insert returns false for duplicates
        self.configured_surfaces.insert(layer.wl_surface().id());
    }
}

impl ShmHandler for HolderState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

// CompositorHandler is required by delegate_compositor!
impl CompositorHandler for HolderState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wayland_client::protocol::wl_surface::WlSurface,
        _new_factor: i32,
    ) {
        // Holder uses 1x1 transparent pixel - scale doesn't matter
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wayland_client::protocol::wl_surface::WlSurface,
        _time: u32,
    ) {
        // No-op for holder - we don't need frame callbacks
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wayland_client::protocol::wl_surface::WlSurface,
        _transform: wl_output::Transform,
    ) {
        // No-op
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wayland_client::protocol::wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
        // No-op
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wayland_client::protocol::wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
        // No-op
    }
}

// wl_buffer release event handler
// This requires manual Dispatch implementation since SCTK doesn't provide delegate_buffer
impl Dispatch<wl_buffer::WlBuffer, ()> for HolderState {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            // Compositor is done with this buffer - safe to drop
            state.active_buffers.retain(|b| b.id() != buffer.id());
        }
    }
}

fn main() {
    if let Err(e) = run() {
        cflp_error(&format!("Holder error: {}", e));
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();

    cflp_info(1, args.verbose, "Starting mpvpaper-rs-holder...");

    // Wayland connection (SCTK 0.20)
    let conn = Connection::connect_to_env().context("Failed to connect to Wayland display")?;
    let (globals, mut event_queue) =
        registry_queue_init(&conn).context("Failed to initialize registry")?;
    let qh = event_queue.handle();

    // Initialize SCTK states
    let registry_state = RegistryState::new(&globals);
    let compositor_state =
        CompositorState::bind(&globals, &qh).context("Failed to bind compositor")?;
    let output_state = OutputState::new(&globals, &qh);
    let layer_shell = LayerShell::bind(&globals, &qh).context("Layer shell not supported")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm not available")?;

    let mut state = HolderState {
        registry_state,
        compositor_state,
        output_state,
        layer_shell,
        shm,
        configured_surfaces: HashSet::new(),
        slot_pool: None,
        active_buffers: Vec::new(),
    };

    // CRITICAL: Roundtrip to collect output list
    // OutputState info/outputs are only populated after event delegation
    event_queue
        .roundtrip(&mut state)
        .context("Initial roundtrip failed")?;

    // Output selection - holder requires explicit output
    let output_name = args
        .output
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Output name is required for holder"))?;

    // Select outputs (ALL/* supported)
    let outputs = select_outputs(&state.output_state, output_name)?;

    if outputs.is_empty() {
        return Err(anyhow::anyhow!("No outputs matched selector: {}", output_name));
    }

    cflp_info(
        1,
        args.verbose,
        &format!("Creating layer surfaces for {} output(s)", outputs.len()),
    );

    // Convert args.layer to SCTK Layer
    let layer = match args.layer {
        mpvpaper_rs_core::cli::LayerArg::Background => Layer::Background,
        mpvpaper_rs_core::cli::LayerArg::Bottom => Layer::Bottom,
        mpvpaper_rs_core::cli::LayerArg::Top => Layer::Top,
        mpvpaper_rs_core::cli::LayerArg::Overlay => Layer::Overlay,
    };

    // Create layer surface for each output
    let mut layer_surfaces = Vec::new();
    for output in &outputs {
        let surface = state.compositor_state.create_surface(&qh);
        let layer_surface = state.layer_shell.create_layer_surface(
            &qh,
            surface,
            layer,
            Some("mpvpaper-rs-holder"),
            Some(output),
        );

        // Full screen coverage
        layer_surface.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.commit();

        layer_surfaces.push(layer_surface);
    }

    // Wait for all configure events
    // HashSet ensures each surface counted only once
    while state.configured_surfaces.len() < layer_surfaces.len() {
        event_queue
            .blocking_dispatch(&mut state)
            .context("Dispatch failed while waiting for configure")?;
    }

    cflp_success("All layer surfaces configured");

    // Create transparent buffer (ARGB8888 format for alpha support)
    // CRITICAL: SlotPool must be stored in state to keep backing memory alive
    let pool_size = 4 * layer_surfaces.len(); // 4 bytes (ARGB8888) per 1x1 pixel per surface
    let mut pool =
        SlotPool::new(pool_size, &state.shm).context("Failed to create SHM slot pool")?;

    // Attach transparent buffer to each layer surface
    for layer_surface in &layer_surfaces {
        // Create 1x1 transparent buffer (ARGB8888: 0x00000000 = fully transparent)
        let (buffer, canvas) = pool
            .create_buffer(1, 1, 4, wl_shm::Format::Argb8888)
            .context("Failed to create buffer")?;

        // Fill with transparent pixel (ARGB = 0x00000000)
        canvas.fill(0);

        let wl_surface = layer_surface.wl_surface();
        buffer
            .attach_to(wl_surface)
            .context("Failed to attach buffer")?;
        wl_surface.damage_buffer(0, 0, 1, 1);
        wl_surface.commit();

        // Keep buffer alive until compositor releases it
        state.active_buffers.push(buffer.wl_buffer().clone());
    }

    // CRITICAL: Store pool in state to keep backing memory valid
    // If pool is dropped, attached buffers become invalid
    state.slot_pool = Some(pool);

    cflp_success("Transparent surfaces attached");

    // Load stoplist for revive check
    let stoplist = load_stoplist();

    // Main loop - check stoplist every second
    cflp_info(1, args.verbose, "Entering main loop, monitoring stoplist...");

    loop {
        // Check if we should revive mpvpaper-rs
        if should_revive(&stoplist) {
            cflp_info(1, args.verbose, "Stoplist clear - reviving mpvpaper-rs");
            revive_mpvpaper_rs(&args)?;
            // revive_mpvpaper_rs calls execv - never returns
            unreachable!();
        }

        // Process pending Wayland events
        event_queue
            .dispatch_pending(&mut state)
            .context("Dispatch pending failed")?;

        // Sleep before next check
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Select outputs matching selector (ALL/* for all outputs)
fn select_outputs(
    output_state: &OutputState,
    selector: &str,
) -> Result<Vec<wl_output::WlOutput>> {
    let outputs: Vec<_> = output_state.outputs().collect();

    if outputs.is_empty() {
        return Err(anyhow::anyhow!(
            "No outputs available (ensure roundtrip completed)"
        ));
    }

    // Wildcard: return all outputs
    if selector == "*" || selector.eq_ignore_ascii_case("all") {
        return Ok(outputs);
    }

    // Match by name or description
    let mut matched = Vec::new();
    for output in &outputs {
        if let Some(info) = output_state.info(output) {
            let name_match = info
                .name
                .as_ref()
                .map(|n| n.contains(selector) || selector.contains(n.as_str()))
                .unwrap_or(false);
            let desc_match = info
                .description
                .as_ref()
                .map(|d| d.contains(selector) || selector.contains(d.as_str()))
                .unwrap_or(false);

            if name_match || desc_match {
                matched.push(output.clone());
            }
        }
    }

    // Numeric index fallback
    if matched.is_empty() {
        if let Ok(index) = selector.parse::<usize>() {
            if index < outputs.len() {
                return Ok(vec![outputs[index].clone()]);
            }
        }
    }

    if matched.is_empty() {
        Err(anyhow::anyhow!("No outputs matched selector: {}", selector))
    } else {
        Ok(matched)
    }
}

/// Load stoplist from config file
fn load_stoplist() -> Vec<String> {
    let stoplist_path = get_stoplist_path();
    if stoplist_path.exists() {
        load_list_file(&stoplist_path).unwrap_or_default()
    } else {
        Vec::new()
    }
}

/// Check if mpvpaper-rs should be revived
/// Returns true when none of the stoplist processes are running
fn should_revive(stoplist: &[String]) -> bool {
    if stoplist.is_empty() {
        // No stoplist configured - stay in holder mode
        // This could happen if stoplist was deleted while holder was running
        return false;
    }

    // Revive when NO stoplist processes are running
    check_watch_list(stoplist).is_none()
}

/// Replace this process with mpvpaper-rs using execv
fn revive_mpvpaper_rs(args: &Args) -> Result<()> {
    // Get path to mpvpaper-rs binary
    let exe_path = std::fs::read_link("/proc/self/exe")
        .context("Failed to read /proc/self/exe")?;

    // Derive mpvpaper-rs path from holder path
    // holder: /path/to/mpvpaper-rs-holder
    // main:   /path/to/mpvpaper-rs
    let parent = exe_path.parent().ok_or_else(|| {
        anyhow::anyhow!("Cannot determine parent directory of holder executable")
    })?;

    let mpvpaper_rs_path = parent.join("mpvpaper-rs");

    if !mpvpaper_rs_path.exists() {
        return Err(anyhow::anyhow!(
            "mpvpaper-rs not found at: {}",
            mpvpaper_rs_path.display()
        ));
    }

    // Build argument list
    let mut c_args: Vec<CString> = Vec::new();

    // argv[0] = program name
    let prog_name = CString::new(mpvpaper_rs_path.as_os_str().as_bytes())
        .context("Invalid program path")?;
    c_args.push(prog_name);

    // Reconstruct original arguments
    // Verbose flags
    for _ in 0..args.verbose {
        c_args.push(CString::new("-v").unwrap());
    }

    // Auto-pause
    if args.auto_pause {
        c_args.push(CString::new("-p").unwrap());
    }

    // Auto-stop (this is what brought us to holder)
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
        mpvpaper_rs_core::cli::LayerArg::Background => "background",
        mpvpaper_rs_core::cli::LayerArg::Bottom => "bottom",
        mpvpaper_rs_core::cli::LayerArg::Top => "top",
        mpvpaper_rs_core::cli::LayerArg::Overlay => "overlay",
    };
    c_args.push(CString::new("-l").unwrap());
    c_args.push(CString::new(layer_str).unwrap());

    // Preserve --no-mpv-config so the revived mpvpaper-rs has the same
    // config-loading behavior as the original invocation.
    if args.no_mpv_config {
        c_args.push(CString::new("--no-mpv-config").unwrap());
    }

    // MPV options
    if let Some(ref opts) = args.mpv_options {
        c_args.push(CString::new("-o").unwrap());
        c_args.push(CString::new(opts.as_str()).context("Invalid MPV options")?);
    }

    // Restore info (playback position) - passed via -Z
    if let Some(ref restore) = args.restore_info {
        c_args.push(CString::new("-Z").unwrap());
        c_args.push(CString::new(restore.as_str()).context("Invalid restore info")?);
    }

    // Output (required)
    if let Some(ref output) = args.output {
        c_args.push(CString::new(output.as_str()).context("Invalid output name")?);
    }

    // Video path (required unless playlist)
    if let Some(ref video) = args.video_path {
        c_args.push(CString::new(video.as_str()).context("Invalid video path")?);
    }

    cflp_info(
        2,
        args.verbose,
        &format!("Executing: {} with {} args", mpvpaper_rs_path.display(), c_args.len()),
    );

    // execv replaces this process
    let path_cstr = CString::new(mpvpaper_rs_path.as_os_str().as_bytes())
        .context("Invalid program path for execv")?;

    nix::unistd::execv(&path_cstr, &c_args).context("execv failed")?;

    // execv never returns on success
    unreachable!()
}
