use std::ffi::c_void;
use std::sync::atomic::Ordering;

use libmpv2_sys::{
    mpv_opengl_fbo, mpv_render_context_render, mpv_render_context_update, mpv_render_param,
    mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y, mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
    mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO, mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME,
};

use crate::error::Result;
use crate::render::egl::EglState;
use crate::wayland::output::DisplayOutput;

use super::context::MpvState;

impl MpvState {
    /// Poll for render updates - MUST be called on every wakeup
    ///
    /// CRITICAL: When MPV_RENDER_PARAM_ADVANCED_CONTROL is enabled:
    /// - Wakeup callback may be called even when no new frame is ready
    /// - If callback is received but mpv_render_context_update() is not called,
    ///   mpv core may block waiting for update
    /// - The MPV_RENDER_UPDATE_FRAME flag in the result determines if actual
    ///   rendering should be performed
    ///
    /// Call flow:
    /// 1. Receive wakeup callback -> signal via channel
    /// 2. Event loop receives signal -> call poll_render_update() (ALWAYS!)
    /// 3. If returns true (MPV_RENDER_UPDATE_FRAME set) -> call render()
    ///
    /// Returns true if a new frame should be rendered.
    pub fn poll_render_update(&self) -> bool {
        unsafe {
            let flags = mpv_render_context_update(self.render_ctx);
            self.last_update_flags.store(flags, Ordering::SeqCst);
            (flags & mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME as u64) != 0
        }
    }

    /// Render a frame to the default framebuffer
    ///
    /// Only call this when poll_render_update() returns true.
    ///
    /// # Arguments
    /// * `width` - Framebuffer width in pixels (after scaling)
    /// * `height` - Framebuffer height in pixels (after scaling)
    pub fn render(&self, width: i32, height: i32) -> Result<()> {
        let fbo = mpv_opengl_fbo {
            fbo: 0, // default framebuffer
            w: width,
            h: height,
            internal_format: 0, // 0 = auto-detect
        };

        // Flip Y for correct orientation
        let flip_y: i32 = 1;

        let params = [
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                data: &fbo as *const _ as *mut c_void,
            },
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                data: &flip_y as *const _ as *mut c_void,
            },
            mpv_render_param {
                type_: mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];

        unsafe {
            let result = mpv_render_context_render(self.render_ctx, params.as_ptr() as *mut _);
            if result < 0 {
                return Err(crate::error::AppError::Config(format!(
                    "MPV render failed: error code {}",
                    result
                )));
            }
        }

        Ok(())
    }
}

/// Render a frame for a specific output
///
/// This function:
/// 1. Makes the EGL context current for the output
/// 2. Sets the viewport
/// 3. Renders the MPV frame
/// 4. Swaps buffers
///
/// # Arguments
/// * `mpv_state` - MPV state with render context
/// * `egl_state` - EGL state for context management
/// * `output` - Display output to render to
pub fn render_frame(
    mpv_state: &MpvState,
    egl_state: &EglState,
    output: &mut DisplayOutput,
) -> Result<()> {
    let egl_surface = output.egl_surface.as_ref().ok_or_else(|| {
        crate::error::AppError::EglInit("EGL surface not available for rendering".into())
    })?;

    // width/height are already the scaled buffer size from configure event
    let width = output.width;
    let height = output.height;

    // Make EGL context current
    egl_state.make_current(egl_surface)?;

    // Set viewport
    unsafe {
        gl::Viewport(0, 0, width as i32, height as i32);
    }

    // Render MPV frame
    mpv_state.render(width as i32, height as i32)?;

    // Swap buffers
    egl_state.swap_buffers(egl_surface)?;

    // Mark that we've rendered
    output.redraw_needed = false;

    Ok(())
}

/// Check if a frame should be rendered for this output
///
/// Returns true if:
/// - The output needs redraw (redraw_needed flag)
/// - MPV has a new frame ready (checked via poll_render_update)
pub fn should_render(output: &DisplayOutput) -> bool {
    output.redraw_needed
}
