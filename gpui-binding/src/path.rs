//! A pool of GPUI paths, addressed by handle.
//!
//! `gpui::PathBuilder` is a consuming builder with no place in the binding's
//! ABI, so a path is created here, edited through handle-taking functions,
//! and finished with `build`. Builders and built paths share one `i64`
//! namespace, and the pool outlives the per-frame element arena: a program
//! builds a path once and repaints it for as long as it likes, and calls
//! `gpui::path_reset()` when it wants to start over.
//!
//! Coordinates are window coordinates, not element-relative ones —
//! `Path<Pixels>` keeps its vertices private and offers no translation, and
//! both upstream path examples place their canvases at the window origin and
//! build at absolute coordinates anyway.

use std::cell::RefCell;

use gpui::{point, px, Path, PathBuilder, Pixels};

enum Slot {
    Building(PathBuilder),
    Built(Path<Pixels>),
}

thread_local! {
    static POOL: RefCell<Vec<Slot>> = const { RefCell::new(Vec::new()) };
}

fn push(slot: Slot) -> i64 {
    POOL.with(|p| {
        let mut pool = p.borrow_mut();
        pool.push(slot);
        (pool.len() - 1) as i64
    })
}

/// A path that will be filled.
pub fn fill() -> i64 {
    push(Slot::Building(PathBuilder::fill()))
}

/// A path that will be stroked at `width`.
pub fn stroke(width: f64) -> i64 {
    push(Slot::Building(PathBuilder::stroke(px(width as f32))))
}

fn edit(handle: i64, f: impl FnOnce(&mut PathBuilder)) {
    POOL.with(|p| {
        let mut pool = p.borrow_mut();
        if let Some(Slot::Building(builder)) = pool.get_mut(handle as usize) {
            f(builder);
        }
    });
}

pub fn move_to(handle: i64, x: f64, y: f64) {
    edit(handle, |b| b.move_to(point(px(x as f32), px(y as f32))));
}

pub fn line_to(handle: i64, x: f64, y: f64) {
    edit(handle, |b| b.line_to(point(px(x as f32), px(y as f32))));
}

pub fn curve_to(handle: i64, x: f64, y: f64, ctrl_x: f64, ctrl_y: f64) {
    edit(handle, |b| {
        b.curve_to(
            point(px(x as f32), px(y as f32)),
            point(px(ctrl_x as f32), px(ctrl_y as f32)),
        )
    });
}

/// A cubic Bézier: the endpoint first, then the two control points — the same
/// order [`curve_to`] uses for its one control.
///
/// The two are not interchangeable, and that is why this exists: a quadratic
/// has a single control and cannot carry the outlines `lyon`'s
/// `extra::rust_logo` produces, which are cubics. With only `curve_to` a
/// program could describe a *nearby* logo and not the one upstream draws.
pub fn cubic_to(
    handle: i64,
    x: f64,
    y: f64,
    ctrl1_x: f64,
    ctrl1_y: f64,
    ctrl2_x: f64,
    ctrl2_y: f64,
) {
    edit(handle, |b| {
        b.cubic_bezier_to(
            point(px(ctrl1_x as f32), px(ctrl1_y as f32)),
            point(px(ctrl2_x as f32), px(ctrl2_y as f32)),
            point(px(x as f32), px(y as f32)),
        )
    });
}

pub fn arc_to(
    handle: i64,
    rx: f64,
    ry: f64,
    x_rotation: f64,
    large_arc: bool,
    sweep: bool,
    x: f64,
    y: f64,
) {
    edit(handle, |b| {
        b.arc_to(
            point(px(rx as f32), px(ry as f32)),
            px(x_rotation as f32),
            large_arc,
            sweep,
            point(px(x as f32), px(y as f32)),
        )
    });
}

/// `points` is a flat `x, y, x, y …` run.
pub fn polygon(handle: i64, points: Vec<f64>, closed: bool) {
    let mut vertices = Vec::with_capacity(points.len() / 2);
    for pair in points.chunks_exact(2) {
        vertices.push(point(px(pair[0] as f32), px(pair[1] as f32)));
    }
    edit(handle, |b| b.add_polygon(&vertices, closed));
}

pub fn close(handle: i64) {
    edit(handle, |b| b.close());
}

pub fn dash(handle: i64, dashes: Vec<f64>) {
    POOL.with(|p| {
        let mut pool = p.borrow_mut();
        if let Some(slot) = pool.get_mut(handle as usize) {
            if let Slot::Building(builder) = slot {
                let dashes: Vec<gpui::Pixels> = dashes.iter().map(|d| px(*d as f32)).collect();
                let taken = std::mem::replace(builder, PathBuilder::fill());
                *builder = taken.dash_array(&dashes);
            }
        }
    });
}

pub fn transform(handle: i64, translate_x: f64, translate_y: f64, scale: f64) {
    edit(handle, |b| {
        if scale != 1.0 {
            b.scale(scale as f32);
        }
        b.translate(point(px(translate_x as f32), px(translate_y as f32)));
    });
}

/// Finishes a builder. The handle keeps its identity and now names a built
/// path.
pub fn build(handle: i64) -> Result<i64, String> {
    POOL.with(|p| {
        let mut pool = p.borrow_mut();
        let index = usize::try_from(handle).map_err(|_| format!("bad path handle {handle}"))?;
        match pool.get(index) {
            Some(Slot::Building(_)) => {}
            Some(Slot::Built(_)) => return Ok(handle),
            None => return Err(format!("unknown path handle {handle}")),
        }
        let taken = std::mem::replace(&mut pool[index], Slot::Building(PathBuilder::fill()));
        match taken {
            Slot::Building(builder) => match builder.build() {
                Ok(path) => {
                    pool[index] = Slot::Built(path);
                    Ok(handle)
                }
                Err(e) => Err(format!("{e}")),
            },
            Slot::Built(path) => {
                pool[index] = Slot::Built(path);
                Ok(handle)
            }
        }
    })
}

/// The built path behind a handle, cloned so it can be painted.
pub fn get(handle: i64) -> Option<Path<Pixels>> {
    POOL.with(|p| match p.borrow().get(handle as usize) {
        Some(Slot::Built(path)) => Some(path.clone()),
        _ => None,
    })
}

/// Empties the pool. Handles issued before this are no longer valid.
pub fn reset() {
    POOL.with(|p| p.borrow_mut().clear());
}
