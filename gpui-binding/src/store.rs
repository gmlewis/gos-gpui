//! Application state, the event queue, and the requests a frame can raise.
//!
//! All three are scoped to one window: each `gpui::run` window has its own view
//! callback, its own state, its own events and its own requests, and Gossamer
//! reaches them through module functions rather than through a handle it has to
//! thread around. Which window that is comes from [`crate::window`], which
//! tracks the frame — or the callback — running on this thread.
//!
//! Every accessor has an `_in` twin that takes the window explicitly. That is
//! what makes the scoping testable without a window at all: `gos test` runs
//! outside every frame, where the current window is `0`, and can still address
//! window 1 and window 2 by hand and show that they do not see each other.

use std::collections::{HashMap, VecDeque};
use std::sync::{LazyLock, Mutex};

use crate::window;

#[derive(Default)]
pub struct Store {
    ints: HashMap<String, i64>,
    floats: HashMap<String, f64>,
    flags: HashMap<String, bool>,
    texts: HashMap<String, String>,
    lists: HashMap<String, Vec<String>>,
    float_lists: HashMap<String, Vec<f64>>,
}

/// One [`Store`] per window. Window `0` is the one a program running outside a
/// frame — `gos test` — reads and writes.
static STORES: LazyLock<Mutex<HashMap<i64, Store>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Runs `f` against `win`'s store, creating it the first time the window is
/// touched. One lock covers every window, so a mutation cannot interleave with
/// a read of another.
fn store<T>(win: i64, f: impl FnOnce(&mut Store) -> T) -> T {
    let mut stores = STORES.lock().unwrap();
    f(stores.entry(win).or_default())
}

// ---------------------------------------------------------------------------
// events
// ---------------------------------------------------------------------------

/// Events raised since the last frame was built, one queue per window. An entry
/// is either a bare `"id"` or `"id|payload"`.
static EVENTS: LazyLock<Mutex<HashMap<i64, VecDeque<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Raises `event` for the window that is current.
pub fn push_event(event: impl Into<String>) {
    let win = window::current();
    let mut events = EVENTS.lock().unwrap();
    events.entry(win).or_default().push_back(event.into());
}

pub fn push_event_with(id: &str, payload: impl AsRef<str>) {
    push_event(format!("{id}|{}", payload.as_ref()));
}

