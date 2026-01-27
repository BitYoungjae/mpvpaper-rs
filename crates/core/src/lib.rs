pub mod cli;
pub mod config;
pub mod control;
pub mod error;
pub mod logging;
pub mod mpv;
pub mod process;
pub mod render;
pub mod wayland;

pub use error::{AppError, Result};
