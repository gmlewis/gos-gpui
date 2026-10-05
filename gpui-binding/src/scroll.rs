//! Scroll handles, one per `(window, element)`.
//!
//! A `ScrollHandle` is an `Rc<RefCell<…>>` — not `Send` — so it lives in a
//! thread-local map, like every other registry in this crate. It has to live
//! *outside* the element tree, because the tree is described from scratch every
//! frame: keeping the handle is what makes a scroll offset survive a rebuild, a
//! tab switch or a list recycle, and it is what lets the program read where a
//! container has been scrolled to.
//!
//! # The sign
//!
//! gpui's convention, kept verbatim rather than normalised: the offset is the
//! distance from the container's top-left to its first child's, so it is **zero
//! at the top and negative as the content scrolls down**, and it is clamped to
//! `-max_offset ..= 0`. `max_offset` is the *positive* amount of content that
//! can be scrolled past — the content size minus the container's. A scrollbar
//! thumb therefore sits at `-offset / max_offset`.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::ScrollHandle;

thread_local! {
    static HANDLES: RefCell<HashMap<(i64, String), ScrollHandle>> = RefCell::new(HashMap::new());
}

/// The handle for `id` in window `win`, created the first time it is asked for.
///
/// The render path calls this once per scrolled element per frame; the program's
/// own reads go through [`find`], which never creates one.
pub fn handle(win: i64, id: &str) -> ScrollHandle {
    HANDLES.with(|h| {
        h.borrow_mut()
            .entry((win, id.to_string()))
            .or_insert_with(ScrollHandle::new)
            .clone()
    })
}

/// The handle for `id` in window `win`, if the element has ever been rendered.
///
/// `None` is the ordinary answer for an id the program has not drawn a scrolled
/// container under, and for one drawn for the first time this frame — a reader
/// has no offset to report until the element has been laid out once.
pub fn find(win: i64, id: &str) -> Option<ScrollHandle> {
    HANDLES.with(|h| h.borrow().get(&(win, id.to_string())).cloned())
}
