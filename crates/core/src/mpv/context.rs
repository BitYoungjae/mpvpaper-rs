use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex;

use calloop::channel::Sender;
use libmpv2::Mpv;
use libmpv2_sys::{
    mpv_opengl_init_params, mpv_render_context, mpv_render_context_create, mpv_render_context_free,
    mpv_render_context_set_update_callback, mpv_render_param,
    mpv_render_param_type_MPV_RENDER_PARAM_ADVANCED_CONTROL,
    mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
    mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
    mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
    mpv_render_param_type_MPV_RENDER_PARAM_WL_DISPLAY,
};

use crate::error::{AppError, Result};
use crate::logging::cflp_success;

use super::options::{
    apply_init_options, apply_runtime_properties, apply_wallpaper_defaults, ParsedMpvOptions,
};

/// Closure resolving an OpenGL function name to its address.
type GetProcAddrFn = dyn Fn(&CStr) -> *mut c_void;

/// MPV state with render context for OpenGL rendering
///
/// CRITICAL: Drop order matters!
/// render_ctx must be freed before mpv handle is dropped.
/// This is handled in the Drop implementation.
pub struct MpvState {
    pub mpv: Mpv,
    pub render_ctx: *mut mpv_render_context,
    pub render_wakeup_tx: Sender<()>,
    /// Last update flags from mpv_render_context_update
    pub last_update_flags: AtomicU64,
    /// Box holding the get_proc_addr closure - must outlive render_ctx
    _get_proc_addr_box: Option<Box<GetProcAddrFn>>,
    /// Stored data for deferred render context creation
    init_data: Mutex<Option<MpvInitData>>,
    /// Raw pointer to the Sender leaked for mpv callback - must be freed in Drop
    callback_sender_ptr: *mut Sender<()>,
}

/// Data stored for deferred render context creation
struct MpvInitData {
    wl_display: *mut c_void,
    render_wakeup_tx: Sender<()>,
}

// MpvState contains raw pointer but it's safe because:
// - render_ctx is only accessed from the render thread
// - All other access is through the safe Mpv wrapper
unsafe impl Send for MpvState {}
unsafe impl Sync for MpvState {}

impl MpvState {
    /// Initialize MPV handle (without render context)
    ///
    /// The render context must be created later by calling `init_render_context()`
    /// after an EGL context is made current.
    ///
    /// # Arguments
    /// * `wl_display` - Wayland display pointer (from Connection::backend().display_ptr())
    /// * `render_wakeup_tx` - Channel sender for render wakeup notifications
    /// * `load_config` - Whether to load mpv config files
    /// * `parsed_options` - Pre-parsed user options (init-only and runtime separated)
    ///
    /// # Safety
    /// * `wl_display` must be a valid pointer to a Wayland display
    /// * The display must remain valid for the lifetime of MpvState
    pub fn new(
        wl_display: *mut c_void,
        render_wakeup_tx: Sender<()>,
        load_config: bool,
        parsed_options: &ParsedMpvOptions,
    ) -> Result<Self> {
        // Create a reference to user init options for the closure
        let user_init_options = parsed_options.init_options.clone();

        // Use Mpv::with_initializer for init-only options
        // CRITICAL: vo, background, etc. are init-only and must be set here
        // set_property() after initialization will not work or cause inconsistency
        let mpv = Mpv::with_initializer(move |init| {
            // CRITICAL: vo=libmpv is required for libmpv embedding
            // mpv 0.35+ requires explicit setting (default changed)
            init.set_option("vo", "libmpv")?;

            // Transparent background for wallpaper use
            // --background option:
            //   - "none": Disable background rendering completely (transparent)
            //   - "color": Use color specified by background-color
            //   - "tiles": Checkerboard pattern (for debugging)
            init.set_option("background", "none")?;

            // Terminal and input options (init-only)
            init.set_option("terminal", "yes")?;
            init.set_option("input-default-bindings", "yes")?;
            init.set_option("input-terminal", "yes")?;

            if load_config {
                init.set_option("config", "yes")?;
            }

            // Apply user init-only options
            apply_init_options(&init, &user_init_options)?;

            Ok(())
        })
        .map_err(AppError::Mpv)?;

        // Apply wallpaper-friendly defaults BEFORE user runtime options so
        // user `-o` options can still override them.
        // CPU-relevant defaults: audio=no, hwdec=auto-safe.
        apply_wallpaper_defaults(&mpv);

        // Apply runtime properties (user -o options)
        apply_runtime_properties(&mpv, &parsed_options.runtime_properties)?;

        cflp_success("MPV handle created");

        // Clone the sender for init_data before moving
        let render_wakeup_tx_clone = render_wakeup_tx.clone();

        Ok(Self {
            mpv,
            render_ctx: ptr::null_mut(),
            render_wakeup_tx,
            last_update_flags: AtomicU64::new(0),
            _get_proc_addr_box: None,
            init_data: Mutex::new(Some(MpvInitData {
                wl_display,
                render_wakeup_tx: render_wakeup_tx_clone,
            })),
            callback_sender_ptr: ptr::null_mut(),
        })
    }

