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
    push_event_in(window::current(), event);
}

pub fn push_event_with(id: &str, payload: impl AsRef<str>) {
    push_event_with_in(window::current(), id, payload);
}

/// Raises `event` for `win`, which the caller names.
pub fn push_event_in(win: i64, event: impl Into<String>) {
    let mut events = EVENTS.lock().unwrap();
    events.entry(win).or_default().push_back(event.into());
}

pub fn push_event_with_in(win: i64, id: &str, payload: impl AsRef<str>) {
    push_event_in(win, format!("{id}|{}", payload.as_ref()));
}

/// Every event raised for the current window since the previous frame, drained.
pub fn take_events() -> Vec<String> {
    take_events_in(window::current())
}

/// Every event raised for `win` since the previous frame, drained.
///
/// The twin the rest of this module's accessors have, and the one that makes the per-window
/// question answerable from `gos test`, where the current window is 0 and a test can still address
/// window 1 and window 2 by hand.
pub fn take_events_in(win: i64) -> Vec<String> {
    let mut events = EVENTS.lock().unwrap();
    events.remove(&win).map(|queue| queue.into_iter().collect()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// who hears about what
// ---------------------------------------------------------------------------

/// The subscribers of each topic: the ids [`subscribe_in`] recorded, in the order it recorded
/// them, one roster per (window, topic).
///
/// This is the shape `cx.emit` and `cx.subscribe` take for a program with no entity graph. A
/// subscriber is an **event id**, not a callback: the program already handles its events once a
/// frame, and a topic that raised `"<id>|<payload>"` is heard in the same place as everything else.
static SUBSCRIBERS: LazyLock<Mutex<HashMap<(i64, String), Vec<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Records that `id` wants to hear `topic` in `win`. Answers `false` when it already did, so a
/// program that subscribes every frame — which is what a rebuild-every-frame program does — does
/// not hear every event once per frame it has been subscribed.
pub fn subscribe_in(win: i64, topic: &str, id: &str) -> bool {
    let mut rosters = SUBSCRIBERS.lock().unwrap();
    let roster = rosters.entry((win, topic.to_string())).or_default();
    if roster.iter().any(|existing| existing == id) {
        return false;
    }
    roster.push(id.to_string());
    true
}

/// Removes `id` from `topic` in `win`. Answers `false` when it was not there.
pub fn unsubscribe_in(win: i64, topic: &str, id: &str) -> bool {
    let mut rosters = SUBSCRIBERS.lock().unwrap();
    let key = (win, topic.to_string());
    let Some(roster) = rosters.get_mut(&key) else {
        return false;
    };
    let before = roster.len();
    roster.retain(|existing| existing != id);
    let removed = roster.len() != before;
    if roster.is_empty() {
        rosters.remove(&key);
    }
    removed
}

/// The ids subscribed to `topic` in `win`, in the order they subscribed.
pub fn subscribers_in(win: i64, topic: &str) -> Vec<String> {
    SUBSCRIBERS
        .lock()
        .unwrap()
        .get(&(win, topic.to_string()))
        .cloned()
        .unwrap_or_default()
}

/// Every topic that has a subscriber in `win`, sorted, so a program can print them.
pub fn topics_in(win: i64) -> Vec<String> {
    let mut topics: Vec<String> = SUBSCRIBERS
        .lock()
        .unwrap()
        .iter()
        .filter(|((window, _), roster)| *window == win && !roster.is_empty())
        .map(|((_, topic), _)| topic.clone())
        .collect();
    topics.sort();
    topics
}

/// Raises `"<id>|<payload>"` in `win` for every subscriber of `topic`, and answers how many heard
/// it.
///
/// A topic nobody is on raises nothing and answers `0` — a signal nobody is listening for is not
/// an error. The subscriber list is copied before anything is raised, so a subscriber that
/// unsubscribes while the events are being drained still heard this one.
pub fn publish_in(win: i64, topic: &str, payload: &str) -> i64 {
    let roster = subscribers_in(win, topic);
    for id in &roster {
        push_event_with_in(win, id.as_str(), payload);
    }
    roster.len() as i64
}

// ---------------------------------------------------------------------------
// requests
// ---------------------------------------------------------------------------

/// Things a frame asked for that only the window/app context can do.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Emit `id` every `ms` milliseconds, rescheduling it if it already runs.
    Tick { id: String, ms: u64 },
    /// Stop the repeating timer `id`.
    TickStop { id: String },
    /// Raise `id` once, `ms` milliseconds from now.
    Delay { id: String, ms: u64 },
    /// Take back a pending one-shot delay.
    DelayCancel { id: String },
    /// Start, or restart, a named animation that runs for `ms` and eases by `easing`.
    Animation { id: String, ms: u64, easing: String },
    /// Hold a running animation where it is.
    AnimationPause { id: String },
    /// Carry on from where it was held.
    AnimationResume { id: String },
    /// Take an animation away.
    AnimationStop { id: String },
    /// Stop background work for `id` that is still in flight.
    TaskCancel { id: String },
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
    /// Ask which window the platform calls active, for the asking window.
    AskActiveWindow,
    /// Hide the whole application, as the platform's Hide does.
    HideApplication,
    /// Ask the platform to re-read its windows — after a display change.
    RefreshWindows,
    /// Show a path in the platform's file manager.
    Reveal { path: String },
    /// Hand a path to the platform's own handler for its kind.
    OpenWithSystem { path: String },
    /// Open a URL in whatever the platform uses for URLs.
    OpenUrl { url: String },
    /// Open the platform's file or folder picker for `id`.
    OpenPicker { id: String, what: String, prompt: String },
    /// Open the platform's save panel for `id`.
    SavePicker { id: String, directory: String, suggested: String },
    /// Give focus to the element with this id, if it has a handle.
    Focus { id: String },
    /// Set the window's title, and read back what the platform says it is now.
    SetTitle { title: String },
    /// Write the current window's bounds to a settings file under `key`.
    ResizeWindow { w: f64, h: f64 },
    /// Toggle the window in and out of fullscreen.
    ToggleFullscreen,
    /// Zoom (maximize) the window, or put it back to the size it had.
    ZoomWindow,
    /// Ask the platform who draws the window's decoration: the window manager
    /// (`false`) or the program itself (`true`).
    RequestDecorations { client: bool },
    /// Tell the platform how wide the invisible strip a program draws its own
    /// window decoration in is, so that anything the platform draws around the
    /// window is offset by it.
    SetClientInset { px: f64 },
    /// Hand the window to the window manager to be dragged until the button that
    /// asked for it comes up. This is what a program-drawn titlebar calls on a
    /// press, and it is `gpui::Window::start_window_move`.
    StartWindowMove,
    /// Hand the window to the window manager to be resized by `edge`, for as long
    /// as the button is held. `edge` is one of `top-left`, `top`, `top-right`,
    /// `right`, `bottom-right`, `bottom`, `bottom-left`, `left`.
    StartWindowResize { edge: String },
    /// Set the window's rem size, which every `rem` length and every ramp size
    /// is measured against.
    SetRemSize { px: f64 },
    /// Install an application menu.
    SetMenus { name: String, items: Vec<String> },
    /// Write the current window's bounds to a settings file under `key`.
    SaveWindowState { path: String, key: String },
    /// Install the menu the dock icon offers on a right-click.
    SetDockMenu { items: Vec<String> },
    /// Bind the program's own keymap: `"<stroke>=<name>"` pairs, `;`-separated.
    BindKeys { spec: String },
    /// Observe every keystroke the window receives.
    ObserveKeys { id: String },
    /// Deliver a keystroke to the window as if it had been typed.
    SendKey { stroke: String },
    /// Deliver `text` to a field as a composition, the way an input method does.
    Compose { key: String, text: String },
    /// Raise `id` with the new geometry whenever the window is moved or resized.
    ObserveBounds { id: String },
    /// Raise `id` with the new appearance whenever the system's changes.
    ObserveAppearance { id: String },
    /// Choose which of the platform's two palettes the colour table answers from.
    SetTheme { name: String },
    /// Put `text` on the system clipboard.
    SetClipboard { text: String },
    /// Throw away what has been loaded for the asset at `path`, so the next frame
    /// loads it again.
    ForgetAsset { path: String },
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
    // Read the old value first, write, and only then tell the history — in three statements rather
    // than one, because the history takes its own lock and nothing here holds both at once.
    let before = get_text_in(win, key, "");
    store(win, |s| s.texts.insert(key.to_string(), value.to_string()));
    crate::history::note(win, key, &before, value);
}

/// Writes text **without** recording a step, for a writer that *is* the history.
///
/// An undo puts an older text back; recording that as a change would make undoing an edit into
/// editing again, which is exactly the loop a history exists to avoid.
pub fn set_text_quiet_in(win: i64, key: &str, value: &str) {
    store(win, |s| s.texts.insert(key.to_string(), value.to_string()));
}

pub fn set_text_quiet(key: &str, value: &str) {
    set_text_quiet_in(window::current(), key, value)
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
