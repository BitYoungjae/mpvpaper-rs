use dashmap::DashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use wayland_client::backend::ObjectId;

/// Per-output frame callback state (multi-output support)
///
/// Deadman switch: tracks whether each output received a frame callback.
/// Used by auto_pause/auto_stop to determine if all outputs are hidden.
///
/// CRITICAL: OutputHandler's new_output/output_destroyed must call
/// add_output/remove_output respectively.
/// Otherwise frame_ready map will be empty and all_hidden() always returns false
/// -> auto_pause/auto_stop never triggers.
///
/// Usage pattern:
/// 1. OutputHandler::new_output calls add_output(output.id())
/// 2. OutputHandler::output_destroyed calls remove_output(&output.id())
/// 3. CompositorHandler::frame calls mark_ready(&output.id())
/// 4. Auto handler periodically calls reset_all() and checks any_ready()/all_hidden()
pub struct OutputFrameState {
    /// Output ID -> frame callback received flag
    frame_ready: DashMap<ObjectId, AtomicBool>,
}

impl OutputFrameState {
    pub fn new() -> Self {
        Self {
            frame_ready: DashMap::new(),
        }
    }

    /// Add output (MUST be called in OutputHandler::new_output!)
    pub fn add_output(&self, id: ObjectId) {
        self.frame_ready.insert(id, AtomicBool::new(false));
    }

    /// Remove output (MUST be called in OutputHandler::output_destroyed!)
    pub fn remove_output(&self, id: &ObjectId) {
        self.frame_ready.remove(id);
    }

    /// Mark frame callback received for specific output (called in frame callback)
    pub fn mark_ready(&self, id: &ObjectId) {
        if let Some(ready) = self.frame_ready.get(id) {
            ready.store(true, Ordering::SeqCst);
        }
    }

    /// Reset all output frame states (called periodically by auto handler)
    pub fn reset_all(&self) {
        for entry in self.frame_ready.iter() {
            entry.value().store(false, Ordering::SeqCst);
        }
    }

    /// Check if at least one output received a frame callback
    pub fn any_ready(&self) -> bool {
        self.frame_ready
            .iter()
            .any(|e| e.value().load(Ordering::SeqCst))
    }

    /// Check if all outputs failed to receive frame callbacks (all hidden)
    pub fn all_hidden(&self) -> bool {
        !self.frame_ready.is_empty()
            && self
                .frame_ready
                .iter()
                .all(|e| !e.value().load(Ordering::SeqCst))
    }
}

impl Default for OutputFrameState {
    fn default() -> Self {
        Self::new()
    }
}

pub struct HaltInfo {
    /// Watch lists
    pub pauselist: Option<Vec<String>>,
    pub stoplist: Option<Vec<String>>,

    /// Auto features
    pub auto_pause: bool,
    pub auto_stop: bool,

    /// State (shared across threads)
    /// Pause counter: >0 means paused (multiple sources can pause)
    pub is_paused: AtomicI32,
    /// Per-output frame state (deadman switch - unified via OutputFrameState)
    pub output_frame_state: Arc<OutputFrameState>,
    /// Signal to stop the render loop
    pub stop_render_loop: AtomicBool,
    /// Track if paused by watchlist
    pub list_paused: AtomicBool,
    /// Track if user manually paused (prevents auto-unpause override)
    pub user_paused: AtomicBool,
    /// Signal exit requested (SIGINT/SIGTERM) - prevents holder transition
    pub signal_exit: AtomicBool,
}

impl Clone for HaltInfo {
    fn clone(&self) -> Self {
        Self {
            pauselist: self.pauselist.clone(),
            stoplist: self.stoplist.clone(),
            auto_pause: self.auto_pause,
            auto_stop: self.auto_stop,
            is_paused: AtomicI32::new(self.is_paused.load(Ordering::SeqCst)),
            output_frame_state: Arc::clone(&self.output_frame_state),
            stop_render_loop: AtomicBool::new(self.stop_render_loop.load(Ordering::SeqCst)),
            list_paused: AtomicBool::new(self.list_paused.load(Ordering::SeqCst)),
            user_paused: AtomicBool::new(self.user_paused.load(Ordering::SeqCst)),
            signal_exit: AtomicBool::new(self.signal_exit.load(Ordering::SeqCst)),
        }
    }
}

impl HaltInfo {
    pub fn new(auto_pause: bool, auto_stop: bool) -> Self {
        Self {
            pauselist: None,
            stoplist: None,
            auto_pause,
            auto_stop,
            is_paused: AtomicI32::new(0),
            output_frame_state: Arc::new(OutputFrameState::new()),
            stop_render_loop: AtomicBool::new(false),
            list_paused: AtomicBool::new(false),
            user_paused: AtomicBool::new(false),
            signal_exit: AtomicBool::new(false),
        }
    }

    /// Check if the program should exec holder on exit
    ///
    /// Returns true if auto_stop is enabled and stop_render_loop was set
    /// (indicating the wallpaper was hidden and we should switch to holder)
    /// but NOT if exit was triggered by signal (SIGINT/SIGTERM)
    pub fn should_exec_holder(&self) -> bool {
        self.auto_stop
            && self.stop_render_loop.load(Ordering::SeqCst)
            && !self.signal_exit.load(Ordering::SeqCst)
    }
}

impl Default for HaltInfo {
    fn default() -> Self {
        Self::new(false, false)
    }
}
