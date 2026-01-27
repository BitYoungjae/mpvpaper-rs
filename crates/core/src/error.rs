use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Wayland connection failed: {0}")]
    WaylandConnection(#[from] wayland_client::ConnectError),

    #[error("Wayland global error: {0}")]
    WaylandGlobal(#[from] wayland_client::globals::GlobalError),

    #[error("Wayland dispatch error: {0}")]
    WaylandDispatch(#[from] wayland_client::DispatchError),

    #[error("Wayland backend error: {0}")]
    WaylandBackend(#[from] wayland_client::backend::WaylandError),

    #[error("Wayland binding error: {0}")]
    WaylandBind(#[from] wayland_client::globals::BindError),

    #[error("wlr-layer-shell protocol not supported by compositor")]
    LayerShellNotSupported,

    #[error("EGL initialization failed: {0}")]
    EglInit(String),

    #[error("MPV error: {0}")]
    Mpv(#[from] libmpv2::Error),

    #[error("No matching output found: {0}")]
    OutputNotFound(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, AppError>;
