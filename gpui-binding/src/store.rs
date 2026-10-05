//! Application state, the event queue, and the requests a frame can raise.
//!
//! All three are process-wide: a `gpui::run` program has exactly one window
//! and one view callback, and Gossamer code reaches them through module
//! functions rather than through a handle it has to thread around.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};

#[derive(Default)]
pub struct Store {
    ints: HashMap<String, i64>,
    floats: HashMap<String, f64>,
    flags: HashMap<String, bool>,
    texts: HashMap<String, String>,
    lists: HashMap<String, Vec<String>>,
    float_lists: HashMap<String, Vec<f64>>,
}

pub static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

/// Events raised since the last frame was built. An entry is either a bare
/// `"id"` or `"id|payload"`.
static EVENTS: LazyLock<Mutex<VecDeque<String>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

/// Things a frame asked for that only the window/app context can do.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Emit `id` every `ms` milliseconds.
    Tick { id: String, ms: u64 },
    /// Do slow work off the UI thread, then emit `id`.
    Task { id: String, ms: u64 },
    /// Ask for another frame as soon as this one is on screen.
    Animate,
    /// Close the window and end the program.
    Quit,
    /// Move focus to the next focusable element.
    FocusNext,
    /// Move focus to the previous focusable element.
    FocusPrev,
    /// Close the window this frame belongs to.
    CloseWindow,
    /// Resize the window this frame belongs to.
    ResizeWindow { w: f64, h: f64 },
    /// Install an application menu.
    SetMenus { name: String, items: Vec<String> },
    /// Observe every keystroke the window receives.
    ObserveKeys { id: String },
}

static REQUESTS: LazyLock<Mutex<Vec<Request>>> = LazyLock::new(|| Mutex::new(Vec::new()));

pub fn request(r: Request) {
    REQUESTS.lock().unwrap().push(r);
}

pub fn take_requests() -> Vec<Request> {
    REQUESTS.lock().unwrap().drain(..).collect()
}

pub fn push_event(event: impl Into<String>) {
    EVENTS.lock().unwrap().push_back(event.into());
}

pub fn push_event_with(id: &str, payload: impl AsRef<str>) {
    push_event(format!("{id}|{}", payload.as_ref()));
}

pub fn take_events() -> Vec<String> {
    EVENTS.lock().unwrap().drain(..).collect()
}

// ---------------------------------------------------------------------------
// typed accessors
// ---------------------------------------------------------------------------

pub fn get_int(key: &str, default: i64) -> i64 {
    STORE.lock().unwrap().ints.get(key).copied().unwrap_or(default)
}

pub fn set_int(key: &str, value: i64) {
    STORE.lock().unwrap().ints.insert(key.to_string(), value);
}

pub fn get_float(key: &str, default: f64) -> f64 {
    STORE.lock().unwrap().floats.get(key).copied().unwrap_or(default)
}

pub fn set_float(key: &str, value: f64) {
    STORE.lock().unwrap().floats.insert(key.to_string(), value);
}

pub fn get_flag(key: &str, default: bool) -> bool {
    STORE.lock().unwrap().flags.get(key).copied().unwrap_or(default)
}

pub fn set_flag(key: &str, value: bool) {
    STORE.lock().unwrap().flags.insert(key.to_string(), value);
}

pub fn get_text(key: &str, default: &str) -> String {
    STORE
        .lock()
        .unwrap()
        .texts
        .get(key)
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

pub fn set_text(key: &str, value: &str) {
    STORE.lock().unwrap().texts.insert(key.to_string(), value.to_string());
}

pub fn push_text(key: &str, value: &str) {
    STORE
        .lock()
        .unwrap()
        .lists
        .entry(key.to_string())
        .or_default()
        .push(value.to_string());
}

pub fn get_texts(key: &str) -> Vec<String> {
    STORE.lock().unwrap().lists.get(key).cloned().unwrap_or_default()
}

pub fn set_texts(key: &str, values: Vec<String>) {
    STORE.lock().unwrap().lists.insert(key.to_string(), values);
}

pub fn text_len(key: &str) -> i64 {
    STORE.lock().unwrap().lists.get(key).map_or(0, |v| v.len() as i64)
}

pub fn text_at(key: &str, index: i64) -> String {
    STORE
        .lock()
        .unwrap()
        .lists
        .get(key)
        .and_then(|v| v.get(index.max(0) as usize))
        .cloned()
        .unwrap_or_default()
}

pub fn text_remove(key: &str, index: i64) {
    if let Some(v) = STORE.lock().unwrap().lists.get_mut(key) {
        if (index as usize) < v.len() {
            v.remove(index as usize);
        }
    }
}

pub fn text_insert(key: &str, index: i64, value: &str) {
    if let Some(v) = STORE.lock().unwrap().lists.get_mut(key) {
        v.insert(index.max(0) as usize, value.to_string());
    }
}

pub fn push_float(key: &str, value: f64) {
    STORE
        .lock()
        .unwrap()
        .float_lists
        .entry(key.to_string())
        .or_default()
        .push(value);
}

pub fn get_floats(key: &str) -> Vec<f64> {
    STORE
        .lock()
        .unwrap()
        .float_lists
        .get(key)
        .cloned()
        .unwrap_or_default()
}

pub fn forget(key: &str) {
    let mut s = STORE.lock().unwrap();
    s.ints.remove(key);
    s.floats.remove(key);
    s.flags.remove(key);
    s.texts.remove(key);
    s.lists.remove(key);
    s.float_lists.remove(key);
}
