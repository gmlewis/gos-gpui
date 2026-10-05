//! Window identity: which window's frame — or callback — is running now.
//!
//! Everything a frame touches is scoped to one window: the state store, the
//! event queue, the requests it raises, the focus / mouse / slider maps, the
//! element arena and the editor registry. The handle is a plain `i64`, so
//! Gossamer can carry it, compare it and print it.
//!
//! Window `0` is "no window": a program run outside a frame, which is what
//! `gos test` does, reads and writes that namespace.

use std::cell::Cell;
use std::sync::atomic::{AtomicI64, Ordering};

thread_local! {
    static CURRENT: Cell<i64> = const { Cell::new(0) };
}

/// The next id to hand out. The first window is 1, so `0` can mean "none".
static NEXT: AtomicI64 = AtomicI64::new(1);

/// Allocates the id for a window that is about to be opened.
pub fn allocate() -> i64 {
    NEXT.fetch_add(1, Ordering::SeqCst)
}

/// The window whose frame or callback is running on this thread.
pub fn current() -> i64 {
    CURRENT.with(|c| c.get())
}

/// Puts the previous window back when the scope ends — by returning or by
/// unwinding, so a panic cannot leave the thread pointed at the wrong window.
struct Restore(i64);

impl Drop for Restore {
    fn drop(&mut self) {
        CURRENT.with(|c| c.set(self.0));
    }
}

/// Runs `f` as `id`'s window, and restores the window that was current before.
///
/// Every GPUI callback that outlives the frame that built it must go through
/// this: a click handler runs after that frame has been painted, and the state
/// it writes has to be the state of the window it is a click in.
pub fn with<T>(id: i64, f: impl FnOnce() -> T) -> T {
    let previous = CURRENT.with(|c| c.replace(id));
    let _restore = Restore(previous);
    f()
}
