// MPV integration module
// Phase 4: Context initialization, options parsing, and frame rendering

pub mod context;
pub mod options;
pub mod render;

pub use context::MpvState;
pub use options::{
    apply_init_options, apply_runtime_properties, apply_slideshow_options,
    apply_wallpaper_defaults, parse_user_options, ParsedMpvOptions,
};
pub use render::{render_frame, should_render};
