//! The described element tree, and its translation into real GPUI elements.
//!
//! Gossamer never sees a GPUI type. `gpui::column(style)` and friends append a
//! description — an [`El`] — to a per-frame arena and answer its `i64` handle;
//! `gpui::child(parent, child)` links one handle under another, and the view
//! callback ends by answering the root's handle. This module turns that
//! described tree into a GPUI element tree once the frame is built.
//!
//! Handles are plain integers in Gossamer, so the ergonomic layer — a type
//! with `.child(...)` methods — lives on the Gossamer side, in `examples/ui`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{
    canvas, div, fill, img, point, px, relative, svg, uniform_list, AnyElement, App, Bounds,
    ElementId, EntityId, FocusHandle, KeyDownEvent, MouseButton, Pixels, SharedString, Svg, Window,
};

use gossamer_binding::{NativeDispatch, PersistentCallback, Value};

use crate::style;
use crate::store;

// ---------------------------------------------------------------------------
// dispatching back into Gossamer
// ---------------------------------------------------------------------------

/// A pointer to the dispatch the outer `run` binding was called with.
///
/// `NativeDispatch` is borrowed for the duration of that call, and the whole
/// GPUI run loop happens inside it, so the pointer stays valid for as long as
/// anything here can use it.
type DispatchPtr = *mut (dyn NativeDispatch + 'static);

thread_local! {
    static DISPATCH: Cell<Option<DispatchPtr>> = const { Cell::new(None) };
}

pub fn stash_dispatch(d: &mut dyn NativeDispatch) {
    // SAFETY: erases the borrow's lifetime only. The referent outlives the
    // GPUI run loop, and nothing else touches it while the loop runs.
    let erased: DispatchPtr = unsafe {
        std::mem::transmute::<*mut dyn NativeDispatch, DispatchPtr>(d as *mut dyn NativeDispatch)
    };
    DISPATCH.with(|cell| cell.set(Some(erased)));
}

pub fn clear_dispatch() {
    DISPATCH.with(|cell| cell.set(None));
}

/// Calls back into a Gossamer closure — the view callback, or a list's row
/// builder — and answers what it answered.
pub fn invoke(view: &PersistentCallback, args: Vec<Value>) -> Result<Value, String> {
    let Some(raw) = DISPATCH.with(|cell| cell.get()) else {
        return Err("a Gossamer callback was asked to run outside gpui::run".into());
    };
    // SAFETY: see `stash_dispatch`. `raw` stays live for the whole run loop,
    // and every frame is built on the UI thread, one at a time.
    let dispatch = unsafe { &mut *raw };
    view.invoke(dispatch, args).map_err(|e| format!("{e:?}"))
}

// ---------------------------------------------------------------------------
// the described tree
// ---------------------------------------------------------------------------

pub enum Kind {
    /// A plain box, a row/column, a scroll area — everything the style string
    /// can turn it into.
    Plain,
    Text(String),
    Button(String),
    Checkbox { label: String, checked: bool },
    Input { placeholder: String },
    Slider { min: f64, max: f64 },
    Progress(f64),
    Image(String),
    Icon(String),
    Divider,
    Spacer(f64),
    /// A `uniform_list`: `build` is called once per *visible* row and answers
    /// that row's element handle.
    Virtual { count: usize, build: PersistentCallback, row_height: f64 },
    /// A `canvas` painted by one of the routines below.
    Canvas { painter: String, phase: f64 },
}

pub struct El {
    pub kind: Kind,
    pub style: String,
    pub id: String,
    pub children: Vec<usize>,
}

impl El {
    fn blank() -> Self {
        Self { kind: Kind::Plain, style: String::new(), id: String::new(), children: Vec::new() }
    }
}

// ---------------------------------------------------------------------------
// the arena of described elements
// ---------------------------------------------------------------------------

struct Arena {
    els: Vec<El>,
    errors: Vec<String>,
    next_id: u64,
}

impl Arena {
    fn new() -> Self {
        Self { els: vec![El::blank()], errors: Vec::new(), next_id: 0 }
    }

    fn reset(&mut self) {
        self.els.truncate(1);
        self.errors.clear();
    }

    fn push(&mut self, kind: Kind, style: String, id: String) -> i64 {
        let id = if id.is_empty() {
            self.next_id += 1;
            format!("gpui-auto-{}", self.next_id)
        } else {
            id
        };
        self.els.push(El { kind, style, id, children: Vec::new() });
        (self.els.len() - 1) as i64
    }
}

thread_local! {
    static ARENA: RefCell<Arena> = RefCell::new(Arena::new());
    static FRAME: Cell<u64> = const { Cell::new(0) };
    static START: std::cell::OnceCell<std::time::Instant> = const { std::cell::OnceCell::new() };
    static ROOT: Cell<i64> = const { Cell::new(0) };
    static REPORTED: RefCell<std::collections::HashSet<String>> = RefCell::new(std::collections::HashSet::new());
}

/// Starts a frame: an empty arena, and one more frame on the clock.
pub fn reset() {
    ARENA.with(|a| a.borrow_mut().reset());
    ROOT.with(|r| r.set(0));
    FRAME.with(|f| f.set(f.get() + 1));
}

/// Names the element this frame should render as the window's root.
pub fn set_root(handle: i64) {
    ROOT.with(|r| r.set(handle));
}

/// The root the frame named, or `None` when it named none.
pub fn named_root() -> Option<i64> {
    ROOT.with(|r| r.get()).checked_abs().filter(|h| *h > 0)
}

pub fn frame_number() -> u64 {
    FRAME.with(|f| f.get())
}

pub fn seconds_since_start() -> f64 {
    START.with(|s| s.get_or_init(std::time::Instant::now).elapsed().as_secs_f64())
}

/// Appends one described element and answers its handle.
pub fn push(kind: Kind, style: String, id: String) -> i64 {
    check_style(&style);
    ARENA.with(|a| a.borrow_mut().push(kind, style, id))
}

/// Links `child` under `parent`, and answers `parent` so the call can start a
/// chain.
pub fn child(parent: i64, child: i64) -> i64 {
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        link(&mut a, parent, child);
    });
    parent
}

