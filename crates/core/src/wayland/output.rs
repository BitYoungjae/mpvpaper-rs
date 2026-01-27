use khronos_egl as egl;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use smithay_client_toolkit::shell::WaylandSurface;
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_client::{Connection, Proxy, QueueHandle};
use wayland_egl::WlEglSurface;

use crate::error::{AppError, Result};
use crate::render::egl::EglState;

use super::state::AppState;

/// Display output information and associated Wayland/EGL surfaces
#[derive(Debug)]
pub struct DisplayOutput {
    pub wl_output: wl_output::WlOutput,
    pub name: String,
    pub identifier: String,

    // Surfaces (None until configured)
    // Drop order matters: egl_surface -> egl_window -> layer_surface
    pub egl_surface: Option<egl::Surface>,
    pub egl_window: Option<WlEglSurface>,
    pub layer_surface: Option<LayerSurface>,
    pub surface: Option<wl_surface::WlSurface>,

    // Dimensions
    pub width: u32,
    pub height: u32,
    pub scale: i32,

    // Frame sync (will be used in Phase 6)
    pub redraw_needed: bool,
}

impl DisplayOutput {
    pub fn new(wl_output: wl_output::WlOutput) -> Self {
        Self {
            wl_output,
            name: String::new(),
            identifier: String::new(),
            egl_surface: None,
            egl_window: None,
            layer_surface: None,
            surface: None,
            width: 0,
            height: 0,
            scale: 1,
            redraw_needed: false,
        }
    }

    pub fn setup_egl(&mut self, egl_state: &EglState, width: u32, height: u32) -> Result<()> {
        let layer_surface = self.layer_surface.as_ref().ok_or_else(|| {
            AppError::EglInit("Layer surface not available for EGL setup".into())
        })?;

        self.egl_surface = None;
        self.egl_window = None;

        // The width/height from configure are already the buffer size we need
        // No need to scale again - the compositor has already accounted for scale
        let width_i32 =
            i32::try_from(width).map_err(|_| AppError::EglInit("Width too large".into()))?;
        let height_i32 = i32::try_from(height)
            .map_err(|_| AppError::EglInit("Height too large".into()))?;

        let wl_surface = layer_surface.wl_surface();
        let egl_window = WlEglSurface::new(wl_surface.id(), width_i32, height_i32)
            .map_err(|e| AppError::EglInit(format!("Failed to create WlEglSurface: {e}")))?;

        let egl_surface = egl_state.create_surface(&egl_window)?;

        self.egl_window = Some(egl_window);
        self.egl_surface = Some(egl_surface);

        Ok(())
    }

    /// Resize the EGL window to match new dimensions
    ///
    /// Called when the layer surface is reconfigured (e.g., output resize, scale change)
    pub fn resize_egl_window(&mut self, width: u32, height: u32) {
        if let Some(egl_window) = &self.egl_window {
            let width_i32 = width as i32;
            let height_i32 = height as i32;
            egl_window.resize(width_i32, height_i32, 0, 0);
            self.width = width;
            self.height = height;
            self.redraw_needed = true;
        }
    }
}

impl Default for DisplayOutput {
    fn default() -> Self {
        // This is only used when we have a placeholder wl_output
        // In practice, always use DisplayOutput::new(wl_output)
        panic!("DisplayOutput::default() should not be called - use DisplayOutput::new(wl_output)")
    }
}

impl OutputHandler for AppState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        // CRITICAL: Register output in OutputFrameState (required for auto_pause/auto_stop)
        // Without this call, output_frame_state map will be empty and
        // all_hidden() always returns false -> auto_pause/auto_stop never triggers
        self.halt_info.output_frame_state.add_output(output.id());

        // Extract info from OutputState
        // Note: info() returns None if output info hasn't arrived yet or output was destroyed
        // If None, register empty DisplayOutput and fill in update_output
        let display_output = if let Some(info) = self.output_state.info(&output) {
            DisplayOutput {
                wl_output: output.clone(),
                name: info.name.clone().unwrap_or_default(),
                identifier: info.description.clone().unwrap_or_default(),
                width: info.logical_size.map(|(w, _)| w as u32).unwrap_or(0),
                height: info.logical_size.map(|(_, h)| h as u32).unwrap_or(0),
                scale: info.scale_factor,
                egl_surface: None,
                egl_window: None,
                layer_surface: None,
                surface: None,
                redraw_needed: false,
            }
        } else {
            // info not yet available - register empty output, will be filled in update_output
            DisplayOutput::new(output.clone())
        };
        self.outputs.insert(output.id(), display_output);
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        // Output info update (scale change, etc.)
        // If info was None in new_output, it gets filled here
        if let Some(display_output) = self.outputs.get_mut(&output.id()) {
            if let Some(info) = self.output_state.info(&output) {
                display_output.name = info.name.clone().unwrap_or_default();
                display_output.identifier = info.description.clone().unwrap_or_default();
                if let Some((w, h)) = info.logical_size {
                    display_output.width = w as u32;
                    display_output.height = h as u32;
                }
                // Check if scale factor changed
                let scale_changed = display_output.scale != info.scale_factor;
                display_output.scale = info.scale_factor;

                // Mark for redraw if scale changed (compositor will send new configure)
                if scale_changed {
                    display_output.redraw_needed = true;
                }
            }
        }
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        // CRITICAL: Remove output from OutputFrameState (required for auto_pause/auto_stop accuracy)
        // If not removed, non-existent output's frame_ready state remains and
        // all_hidden() determination becomes inaccurate
        self.halt_info.output_frame_state.remove_output(&output.id());

        self.outputs.remove(&output.id());
    }
}

impl Drop for DisplayOutput {
    fn drop(&mut self) {
        self.egl_surface = None;
        self.egl_window = None;
        self.layer_surface = None;
        self.surface = None;
    }
}
