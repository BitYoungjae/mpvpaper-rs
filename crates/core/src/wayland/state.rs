use std::collections::HashMap;
use std::sync::Arc;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry,
    output::OutputState,
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::wlr_layer::LayerShell,
    shell::WaylandSurface,
};
use wayland_client::backend::ObjectId;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_client::{Connection, EventQueue, Proxy, QueueHandle};

use crate::control::HaltInfo;
use crate::error::{AppError, Result};

use super::output::DisplayOutput;

/// CRITICAL: Field declaration order affects Drop order
///
/// Rust structs are dropped in declaration order.
/// Connection::backend().display_ptr() is used by EGL/MPV,
/// so Connection must be dropped last.
///
/// Drop order (reverse):
/// 1. halt_info - thread synchronization state
/// 2. outputs - DisplayOutputs (containing EGL surfaces)
/// 3. layer_shell, output_state, etc. - Wayland state
/// 4. conn - Wayland connection (last)
pub struct AppState {
    // Wayland connection - declared first = dropped last
    // EGL/MPV use display_ptr() so conn must be dropped last
    pub conn: Connection,
    pub qh: QueueHandle<Self>,
    pub registry_state: RegistryState,
    pub compositor_state: CompositorState,
    pub output_state: OutputState,
    pub layer_shell: LayerShell,
    // outputs contain EGL surfaces, so drop before conn
    pub outputs: HashMap<ObjectId, DisplayOutput>,
    pub halt_info: Arc<HaltInfo>,
}

impl AppState {
    /// Create new AppState and return with EventQueue
    /// EventQueue is returned to be passed to WaylandSource
    pub fn new() -> Result<(Self, EventQueue<Self>)> {
        let conn = Connection::connect_to_env()?;
        let (globals, mut event_queue) = registry_queue_init(&conn)?;
        let qh = event_queue.handle();

        let registry_state = RegistryState::new(&globals);
        let compositor_state = CompositorState::bind(&globals, &qh)?;
        let output_state = OutputState::new(&globals, &qh);

        // Layer shell binding failure with clear error message
        let layer_shell =
            LayerShell::bind(&globals, &qh).map_err(|_| AppError::LayerShellNotSupported)?;

        let mut state = Self {
            conn,
            qh,
            registry_state,
            compositor_state,
            output_state,
            layer_shell,
            outputs: HashMap::new(),
            halt_info: Arc::new(HaltInfo::default()),
        };

        // CRITICAL: Initial event processing needed to collect output list
        // OutputState info/outputs are only populated after event delegation
        // roundtrip waits until all output info is received from server
        event_queue.roundtrip(&mut state)?;

        Ok((state, event_queue))
    }

    /// Create AppState with custom HaltInfo
    pub fn with_halt_info(halt_info: Arc<HaltInfo>) -> Result<(Self, EventQueue<Self>)> {
        let (mut state, event_queue) = Self::new()?;
        state.halt_info = halt_info;
        Ok((state, event_queue))
    }
}

