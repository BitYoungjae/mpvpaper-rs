use crate::logging::cflp_success;
use crate::render::egl::EglState;

/// Load GL functions using the EGL instance proc loader.
pub fn load_gl_functions(egl_state: &EglState) {
    gl::load_with(|name| {
        egl_state
            .instance
            .get_proc_address(name)
            .map(|p| p as *const std::ffi::c_void)
            .unwrap_or(std::ptr::null())
    });
    cflp_success("Loaded OpenGL function pointers");
}
