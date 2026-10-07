//! A settings file: the few strings and numbers a program wants to survive a restart.
//!
//! One `key=value` per line, so a person can read the file and a program can write it without a
//! schema, and so a stale key is something you can see and delete by hand. The path is the
//! program's to choose — this module never guesses one — and the file is rewritten whole on every
//! change, which is what keeps the two halves of a window's state from going in separately.
//!
//! The window-shaped half of it is three functions on top: [`remember_bounds`] writes the four
//! numbers a window's bounds are, [`bounds`] reads them back as the `"x,y,w,h"` string
//! `gpui::observe_window_bounds` raises, and [`field`] answers one of them at a time for a program
//! putting them into an `open_window` call.
//!
//! **A half-written window state reads as nothing.** If any of the four is missing or is not a
//! number, [`bounds`] is empty and [`field`] is `0.0` — a window that opens at 0,0 because three
//! of its four numbers survived is worse than one that opens where it was told to.

use std::fs;

/// The four numbers a window's bounds are, in the order they are stored and reported.
pub const BOUND_FIELDS: [&str; 4] = ["x", "y", "w", "h"];

/// Reads a settings file into its keys and values, in the order the file has them.
///
/// A file that is not there contributes nothing — a program's first run has nothing to read, and
/// that is not a failure — and so does a line that is not a `key=value`, so a file somebody edited
/// by hand stays readable.
fn load(path: &str) -> Vec<(String, String)> {
    fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .filter_map(|line| line.split_once('='))
                .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
                .filter(|(key, _)| !key.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Writes the whole file, creating the directory above it if it is not there.
fn store(path: &str, entries: &[(String, String)]) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|problem| problem.to_string())?;
        }
    }
    let text: String = entries
        .iter()
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect();
    fs::write(path, text).map_err(|problem| problem.to_string())
}

/// Writes one setting, replacing it if it was already there.
pub fn set(path: &str, key: &str, value: &str) -> Result<(), String> {
    let mut entries = load(path);
    match entries.iter_mut().find(|(existing, _)| existing == key) {
        Some(entry) => entry.1 = value.to_string(),
        None => entries.push((key.to_string(), value.to_string())),
    }
    store(path, &entries)
}

/// One setting, or `""` when it is not there.
pub fn get(path: &str, key: &str) -> String {
    load(path)
        .into_iter()
        .find(|(existing, _)| existing == key)
        .map(|(_, value)| value)
        .unwrap_or_default()
}

/// Removes one setting and leaves the rest.
pub fn forget(path: &str, key: &str) -> Result<(), String> {
    let entries: Vec<(String, String)> = load(path)
        .into_iter()
        .filter(|(existing, _)| existing != key)
        .collect();
    store(path, &entries)
}

/// Remembers a window's bounds under `key`, as the four numbers of [`BOUND_FIELDS`].
pub fn remember_bounds(
    path: &str,
    key: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<(), String> {
    let mut entries = load(path);
    for (field, value) in BOUND_FIELDS.iter().zip([x, y, w, h]) {
        let full = format!("{key}.{field}");
        let text = number(value);
        match entries.iter_mut().find(|(existing, _)| *existing == full) {
            Some(entry) => entry.1 = text,
            None => entries.push((full, text)),
        }
    }
    store(path, &entries)
}

/// A remembered window's bounds as `"x,y,w,h"`, or `""` when the state is not whole.
pub fn bounds(path: &str, key: &str) -> String {
    let mut parts = Vec::new();
    for field in BOUND_FIELDS {
        let Ok(value) = get(path, &format!("{key}.{field}")).parse::<f64>() else {
            return String::new();
        };
        parts.push(number(value));
    }
    parts.join(",")
}

/// One field of a remembered window, or `0.0` when the state is not whole.
pub fn field(path: &str, key: &str, field: &str) -> f64 {
    let whole = bounds(path, key);
    if whole.is_empty() {
        return 0.0;
    }
    whole
        .split(',')
        .zip(BOUND_FIELDS)
        .find(|(_, name)| *name == field)
        .and_then(|(value, _)| value.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// A number as text, without a trailing `.0`, because a settings file is read by people.
fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}
