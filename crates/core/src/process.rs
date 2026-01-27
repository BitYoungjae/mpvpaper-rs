use std::fs;
use std::path::Path;

/// Check if a process is running by name (pidof replacement)
///
/// Searches through /proc for processes with the given name.
/// Uses /proc/[pid]/comm first, then falls back to /proc/[pid]/cmdline
/// for process names longer than 15 characters (comm is truncated at 15 chars).
pub fn is_process_running(name: &str) -> bool {
    let proc_dir = Path::new("/proc");

    if !proc_dir.exists() {
        return false;
    }

    let entries = match fs::read_dir(proc_dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Only check directories with numeric names (PIDs)
        if !path.is_dir() {
            continue;
        }

        let dir_name = match path.file_name() {
            Some(name) => name.to_string_lossy(),
            None => continue,
        };

        if !dir_name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        // Read the comm file to get the process name
        let comm_path = path.join("comm");
        if let Ok(comm) = fs::read_to_string(&comm_path) {
            let process_name = comm.trim();
            if process_name == name {
                return true;
            }

            // If comm is truncated (15 chars) and search name is longer,
            // fall back to cmdline check
            if name.len() > 15 && process_name.len() == 15 && check_cmdline(&path, name) {
                return true;
            }
        }
    }

    false
}

/// Check /proc/[pid]/cmdline for process name match
///
/// cmdline contains the full command line with null-separated arguments.
/// The first argument (argv[0]) is the program path/name.
fn check_cmdline(proc_path: &Path, name: &str) -> bool {
    let cmdline_path = proc_path.join("cmdline");
    if let Ok(cmdline) = fs::read(&cmdline_path) {
        // cmdline is null-separated, get first argument (program name/path)
        if let Some(first_arg) = cmdline.split(|&b| b == 0).next() {
            if let Ok(arg_str) = std::str::from_utf8(first_arg) {
                // Extract basename from path (e.g., /usr/bin/firefox -> firefox)
                let basename = arg_str.rsplit('/').next().unwrap_or(arg_str);
                if basename == name {
                    return true;
                }
            }
        }
    }
    false
}

/// Check watch list and return first running process
///
/// Returns the name of the first process in the list that is currently running
pub fn check_watch_list(list: &[String]) -> Option<String> {
    for process_name in list {
        if is_process_running(process_name) {
            return Some(process_name.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_process_running_nonexistent() {
        // A process name that definitely doesn't exist
        assert!(!is_process_running("this_process_does_not_exist_12345"));
    }

    #[test]
    fn test_check_watch_list_empty() {
        let list: Vec<String> = vec![];
        assert!(check_watch_list(&list).is_none());
    }

    #[test]
    fn test_check_watch_list_none_running() {
        let list = vec![
            "nonexistent_process_1".to_string(),
            "nonexistent_process_2".to_string(),
        ];
        assert!(check_watch_list(&list).is_none());
    }
}
