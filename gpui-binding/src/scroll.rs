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

use crate::window;

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

/// Where a scrollbar's thumb goes and how long it is, in whole pixels, as `"<start>,<length>"`.
///
/// `offset` is gpui's — zero at the top, negative going down, clamped to `-max ..= 0` — `max` is the
/// positive amount of content that can be scrolled past, `track` is the length of the track in the
/// same axis, and `min` is the shortest thumb worth drawing: below it a thumb is too small to see,
/// let alone drag.
///
/// `""` when there is nothing to scroll or nowhere to draw — a track of no length, or a `max` of
/// zero, is a container whose content fits, and a bar with no thumb says so honestly.
///
/// The length is the track's share of the document it is showing, `track * track / (track + max)`,
/// floored at `min` and capped at the track. The start is the fraction scrolled, measured over the
/// thumb's **travel** (`track - length`) rather than over the whole track, so that its far end meets
/// the track's end at the bottom — which is where a thumb that is merely proportional looks wrong.
pub fn thumb(offset: f64, max: f64, track: f64, min: f64) -> String {
    match thumb_pixels(offset, max, track, min) {
        Some((start, length)) => format!("{},{}", start.round() as i64, length.round() as i64),
        None => String::new(),
    }
}

/// The same arithmetic as numbers rather than as text, for the element that draws with them.
///
/// `None` when there is nothing to scroll, which is the same condition [`thumb`] answers `""` for.
pub fn thumb_pixels(offset: f64, max: f64, track: f64, min: f64) -> Option<(f64, f64)> {
    if track <= 0.0 || max <= 0.0 {
        return None;
    }
    let document = track + max;
    let length = (track * track / document).clamp(min.min(track), track);
    let scrolled = (-offset / max).clamp(0.0, 1.0);
    Some((scrolled * (track - length), length))
}

/// Where a scrolled element's thumb is, as numbers: `(offset, max, start, length)`.
///
/// `None` when no scrolled container has been drawn under `id`, which is the ordinary answer for an
/// id that has never been rendered.
pub fn thumb_of(win: i64, id: &str, axis: &str, track: f64, min: f64) -> Option<(f64, f64, f64, f64)> {
    let handle = find(win, id)?;
    let (offset, max) = if axis == "x" {
        (
            f32::from(handle.offset().x) as f64,
            f32::from(handle.max_offset().width) as f64,
        )
    } else {
        (
            f32::from(handle.offset().y) as f64,
            f32::from(handle.max_offset().height) as f64,
        )
    };
    let (start, length) = thumb_pixels(offset, max, track, min)?;
    Some((offset, max, start, length))
}

/// [`thumb`] for a scrolled element the binding already keeps an offset for, as
/// `"<start>,<length>"` — or `""` when no scrolled container has been drawn under `id`, or its
/// content fits.
///
/// The element-shaped half of the same arithmetic: one function, two callers, so what a program
/// reading `gpui::scrollbar_thumb` is told is what the scrollbar element draws.
pub fn scrollbar_thumb(win: i64, id: &str, axis: &str, track: f64, min: f64) -> String {
    match thumb_of(win, id, axis, track, min) {
        Some((_, _, start, length)) => format!("{},{}", start.round() as i64, length.round() as i64),
        None => String::new(),
    }
}

/// The handle for `id` in window `win`, if the element has ever been rendered.
///
/// `None` is the ordinary answer for an id the program has not drawn a scrolled
/// container under, and for one drawn for the first time this frame — a reader
/// has no offset to report until the element has been laid out once.
pub fn find(win: i64, id: &str) -> Option<ScrollHandle> {
    HANDLES.with(|h| h.borrow().get(&(win, id.to_string())).cloned())
}

/// The shortest thumb this crate draws when the element did not say: about what a scrollbar thumb
/// is on a desktop, and long enough to be worth aiming at.
pub const DEFAULT_MIN_THUMB: f64 = 24.0;

thread_local! {
    /// Where each scrollbar's track was last painted, in window pixels.
    ///
    /// A press knows where the *pointer* is but not where the track is, and a canvas is given its
    /// bounds only while it paints — so the paint pass leaves them here for the press to read. Same
    /// shape, and same reason, as the bounds registry the mouse refinements use.
    static TRACKS: RefCell<HashMap<(i64, String), gpui::Bounds<gpui::Pixels>>> =
        RefCell::new(HashMap::new());
}

/// The length of `axis` in `bounds`: what a scrollbar's track is measured along.
fn along(axis: &str, bounds: gpui::Bounds<gpui::Pixels>) -> f64 {
    if axis == "x" {
        f32::from(bounds.size.width) as f64
    } else {
        f32::from(bounds.size.height) as f64
    }
}

