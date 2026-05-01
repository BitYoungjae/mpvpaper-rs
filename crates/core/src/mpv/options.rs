use libmpv2::Mpv;

use crate::error::{AppError, Result};

/// Init-only options that must be set via Mpv::with_initializer
///
/// Based on libmpv2 Mpv::with_initializer documentation:
/// https://docs.rs/libmpv2/latest/libmpv2/struct.Mpv.html
///
/// Documented init-only options:
/// - config, config-dir: Configuration file related
/// - input-conf: Input configuration file
/// - load-scripts, script: Script loading
/// - player-operation-mode: Player mode
/// - input-app-events: Input events (macOS, but listed)
/// - All encoding options
///
/// Additional init-only options (from mpv source and empirical testing):
/// - vo: Video output driver (vo=libmpv required for embedding)
/// - background: Background rendering mode
/// - terminal, input-default-bindings, input-terminal: Terminal/input related
///
/// Note: This list may change with mpv versions.
/// mpv 0.35+ changed behavior of some options.
const INIT_ONLY_OPTIONS: &[&str] = &[
    // libmpv2 documentation specified
    "config",
    "config-dir",
    "input-conf",
    "load-scripts",
    "script",
    "player-operation-mode",
    "input-app-events",
    // encoding related (libmpv2 docs: "and all encoding options")
    "ovc",
    "oac",
    "of",
    "ofopts",
    "ofps",
    "oautofps",
    "oharddup",
    "ocopyts",
    "orawts",
    "oneverdrop",
    // Empirically init-only
    "vo",
    "background",
    "terminal",
    "input-default-bindings",
    "input-terminal",
];

/// Parsed user options separated into init-time and runtime
pub struct ParsedMpvOptions {
    /// Options to set before initialization (init.set_option)
    pub init_options: Vec<(String, String)>,
    /// Properties to set after initialization (mpv.set_property)
    pub runtime_properties: Vec<(String, String)>,
}

/// Parse user options string into init-time and runtime options
///
/// Supported formats:
/// - --option=value: Standard mpv option format (recommended)
/// - --no-option: mpv interprets as option=no (e.g., --no-audio -> audio=no)
/// - --option: Flag without value (interpreted as option=yes)
///
/// Unsupported formats (returns error):
/// - --option value (space separated): Deprecated in mpv manual
/// - -option=value (single dash): Non-standard
///
/// CRITICAL: mpv's --no-<option> handling
/// mpv interprets --no-foo as foo=no. Examples:
///   --no-audio -> audio=no
///   --no-video -> video=no
///   --no-osc   -> osc=no
pub fn parse_user_options(options: &str) -> Result<ParsedMpvOptions> {
    let mut result = ParsedMpvOptions {
        init_options: Vec::new(),
        runtime_properties: Vec::new(),
    };

    if options.trim().is_empty() {
        return Ok(result);
    }

    // Shell-style parsing to handle quoted values and spaces correctly
    let parsed = shlex::split(options)
        .ok_or_else(|| AppError::Config("Invalid mpv options: unmatched quotes".into()))?;

    for opt in parsed {
        if let Some(stripped) = opt.strip_prefix("--") {
            let (key, value) = if let Some((k, v)) = stripped.split_once('=') {
                // --key=value format (standard)
                // Note: --no-key=value is also possible, so no- handling only for = absent case
                (k.to_string(), v.to_string())
            } else if let Some(no_stripped) = stripped.strip_prefix("no-") {
                // --no-foo format -> foo=no
                (no_stripped.to_string(), "no".to_string())
            } else {
                // --foo format -> foo=yes (flag option)
                // Warning: Using this format for options requiring values will cause mpv error
                (stripped.to_string(), "yes".to_string())
            };

            if is_init_only_option(&key) {
                result.init_options.push((key, value));
            } else {
                result.runtime_properties.push((key, value));
            }
        } else if opt.starts_with('-') && !opt.starts_with("--") {
            // -option format not supported
            return Err(AppError::Config(format!(
                "Invalid mpv option format '{}': use --option=value format",
                opt
            )));
        } else if !opt.starts_with('-') {
            // Value appearing alone - likely --option value format was used
            return Err(AppError::Config(format!(
                "Unexpected value '{}': use --option=value format (not --option value)",
                opt
            )));
        }
    }

    Ok(result)
}