    /// Initialize the render context
    ///
    /// # Safety
    /// * Must be called with an active/current OpenGL context
    /// * `get_proc_addr` must return valid function pointers
    pub unsafe fn init_render_context<F>(&mut self, get_proc_addr: F) -> Result<()>
    where
        F: Fn(&CStr) -> *mut c_void + 'static,
    {
        if !self.render_ctx.is_null() {
            return Err(AppError::Config(
                "Render context already initialized".into(),
            ));
        }

        let init_data = self
            .init_data
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| AppError::Config("Missing init data for render context".into()))?;

        // Box the get_proc_addr closure so it lives as long as MpvState
        let get_proc_addr_box: Box<GetProcAddrFn> = Box::new(get_proc_addr);

        // Create render context
        let render_ctx = create_render_context(
            self.mpv.ctx.as_ptr(),
            init_data.wl_display,
            &get_proc_addr_box,
        )?;

        // Set update callback and store the sender pointer for cleanup
        let sender_ptr = set_render_update_callback(render_ctx, init_data.render_wakeup_tx)?;

        self.render_ctx = render_ctx;
        self._get_proc_addr_box = Some(get_proc_addr_box);
        self.callback_sender_ptr = sender_ptr;

        cflp_success("MPV render context created");

        Ok(())
    }

    /// Load a video file
    pub fn load_file(&self, path: &str) -> Result<()> {
        self.mpv
            .command("loadfile", &[path])
            .map_err(AppError::Mpv)?;
        cflp_success(&format!("Loaded: {}", path));
        Ok(())
    }

    /// Restore playback position from saved info
    ///
    /// Format: "playlist_pos:time_pos" or "time_pos"
    pub fn restore_position(&self, save_info: &str) -> Result<()> {
        if let Some((playlist_pos, time_pos)) = save_info.split_once(':') {
            if let Ok(pos) = playlist_pos.parse::<i64>() {
                self.mpv
                    .set_property("playlist-pos", pos)
                    .map_err(AppError::Mpv)?;
            }
            if let Ok(time) = time_pos.parse::<f64>() {
                self.mpv
                    .set_property("time-pos", time)
                    .map_err(AppError::Mpv)?;
            }
        } else if let Ok(time) = save_info.parse::<f64>() {
            self.mpv
                .set_property("time-pos", time)
                .map_err(AppError::Mpv)?;
        }
        Ok(())
    }

    /// Get current position for saving
    ///
    /// Returns format: "playlist_pos:time_pos"
    pub fn get_save_info(&self) -> Result<String> {
        let playlist_pos: i64 = self.mpv.get_property("playlist-pos").unwrap_or(0);
        let time_pos: f64 = self.mpv.get_property("time-pos").unwrap_or(0.0);
        Ok(format!("{}:{}", playlist_pos, time_pos))
    }

    /// Get whether playback is paused
    pub fn is_paused(&self) -> bool {
        self.mpv.get_property::<bool>("pause").unwrap_or(false)
    }

    /// Set pause state
    pub fn set_pause(&self, paused: bool) -> Result<()> {
        self.mpv
            .set_property("pause", paused)
            .map_err(AppError::Mpv)
    }

    /// Pause playback
    pub fn pause(&self) -> Result<()> {
        self.set_pause(true)
    }

    /// Unpause (resume) playback
    pub fn unpause(&self) -> Result<()> {
        self.set_pause(false)
    }

    /// Advance to the next item in the playlist
    pub fn playlist_next(&self) -> Result<()> {
        self.mpv
            .command("playlist-next", &["weak"])
            .map_err(AppError::Mpv)
    }
}

impl Drop for MpvState {
    fn drop(&mut self) {
        // CRITICAL: Free render context before mpv handle
        // mpv_render_context_free must be called while mpv handle is still valid
        if !self.render_ctx.is_null() {
            unsafe {
                mpv_render_context_free(self.render_ctx);
            }
            self.render_ctx = ptr::null_mut();
        }

        // Free the callback sender that was leaked for mpv callback
        if !self.callback_sender_ptr.is_null() {
            unsafe {
                let _ = Box::from_raw(self.callback_sender_ptr);
            }
            self.callback_sender_ptr = ptr::null_mut();
        }

        // mpv handle is dropped automatically by libmpv2 crate
    }
}

