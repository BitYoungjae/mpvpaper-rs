use std::ffi::c_void;
use std::fmt;
use std::sync::Arc;

use khronos_egl as egl;
use libloading::Library;
use wayland_client::Connection;
use wayland_egl::WlEglSurface;

use crate::error::{AppError, Result};
use crate::logging::{cflp_error, cflp_success};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlApiType {
    OpenGL,
    OpenGLES,
}

pub struct EglState {
    pub instance: egl::Instance<egl::Dynamic<Library, egl::EGL1_5>>,
    pub display: egl::Display,
    pub config: egl::Config,
    pub context: egl::Context,
    pub api_type: GlApiType,
}

pub struct EglSurface {
    surface: Option<egl::Surface>,
    egl_state: Arc<EglState>,
}

impl EglSurface {
    pub fn new(surface: egl::Surface, egl_state: Arc<EglState>) -> Self {
        Self {
            surface: Some(surface),
            egl_state,
        }
    }

    pub fn raw(&self) -> &egl::Surface {
        self.surface
            .as_ref()
            .expect("EGL surface handle should be present until drop")
    }
}

impl fmt::Debug for EglSurface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EglSurface")
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}

impl Drop for EglSurface {
    fn drop(&mut self) {
        if let Some(surface) = self.surface.take() {
            if let Err(e) = self.egl_state.destroy_surface(surface) {
                cflp_error(&format!("Failed to destroy EGL surface: {e:?}"));
            }
        }
    }
}

// SAFETY: EglState is only accessed from the main render thread (where the
// EGL context is made current). Worker threads receive `Arc<EglState>`
// transitively (via captured closures in MpvState) but never invoke EglState
// methods or dereference its fields. khronos_egl wraps EGL handles in raw
// pointers which make the type !Send/!Sync by default; declaring them safe
// here reflects our actual single-threaded usage discipline.
unsafe impl Send for EglState {}
unsafe impl Sync for EglState {}

impl EglState {
    /// Create EGL display from Wayland Connection
    ///
    /// # Ownership note
    /// `Connection::backend().display_ptr()` is only valid while the Connection lives.
    /// EglState must be dropped before Connection.
    pub fn new(conn: &Connection) -> Result<Self> {
        let wl_display = conn.backend().display_ptr() as *mut c_void;

        let lib = unsafe { Library::new("libEGL.so.1") }
            .map_err(|e| AppError::EglInit(format!("Failed to load libEGL.so.1: {e:?}")))?;

        let egl14 = unsafe { egl::DynamicInstance::<egl::EGL1_4>::load_required_from(lib) }
            .map_err(|e| AppError::EglInit(format!("Failed to load EGL 1.4: {e:?}")))?;

        let instance = egl14
            .try_cast_into::<egl::Dynamic<Library, egl::EGL1_5>>()
            .map_err(|_| AppError::EglInit("EGL 1.5 not supported".into()))?;

        let display = unsafe {
            instance
                .get_display(wl_display)
                .ok_or_else(|| AppError::EglInit("Failed to get EGL display".into()))?
        };

        instance
            .initialize(display)
            .map_err(|e| AppError::EglInit(format!("EGL initialize failed: {e:?}")))?;

        let api_configs = [
            (egl::OPENGL_API, egl::OPENGL_BIT, GlApiType::OpenGL),
            (egl::OPENGL_ES_API, egl::OPENGL_ES2_BIT, GlApiType::OpenGLES),
        ];

        let mut last_error: Option<String> = None;
        for (api, renderable_type, api_type) in api_configs {
            if let Err(e) = instance.bind_api(api) {
                last_error = Some(format!("bind_api failed for {:?}: {e:?}", api_type));
                continue;
            }

            let config_attrs = [
                egl::RED_SIZE,
                8,
                egl::GREEN_SIZE,
                8,
                egl::BLUE_SIZE,
                8,
                egl::ALPHA_SIZE,
                8,
                egl::RENDERABLE_TYPE,
                renderable_type as egl::Int,
                egl::NONE,
            ];

            let config = match instance.choose_first_config(display, &config_attrs) {
                Ok(Some(c)) => c,
                Ok(None) => {
                    last_error = Some(format!("No matching config for {:?}", api_type));
                    continue;
                }
                Err(e) => {
                    last_error = Some(format!("Config selection failed for {:?}: {e:?}", api_type));
                    continue;
                }
            };

            match try_create_context_with_fallback(&instance, display, config, api_type) {
                Ok(context) => {
                    cflp_success(&format!("EGL initialized with {:?}", api_type));
                    return Ok(Self {
                        instance,
                        display,
                        config,
                        context,
                        api_type,
                    });
                }
                Err(e) => {
                    last_error = Some(e.to_string());
                    continue;
                }
            }
        }

        Err(AppError::EglInit(
            last_error.unwrap_or_else(|| "No supported GL API found".into()),
        ))
    }