/// Check if an option is init-only
fn is_init_only_option(key: &str) -> bool {
    INIT_ONLY_OPTIONS.contains(&key)
}

/// Apply init-time options (call inside Mpv::with_initializer)
pub fn apply_init_options(
    init: &libmpv2::MpvInitializer,
    options: &[(String, String)],
) -> std::result::Result<(), libmpv2::Error> {
    for (key, value) in options {
        init.set_option(key, value.clone())?;
    }
    Ok(())
}

/// Apply runtime properties (call after initialization)
pub fn apply_runtime_properties(mpv: &Mpv, properties: &[(String, String)]) -> Result<()> {
    for (key, value) in properties {
        mpv.set_property(key, value.clone()).map_err(|e| {
            AppError::Config(format!("Failed to set property '{}={}': {}", key, value, e))
        })?;
    }
    Ok(())
}

/// Apply slideshow-specific options
pub fn apply_slideshow_options(mpv: &Mpv) -> Result<()> {
    mpv.set_property("loop", "yes")
        .map_err(|e| AppError::Config(format!("Failed to set loop: {}", e)))?;
    mpv.set_property("loop-playlist", "yes")
        .map_err(|e| AppError::Config(format!("Failed to set loop-playlist: {}", e)))?;
    Ok(())
}

/// Apply wallpaper-friendly defaults.
///
/// Sets `audio=no` and `hwdec=auto-safe` to keep CPU usage low for typical
/// wallpaper usage. User `-o` options applied AFTER this still override these
/// (apply order is: defaults -> user runtime properties).
///
/// Errors are intentionally ignored:
/// - mpv versions older than 0.32 don't recognize `auto-safe` (we fall back to `auto`).
/// - In the unlikely case `audio` cannot be set, the worst outcome is audio plays;
///   not worth aborting startup.
pub fn apply_wallpaper_defaults(mpv: &Mpv) {
    let _ = mpv.set_property("audio", "no");
    if mpv.set_property("hwdec", "auto-safe").is_err() {
        let _ = mpv.set_property("hwdec", "auto");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_standard_option() {
        let result = parse_user_options("--volume=50").unwrap();
        assert_eq!(result.runtime_properties.len(), 1);
        assert_eq!(
            result.runtime_properties[0],
            ("volume".to_string(), "50".to_string())
        );
    }

    #[test]
    fn test_parse_no_prefix() {
        let result = parse_user_options("--no-audio").unwrap();
        assert_eq!(result.runtime_properties.len(), 1);
        assert_eq!(
            result.runtime_properties[0],
            ("audio".to_string(), "no".to_string())
        );
    }

    #[test]
    fn test_parse_flag_option() {
        let result = parse_user_options("--pause").unwrap();
        assert_eq!(result.runtime_properties.len(), 1);
        assert_eq!(
            result.runtime_properties[0],
            ("pause".to_string(), "yes".to_string())
        );
    }

    #[test]
    fn test_parse_init_only_option() {
        let result = parse_user_options("--vo=libmpv").unwrap();
        assert_eq!(result.init_options.len(), 1);
        assert_eq!(
            result.init_options[0],
            ("vo".to_string(), "libmpv".to_string())
        );
        assert!(result.runtime_properties.is_empty());
    }

    #[test]
    fn test_parse_mixed_options() {
        let result = parse_user_options("--vo=libmpv --volume=50 --no-osc").unwrap();
        assert_eq!(result.init_options.len(), 1);
        assert_eq!(result.runtime_properties.len(), 2);
    }

    #[test]
    fn test_parse_quoted_value() {
        let result = parse_user_options("--title=\"My Video\"").unwrap();
        assert_eq!(result.runtime_properties.len(), 1);
        assert_eq!(
            result.runtime_properties[0],
            ("title".to_string(), "My Video".to_string())
        );
    }

    #[test]
    fn test_reject_space_separated() {
        let result = parse_user_options("--volume 50");
        assert!(result.is_err());
    }

    #[test]
    fn test_reject_single_dash() {
        let result = parse_user_options("-volume=50");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_options() {
        let result = parse_user_options("").unwrap();
        assert!(result.init_options.is_empty());
        assert!(result.runtime_properties.is_empty());
    }

    #[test]
    fn test_whitespace_only() {
        let result = parse_user_options("   ").unwrap();
        assert!(result.init_options.is_empty());
        assert!(result.runtime_properties.is_empty());
    }
}