/// Links a whole run of children, in order, and answers `parent`.
pub fn children(parent: i64, nodes: Vec<i64>) -> i64 {
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        for node in nodes {
            link(&mut a, parent, node);
        }
    });
    parent
}

fn link(arena: &mut Arena, parent: i64, child: i64) {
    let last = arena.els.len().saturating_sub(1);
    let target = usize::try_from(parent)
        .ok()
        .filter(|p| *p > 0 && *p <= last)
        .unwrap_or(last);
    if let Some(el) = arena.els.get_mut(target) {
        el.children.push(usize::try_from(child).unwrap_or(0));
    }
}

/// The arena's current length, so a list's row builder can hand its elements
/// back once the row has been turned into a real GPUI element.
pub fn mark() -> usize {
    ARENA.with(|a| a.borrow().els.len())
}

pub fn release_to(mark: usize) {
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        if mark >= 1 && mark <= a.els.len() {
            a.els.truncate(mark);
        }
    });
}

pub fn note_error(message: impl Into<String>) {
    let message = message.into();
    ARENA.with(|a| a.borrow_mut().errors.push(message));
}

/// Typos in a style string are reported once, on screen, rather than silently
/// doing nothing.
fn check_style(style: &str) {
    let unknown = style::unknown_tokens(style);
    if unknown.is_empty() {
        return;
    }
    let fresh = unknown
        .into_iter()
        .filter(|token| REPORTED.with(|r| r.borrow_mut().insert(token.clone())))
        .collect::<Vec<_>>();
    if !fresh.is_empty() {
        let message = format!("unknown style tokens: {}", fresh.join(", "));
        eprintln!("gpui: {message}");
        note_error(message);
    }
}