/// Every event raised for the current window since the previous frame, drained.
pub fn take_events() -> Vec<String> {
    let win = window::current();
    let mut events = EVENTS.lock().unwrap();
    events.remove(&win).map(|queue| queue.into_iter().collect()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// requests
// ---------------------------------------------------------------------------

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
    /// Close another window, named by the id [`crate::window::current`] would
    /// have answered in it.
    CloseWindowAt { target: i64 },
    /// Resize the window this frame belongs to.
    ResizeWindow { w: f64, h: f64 },
    /// Set the window's rem size, which every `rem` length and every ramp size
    /// is measured against.
    SetRemSize { px: f64 },
    /// Install an application menu.
    SetMenus { name: String, items: Vec<String> },
    /// Observe every keystroke the window receives.
    ObserveKeys { id: String },
}

/// Pending requests, one queue per window. A frame is answered by the window it
/// was built in, so `close_window()` in window A cannot close window B.
static REQUESTS: LazyLock<Mutex<HashMap<i64, Vec<Request>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn request(r: Request) {
    let win = window::current();
    let mut requests = REQUESTS.lock().unwrap();
    requests.entry(win).or_default().push(r);
}

pub fn take_requests() -> Vec<Request> {
    let win = window::current();
    let mut requests = REQUESTS.lock().unwrap();
    requests.remove(&win).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// typed accessors
// ---------------------------------------------------------------------------

pub fn get_int_in(win: i64, key: &str, default: i64) -> i64 {
    store(win, |s| s.ints.get(key).copied()).unwrap_or(default)
}

pub fn set_int_in(win: i64, key: &str, value: i64) {
    store(win, |s| s.ints.insert(key.to_string(), value));
}

pub fn get_int(key: &str, default: i64) -> i64 {
    get_int_in(window::current(), key, default)
}

pub fn set_int(key: &str, value: i64) {
    set_int_in(window::current(), key, value)
}

pub fn get_float_in(win: i64, key: &str, default: f64) -> f64 {
    store(win, |s| s.floats.get(key).copied()).unwrap_or(default)
}

pub fn set_float_in(win: i64, key: &str, value: f64) {
    store(win, |s| s.floats.insert(key.to_string(), value));
}

pub fn get_float(key: &str, default: f64) -> f64 {
    get_float_in(window::current(), key, default)
}

pub fn set_float(key: &str, value: f64) {
    set_float_in(window::current(), key, value)
}

pub fn get_flag_in(win: i64, key: &str, default: bool) -> bool {
    store(win, |s| s.flags.get(key).copied()).unwrap_or(default)
}

pub fn set_flag_in(win: i64, key: &str, value: bool) {
    store(win, |s| s.flags.insert(key.to_string(), value));
}

pub fn get_flag(key: &str, default: bool) -> bool {
    get_flag_in(window::current(), key, default)
}

pub fn set_flag(key: &str, value: bool) {
    set_flag_in(window::current(), key, value)
}

pub fn get_text_in(win: i64, key: &str, default: &str) -> String {
    store(win, |s| s.texts.get(key).cloned()).unwrap_or_else(|| default.to_string())
}

pub fn set_text_in(win: i64, key: &str, value: &str) {
    store(win, |s| s.texts.insert(key.to_string(), value.to_string()));
}

pub fn get_text(key: &str, default: &str) -> String {
    get_text_in(window::current(), key, default)
}

pub fn set_text(key: &str, value: &str) {
    set_text_in(window::current(), key, value)
}

// ---- list-valued state ------------------------------------------------

pub fn push_text(key: &str, value: &str) {
    let win = window::current();
    store(win, |s| {
        s.lists.entry(key.to_string()).or_default().push(value.to_string())
    });
}

pub fn get_texts_in(win: i64, key: &str) -> Vec<String> {
    store(win, |s| s.lists.get(key).cloned()).unwrap_or_default()
}

pub fn get_texts(key: &str) -> Vec<String> {
    get_texts_in(window::current(), key)
}

pub fn set_texts_in(win: i64, key: &str, values: Vec<String>) {
    store(win, |s| s.lists.insert(key.to_string(), values));
}

pub fn set_texts(key: &str, values: Vec<String>) {
    set_texts_in(window::current(), key, values)
}

pub fn text_len_in(win: i64, key: &str) -> i64 {
    store(win, |s| s.lists.get(key).map_or(0, |v| v.len() as i64))
}

pub fn text_len(key: &str) -> i64 {
    text_len_in(window::current(), key)
}

pub fn text_at_in(win: i64, key: &str, index: i64) -> String {
    store(win, |s| {
        s.lists.get(key).and_then(|v| v.get(index.max(0) as usize)).cloned()
    })
    .unwrap_or_default()
}

pub fn text_at(key: &str, index: i64) -> String {
    text_at_in(window::current(), key, index)
}

pub fn text_remove_in(win: i64, key: &str, index: i64) {
    store(win, |s| {
        if let Some(v) = s.lists.get_mut(key) {
            if (index as usize) < v.len() {
                v.remove(index as usize);
            }
        }
    });
}

pub fn text_remove(key: &str, index: i64) {
    text_remove_in(window::current(), key, index)
}

pub fn text_insert_in(win: i64, key: &str, index: i64, value: &str) {
    store(win, |s| {
        if let Some(v) = s.lists.get_mut(key) {
            v.insert(index.max(0) as usize, value.to_string());
        }
    });
}

pub fn text_insert(key: &str, index: i64, value: &str) {
    text_insert_in(window::current(), key, index, value)
}

pub fn push_float(key: &str, value: f64) {
    let win = window::current();
    store(win, |s| {
        s.float_lists.entry(key.to_string()).or_default().push(value)
    });
}

pub fn get_floats_in(win: i64, key: &str) -> Vec<f64> {
    store(win, |s| s.float_lists.get(key).cloned()).unwrap_or_default()
}

pub fn get_floats(key: &str) -> Vec<f64> {
    get_floats_in(window::current(), key)
}

pub fn forget_in(win: i64, key: &str) {
    store(win, |s| {
        s.ints.remove(key);
        s.floats.remove(key);
        s.flags.remove(key);
        s.texts.remove(key);
        s.lists.remove(key);
        s.float_lists.remove(key);
    });
}

pub fn forget(key: &str) {
    forget_in(window::current(), key)
}
