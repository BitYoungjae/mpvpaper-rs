//! Thread management for mpvpaper-rs
//!
//! This module handles worker threads for:
//! - MPV event handling and slideshow timer
//! - Auto-pause based on output visibility
//! - Auto-stop (switch to holder) when hidden
//! - Watchlist monitoring (pauselist/stoplist)

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::logging::cflp_info;
use crate::mpv::MpvState;
use crate::process::check_watch_list;

use super::HaltInfo;

/// Handles for all worker threads
pub struct ThreadHandles {
    mpv_events: Option<JoinHandle<()>>,
    auto_pause: Option<JoinHandle<()>>,
    auto_stop: Option<JoinHandle<()>>,
    pauselist_monitor: Option<JoinHandle<()>>,
    stoplist_monitor: Option<JoinHandle<()>>,
}

impl ThreadHandles {
    /// Spawn all worker threads based on configuration
    ///
    /// # Arguments
    /// * `halt_info` - Shared halt/pause state
    /// * `mpv` - Shared MPV state
    /// * `slideshow_time` - Optional slideshow interval in seconds
    /// * `verbose` - Verbosity level for logging
    pub fn spawn_all(
        halt_info: Arc<HaltInfo>,
        mpv: Arc<MpvState>,
        slideshow_time: Option<u32>,
        verbose: u8,
    ) -> Self {
        // Spawn MPV event handler thread
        let mpv_events = {
            let halt_info = Arc::clone(&halt_info);
            let mpv = Arc::clone(&mpv);
            Some(thread::spawn(move || {
                mpv_event_handler(halt_info, mpv, slideshow_time, verbose);
            }))
        };

        // Spawn auto-pause handler if enabled
        let auto_pause = if halt_info.auto_pause {
            let halt_info = Arc::clone(&halt_info);
            let mpv = Arc::clone(&mpv);
            Some(thread::spawn(move || {
                auto_pause_handler(halt_info, mpv);
            }))
        } else {
            None
        };

        // Spawn auto-stop handler if enabled
        let auto_stop = if halt_info.auto_stop {
            let halt_info = Arc::clone(&halt_info);
            Some(thread::spawn(move || {
                auto_stop_handler(halt_info);
            }))
        } else {
            None
        };

        // Spawn pauselist monitor if list exists
        let pauselist_monitor = if halt_info.pauselist.is_some() {
            let halt_info = Arc::clone(&halt_info);
            let mpv = Arc::clone(&mpv);
            Some(thread::spawn(move || {
                pauselist_monitor_handler(halt_info, mpv, verbose);
            }))
        } else {
            None
        };

        // Spawn stoplist monitor if list exists
        let stoplist_monitor = if halt_info.stoplist.is_some() {
            let halt_info = Arc::clone(&halt_info);
            Some(thread::spawn(move || {
                stoplist_monitor_handler(halt_info, verbose);
            }))
        } else {
            None
        };

        Self {
            mpv_events,
            auto_pause,
            auto_stop,
            pauselist_monitor,
            stoplist_monitor,
        }
    }

    /// Shutdown all worker threads
    ///
    /// Sets stop_render_loop to true and waits for all threads to join
    pub fn shutdown_all(&mut self, halt_info: &HaltInfo) {
        // Signal all threads to stop
        halt_info.stop_render_loop.store(true, Ordering::SeqCst);

        // Join all threads
        if let Some(handle) = self.mpv_events.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.auto_pause.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.auto_stop.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.pauselist_monitor.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stoplist_monitor.take() {
            let _ = handle.join();
        }
    }
}

/// MPV event handler thread function
///
/// Handles:
/// - Slideshow timer (time-based, drift-resistant)
/// - User pause tracking (by polling mpv pause state)
/// - Auto-unpause when is_paused == 0 && !user_paused
fn mpv_event_handler(
    halt_info: Arc<HaltInfo>,
    mpv: Arc<MpvState>,
    slideshow_time: Option<u32>,
    _verbose: u8,
) {
    // Time-based slideshow timer (drift prevention)
    let mut last_slideshow_advance = Instant::now();

    // Track previous pause state to detect user-initiated changes
    let mut last_mpv_paused = mpv.is_paused();

    while !halt_info.stop_render_loop.load(Ordering::SeqCst) {
        // Slideshow timer (time-based)
        if let Some(interval) = slideshow_time {
            if last_slideshow_advance.elapsed() >= Duration::from_secs(interval as u64) {
                let _ = mpv.playlist_next();
                last_slideshow_advance = Instant::now();
            }
        }

        // Track user-initiated pause by polling mpv's pause state
        let current_mpv_paused = mpv.is_paused();
        if current_mpv_paused != last_mpv_paused {
            // Pause state changed
            if current_mpv_paused && halt_info.is_paused.load(Ordering::SeqCst) == 0 {
                // MPV is paused but no auto-pause is active = user action
                halt_info.user_paused.store(true, Ordering::SeqCst);
            } else if !current_mpv_paused {
                // User unpaused
                halt_info.user_paused.store(false, Ordering::SeqCst);
            }
            last_mpv_paused = current_mpv_paused;
        }

        // Auto-unpause logic (respects user manual pause)
        if halt_info.is_paused.load(Ordering::SeqCst) == 0
            && !halt_info.user_paused.load(Ordering::SeqCst)
        {
            // Only unpause if not already playing
            if mpv.is_paused() {
                let _ = mpv.unpause();
            }
        }

        thread::sleep(Duration::from_millis(20));
    }
}

