mod app;

use std::fs::File;
use std::os::fd::AsRawFd;

use clap::Parser;
use nix::unistd::{close, fork, setsid, ForkResult};

use mpvpaper_rs_core::cli::Args;
use mpvpaper_rs_core::logging::{cflp_error, cflp_info, cflp_success};
use mpvpaper_rs_core::wayland::{list_outputs, AppState};

fn main() {
    let args = Args::parse();

    cflp_info(1, args.verbose, "Starting mpvpaper-rs...");

    if let Err(e) = args.validate() {
        cflp_error(&e);
        std::process::exit(1);
    }

    if args.show_outputs {
        show_available_outputs(args.verbose);
        return;
    }

    // Fork mode: daemonize the process
    if args.fork {
        daemonize(args.verbose);
    }

    // Run the main application
    if let Err(e) = app::run_app(&args) {
        cflp_error(&format!("Application error: {}", e));
        std::process::exit(1);
    }

    cflp_success("Exiting gracefully");
}

fn show_available_outputs(verbose: u8) {
    cflp_info(1, verbose, "Connecting to Wayland display...");

    match AppState::new() {
        Ok((state, _event_queue)) => {
            let outputs = list_outputs(&state.output_state);

            if outputs.is_empty() {
                cflp_error("No outputs found");
                std::process::exit(1);
            }

            cflp_success(&format!("Found {} output(s):", outputs.len()));
            println!();
            for output in outputs {
                println!("  {}", output);
            }
            println!();
            println!("Usage: mpvpaper-rs <OUTPUT> <VIDEO_PATH>");
            println!("  OUTPUT can be: name (DP-2), index (0), or ALL/*");
        }
        Err(e) => {
            cflp_error(&format!("Failed to connect to Wayland: {}", e));
            std::process::exit(1);
        }
    }
}

/// Daemonize the process (fork and detach from terminal)
///
/// After this function returns in the child process:
/// - Parent has exited, returning control to terminal
/// - Child is session leader (new session, detached from controlling terminal)
/// - stdin/stdout/stderr redirected to /dev/null
fn daemonize(verbose: u8) {
    // Safety: fork is safe here since we haven't spawned any threads yet
    match unsafe { fork() } {
        Ok(ForkResult::Parent { .. }) => {
            // Parent: exit immediately to return control to terminal
            cflp_info(1, verbose, "Forking to background...");
            std::process::exit(0);
        }
        Ok(ForkResult::Child) => {
            // Child: continue with daemonization

            // Become session leader (detach from controlling terminal)
            if let Err(e) = setsid() {
                cflp_error(&format!("setsid failed: {}", e));
                std::process::exit(1);
            }

            // Redirect stdin/stdout/stderr to /dev/null
            // Use libc directly for dup2 since nix's dup2 API changed
            if let Ok(dev_null) = File::open("/dev/null") {
                let null_fd = dev_null.as_raw_fd();

                unsafe {
                    // Redirect stdin (fd 0)
                    nix::libc::dup2(null_fd, 0);
                    // Redirect stdout (fd 1)
                    nix::libc::dup2(null_fd, 1);
                    // Redirect stderr (fd 2)
                    nix::libc::dup2(null_fd, 2);
                }

                // Close the original /dev/null fd if it's not one of 0,1,2
                if null_fd > 2 {
                    let _ = close(null_fd);
                }

                // Prevent the File from closing null_fd again on drop
                std::mem::forget(dev_null);
            }

            // Child continues execution after this function returns
        }
        Err(e) => {
            cflp_error(&format!("Fork failed: {}", e));
            std::process::exit(1);
        }
    }
}