/// Copies one described element out of the arena, so nothing borrows it while
/// a nested callback re-enters.
fn snapshot(handle: i64) -> Option<El> {
    ARENA.with(|a| {
        let arena = a.borrow();
        let el = usize::try_from(handle).ok().and_then(|h| arena.els.get(h))?;
        Some(El {
            kind: match &el.kind {
                Kind::Plain => Kind::Plain,
                Kind::Text(s) => Kind::Text(s.clone()),
                Kind::Button(s) => Kind::Button(s.clone()),
                Kind::Checkbox { label, checked } => {
                    Kind::Checkbox { label: label.clone(), checked: *checked }
                }
                Kind::Input { placeholder } => Kind::Input { placeholder: placeholder.clone() },
                Kind::Slider { min, max } => Kind::Slider { min: *min, max: *max },
                Kind::Progress(v) => Kind::Progress(*v),
                Kind::Image(s) => Kind::Image(s.clone()),
                Kind::Icon(s) => Kind::Icon(s.clone()),
                Kind::Divider => Kind::Divider,
                Kind::Spacer(s) => Kind::Spacer(*s),
                Kind::Virtual { count, build, row_height } => {
                    Kind::Virtual { count: *count, build: build.clone(), row_height: *row_height }
                }
                Kind::Canvas { painter, phase } => {
                    Kind::Canvas { painter: painter.clone(), phase: *phase }
                }
            },
            style: el.style.clone(),
            id: el.id.clone(),
            children: el.children.clone(),
        })
    })
}

/// Turns the frame the view callback described into GPUI elements.
pub fn finish(root: Option<i64>, window: &mut Window, cx: &mut App, entity: EntityId) -> AnyElement {
    let errors = ARENA.with(|a| std::mem::take(&mut a.borrow_mut().errors));

    let mut column = div().flex().flex_col().size_full();
    for message in errors {
        column = column.child(error_card(message));
    }

    match root.and_then(snapshot) {
        Some(el) => column.child(render(&el, window, cx, entity)).into_any_element(),
        None => column
            .child(error_card(
                "the view callback answered no root element — end it with an element, \
                 such as `ui::column(..)`, rather than a bare statement"
                    .into(),
            ))
            .into_any_element(),
    }
}

fn error_card(message: String) -> AnyElement {
    div()
        .w_full()
        .p(px(10.0))
        .bg(style::colour("#7f1d1d"))
        .text_color(style::colour("#fee2e2"))
        .child(message)
        .into_any_element()
}

// ---------------------------------------------------------------------------
// described tree -> GPUI elements
// ---------------------------------------------------------------------------

thread_local! {
    static FOCUS: RefCell<HashMap<String, FocusHandle>> = RefCell::new(HashMap::new());
    static SLIDER_BOUNDS: RefCell<HashMap<String, Bounds<Pixels>>> = RefCell::new(HashMap::new());
}

fn focus_for(id: &str, cx: &mut App) -> FocusHandle {
    FOCUS.with(|f| {
        f.borrow_mut()
            .entry(id.to_string())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    })
}

