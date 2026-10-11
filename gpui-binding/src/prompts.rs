//! Prompts a program asks for, one at a time.
//!
//! A prompt is a question with buttons, and gpui's own dialog — `Window::prompt`
//! (`gpui-0.2.2/src/window.rs:4141`), which hands back a `oneshot::Receiver<usize>` carrying the
//! index of the button clicked — can only be up once: a second one raised in the same frame is a
//! re-entrant call gpui refuses outright, and the panic is in gpui's own words
//! (`unreachable!("Re-entrant window prompting is not supported by GPUI")`). So two prompts in one
//! frame need a queue, and that queue is this module.
//!
//! **The queue is the whole of it, and the dialog is somebody else's.** A program reads the head
//! with [`head_in`] and [`Prompt::buttons`], draws it with the ordinary element tree — `ui::rect`
//! and `ui::button` are enough — and answers it with [`answer_in`], which raises an event naming the
//! prompt and the button. That is a complete dialog loop with no window involved, and it is the one
//! a check can reach. The *native* dialog would be a second consumer of the same queue: the binding
//! would take the head, call `Window::prompt`, await the receiver in a task and answer with the
//! index that comes back. It is not implemented, and the reason is in the register row: it cannot be
//! observed without a window, and it is not needed for a program to be able to ask.
//!
//! One queue per window, like every other registry here, and FIFO: a prompt waits its turn, and only
//! the head can be answered — which is the difference between a queue and a list of questions.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// One question waiting for an answer.
#[derive(Clone)]
pub struct Prompt {
    pub id: i64,
    pub title: String,
    pub message: String,
    /// The labels of the buttons, in the order they were given. Never empty: a prompt with nothing
    /// to click is refused rather than queued.
    pub buttons: Vec<String>,
}

thread_local! {
    static QUEUES: RefCell<HashMap<i64, Vec<Prompt>>> = RefCell::new(HashMap::new());
    /// Ids are never reused within a run, so an answer that arrives late names a prompt that is
    /// gone rather than a different one that took its number.
    static NEXT_ID: Cell<i64> = Cell::new(1);
}

/// Queues a prompt for `win`, with `buttons` a `;`-separated list of labels.
///
/// Answers the prompt's id, or `0` when there is nothing to answer with: a dialog with no buttons is
/// a dead end, and empty labels do not count. Whitespace around each label is trimmed, so
/// `"Yes ; No"` is two buttons.
pub fn ask_in(win: i64, title: &str, message: &str, buttons: &str) -> i64 {
    let labels: Vec<String> = buttons
        .split(';')
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
        .collect();
    if labels.is_empty() {
        return 0;
    }
    let id = NEXT_ID.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    });
    QUEUES.with(|queues| {
        queues.borrow_mut().entry(win).or_default().push(Prompt {
            id,
            title: title.to_string(),
            message: message.to_string(),
            buttons: labels,
        })
    });
    id
}

/// The prompt whose turn it is in `win`, or `None` when nothing is waiting.
pub fn head_in(win: i64) -> Option<Prompt> {
    QUEUES.with(|queues| {
        queues
            .borrow()
            .get(&win)
            .and_then(|queue| queue.first().cloned())
    })
}

/// Answers the head of `win`'s queue if it is `id`, and answers with the prompt that was answered.
///
/// `None` when that prompt is not the one whose turn it is — a queued prompt cannot be answered
/// before it comes up, which is what makes this a queue — or when `button` names no button.
pub fn answer_in(win: i64, id: i64, button: i64) -> Option<Prompt> {
    QUEUES.with(|queues| {
        let mut queues = queues.borrow_mut();
        let queue = queues.get_mut(&win)?;
        let head = queue.first()?;
        if head.id != id || button < 0 || button as usize >= head.buttons.len() {
            return None;
        }
        Some(queue.remove(0))
    })
}

/// Drops the prompt `id` wherever it is in `win`'s queue. Answers whether it was there.
///
/// Cancelling is not answering: a prompt that is dropped never takes its turn and raises nothing.
pub fn cancel_in(win: i64, id: i64) -> bool {
    QUEUES.with(|queues| {
        let mut queues = queues.borrow_mut();
        let Some(queue) = queues.get_mut(&win) else {
            return false;
        };
        let before = queue.len();
        queue.retain(|prompt| prompt.id != id);
        queue.len() != before
    })
}

/// Every prompt waiting in `win`, in order. The first is the one whose turn it is.
pub fn queue_in(win: i64) -> Vec<Prompt> {
    QUEUES.with(|queues| queues.borrow().get(&win).cloned().unwrap_or_default())
}

