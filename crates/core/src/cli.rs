use clap::{ArgAction, Parser, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LayerArg {
    Background,
    Bottom,
    Top,
    Overlay,
}

#[derive(Parser, Debug, Clone)]
#[command(name = "mpvpaper-rs")]
#[command(version, about = "Video wallpaper player using mpv for wlroots")]
pub struct Args {
    /// Displays all available outputs and quits
    #[arg(short = 'd', long = "help-output")]
    pub show_outputs: bool,

    /// Be more verbose (-v, -vv for higher verbosity)
    #[arg(short, long, action = ArgAction::Count)]
    pub verbose: u8,

    /// Forks mpvpaper-rs so you can close the terminal
    #[arg(short, long)]
    pub fork: bool,

    /// Automagically pause mpv when the wallpaper is hidden
    #[arg(short = 'p', long = "auto-pause")]
    pub auto_pause: bool,

    /// Automagically stop mpv when the wallpaper is hidden
    #[arg(short = 's', long = "auto-stop")]
    pub auto_stop: bool,

    /// Slideshow mode: advances to next video every N seconds
    #[arg(short = 'n', long)]
    pub slideshow: Option<u32>,

    /// Shell surface layer to render on
    #[arg(short, long, default_value = "background", value_enum)]
    pub layer: LayerArg,

    /// Forward options to mpv (quoted string)
    #[arg(short = 'o', long = "mpv-options")]
    pub mpv_options: Option<String>,

    /// Hidden: restore video position (used internally by holder)
    #[arg(short = 'Z', hide = true)]
    pub restore_info: Option<String>,

    /// Output name (DP-2, HDMI-A-1, ALL, *, etc.)
    /// Optional when using -d/--help-output
    pub output: Option<String>,

    /// Video file path or URL (optional if --playlist= in mpv_options)
    pub video_path: Option<String>,
}

impl Args {
    /// Validate arguments after parsing
    pub fn validate(&self) -> Result<(), String> {
        // -d doesn't require output
        if !self.show_outputs && self.output.is_none() {
            return Err("Output name is required (e.g., DP-2, HDMI-A-1, ALL)".to_string());
        }

        // video_path required unless --playlist= in mpv_options
        if !self.show_outputs && self.video_path.is_none() {
            let has_playlist = self
                .mpv_options
                .as_ref()
                .map(|o| o.contains("--playlist=") || o.contains("playlist="))
                .unwrap_or(false);
            if !has_playlist {
                return Err("Video path is required unless --playlist= is specified".to_string());
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_show_outputs_no_args_needed() {
        let args = Args::parse_from(["mpvpaper-rs", "-d"]);
        assert!(args.show_outputs);
        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_missing_output_error() {
        let args = Args {
            show_outputs: false,
            verbose: 0,
            fork: false,
            auto_pause: false,
            auto_stop: false,
            slideshow: None,
            layer: LayerArg::Background,
            mpv_options: None,
            restore_info: None,
            output: None,
            video_path: Some("/path/to/video.mp4".to_string()),
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn test_missing_video_path_error() {
        let args = Args {
            show_outputs: false,
            verbose: 0,
            fork: false,
            auto_pause: false,
            auto_stop: false,
            slideshow: None,
            layer: LayerArg::Background,
            mpv_options: None,
            restore_info: None,
            output: Some("DP-2".to_string()),
            video_path: None,
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn test_playlist_option_allows_no_video_path() {
        let args = Args {
            show_outputs: false,
            verbose: 0,
            fork: false,
            auto_pause: false,
            auto_stop: false,
            slideshow: None,
            layer: LayerArg::Background,
            mpv_options: Some("--playlist=/path/to/playlist.txt".to_string()),
            restore_info: None,
            output: Some("DP-2".to_string()),
            video_path: None,
        };
        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_valid_args() {
        let args = Args::parse_from(["mpvpaper-rs", "DP-2", "/path/to/video.mp4"]);
        assert_eq!(args.output, Some("DP-2".to_string()));
        assert_eq!(args.video_path, Some("/path/to/video.mp4".to_string()));
        assert!(args.validate().is_ok());
    }

    #[test]
    fn test_layer_arg() {
        let args = Args::parse_from([
            "mpvpaper-rs",
            "-l",
            "overlay",
            "DP-2",
            "/path/to/video.mp4",
        ]);
        assert_eq!(args.layer, LayerArg::Overlay);
    }

    #[test]
    fn test_verbose_count() {
        let args = Args::parse_from(["mpvpaper-rs", "-vvv", "-d"]);
        assert_eq!(args.verbose, 3);
    }
}
