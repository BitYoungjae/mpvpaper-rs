use colored::Colorize;

/// Print a success message (green checkmark)
pub fn cflp_success(msg: &str) {
    eprintln!("{} {}", "✓".green().bold(), msg);
}

/// Print an error message (red X)
pub fn cflp_error(msg: &str) {
    eprintln!("{} {}", "✗".red().bold(), msg);
}

/// Print a warning message (yellow exclamation)
pub fn cflp_warning(msg: &str) {
    eprintln!("{} {}", "!".yellow().bold(), msg);
}

/// Alias for cflp_warning
pub use cflp_warning as cflp_warn;

/// Print an info message if verbose level is high enough
///
/// # Arguments
/// * `required_level` - Minimum verbosity level required to show this message
/// * `current_level` - Current verbosity level from CLI args
/// * `msg` - Message to print
pub fn cflp_info(required_level: u8, current_level: u8, msg: &str) {
    if current_level >= required_level {
        eprintln!("{} {}", "i".blue().bold(), msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cflp_info_shown_when_verbose_enough() {
        // This test just verifies the function doesn't panic
        cflp_info(1, 2, "Test message");
    }

    #[test]
    fn test_cflp_info_hidden_when_not_verbose_enough() {
        // This test just verifies the function doesn't panic
        cflp_info(3, 1, "Test message");
    }
}