/// Drops `win`'s whole queue, however many there were — for a program that has moved on and does
/// not want yesterday's questions.
pub fn clear_in(win: i64) -> i64 {
    QUEUES.with(|queues| {
        queues
            .borrow_mut()
            .remove(&win)
            .map(|queue| queue.len() as i64)
            .unwrap_or(0)
    })
}

/// Answers `id` in `win` with the button at `button`, and raises `"prompt|<id>|<button>"` from that
/// window. Answers whether that was the one whose turn it was.
///
/// **One function, three callers**: `gpui::prompt_answer`, its `_in` twin, and the platform sheet's
/// answer in `app.rs`. A program that draws its own dialog and a program that lets the platform draw
/// one are therefore answered by the same code, and cannot come to mean different things.
pub fn answer_and_raise(win: i64, id: i64, button: i64) -> bool {
    if answer_in(win, id, button).is_none() {
        return false;
    }
    crate::store::push_event_with_in(win, "prompt", format!("{id}|{button}"));
    true
}

/// `"<id>|<title>|<message>"` — the head, as a program drawing its own dialog reads it.
///
/// The first two `|` are the separators; everything after the second is the message, so a message
/// may contain one.
pub fn describe(prompt: &Prompt) -> String {
    format!("{}|{}|{}", prompt.id, prompt.title, prompt.message)
}

/// `"<id>|<title>"` — one queued prompt in a listing, where the message is not wanted.
pub fn listed(prompt: &Prompt) -> String {
    format!("{}|{}", prompt.id, prompt.title)
}

/// One path selection prompt waiting for an answer.
#[derive(Clone)]
pub struct PathPrompt {
    pub id: i64,
    pub files: bool,
    pub directories: bool,
    pub multiple: bool,
    pub prompt: String,
}

/// One new path prompt waiting for an answer.
#[derive(Clone)]
pub struct NewPathPrompt {
    pub id: i64,
    pub directory: String,
    pub suggested_name: String,
}

thread_local! {
    static PATH_QUEUES: RefCell<HashMap<i64, Vec<PathPrompt>>> = RefCell::new(HashMap::new());
    static NEW_PATH_QUEUES: RefCell<HashMap<i64, Vec<NewPathPrompt>>> = RefCell::new(HashMap::new());
}

pub fn ask_path_in(win: i64, files: bool, directories: bool, multiple: bool, prompt: &str) -> i64 {
    let id = NEXT_ID.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    });
    PATH_QUEUES.with(|queues| {
        queues.borrow_mut().entry(win).or_default().push(PathPrompt {
            id,
            files,
            directories,
            multiple,
            prompt: prompt.to_string(),
        })
    });
    id
}

pub fn head_path_in(win: i64) -> Option<PathPrompt> {
    PATH_QUEUES.with(|queues| {
        queues.borrow().get(&win).and_then(|q| q.first().cloned())
    })
}

pub fn answer_path_in(win: i64, id: i64, paths: Vec<String>) -> bool {
    let removed = PATH_QUEUES.with(|queues| {
        let mut queues = queues.borrow_mut();
        let queue = queues.get_mut(&win)?;
        let head = queue.first()?;
        if head.id != id {
            return None;
        }
        Some(queue.remove(0))
    });
    if removed.is_some() {
        crate::store::push_event_with_in(win, "path_prompt", format!("{id}|{}", paths.join(";")));
        true
    } else {
        false
    }
}

pub fn ask_new_path_in(win: i64, directory: &str, suggested_name: &str) -> i64 {
    let id = NEXT_ID.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    });
    NEW_PATH_QUEUES.with(|queues| {
        queues.borrow_mut().entry(win).or_default().push(NewPathPrompt {
            id,
            directory: directory.to_string(),
            suggested_name: suggested_name.to_string(),
        })
    });
    id
}

pub fn head_new_path_in(win: i64) -> Option<NewPathPrompt> {
    NEW_PATH_QUEUES.with(|queues| {
        queues.borrow().get(&win).and_then(|q| q.first().cloned())
    })
}

pub fn answer_new_path_in(win: i64, id: i64, path: &str) -> bool {
    let removed = NEW_PATH_QUEUES.with(|queues| {
        let mut queues = queues.borrow_mut();
        let queue = queues.get_mut(&win)?;
        let head = queue.first()?;
        if head.id != id {
            return None;
        }
        Some(queue.remove(0))
    });
    if removed.is_some() {
        crate::store::push_event_with_in(win, "new_path_prompt", format!("{id}|{path}"));
        true
    } else {
        false
    }
}