/// Wrapper function for OpenGL proc address lookup
///
/// This is called by mpv to resolve OpenGL function addresses.
/// The ctx pointer points to our boxed closure.
unsafe extern "C" fn gl_get_proc_address_wrapper(
    ctx: *mut c_void,
    name: *const c_char,
) -> *mut c_void {
    if ctx.is_null() || name.is_null() {
        return ptr::null_mut();
    }

    let get_proc_addr = &*(ctx as *const Box<GetProcAddrFn>);
    let name_cstr = CStr::from_ptr(name);
    get_proc_addr(name_cstr)
}

/// Create MPV OpenGL render context using libmpv2-sys
///
/// # Safety
/// This function is unsafe because it:
/// - Uses raw pointers for mpv_handle and wl_display
/// - Calls FFI functions directly
/// - The get_proc_addr closure must outlive the render context
///
/// `&Box<GetProcAddrFn>` is intentional (not `&GetProcAddrFn`): the FFI ctx
/// must be a thin pointer to the Box itself so the wrapper can re-create the
/// same `&Box<...>` reference on the C side. A `&dyn Fn` fat pointer cannot
/// round-trip through `*mut c_void`.
#[allow(clippy::borrowed_box)]
unsafe fn create_render_context(
    mpv_handle: *mut libmpv2_sys::mpv_handle,
    wl_display: *mut c_void,
    get_proc_addr: &Box<GetProcAddrFn>,
) -> Result<*mut mpv_render_context> {
    // OpenGL init params
    let gl_init = mpv_opengl_init_params {
        get_proc_address: Some(gl_get_proc_address_wrapper),
        get_proc_address_ctx: get_proc_addr as *const _ as *mut c_void,
    };

    // Advanced control flag
    // CRITICAL: When advanced control is enabled, thread safety constraints apply:
    // - mpv_render_context_render() must only be called from a single render thread
    // - No mpv API calls allowed inside update callback (deadlock risk)
    // - Update callback should only signal (channel send, etc.)
    // - mpv_render_context_update() must be called on render thread after callback
    // - mpv < 0.30.0 has additional deadlock issues (require 0.30.0+)
    let advanced_control: i32 = 1;

    // Render params array - must end with INVALID
    let params = [
        mpv_render_param {
            type_: mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
            data: c"opengl".as_ptr() as *mut c_void,
        },
        mpv_render_param {
            type_: mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
            data: &gl_init as *const _ as *mut c_void,
        },
        mpv_render_param {
            type_: mpv_render_param_type_MPV_RENDER_PARAM_WL_DISPLAY,
            data: wl_display,
        },
        mpv_render_param {
            type_: mpv_render_param_type_MPV_RENDER_PARAM_ADVANCED_CONTROL,
            data: &advanced_control as *const _ as *mut c_void,
        },
        mpv_render_param {
            type_: mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
            data: ptr::null_mut(),
        },
    ];

    let mut ctx: *mut mpv_render_context = ptr::null_mut();
    let result = mpv_render_context_create(&mut ctx, mpv_handle, params.as_ptr() as *mut _);

    if result < 0 {
        return Err(AppError::Config(format!(
            "Failed to create render context: error code {}",
            result
        )));
    }

    Ok(ctx)
}

/// Render update callback - called by mpv when a new frame is ready
///
/// CRITICAL: This callback must only signal, not call mpv APIs directly.
/// Calling mpv APIs here can cause deadlock.
unsafe extern "C" fn render_update_callback(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }

    // The ctx is a raw pointer to our Sender
    let sender = &*(ctx as *const Sender<()>);
    // Ignore send errors - the receiver might be dropped
    let _ = sender.send(());
}

/// Set the render update callback
///
/// Returns the raw pointer to the leaked Sender, which must be freed
/// by the caller when the render context is destroyed.
fn set_render_update_callback(
    render_ctx: *mut mpv_render_context,
    wakeup_tx: Sender<()>,
) -> Result<*mut Sender<()>> {
    // Leak the sender so it lives as long as the render context
    // The caller must free this pointer when the render context is destroyed
    let sender_box = Box::new(wakeup_tx);
    let sender_ptr = Box::into_raw(sender_box);

    unsafe {
        mpv_render_context_set_update_callback(
            render_ctx,
            Some(render_update_callback),
            sender_ptr as *mut c_void,
        );
    }

    Ok(sender_ptr)
}