    /// Create EGL surface for an output
    pub fn create_surface(&self, egl_window: &WlEglSurface) -> Result<egl::Surface> {
        let native_window = egl_window.ptr() as *mut c_void;
        unsafe {
            self.instance
                .create_window_surface(self.display, self.config, native_window, None)
                .map_err(|e| AppError::EglInit(format!("Surface creation failed: {e:?}")))
        }
    }

    /// Make context current for a surface
    pub fn make_current(&self, surface: &egl::Surface) -> Result<()> {
        self.instance
            .make_current(
                self.display,
                Some(*surface),
                Some(*surface),
                Some(self.context),
            )
            .map_err(|e| AppError::EglInit(format!("EGL make_current failed: {e:?}")))
    }

    /// Swap buffers
    pub fn swap_buffers(&self, surface: &egl::Surface) -> Result<()> {
        self.instance
            .swap_buffers(self.display, *surface)
            .map_err(|e| AppError::EglInit(format!("EGL swap_buffers failed: {e:?}")))
    }

    /// Destroy an EGL surface created by this state.
    pub fn destroy_surface(&self, surface: egl::Surface) -> Result<()> {
        self.instance
            .destroy_surface(self.display, surface)
            .map_err(|e| AppError::EglInit(format!("EGL destroy_surface failed: {e:?}")))
    }
}

impl Drop for EglState {
    fn drop(&mut self) {
        if let Err(e) = self.instance.make_current(self.display, None, None, None) {
            cflp_error(&format!("Failed to release current EGL context: {e:?}"));
        }

        if let Err(e) = self.instance.destroy_context(self.display, self.context) {
            cflp_error(&format!("Failed to destroy EGL context: {e:?}"));
        }

        if let Err(e) = self.instance.terminate(self.display) {
            cflp_error(&format!("Failed to terminate EGL display: {e:?}"));
        }

        if let Err(e) = self.instance.release_thread() {
            cflp_error(&format!("Failed to release EGL thread state: {e:?}"));
        }
    }
}

fn try_create_context_with_fallback<A: egl::api::EGL1_5>(
    instance: &egl::Instance<A>,
    display: egl::Display,
    config: egl::Config,
    api_type: GlApiType,
) -> std::result::Result<egl::Context, &'static str> {
    let versions: &[(egl::Int, egl::Int)] = match api_type {
        GlApiType::OpenGL => &[(4, 6), (4, 5), (4, 0), (3, 3), (3, 0)],
        GlApiType::OpenGLES => &[(3, 2), (3, 1), (3, 0), (2, 0)],
    };

    for (major, minor) in versions {
        let attrs = [
            egl::CONTEXT_MAJOR_VERSION,
            *major,
            egl::CONTEXT_MINOR_VERSION,
            *minor,
            egl::NONE,
        ];
        if let Ok(ctx) = instance.create_context(display, config, None, &attrs) {
            cflp_success(&format!(
                "Created {:?} {}.{} context",
                api_type, major, minor
            ));
            return Ok(ctx);
        }
    }

    Err("Failed to create any GL context")
}
