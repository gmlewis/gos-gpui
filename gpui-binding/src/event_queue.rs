//! Thread-safe event queue ring buffer and window invalidation tracker.
//!
//! Solves the callback lifetime barrier via Event Inversion:
//! Rust pushes inert event descriptors into a thread-safe ring buffer,
//! and Gossamer polls and dispatches them natively.

use std::collections::{HashSet, VecDeque};
use std::sync::{LazyLock, Mutex};

#[derive(Clone, Debug, PartialEq)]
pub struct GpuIEvent {
    pub target_id: i64,
    pub event_kind: i64,
    pub payload_i64: i64,
    pub payload_f64: f64,
    pub payload_str: String,
}

static EVENT_QUEUE: LazyLock<Mutex<VecDeque<GpuIEvent>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

static WINDOW_INVALIDATIONS: LazyLock<Mutex<HashSet<i64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

static NOTIFICATIONS: LazyLock<Mutex<HashSet<i64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Pushes an event into the thread-safe queue.
pub fn push_event(target_id: i64, kind: i64, p_i64: i64, p_f64: f64, p_str: String) {
    let mut queue = EVENT_QUEUE.lock().unwrap();
    queue.push_back(GpuIEvent {
        target_id,
        event_kind: kind,
        payload_i64: p_i64,
        payload_f64: p_f64,
        payload_str: p_str,
    });
}

/// Polls and drains all pending events from the queue, returning formatted event strings.
/// Format: "{target_id}|{event_kind}|{payload_i64}|{payload_f64}|{payload_str}"
pub fn poll_events() -> Vec<String> {
    let mut queue = EVENT_QUEUE.lock().unwrap();
    let events: Vec<GpuIEvent> = queue.drain(..).collect();
    events
        .into_iter()
        .map(|ev| {
            format!(
                "{}|{}|{}|{}|{}",
                ev.target_id, ev.event_kind, ev.payload_i64, ev.payload_f64, ev.payload_str
            )
        })
        .collect()
}

/// Number of pending events in the queue.
pub fn event_queue_len() -> i64 {
    EVENT_QUEUE.lock().unwrap().len() as i64
}

/// Records a model or entity notification.
pub fn notify(target_id: i64) {
    NOTIFICATIONS.lock().unwrap().insert(target_id);
}

/// Marks a window as invalidated (requesting a repaint).
pub fn notify_window(window_id: i64) {
    WINDOW_INVALIDATIONS.lock().unwrap().insert(window_id);
}

/// Whether the specified window has a pending invalidation/repaint request.
pub fn is_window_invalidated(window_id: i64) -> bool {
    WINDOW_INVALIDATIONS.lock().unwrap().contains(&window_id)
}

/// Clears the invalidation flag for the specified window, answering whether it was invalidated.
pub fn clear_window_invalidation(window_id: i64) -> bool {
    WINDOW_INVALIDATIONS.lock().unwrap().remove(&window_id)
}

/// Whether target_id has a recorded notification.
#[allow(dead_code)]
pub fn is_notified(target_id: i64) -> bool {
    NOTIFICATIONS.lock().unwrap().contains(&target_id)
}

/// Clears all recorded target notifications.
#[allow(dead_code)]
pub fn clear_notifications() {
    NOTIFICATIONS.lock().unwrap().clear();
}