/// Output selection function - MUST be called after roundtrip
///
/// Matching order:
/// 1. Wildcard ("*", "all", "ALL")
/// 2. Output name (DP-2, HDMI-A-1, etc.) - requires wl_output v4 or xdg-output
/// 3. Output description (make model serial)
/// 4. Numeric index ("0", "1", "2", etc.) - fallback
/// 5. wl_output global id matching - final fallback
///
/// selector takes &str, so when calling from Option<String>:
///   select_output(&state.output_state, args.output.as_deref().unwrap_or("*"))
/// Or validate first then call:
///   let selector = args.output.as_ref().ok_or(AppError::Config("output required"))?;
///   select_output(&state.output_state, selector)
pub fn select_output(output_state: &OutputState, selector: &str) -> Result<wl_output::WlOutput> {
    // Query current known outputs via OutputState::outputs()
    let outputs: Vec<_> = output_state.outputs().collect();

    if outputs.is_empty() {
        return Err(AppError::OutputNotFound(
            "No outputs available (ensure roundtrip completed)".into(),
        ));
    }

    // Wildcards are handled by select_outputs() function (returns Vec)
    // Use this function for single output selection
    if selector == "*" || selector.eq_ignore_ascii_case("all") {
        // Wildcards should use select_outputs()
        return Err(AppError::Config(
            "Use select_outputs() for wildcard selectors (ALL/*)".into(),
        ));
    }

    // Match by output name/identifier
    for output in &outputs {
        if let Some(info) = output_state.info(output) {
            // Name matching (DP-2, HDMI-A-1, etc.)
            // Note: None if wl_output v4 or xdg-output-unstable-v1 not supported
            if info.name.as_deref() == Some(selector) {
                return Ok(output.clone());
            }
            // Identifier matching (make model serial)
            if info.description.as_deref() == Some(selector) {
                return Ok(output.clone());
            }
        }
    }

    // Fallback 1: Match by numeric index (0, 1, 2, etc.)
    if let Ok(index) = selector.parse::<usize>() {
        if index < outputs.len() {
            return Ok(outputs[index].clone());
        }
    }

    // Fallback 2: Match by wl_output global id
    // OutputInfo.id corresponds to wl_output global name, always available
    for output in &outputs {
        if let Some(info) = output_state.info(output) {
            // Compare id as string
            if info.id.to_string() == selector {
                return Ok(output.clone());
            }
        }
    }

    Err(AppError::OutputNotFound(selector.into()))
}

/// Multi-output selection function (ALL/* wildcard support)
///
/// Original mpvpaper behavior:
/// - ALL/*: Play on all outputs simultaneously
/// - Specific name: Select only that output
///
/// Returns: Vec of all matching outputs
pub fn select_outputs(
    output_state: &OutputState,
    selector: &str,
) -> Result<Vec<wl_output::WlOutput>> {
    let outputs: Vec<_> = output_state.outputs().collect();

    if outputs.is_empty() {
        return Err(AppError::OutputNotFound(
            "No outputs available (ensure roundtrip completed)".into(),
        ));
    }

    // Wildcard: return all outputs
    if selector == "*" || selector.eq_ignore_ascii_case("all") {
        return Ok(outputs);
    }

    // Specific output matching (original mpvpaper logic)
    // Uses strstr - substring matching (DP-1 also matches "DP-1-extra")
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
        Err(AppError::OutputNotFound(selector.into()))
    } else {
        Ok(matched)
    }
}

/// Get formatted list of all available outputs for display
pub fn list_outputs(output_state: &OutputState) -> Vec<String> {
    let outputs: Vec<_> = output_state.outputs().collect();
    let mut result = Vec::new();

    for (index, output) in outputs.iter().enumerate() {
        if let Some(info) = output_state.info(output) {
            let name = info.name.as_deref().unwrap_or("(unknown)");
            let desc = info.description.as_deref().unwrap_or("");
            let size = info
                .logical_size
                .map(|(w, h)| format!("{}x{}", w, h))
                .unwrap_or_else(|| "?x?".to_string());
            let scale = info.scale_factor;

            result.push(format!(
                "[{}] {} - {} ({}@{}x)",
                index, name, desc, size, scale
            ));
        } else {
            result.push(format!("[{}] (info not available)", index));
        }
    }

    result
}

// SCTK 0.20 delegate macros
delegate_registry!(AppState);
delegate_compositor!(AppState);
delegate_output!(AppState);
delegate_layer!(AppState);

impl ProvidesRegistryState for AppState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

impl CompositorHandler for AppState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
        // HiDPI scale change handling - will be implemented in Phase 3
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        // Frame callback - set deadman switch (using per-output state)
        // Find the output ObjectId from surface and call mark_ready
        if let Some(output) = self.outputs.values().find(|o| {
            o.layer_surface
                .as_ref()
                .map(|ls| ls.wl_surface().id() == surface.id())
                .unwrap_or(false)
        }) {
            self.halt_info
                .output_frame_state
                .mark_ready(&output.wl_output.id());
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _transform: wl_output::Transform,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}
