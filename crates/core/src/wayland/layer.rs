use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use wayland_client::protocol::wl_output;
use wayland_client::{Connection, Proxy, QueueHandle};

use crate::logging::cflp_error;

use super::state::AppState;

/// Create a layer surface for the given output
pub fn create_layer_surface(
    state: &mut AppState,
    qh: &QueueHandle<AppState>,
    output: &wl_output::WlOutput,
    layer: Layer,
) -> LayerSurface {
    let surface = state.compositor_state.create_surface(qh);

    let layer_surface = state.layer_shell.create_layer_surface(
        qh,
        surface,
        layer,
        Some("mpvpaper-rs"),
        Some(output),
    );

    // Set empty input region (clicks pass through)
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer_surface.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);

    // exclusive_zone setting:
    // -1: Does not move to accommodate other layers, extends to anchored edges
    //     -> Covers "over" panels/bars (wallpaper occupies entire screen)
    //  0: "Avoids" layers that have reserved exclusive zone (placed below panels)
    // For wallpaper use, we use -1 (background should show even below panels)
    layer_surface.set_exclusive_zone(-1);

    // CRITICAL: Initial commit without buffer (requests configure)
    // Do not attach buffer until configure event arrives
    layer_surface.commit();

    layer_surface
}

impl LayerShellHandler for AppState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        // Layer surface closed by compositor
        // Find and remove the corresponding output
        let surface_id = layer.wl_surface().id();
        if let Some(output_id) = self
            .outputs
            .iter()
            .find(|(_, o)| {
                o.layer_surface
                    .as_ref()
                    .map(|ls| ls.wl_surface().id() == surface_id)
                    .unwrap_or(false)
            })
            .map(|(id, _)| id.clone())
        {
            self.outputs.remove(&output_id);
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        // Note: ack_configure is automatically called by SCTK's dispatch layer

        // 1. Extract size
        // Note: width/height == 0 means "use size requested by client"
        // In that case, substitute with output size or previously requested size
        let (mut width, mut height) = configure.new_size;
        if width == 0 || height == 0 {
            // Get size from output info
            if let Some(output_size) = self.get_output_size_for_layer(layer) {
                width = output_size.0;
                height = output_size.1;
            } else {
                // No output size either - log error and return
                cflp_error("Configure with zero size and no output info available");
                return;
            }
        }

        // 3. Update DisplayOutput with configured size
        let surface_id = layer.wl_surface().id();
        if let Some(display_output) = self.outputs.values_mut().find(|o| {
            o.layer_surface
                .as_ref()
                .map(|ls| ls.wl_surface().id() == surface_id)
                .unwrap_or(false)
        }) {
            // Check if size changed and EGL window exists
            let size_changed = display_output.width != width || display_output.height != height;

            if size_changed && display_output.egl_window.is_some() {
                // Resize existing EGL window
                display_output.resize_egl_window(width, height);
            } else {
                // Initial setup or no EGL yet - just update dimensions
                display_output.width = width;
                display_output.height = height;
                display_output.redraw_needed = true;
            }
        }

        // 4. Commit surface to acknowledge configure
        layer.wl_surface().commit();
    }
}

impl AppState {
    /// Get output size for a layer surface
    pub fn get_output_size_for_layer(&self, layer: &LayerSurface) -> Option<(u32, u32)> {
        let surface_id = layer.wl_surface().id();
        self.outputs
            .values()
            .find(|o| {
                o.layer_surface
                    .as_ref()
                    .map(|ls| ls.wl_surface().id() == surface_id)
                    .unwrap_or(false)
            })
            .filter(|o| o.width > 0 && o.height > 0)
            .map(|o| (o.width, o.height))
    }
}
