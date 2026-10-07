//! Undo and redo over a store key.
//!
//! **The document is the store.** `editor.rs` publishes its content under the editor's key when it
//! changes and takes on whatever the program wrote there when that differs, so a history of the
//! *store's* text is a history of the document — and an undo is a write the editor adopts on its
//! next frame. It is a capability for a program with no editor as well: `ui::input` keeps its value
//! under `key`, so watching that key gives it undo too.
//!
//! A key is watched with [`watch_in`] and the text it holds at that moment is the baseline. Every
//! later change to it — from the program or from the editor, they are the same writer — is
//! [`note`]d. What makes the stack a *history* rather than a log is **coalescing**: a run of
//! insertions, or a run of deletions, is one step, because undoing a word one letter at a time is
//! not what anybody wants.
//!
//! The rule is arithmetic on two strings and nothing else — no timers, no keystroke counts — so it
//! is the same rule in a test as at a keyboard:
//!
//!   * a **pure insertion** is `before` with one contiguous run inserted, and a **pure deletion** is
//!     its mirror; anything else — a replacement, a paste over a selection — is neither;
//!   * two changes of the same kind in a row coalesce, and a change of kind does not;
//!   * a step that has been undone cannot be coalesced onto, and a new change after an undo throws
//!     the redo away, because there is no longer a line to go forward to.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// What kind of change a step was, which is all coalescing needs to know about it.
#[derive(Clone, Copy, PartialEq)]
enum Change {
    Insert,
    Delete,
    /// A replacement, a paste over a selection, or anything else that is not one run moved.
    Other,
}

/// One key's history: the texts it has held, and how far along they are.
struct History {
    /// The baseline, then the text after each step. `steps[0]` is what the key held when it was
    /// watched, before anything was typed.
    steps: Vec<String>,
    /// Which of them is currently applied. `0` is the baseline; `steps.len() - 1` is the newest.
    at: usize,
    /// The kind of the newest step, so a run of the same kind coalesces onto it.
    kind: Option<Change>,
}

/// One history per (window, key), because the store is per window and so is everything read from it.
static HISTORIES: LazyLock<Mutex<HashMap<(i64, String), History>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether `after` is `before` with one contiguous run inserted.
///
/// The longest common prefix and the longest common suffix together cover `before` exactly when
/// everything `before` has is still there in the same order: that is what "one run inserted" means.
/// Counted in bytes and only compared, never sliced, so a multi-byte character cannot split.
fn pure_insertion(before: &str, after: &str) -> bool {
    if after.len() <= before.len() {
        return false;
    }
    let prefix = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = before
        .bytes()
        .rev()
        .zip(after.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    prefix + suffix >= before.len()
}

/// Whether `after` is `before` with one contiguous run removed. The mirror of [`pure_insertion`].
fn pure_deletion(before: &str, after: &str) -> bool {
    pure_insertion(after, before)
}

/// What kind of change took `before` to `after`.
fn change_of(before: &str, after: &str) -> Change {
    if pure_insertion(before, after) {
        Change::Insert
    } else if pure_deletion(before, after) {
        Change::Delete
    } else {
        Change::Other
    }
}

/// Starts watching `key` in `win`, with `current` as the baseline.
///
/// Watching a key again starts over: the history it had is dropped and the text it holds now is the
/// baseline, which is the only sane reading of "begin recording from here".
pub fn watch_in(win: i64, key: &str, current: &str) {
    HISTORIES.lock().unwrap().insert(
        (win, key.to_string()),
        History {
            steps: vec![current.to_string()],
            at: 0,
            kind: None,
        },
    );
}

/// Stops watching `key`, dropping its history. Answers whether anything was being watched.
pub fn unwatch_in(win: i64, key: &str) -> bool {
    HISTORIES
        .lock()
        .unwrap()
        .remove(&(win, key.to_string()))
        .is_some()
}

/// Records that `key`'s text went from `before` to `after`. Does nothing when it is not watched.
pub fn note(win: i64, key: &str, before: &str, after: &str) {
    if before == after {
        return;
    }
    let mut all = HISTORIES.lock().unwrap();
    let Some(history) = all.get_mut(&(win, key.to_string())) else {
        return;
    };
    let change = change_of(before, after);

    // At the tip, and the same kind of change as the step being added to: coalesce.
    let at_tip = history.at + 1 == history.steps.len();
    if at_tip && change != Change::Other && history.kind == Some(change) {
        let last = history.steps.len() - 1;
        history.steps[last] = after.to_string();
        return;
    }

    // Otherwise it is a step of its own, and everything after the current one — the redo — is gone.
    history.steps.truncate(history.at + 1);
    history.steps.push(after.to_string());
    history.at = history.steps.len() - 1;
    history.kind = Some(change);
}

/// Steps back one step, answering the text that is now current, or `None` at the baseline.
pub fn undo_in(win: i64, key: &str) -> Option<String> {
    let mut all = HISTORIES.lock().unwrap();
    let history = all.get_mut(&(win, key.to_string()))?;
    if history.at == 0 {
        return None;
    }
    history.at -= 1;
    // A run cannot continue across an undo: the next insertion is a new step.
    history.kind = None;
    Some(history.steps[history.at].clone())
}

/// Steps forward one step, answering the text that is now current, or `None` at the newest.
pub fn redo_in(win: i64, key: &str) -> Option<String> {
    let mut all = HISTORIES.lock().unwrap();
    let history = all.get_mut(&(win, key.to_string()))?;
    if history.at + 1 >= history.steps.len() {
        return None;
    }
    history.at += 1;
    history.kind = None;
    Some(history.steps[history.at].clone())
}

/// Takes one step, forward or back, writing the text it lands on into the store the document
/// lives in — quietly, because an undo is not an edit. Answers whether there was a step to take.
///
/// One function for both directions and for every caller: `gpui::history_undo`, `gpui::history_redo`
/// and the editor's own `cmd-z` all go through this, so the binding's undo and the widget's cannot
/// land on different texts.
pub fn step(win: i64, key: &str, forward: bool) -> bool {
    let landed = if forward {
        redo_in(win, key)
    } else {
        undo_in(win, key)
    };
    match landed {
        Some(text) => {
            crate::store::set_text_quiet_in(win, key, &text);
            true
        }
        None => false,
    }
}

/// How many steps back it is possible to go.
pub fn depth_in(win: i64, key: &str) -> i64 {
    HISTORIES
        .lock()
        .unwrap()
        .get(&(win, key.to_string()))
        .map(|history| history.at as i64)
        .unwrap_or(0)
}

/// Whether there is a step to undo.
pub fn can_undo_in(win: i64, key: &str) -> bool {
    HISTORIES
        .lock()
        .unwrap()
        .get(&(win, key.to_string()))
        .map(|history| history.at > 0)
        .unwrap_or(false)
}

/// Whether there is a step to redo.
pub fn can_redo_in(win: i64, key: &str) -> bool {
    HISTORIES
        .lock()
        .unwrap()
        .get(&(win, key.to_string()))
        .map(|history| history.at + 1 < history.steps.len())
        .unwrap_or(false)
}