/// Paints a scrolled element's thumb over `bounds`, which is the track.
///
/// The track itself is the element's own background — a scrollbar is a normal styled box with a bar
/// drawn over it — so this draws one quad, where [`thumb_pixels`] says it goes. A track with nothing
/// to scroll draws nothing at all, which is how a person sees that the content fits.
///
/// It also remembers where the track was, so that a press on it can be turned into an offset.
pub fn paint(
    id: &str,
    axis: &str,
    bounds: gpui::Bounds<gpui::Pixels>,
    colour: gpui::Hsla,
    window: &mut gpui::Window,
) {
    let win = window::current();
    TRACKS.with(|tracks| tracks.borrow_mut().insert((win, id.to_string()), bounds));
    let Some((_, _, start, length)) = thumb_of(win, id, axis, along(axis, bounds), DEFAULT_MIN_THUMB)
    else {
        return;
    };
    let quad = if axis == "x" {
        gpui::Bounds {
            origin: gpui::point(bounds.origin.x + gpui::px(start as f32), bounds.origin.y),
            size: gpui::size(gpui::px(length as f32), bounds.size.height),
        }
    } else {
        gpui::Bounds {
            origin: gpui::point(bounds.origin.x, bounds.origin.y + gpui::px(start as f32)),
            size: gpui::size(bounds.size.width, gpui::px(length as f32)),
        }
    };
    window.paint_quad(gpui::fill(quad, colour));
}

/// Pulls a scrolled element's thumb so that its middle is under the pointer, and answers whether
/// anything moved.
///
/// This is the half of a scrollbar that the inert thumb in `02_data_table` does not have: a press
/// jumps to where it was pressed and a drag follows, and both are the same rule, so there is no
/// drag state to lose when the pointer leaves the track. Nothing happens when there is nothing to
/// scroll.
pub fn jump(id: &str, axis: &str, position: gpui::Point<gpui::Pixels>) -> bool {
    let win = window::current();
    let Some(bounds) = TRACKS.with(|tracks| tracks.borrow().get(&(win, id.to_string())).copied())
    else {
        return false;
    };
    let track = along(axis, bounds);
    let Some((_, max, _, length)) = thumb_of(win, id, axis, track, DEFAULT_MIN_THUMB) else {
        return false;
    };
    let travel = track - length;
    if travel <= 0.0 {
        return false;
    }
    let at = if axis == "x" {
        f32::from(position.x - bounds.origin.x) as f64
    } else {
        f32::from(position.y - bounds.origin.y) as f64
    };
    let fraction = ((at - length / 2.0) / travel).clamp(0.0, 1.0);
    let Some(handle) = find(win, id) else {
        return false;
    };
    let offset = handle.offset();
    let point = if axis == "x" {
        gpui::point(gpui::px((-fraction * max) as f32), offset.y)
    } else {
        gpui::point(offset.x, gpui::px((-fraction * max) as f32))
    };
    handle.set_offset(point);
    true
}

/// The current or first scrolled container handle for window `win`.
pub fn current_scroll(win: i64) -> Option<(String, gpui::ScrollHandle)> {
    HANDLES.with(|h| {
        h.borrow()
            .iter()
            .find(|((w, _), _)| *w == win)
            .map(|((_, id), handle)| (id.clone(), handle.clone()))
    })
}

/// Scrolls container `id` in window `win` according to navigation action `nav`.
pub fn scroll_nav(win: i64, id: &str, nav: crate::view::NavAction) -> bool {
    let Some(handle) = find(win, id) else {
        return false;
    };
    let offset = handle.offset();
    let max = handle.max_offset();
    if max.height <= gpui::px(0.0) && max.width <= gpui::px(0.0) {
        return false;
    }
    let page_step = gpui::px(300.0);
    let line_step = gpui::px(30.0);

    let (new_x, new_y) = match nav {
        crate::view::NavAction::Home => (gpui::px(0.0), gpui::px(0.0)),
        crate::view::NavAction::End => (-max.width, -max.height),
        crate::view::NavAction::PageUp => (offset.x, (offset.y + page_step).min(gpui::px(0.0))),
        crate::view::NavAction::PageDown => (offset.x, (offset.y - page_step).max(-max.height)),
        crate::view::NavAction::StepUp => (offset.x, (offset.y + line_step).min(gpui::px(0.0))),
        crate::view::NavAction::StepDown => (offset.y, (offset.y - line_step).max(-max.height)),
        crate::view::NavAction::StepLeft => ((offset.x + line_step).min(gpui::px(0.0)), offset.y),
        crate::view::NavAction::StepRight => ((offset.x - line_step).max(-max.width), offset.y),
    };
    handle.set_offset(gpui::point(new_x, new_y));
    true
}
