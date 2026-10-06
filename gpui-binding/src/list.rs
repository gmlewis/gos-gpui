//! Scroll handles and visible ranges for virtual lists, one per
//! `(window, list)`.
//!
//! A `uniform_list` is the one scrolled thing in this crate that is not a plain
//! `div`, so it needs its own registry beside [`crate::scroll`]'s. It has the
//! same shape and the same reason: `UniformListScrollHandle` is an
//! `Rc<RefCell<…>>`, not `Send`, so it lives in a thread-local map, and it has to
//! live outside the element tree because the tree is described from scratch
//! every frame.
//!
//! # Why the range is recorded here rather than read back from gpui
//!
//! gpui's `UniformListScrollHandle::logical_scroll_top_index` would answer "which
//! row is at the top", but it is `#[cfg(any(test, feature = "test-support"))]`
//! (`gpui-0.2.2/src/elements/uniform_list.rs:218`) — it does not exist in a
//! normal build. What *is* available is the closure gpui hands the list when it
//! lays out, whose argument is exactly the range of rows that are on screen. So
//! the binding records that range as it builds the rows, and answers it back.
//! That is the same number, measured at the only place it is reliably visible.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{ScrollHandle, UniformListScrollHandle};

thread_local! {
    static HANDLES: RefCell<HashMap<(i64, String), UniformListScrollHandle>> =
        RefCell::new(HashMap::new());
    static RANGES: RefCell<HashMap<(i64, String), (usize, usize)>> =
        RefCell::new(HashMap::new());
}

/// The handle for `id` in window `win`, created the first time it is asked for.
pub fn handle(win: i64, id: &str) -> UniformListScrollHandle {
    HANDLES.with(|h| {
        h.borrow_mut()
            .entry((win, id.to_string()))
            .or_insert_with(UniformListScrollHandle::new)
            .clone()
    })
}

/// The handle for `id` in window `win`, if the list has ever been rendered.
pub fn find(win: i64, id: &str) -> Option<UniformListScrollHandle> {
    HANDLES.with(|h| h.borrow().get(&(win, id.to_string())).cloned())
}

/// Records the rows the list for `id` built the last time it laid out.
///
/// `first` and `last` are inclusive, so an empty range is never stored — a list
/// with no rows on screen keeps whatever range it had, rather than reporting
/// that row 0 is visible.
pub fn set_range(win: i64, id: &str, first: usize, last: usize) {
    if last < first {
        return;
    }
    RANGES.with(|r| {
        r.borrow_mut().insert((win, id.to_string()), (first, last));
    });
}

/// The plain `ScrollHandle` underneath a virtual list's own.
///
/// gpui runs a `uniform_list`'s scrolling through a `ScrollHandle` kept inside
/// the `UniformListScrollHandle`, and hands that same handle to the inner
/// scroller (`gpui-0.2.2/src/elements/uniform_list.rs:676`). The list then shares
/// the handle's offset cell outright — `uniform_list.rs:349` clones the same
/// `Rc` into the list's own layout state — so a list's offset, its maximum and
/// its `set_offset` are the same three operations a plain scrolled container
/// has, through the same type.
///
/// That is what lets `gpui::scroll_offset`, `gpui::scroll_max_offset` and
/// `gpui::scroll_to` work for a list without a second vocabulary, and what lets
/// a scrollbar drawn beside a list be a scrollbar rather than a picture of one.
pub fn base_handle(win: i64, id: &str) -> Option<ScrollHandle> {
    find(win, id).map(|h| h.0.borrow().base_handle.clone())
}

/// The rows the list for `id` built the last time it laid out, inclusive, or
/// `None` if it has never laid out.
pub fn range(win: i64, id: &str) -> Option<(usize, usize)> {
    RANGES.with(|r| r.borrow().get(&(win, id.to_string())).copied())
}