pub fn render(el: &El, window: &mut Window, cx: &mut App, entity: EntityId) -> AnyElement {
    match &el.kind {
        Kind::Plain => {
            let children: Vec<AnyElement> = el
                .children
                .iter()
                .filter_map(|h| snapshot(*h as i64))
                .map(|child| render(&child, window, cx, entity))
                .collect();
            let base = style::apply(div(), &el.style);
            match style::scroll_axis(&el.style) {
                Some("x") => base
                    .id(ElementId::Name(el.id.clone().into()))
                    .overflow_x_scroll()
                    .children(children)
                    .into_any_element(),
                Some(_) => base
                    .id(ElementId::Name(el.id.clone().into()))
                    .overflow_y_scroll()
                    .children(children)
                    .into_any_element(),
                None => base.children(children).into_any_element(),
            }
        }

        Kind::Text(text) => {
            let text = text.clone();
            style::apply(div(), &el.style).child(text).into_any_element()
        }

        Kind::Button(label) => {
            let id = el.id.clone();
            let click_id = id.clone();
            let base = style::apply(
                div().flex().items_center().justify_center().cursor_pointer(),
                &el.style,
            )
            .id(ElementId::Name(id.into()));
            let base = apply_interactions(base, &el.style);
            base.child(label.clone())
                .on_click(move |_event, _window, cx| {
                    store::push_event(click_id.clone());
                    cx.notify(entity);
                })
                .into_any_element()
        }

        Kind::Checkbox { label, checked } => {
            let id = el.id.clone();
            let click_id = id.clone();
            let checked = *checked;
            let tick_colour = store::get_text(&format!("{id}#tick"), "#11111b");
            let box_ = div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(18.0))
                .rounded(px(4.0))
                .border(px(1.5))
                .border_color(style::colour(if checked { "#89b4fa" } else { "#585b70" }))
                .bg(style::colour(if checked { "#89b4fa" } else { "#00000000" }))
                .text_size(px(12.0))
                .text_color(style::colour(tick_colour.as_str()))
                .child(if checked { "✓" } else { "" });
            let base = style::apply(
                div().flex().flex_row().items_center().gap(px(8.0)).cursor_pointer(),
                &el.style,
            )
            .id(ElementId::Name(id.into()));
            let base = apply_interactions(base, &el.style);
            base.child(box_)
                .child(label.clone())
                .on_click(move |_event, _window, cx| {
                    let on = !store::get_flag(&click_id, false);
                    store::set_flag(&click_id, on);
                    store::push_event_with(&click_id, if on { "on" } else { "off" });
                    cx.notify(entity);
                })
                .into_any_element()
        }

        Kind::Input { placeholder } => {
            let handle = focus_for(&el.id, cx);
            let focused = handle.is_focused(window);
            let key = el.id.clone();
            let key_for_click = key.clone();
            let handle_for_click = handle.clone();
            let value = store::get_text(&el.id, "");
            let empty = value.is_empty();
            let shown = if empty { placeholder.clone() } else { value.clone() };
            let shown_colour = if empty { "#6c7086" } else { "inherit" };

            let field = style::apply(
                div().flex().flex_row().items_center().gap(px(1.0)).min_h(px(28.0)).cursor_text(),
                &el.style,
            );
            field
                .id(ElementId::Name(el.id.clone().into()))
                .track_focus(&handle)
                .child(if shown_colour == "inherit" {
                    div().child(shown)
                } else {
                    div().text_color(style::colour(shown_colour)).child(shown)
                })
                .child(
                    div()
                        .w(px(1.5))
                        .h(px(16.0))
                        .bg(style::colour(if focused { "#f5e0dc" } else { "#00000000" })),
                )
                .on_click(move |_event, window, _cx| {
                    window.focus(&handle_for_click);
                    store::push_event_with(&key_for_click, "focus");
                })
                .on_key_down(move |event: &KeyDownEvent, _window, cx| {
                    let mut text = store::get_text(&key, "");
                    match event.keystroke.key.as_str() {
                        "backspace" => {
                            text.pop();
                            store::set_text(&key, &text);
                        }
                        "enter" => store::push_event_with(&key, "submit"),
                        "escape" => store::push_event_with(&key, "cancel"),
                        _ => {
                            if let Some(ch) = &event.keystroke.key_char {
                                if !event.keystroke.modifiers.control
                                    && !event.keystroke.modifiers.platform
                                {
                                    text.push_str(ch);
                                    store::set_text(&key, &text);
                                }
                            }
                        }
                    }
                    cx.notify(entity);
                })
                .into_any_element()
        }

        Kind::Slider { min, max } => {
            let key = el.id.clone();
            let span = (*max - *min).max(f64::EPSILON);
            let value = store::get_float(&el.id, *min);
            let frac = ((value - *min) / span).clamp(0.0, 1.0) as f32;
            let (min, span) = (*min, span);

            let track_key = key.clone();
            let down_key = key.clone();
            let move_key = key.clone();
            let fraction = move |key: &str, x: Pixels| {
                let bounds = SLIDER_BOUNDS.with(|b| b.borrow().get(key).copied());
                let Some(bounds) = bounds else { return None };
                let width = f32::from(bounds.size.width).max(1.0);
                let offset = f32::from(x - bounds.origin.x);
                Some((offset / width) as f64)
            };
            let fraction_down = fraction.clone();
            let fraction_move = fraction.clone();
            let _ = fraction;

            let fill_bar = div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .h_full()
                .w(relative(frac))
                .bg(style::colour("#89b4fa"))
                .rounded(px(999.0));
            let knob = div()
                .absolute()
                .left(relative(frac))
                .top(px(0.0))
                .h_full()
                .w(px(18.0))
                .ml(px(-9.0))
                .rounded_full()
                .bg(style::colour("#f5e0dc"))
                .shadow_sm();
            let gutter = div().absolute().left(px(0.0)).top(px(0.0)).size_full();

            style::apply(
                div().relative().flex().items_center().h(px(22.0)).w_full().cursor_pointer(),
                &el.style,
            )
            .on_children_prepainted(move |bounds, _window, _cx| {
                if let Some(first) = bounds.first() {
                    SLIDER_BOUNDS.with(|b| b.borrow_mut().insert(track_key.clone(), *first));
                }
            })
            .id(ElementId::Name(el.id.clone().into()))
            .child(gutter)
            .child(fill_bar)
            .child(knob)
            .on_mouse_down(MouseButton::Left, move |event, _window, cx| {
                if let Some(frac) = fraction_down(&down_key, event.position.x) {
                    store::set_float(&down_key, min + frac * span);
                    store::push_event_with(&down_key, format!("{:.3}", min + frac * span));
                    cx.notify(entity);
                }
            })
            .on_mouse_move(move |event, _window, cx| {
                if !event.dragging() {
                    return;
                }
                if let Some(frac) = fraction_move(&move_key, event.position.x) {
                    store::set_float(&move_key, min + frac * span);
                    store::push_event_with(&move_key, format!("{:.3}", min + frac * span));
                    cx.notify(entity);
                }
            })
            .into_any_element()
        }

        Kind::Progress(value) => {
            let frac = value.clamp(0.0, 1.0) as f32;
            let outer = style::apply(
                div()
                    .relative()
                    .h(px(10.0))
                    .w_full()
                    .rounded(px(999.0))
                    .bg(style::colour("#313244")),
                &el.style,
            );
            outer
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .h_full()
                        .w(relative(frac))
                        .rounded(px(999.0))
                        .bg(style::colour("#a6e3a1")),
                )
                .into_any_element()
        }

        Kind::Image(path) => style::apply(img(PathBuf::from(path)), &el.style).into_any_element(),

        Kind::Icon(path) => {
            let icon: Svg = svg().path(SharedString::from(path.clone()));
            style::apply(icon, &el.style).into_any_element()
        }

        Kind::Divider => {
            style::apply(div().h(px(1.0)).w_full().bg(style::colour("#313244")), &el.style)
                .into_any_element()
        }

        Kind::Spacer(size) => {
            style::apply(div().h(px(*size as f32)).w_full(), &el.style).into_any_element()
        }

        Kind::Virtual { count, build, row_height } => {
            let count = *count;
            let row_height = *row_height;
            let build = build.clone();
            let list = uniform_list(ElementId::Name(el.id.clone().into()), count, {
                move |range, window, cx| {
                    let mut rows = Vec::with_capacity(range.len());
                    for index in range {
                        // The row builder appends its own elements to the
                        // arena. They are only needed until the row has been
                        // turned into a real GPUI element, so the arena is
                        // handed back straight afterwards.
                        let mark = mark();
                        let row = match invoke(&build, vec![Value::Int(index as i64)]) {
                            Ok(Value::Int(handle)) if handle > 0 => snapshot(handle),
                            Ok(_) => {
                                note_error(format!(
                                    "the row builder for row {index} answered no element"
                                ));
                                None
                            }
                            Err(e) => {
                                note_error(format!("the row builder for row {index} failed: {e}"));
                                None
                            }
                        };
                        let element = match row {
                            Some(row) => render(&row, window, cx, entity),
                            None => div().into_any_element(),
                        };
                        release_to(mark);
                        rows.push(
                            div()
                                .h(px(row_height as f32))
                                .w_full()
                                .child(element)
                                .into_any_element(),
                        );
                    }
                    rows
                }
            });
            style::apply(list, &el.style).into_any_element()
        }

        Kind::Canvas { painter, phase } => {
            let painter = painter.clone();
            let phase = *phase;
            let element = canvas(
                move |_bounds, _window, _cx| (),
                move |bounds, _state, window, _cx| paint(&painter, bounds, phase, window),
            );
            style::apply(element, &el.style).into_any_element()
        }
    }
}