/// Auto-pause handler thread function
///
/// Uses OutputFrameState as a deadman switch:
/// - Periodically resets all frame states
/// - If no output received a frame callback within 2 seconds, all outputs are hidden
/// - Pauses playback while hidden, resumes when any output becomes visible
fn auto_pause_handler(halt_info: Arc<HaltInfo>, mpv: Arc<MpvState>) {
    let output_frame_state = &halt_info.output_frame_state;

    while halt_info.auto_pause && !halt_info.stop_render_loop.load(Ordering::SeqCst) {
        // Reset deadman switch for all outputs
        output_frame_state.reset_all();

        // Wait 2 seconds (split into 100ms intervals for faster shutdown response)
        for _ in 0..20 {
            if halt_info.stop_render_loop.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }

        // If all outputs failed to receive frame callbacks, wallpaper is fully hidden
        // Only pause if not already paused (is_paused == 0)
        if output_frame_state.all_hidden() && halt_info.is_paused.load(Ordering::SeqCst) == 0 {
            let _ = mpv.pause();
            halt_info.is_paused.fetch_add(1, Ordering::SeqCst);

            // Wait until any output becomes visible again
            while !output_frame_state.any_ready()
                && !halt_info.stop_render_loop.load(Ordering::SeqCst)
            {
                thread::sleep(Duration::from_millis(100));
            }

            halt_info.is_paused.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// Auto-stop handler thread function
///
/// Similar to auto_pause but switches to holder when all outputs are hidden
fn auto_stop_handler(halt_info: Arc<HaltInfo>) {
    let output_frame_state = &halt_info.output_frame_state;

    while halt_info.auto_stop && !halt_info.stop_render_loop.load(Ordering::SeqCst) {
        // Reset deadman switch for all outputs
        output_frame_state.reset_all();

        // Wait 2 seconds (split into 100ms intervals)
        for _ in 0..20 {
            if halt_info.stop_render_loop.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }

        // If all outputs are hidden, stop and switch to holder
        if output_frame_state.all_hidden() {
            stop_mpvpaper_rs(&halt_info);
        }
    }
}

/// Stop mpvpaper-rs (sets stop signal, actual exec happens in main)
///
/// This only sets the stop_render_loop flag. The actual exec to holder
/// is performed in main() after the render loop terminates.
fn stop_mpvpaper_rs(halt_info: &HaltInfo) {
    // Signal render loop to stop
    // The main loop will check should_exec_holder() and exec if needed
    halt_info.stop_render_loop.store(true, Ordering::SeqCst);
}

/// Pauselist monitor thread function
///
/// Checks every second if any process in pauselist is running.
/// Pauses playback when a listed process is detected.
fn pauselist_monitor_handler(halt_info: Arc<HaltInfo>, mpv: Arc<MpvState>, verbose: u8) {
    let Some(ref pauselist) = halt_info.pauselist else {
        return;
    };

    while !halt_info.stop_render_loop.load(Ordering::SeqCst) {
        if let Some(app) = check_watch_list(pauselist) {
            if !halt_info.list_paused.load(Ordering::SeqCst) {
                cflp_info(1, verbose, &format!("Pausing for advancement of {}", app));
                let _ = mpv.pause();
                halt_info.list_paused.store(true, Ordering::SeqCst);
                halt_info.is_paused.fetch_add(1, Ordering::SeqCst);
            }
        } else if halt_info.list_paused.load(Ordering::SeqCst) {
            halt_info.list_paused.store(false, Ordering::SeqCst);
            halt_info.is_paused.fetch_sub(1, Ordering::SeqCst);
        }

        thread::sleep(Duration::from_secs(1));
    }
}

/// Stoplist monitor thread function
///
/// Checks every second if any process in stoplist is running.
/// Switches to holder when a listed process is detected.
fn stoplist_monitor_handler(halt_info: Arc<HaltInfo>, verbose: u8) {
    let Some(ref stoplist) = halt_info.stoplist else {
        return;
    };

    while !halt_info.stop_render_loop.load(Ordering::SeqCst) {
        if let Some(app) = check_watch_list(stoplist) {
            cflp_info(1, verbose, &format!("Stopping for advancement of {}", app));
            stop_mpvpaper_rs(&halt_info);
            return; // Exit thread after triggering stop
        }

        thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_halt_info_should_exec_holder() {
        let halt_info = HaltInfo::new(false, true); // auto_stop enabled
        assert!(!halt_info.should_exec_holder()); // Not set yet

        halt_info.stop_render_loop.store(true, Ordering::SeqCst);
        assert!(halt_info.should_exec_holder()); // Now should be true
    }

    #[test]
    fn test_halt_info_should_exec_holder_disabled() {
        let halt_info = HaltInfo::new(false, false); // auto_stop disabled
        halt_info.stop_render_loop.store(true, Ordering::SeqCst);
        assert!(!halt_info.should_exec_holder()); // Still false because auto_stop is disabled
    }
}
