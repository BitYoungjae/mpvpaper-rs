use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const CONFIG_DIR_NAME: &str = "mpvpaper-rs";
const PAUSELIST_FILENAME: &str = "pauselist";
const STOPLIST_FILENAME: &str = "stoplist";

/// Get the configuration directory path (~/.config/mpvpaper-rs/)
pub fn get_config_dir() -> PathBuf {
    dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(CONFIG_DIR_NAME)
}

/// Get the pauselist file path (~/.config/mpvpaper-rs/pauselist)
pub fn get_pauselist_path() -> PathBuf {
    get_config_dir().join(PAUSELIST_FILENAME)
}

/// Get the stoplist file path (~/.config/mpvpaper-rs/stoplist)
pub fn get_stoplist_path() -> PathBuf {
    get_config_dir().join(STOPLIST_FILENAME)
}

/// Load a list file (pauselist or stoplist)
///
/// Format: one process name per line, blank lines and # comments are ignored
pub fn load_list_file(path: &Path) -> io::Result<Vec<String>> {
    let content = fs::read_to_string(path)?;
    let list: Vec<String> = content
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.to_string())
        .collect();
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_dir_ends_with_app_name() {
        let path = get_config_dir();
        assert!(path.ends_with(CONFIG_DIR_NAME));
    }

    #[test]
    fn test_pauselist_path() {
        let path = get_pauselist_path();
        assert!(path.ends_with(PAUSELIST_FILENAME));
        assert!(path.parent().unwrap().ends_with(CONFIG_DIR_NAME));
    }

    #[test]
    fn test_stoplist_path() {
        let path = get_stoplist_path();
        assert!(path.ends_with(STOPLIST_FILENAME));
        assert!(path.parent().unwrap().ends_with(CONFIG_DIR_NAME));
    }
}