/// Applies `hover=`/`active=` refinements to an already identified element.
fn apply_interactions<T: StatefulInteractiveElement + 'static>(el: T, spec: &str) -> T {
    let (hover, active) = style::interaction(spec);
    let mut el = el;
    if let Some(hover) = hover {
        el = el.hover(move |r| style::refine(r, &hover));
    }
    if let Some(active) = active {
        el = el.active(move |r| style::refine(r, &active));
    }
    el
}

// ---------------------------------------------------------------------------
// built-in canvas painters
// ---------------------------------------------------------------------------

/// Paints one of the canvas routines over `bounds`.
///
/// These exist so a Gossamer program can reach GPUI's immediate painting API
/// (`Window::paint_quad`, `Window::paint_path`) without owning any GPUI types.
fn paint(painter: &str, bounds: Bounds<Pixels>, phase: f64, window: &mut Window) {
    let palette = ["#89b4fa", "#a6e3a1", "#f9e2af", "#f38ba8", "#cba6f7", "#94e2d5"];
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let x0 = f32::from(bounds.origin.x);
    let y0 = f32::from(bounds.origin.y);
    match painter {
        "bars" => {
            let count = 28;
            let gap = 4.0_f32;
            let bar = (width - gap * (count as f32 + 1.0)) / count as f32;
            for i in 0..count {
                let t = phase * 2.0 + i as f64 * 0.35;
                let amplitude = (t.sin() * 0.5 + 0.5) * 0.75 + 0.15;
                let bar_height = height * amplitude as f32;
                let x = x0 + gap + i as f32 * (bar + gap);
                let y = y0 + height - bar_height;
                window.paint_quad(
                    fill(
                        Bounds {
                            origin: point(px(x), px(y)),
                            size: gpui::size(px(bar), px(bar_height)),
                        },
                        style::colour(palette[i % palette.len()]),
                    )
                    .corner_radii(px(3.0)),
                );
            }
        }
        "wave" => {
            let mut path = gpui::PathBuilder::stroke(px(3.0));
            let steps = 200;
            for i in 0..=steps {
                let x = width * (i as f32 / steps as f32);
                let t = i as f64 / steps as f64;
                let y = height * 0.5
                    + (t * std::f64::consts::TAU * 2.0 + phase).sin() as f32 * height * 0.3;
                if i == 0 {
                    path.move_to(point(px(x0 + x), px(y0 + y)));
                } else {
                    path.line_to(point(px(x0 + x), px(y0 + y)));
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, style::colour("#cba6f7"));
            }
        }
        "stripes" => {
            let band = 18.0_f32;
            let shift = (phase * 40.0) as f32 % (band * 2.0);
            let mut x = x0 - band * 2.0 + shift;
            let mut i = 0;
            while x < x0 + width {
                window.paint_quad(fill(
                    Bounds {
                        origin: point(px(x), px(y0)),
                        size: gpui::size(px(band), px(height)),
                    },
                    style::colour(palette[i % palette.len()]),
                ));
                x += band * 2.0;
                i += 1;
            }
        }
        _ => {
            window.paint_quad(fill(bounds, style::colour("#45475a")));
        }
    }
}
