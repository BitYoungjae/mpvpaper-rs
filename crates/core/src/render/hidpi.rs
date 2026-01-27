use wayland_client::protocol::wl_surface;
use wayland_egl::WlEglSurface;

use crate::wayland::DisplayOutput;

pub fn configure_hidpi_surface(surface: &wl_surface::WlSurface, output: &DisplayOutput) {
    let scale = output.scale.max(1) as u32;
    surface.set_buffer_scale(output.scale);

    let width = scaled_dimension_i32(output.width, scale);
    let height = scaled_dimension_i32(output.height, scale);
    surface.damage_buffer(0, 0, width, height);
}

pub fn resize_egl_window(egl_window: &mut WlEglSurface, output: &DisplayOutput) {
    let scale = output.scale.max(1) as u32;
    let width = scaled_dimension_i32(output.width, scale);
    let height = scaled_dimension_i32(output.height, scale);
    egl_window.resize(width, height, 0, 0);
}

fn scaled_dimension_i32(value: u32, scale: u32) -> i32 {
    let scaled = value.saturating_mul(scale);
    i32::try_from(scaled).unwrap_or(i32::MAX)
}
