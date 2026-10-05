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

use gossamer_binding::{NativeDispatch, RetainedCallback, Value};

use crate::editor;
use crate::path;
use crate::scroll;
use crate::style;
use crate::store;
use crate::window;

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
pub fn invoke(view: &RetainedCallback, args: Vec<Value>) -> Result<Value, String> {
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
    /// The row builder. A retained callback: a list is built during
    /// layout, after `gpui::list` has returned.
    Virtual { count: usize, build: RetainedCallback, row_height: f64 },
    /// A `canvas` painted by one of the routines below.
    Canvas { painter: String, phase: f64 },
    /// A `canvas` that paints built paths, in order.
    Paths { paths: Vec<i64>, backgrounds: Vec<String> },
    /// A wrapper that reports pointer events for the element it holds.
    Mouse { child: usize, id: String, kinds: String },
    /// Text with per-byte-range highlights.
    StyledText { content: String, highlights: Vec<String> },
    /// A binding-owned text editor, registered under `key`.
    Editor { key: String, placeholder: String },
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
        // Auto ids must be identical from one frame to the next: GPUI keeps
        // per-element state (an image's animation cursor, hover and active
        // flags) in a map keyed by the element id. Leaving this counter at its
        // process-lifetime value renames every element every frame, so that
        // state is thrown away and rebuilt — a GIF never leaves frame 0, and
        // `hover=` / `active=` never fire.
        self.next_id = 0;
    }

    fn push(&mut self, kind: Kind, style: String, id: String) -> i64 {
        // A leaf that takes an id passes it in; every other element can name
        // itself with `id=` in its style. Falling back to a per-frame auto id
        // keeps the element addressable by GPUI — hover, image animation and
        // scroll state are all keyed by it — even when the program does not care
        // what it is called.
        let id = if id.is_empty() {
            match style::named(&style) {
                Some(name) => name,
                None => {
                    self.next_id += 1;
                    format!("gpui-auto-{}", self.next_id)
                }
            }
        } else {
            id
        };
        self.els.push(El { kind, style, id, children: Vec::new() });
        (self.els.len() - 1) as i64
    }
}

thread_local! {
    /// One arena per window.
    ///
    /// A frame describes its whole tree into an arena, and parts of that tree
    /// — a `uniform_list`'s rows — are still being read during layout, after
    /// the frame's callback has returned. A second window rendering in between
    /// would reset the arena the first is still using, so each window gets its
    /// own.
    static ARENAS: RefCell<HashMap<i64, Arena>> = RefCell::new(HashMap::new());
    /// Frames painted, per window: `gpui::frame()` counts *this* window's.
    static FRAMES: RefCell<HashMap<i64, u64>> = RefCell::new(HashMap::new());
    /// The root each window named for the frame it is building.
    static ROOTS: RefCell<HashMap<i64, i64>> = RefCell::new(HashMap::new());
    static START: std::cell::OnceCell<std::time::Instant> = const { std::cell::OnceCell::new() };
    static REPORTED: RefCell<std::collections::HashSet<String>> = RefCell::new(std::collections::HashSet::new());
}

/// Runs `f` against the current window's arena, creating it the first time.
fn arena<T>(f: impl FnOnce(&mut Arena) -> T) -> T {
    let win = window::current();
    ARENAS.with(|a| f(a.borrow_mut().entry(win).or_insert_with(Arena::new)))
}

/// Thread-local mirrors of the things a binding function can be asked for
/// mid-frame but can only be read with a `Window` in hand. All per window: a
/// callback belonging to window A must not read window B's rectangle.
static MOUSE_POS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

thread_local! {
    static MOUSE_AT: RefCell<HashMap<i64, (f64, f64)>> = RefCell::new(HashMap::new());
    static FOCUSED_NOW: RefCell<HashMap<i64, std::collections::HashSet<String>>> =
        RefCell::new(HashMap::new());
    static COLORS: RefCell<HashMap<i64, HashMap<String, String>>> = RefCell::new(HashMap::new());
    static WINDOW_RECT: RefCell<HashMap<i64, (f64, f64, f64, f64)>> = RefCell::new(HashMap::new());
    static DISPLAYS: RefCell<HashMap<i64, Vec<Display>>> = RefCell::new(HashMap::new());
    static KEYBOARD: RefCell<String> = const { RefCell::new(String::new()) };
}


/// `"start-end prop,prop"` over UTF-8 byte ranges, where the properties are
/// the text half of the style language.
fn parse_highlight(spec: &str) -> Option<(std::ops::Range<usize>, gpui::HighlightStyle)> {
    let (range, props) = spec.split_once(' ')?;
    let (start, end) = range.split_once('-')?;
    let mut style = gpui::HighlightStyle::default();
    for (key, value) in style::tokens(props) {
        let v = value.as_deref().unwrap_or("");
        match key.as_str() {
            "bold" => style.font_weight = Some(gpui::FontWeight::BOLD),
            "semibold" => style.font_weight = Some(gpui::FontWeight::SEMIBOLD),
            "weight" => style.font_weight = Some(gpui::FontWeight::from(v.parse::<f32>().unwrap_or(400.0))),
            "italic" => style.font_style = Some(gpui::FontStyle::Italic),
            "color" => style.color = Some(style::colour(v)),
            "bg" => style.background_color = Some(style::colour(v)),
            "underline" => style.underline = Some(gpui::UnderlineStyle::default()),
            "strike" => style.strikethrough = Some(gpui::StrikethroughStyle::default()),
            _ => {}
        }
    }
    Some((start.parse().ok()?..end.parse().ok()?, style))
}

/// The default colours, cached at frame start.
pub fn default_color(name: &str) -> String {
    let win = window::current();
    COLORS.with(|c| c.borrow().get(&win).and_then(|m| m.get(name)).cloned().unwrap_or_default())
}

/// How far the scrolled element `id` has scrolled in this window, on `axis`.
///
/// gpui's sign, kept as it is: zero at the top, negative as the content scrolls
/// down. Answers `0.0` for an id no scrolled container has been drawn under —
/// which is also what a container that has not scrolled yet answers, so a
/// program that needs to tell the two apart asks for [`scroll_max_offset`],
/// which is positive only where a scrolled container exists.
pub fn scroll_offset(id: &str, axis: &str) -> f64 {
    match scroll::find(window::current(), id) {
        Some(handle) => axis_value(handle.offset(), axis),
        None => 0.0,
    }
}

/// How much content the scrolled element `id` has to scroll past on `axis`: the
/// content size minus the container's, clamped at zero, and **positive**.
pub fn scroll_max_offset(id: &str, axis: &str) -> f64 {
    let Some(handle) = scroll::find(window::current(), id) else {
        return 0.0;
    };
    let max = handle.max_offset();
    match axis {
        "x" => f32::from(max.width) as f64,
        _ => f32::from(max.height) as f64,
    }
}

/// Sets the offset of the scrolled element `id` in this window, in gpui's sign.
///
/// Answers `false` when no scrolled container has been drawn under that id, so a
/// program that misspells one is told rather than silently left at the top. The
/// offset is clamped to the scrolled range during the next prepaint, so setting
/// one past the end parks at the end.
pub fn scroll_to(id: &str, x: f64, y: f64) -> bool {
    let Some(handle) = scroll::find(window::current(), id) else {
        return false;
    };
    handle.set_offset(gpui::point(px(x as f32), px(y as f32)));
    true
}

fn axis_value(point: gpui::Point<Pixels>, axis: &str) -> f64 {
    match axis {
        "x" => f32::from(point.x) as f64,
        _ => f32::from(point.y) as f64,
    }
}

/// Reads what the frame will ask for while it still has the window: the
/// pointer's position, which focus handles are focused, and the default
/// colours. Called once, before the view callback runs.
pub fn begin_frame(window: &mut Window, cx: &App) {
    let win = window::current();
    let position = window.mouse_position();
    MOUSE_AT.with(|p| {
        p.borrow_mut().insert(
            win,
            (f32::from(position.x) as f64, f32::from(position.y) as f64),
        )
    });
    let _ = &MOUSE_POS;
    let bounds = window.bounds();
    WINDOW_RECT.with(|r| {
        r.borrow_mut().insert(
            win,
            (
                f32::from(bounds.origin.x) as f64,
                f32::from(bounds.origin.y) as f64,
                f32::from(bounds.size.width) as f64,
                f32::from(bounds.size.height) as f64,
            ),
        )
    });
    DISPLAYS.with(|d| {
        let mut list = Vec::new();
        for display in cx.displays() {
            let bounds = display.bounds();
            list.push(Display {
                id: i64::from(u32::from(display.id())),
                x: f32::from(bounds.origin.x) as f64,
                y: f32::from(bounds.origin.y) as f64,
                w: f32::from(bounds.size.width) as f64,
                h: f32::from(bounds.size.height) as f64,
            });
        }
        d.borrow_mut().insert(win, list);
    });
    // `get_global` panics when nothing installed the colours, which is the
    // case for the frames GPUI draws before a program asks for anything.
    let colors = if cx.has_global::<gpui::colors::GlobalColors>() {
        Some(gpui::colors::Colors::get_global(cx).clone())
    } else {
        None
    };
    if let Some(colors) = colors {
        COLORS.with(|c| {
            let hex = |c: gpui::Rgba| {
                format!(
                    "#{:02x}{:02x}{:02x}{:02x}",
                    (c.r * 255.0).round() as u8,
                    (c.g * 255.0).round() as u8,
                    (c.b * 255.0).round() as u8,
                    (c.a * 255.0).round() as u8,
                )
            };
            let mut map = HashMap::new();
            map.insert("text".into(), hex(colors.text));
            map.insert("selected_text".into(), hex(colors.selected_text));
            map.insert("background".into(), hex(colors.background));
            map.insert("disabled".into(), hex(colors.disabled));
            map.insert("selected".into(), hex(colors.selected));
            map.insert("border".into(), hex(colors.border));
            map.insert("separator".into(), hex(colors.separator));
            map.insert("container".into(), hex(colors.container));
            c.borrow_mut().insert(win, map);
        });
    }
    FOCUSED_NOW.with(|set| {
        let mut focused = std::collections::HashSet::new();
        FOCUS.with(|f| {
            for ((_, id), handle) in f.borrow().iter() {
                if handle.is_focused(window) {
                    focused.insert(id.clone());
                }
            }
        });
        set.borrow_mut().insert(win, focused);
    });
}

/// The window's rectangle, as of the start of this frame.
pub fn window_rect() -> (f64, f64, f64, f64) {
    let win = window::current();
    WINDOW_RECT.with(|r| r.borrow().get(&win).copied().unwrap_or((0.0, 0.0, 0.0, 0.0)))
}

/// One connected display's geometry.
#[derive(Default, Clone)]
pub struct Display {
    pub id: i64,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Records the keyboard layout's name, once, so the frame can name it.
pub fn set_keyboard_layout(name: String) {
    KEYBOARD.with(|k| *k.borrow_mut() = name);
}

/// The keyboard layout's name.
pub fn keyboard_layout() -> String {
    KEYBOARD.with(|k| k.borrow().clone())
}

/// The connected displays, as of the start of this frame.
pub fn displays() -> Vec<Display> {
    let win = window::current();
    DISPLAYS.with(|d| d.borrow().get(&win).cloned()).unwrap_or_default()
}

/// The pointer's position, from the start of this frame.
pub fn mouse_position() -> (f64, f64) {
    let win = window::current();
    MOUSE_AT.with(|p| p.borrow().get(&win).copied()).unwrap_or((0.0, 0.0))
}

/// Whether a registered focus handle is the focused one this frame.
pub fn is_focused(id: &str) -> bool {
    let win = window::current();
    FOCUSED_NOW.with(|set| set.borrow().get(&win).map(|s| s.contains(id)).unwrap_or(false))
}

/// Starts a frame: an empty arena for this window, and one more frame on its
/// clock.
pub fn reset() {
    let win = window::current();
    arena(|a| a.reset());
    ROOTS.with(|r| {
        r.borrow_mut().remove(&win);
    });
    FRAMES.with(|f| {
        *f.borrow_mut().entry(win).or_insert(0) += 1;
    });
}

/// Names the element this frame should render as the window's root.
pub fn set_root(handle: i64) {
    let win = window::current();
    ROOTS.with(|r| {
        r.borrow_mut().insert(win, handle);
    });
}

/// The root the frame named, or `None` when it named none.
pub fn named_root() -> Option<i64> {
    let win = window::current();
    ROOTS
        .with(|r| r.borrow().get(&win).copied())
        .and_then(|h| h.checked_abs())
        .filter(|h| *h > 0)
}

pub fn frame_number() -> u64 {
    let win = window::current();
    FRAMES.with(|f| f.borrow().get(&win).copied().unwrap_or(0))
}

pub fn seconds_since_start() -> f64 {
    START.with(|s| s.get_or_init(std::time::Instant::now).elapsed().as_secs_f64())
}

/// Appends one described element and answers its handle.
pub fn push(kind: Kind, style: String, id: String) -> i64 {
    check_style(&style);
    arena(|a| a.push(kind, style, id))
}

/// Links `child` under `parent`, and answers `parent` so the call can start a
/// chain.
pub fn child(parent: i64, child: i64) -> i64 {
    arena(|a| link(a, parent, child));
    parent
}

/// Links a whole run of children, in order, and answers `parent`.
pub fn children(parent: i64, nodes: Vec<i64>) -> i64 {
    arena(|a| {
        for node in nodes {
            link(a, parent, node);
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
    arena(|a| a.els.len())
}

pub fn release_to(mark: usize) {
    arena(|a| {
        if mark >= 1 && mark <= a.els.len() {
            a.els.truncate(mark);
        }
    });
}

pub fn note_error(message: impl Into<String>) {
    let message = message.into();
    arena(|a| a.errors.push(message));
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
    arena(|arena| {
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
                Kind::Paths { paths, backgrounds } => {
                    Kind::Paths { paths: paths.clone(), backgrounds: backgrounds.clone() }
                }
                Kind::Mouse { child, id, kinds } => {
                    Kind::Mouse { child: *child, id: id.clone(), kinds: kinds.clone() }
                }
                Kind::StyledText { content, highlights } => {
                    Kind::StyledText { content: content.clone(), highlights: highlights.clone() }
                }
                Kind::Editor { key, placeholder } => {
                    Kind::Editor { key: key.clone(), placeholder: placeholder.clone() }
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
    let errors = arena(|a| std::mem::take(&mut a.errors));

    // A clean frame is handed straight to GPUI: the element the program
    // described *is* the window's root, with nothing wrapped around it. The
    // column only appears when there is something to report.
    if errors.is_empty() {
        let element = match root.and_then(snapshot) {
            Some(el) => render(&el, window, cx, entity),
            None => error_card(
                "the view callback answered no root element — end it with an element, \
                 such as `ui::column(..)`, rather than a bare statement"
                    .into(),
            ),
        };
        return listen_for_keys(element, window, cx);
    }

    let mut column = div().flex().flex_col().size_full();
    for message in errors {
        column = column.child(error_card(message));
    }
    match root.and_then(snapshot) {
        Some(el) => column.child(render(&el, window, cx, entity)).into_any_element(),
        None => column
            .child(error_card("the view callback answered no root element".into()))
            .into_any_element(),
    }
}

/// After `gpui::on_keys(true)`, the window itself listens for keystrokes and
/// raises `"key|<name>"`, so a program can use the keyboard without owning a
/// GPUI element. It takes focus once; a click into a text field moves focus
/// there until focus comes back.
fn listen_for_keys(element: AnyElement, window: &mut Window, cx: &mut App) -> AnyElement {
    if !store::get_flag("~keys", false) {
        return element;
    }
    let win = window::current();
    let handle = focus_for(win, "~root", cx);
    if frame_number() <= 2 && !handle.is_focused(window) {
        window.focus(&handle);
    }
    div()
        .size_full()
        .id(ElementId::Name("~root".into()))
        .track_focus(&handle)
        .on_key_down(move |event: &KeyDownEvent, _window, _cx| {
            window::with(win, || {
                store::push_event_with("key", event.keystroke.key.clone());
                // The rest of the keystroke, for a program that wants to echo
                // it: upstream's input example logs both halves.
                store::set_text(
                    "~key.char",
                    event.keystroke.key_char.as_deref().unwrap_or(""),
                );
                store::set_text("~key.unparse", &event.keystroke.unparse());
            });
        })
        .child(element)
        .into_any_element()
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
    /// Focus handles, per window and element id. GPUI's own handle for a
    /// `focusable()` element is built inside the framework and never handed
    /// over, so the binding keeps the handles it builds itself here — and a
    /// handle belonging to window A must not be asked about window B.
    static FOCUS: RefCell<HashMap<(i64, String), FocusHandle>> = RefCell::new(HashMap::new());
    static SLIDER_BOUNDS: RefCell<HashMap<(i64, String), Bounds<Pixels>>> =
        RefCell::new(HashMap::new());
    static MOUSE_BOUNDS: RefCell<HashMap<(i64, String), Bounds<Pixels>>> =
        RefCell::new(HashMap::new());
}

/// Writes a pointer event into the state store and raises `"<id>|<kind>"`.
///
/// Coordinates are element-relative, so a program never has to know where its
/// element sits — the same thing the slider does.
fn report_pointer(
    id: &str,
    kind: &str,
    position: gpui::Point<Pixels>,
    modifiers: gpui::Modifiers,
) {
    let win = window::current();
    let bounds = MOUSE_BOUNDS.with(|m| m.borrow().get(&(win, id.to_string())).copied());
    let (x, y) = match bounds {
        Some(bounds) => (
            f32::from(position.x) - f32::from(bounds.origin.x),
            f32::from(position.y) - f32::from(bounds.origin.y),
        ),
        None => (f32::from(position.x), f32::from(position.y)),
    };
    store::set_float(&format!("{id}.x"), x as f64);
    store::set_float(&format!("{id}.y"), y as f64);
    store::set_flag(&format!("{id}.dragging"), kind == "move");
    store::set_flag(&format!("{id}.shift"), modifiers.shift);
    store::set_flag(&format!("{id}.control"), modifiers.control);
    store::set_flag(&format!("{id}.alt"), modifiers.alt);
    store::set_flag(&format!("{id}.platform"), modifiers.platform);
    store::push_event_with(id, kind);
}

fn focus_for(win: i64, id: &str, cx: &mut App) -> FocusHandle {
    FOCUS.with(|f| {
        f.borrow_mut()
            .entry((win, id.to_string()))
            .or_insert_with(|| cx.focus_handle())
            .clone()
    })
}

pub fn render(el: &El, window: &mut Window, cx: &mut App, entity: EntityId) -> AnyElement {
    // Every callback built below outlives this frame. Each one re-enters
    // through `window::with(win, ..)` so that the state, events and requests it
    // touches belong to the window it was drawn in.
    let win = window::current();
    match &el.kind {
        Kind::Plain => {
            let children: Vec<AnyElement> = el
                .children
                .iter()
                .filter_map(|h| snapshot(*h as i64))
                .map(|child| render(&child, window, cx, entity))
                .collect();
            let base = style::apply(div(), &el.style);
            let (hover, active) = style::interaction(&el.style);
            match style::scroll_axis(&el.style) {
                // A scrolled container is given this window's handle for its id,
                // so the program can read the offset back and set it. The handle
                // outlives the frame — it is what makes the offset survive a
                // rebuild — which is why it is not created here and dropped.
                Some("x") => {
                    let scrolled = apply_focus_order(
                        apply_interactions(
                            base.id(ElementId::Name(el.id.clone().into())),
                            &el.style,
                        ),
                        &el.style,
                    );
                    let handle = scroll::handle(win, &el.id);
                    scrolled
                        .overflow_x_scroll()
                        .track_scroll(&handle)
                        .children(children)
                        .into_any_element()
                }
                Some("both") => {
                    let scrolled = apply_focus_order(
                        apply_interactions(
                            base.id(ElementId::Name(el.id.clone().into())),
                            &el.style,
                        ),
                        &el.style,
                    );
                    let handle = scroll::handle(win, &el.id);
                    scrolled
                        .overflow_scroll()
                        .track_scroll(&handle)
                        .children(children)
                        .into_any_element()
                }
                Some(_) => {
                    let scrolled = apply_focus_order(
                        apply_interactions(
                            base.id(ElementId::Name(el.id.clone().into())),
                            &el.style,
                        ),
                        &el.style,
                    );
                    let handle = scroll::handle(win, &el.id);
                    scrolled
                        .overflow_y_scroll()
                        .track_scroll(&handle)
                        .children(children)
                        .into_any_element()
                }
                // A plain box only becomes interactive when it asks to be, so
                // the common case keeps a stateless element.
                None if hover.is_some() || active.is_some() => {
                    let interactive = apply_focus_order(
                        apply_interactions(
                            base.id(ElementId::Name(el.id.clone().into())),
                            &el.style,
                        ),
                        &el.style,
                    );
                    interactive.children(children).into_any_element()
                }
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
                    window::with(win, || store::push_event(click_id.clone()));
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
                    window::with(win, || {
                        let on = !store::get_flag(&click_id, false);
                        store::set_flag(&click_id, on);
                        store::push_event_with(&click_id, if on { "on" } else { "off" });
                    });
                    cx.notify(entity);
                })
                .into_any_element()
        }

        Kind::Input { placeholder } => {
            let handle = focus_for(win, &el.id, cx);
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
                    window::with(win, || store::push_event_with(&key_for_click, "focus"));
                })
                .on_key_down(move |event: &KeyDownEvent, _window, cx| {
                    window::with(win, || {
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
                    });
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
                let bounds = SLIDER_BOUNDS.with(|b| b.borrow().get(&(win, key.to_string())).copied());
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
                    SLIDER_BOUNDS
                        .with(|b| b.borrow_mut().insert((win, track_key.clone()), *first));
                }
            })
            .id(ElementId::Name(el.id.clone().into()))
            .child(gutter)
            .child(fill_bar)
            .child(knob)
            .on_mouse_down(MouseButton::Left, move |event, _window, cx| {
                if let Some(frac) = fraction_down(&down_key, event.position.x) {
                    window::with(win, || {
                        store::set_float(&down_key, min + frac * span);
                        store::push_event_with(&down_key, format!("{:.3}", min + frac * span));
                    });
                    cx.notify(entity);
                }
            })
            .on_mouse_move(move |event, _window, cx| {
                if !event.dragging() {
                    return;
                }
                if let Some(frac) = fraction_move(&move_key, event.position.x) {
                    window::with(win, || {
                        store::set_float(&move_key, min + frac * span);
                        store::push_event_with(&move_key, format!("{:.3}", min + frac * span));
                    });
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

        // The element id matters here beyond hit-testing: GPUI keeps an
        // image's animation cursor (`ImgState`) keyed by its global element id,
        // so without `.id(..)` an animated GIF never advances a frame.
        Kind::Image(path) => style::apply(img(PathBuf::from(path)), &el.style)
            .id(ElementId::Name(el.id.clone().into()))
            .into_any_element(),

        Kind::Icon(path) => {
            let icon: Svg = svg().path(SharedString::from(path.clone()));
            let icon = style::apply(icon, &el.style);
            match style::rotation(&el.style) {
                Some(degrees) => icon
                    .with_transformation(gpui::Transformation::rotate(gpui::radians(
                        degrees.to_radians(),
                    )))
                    .into_any_element(),
                None => icon.into_any_element(),
            }
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
                    // A list is built during layout, after the frame that asked
                    // for it returned: this window's arena, and this window's
                    // state, are the ones the row builder must see.
                    let mut rows = Vec::with_capacity(range.len());
                    for index in range {
                        // The row builder appends its own elements to the
                        // arena. They are only needed until the row has been
                        // turned into a real GPUI element, so the arena is
                        // handed back straight afterwards.
                        let mark = window::with(win, mark);
                        let row = match window::with(win, || {
                            invoke(&build, vec![Value::Int(index as i64)])
                        }) {
                            Ok(Value::Int(handle)) if handle > 0 => {
                                window::with(win, || snapshot(handle))
                            }
                            Ok(_) => {
                                window::with(win, || {
                                    note_error(format!(
                                        "the row builder for row {index} answered no element"
                                    ))
                                });
                                None
                            }
                            Err(e) => {
                                window::with(win, || {
                                    note_error(format!(
                                        "the row builder for row {index} failed: {e}"
                                    ))
                                });
                                None
                            }
                        };
                        let element = match row {
                            Some(row) => render(&row, window, cx, entity),
                            None => div().into_any_element(),
                        };
                        window::with(win, || release_to(mark));
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

        Kind::Paths { paths, backgrounds } => {
            let handles = paths.clone();
            let specs = backgrounds.clone();
            let element = canvas(
                move |_bounds, _window, _cx| (),
                move |_bounds, _state, window, _cx| {
                    for (index, handle) in handles.iter().enumerate() {
                        let Some(path) = path::get(*handle) else { continue };
                        let background = match specs.get(index) {
                            Some(spec) if !spec.is_empty() => style::background(spec),
                            _ => style::colour("#ffffff").into(),
                        };
                        window.paint_path(path, background);
                    }
                },
            );
            style::apply(element, &el.style).into_any_element()
        }

        Kind::Mouse { child, id, kinds } => {
            let inner = snapshot(*child as i64)
                .map(|c| render(&c, window, cx, entity))
                .unwrap_or_else(|| div().into_any_element());
            let bounds_key = id.clone();
            let mut wrapper = style::apply(div(), &el.style)
                .on_children_prepainted(move |bounds, _window, _cx| {
                    if let Some(first) = bounds.first() {
                        MOUSE_BOUNDS
                            .with(|m| m.borrow_mut().insert((win, bounds_key.clone()), *first));
                    }
                })
                .id(ElementId::Name(el.id.clone().into()))
                .child(inner);

            if kinds.contains("down") {
                let key = id.clone();
                wrapper = wrapper.on_mouse_down(MouseButton::Left, move |event, _w, cx| {
                    window::with(win, || {
                        report_pointer(&key, "down", event.position, event.modifiers)
                    });
                    cx.notify(entity);
                });
            }
            if kinds.contains("up") {
                let key = id.clone();
                wrapper = wrapper.on_mouse_up(MouseButton::Left, move |event, _w, cx| {
                    window::with(win, || {
                        report_pointer(&key, "up", event.position, event.modifiers)
                    });
                    cx.notify(entity);
                });
            }
            if kinds.contains("click") {
                let key = id.clone();
                wrapper = wrapper.on_click(move |event, _w, cx| {
                    window::with(win, || {
                        report_pointer(&key, "click", event.position(), event.modifiers())
                    });
                    cx.notify(entity);
                });
            }
            if kinds.contains("right") {
                let key = id.clone();
                wrapper = wrapper.on_mouse_down(MouseButton::Right, move |event, _w, cx| {
                    window::with(win, || {
                        report_pointer(&key, "right", event.position, event.modifiers)
                    });
                    cx.notify(entity);
                });
            }
            if kinds.contains("move") {
                let key = id.clone();
                wrapper = wrapper.on_mouse_move(move |event, _w, cx| {
                    if event.dragging() {
                        window::with(win, || {
                            report_pointer(&key, "move", event.position, event.modifiers)
                        });
                        cx.notify(entity);
                    }
                });
            }
            wrapper.into_any_element()
        }

        Kind::Editor { key, placeholder } => {
            let field = editor::editor_element(win, key, placeholder, window, cx);
            style::apply(div(), &el.style).child(field).into_any_element()
        }

        Kind::StyledText { content, highlights } => {
            let ranges: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> = highlights
                .iter()
                .filter_map(|spec| parse_highlight(spec))
                .collect();
            let text = gpui::StyledText::new(content.clone()).with_highlights(ranges);
            style::apply(div(), &el.style).child(text).into_any_element()
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

/// Applies the focus-order tokens to an already identified element.
fn apply_focus_order<T: StatefulInteractiveElement + 'static>(el: T, spec: &str) -> T {
    let mut el = el;
    for (key, value) in style::tokens(spec) {
        let v = value.as_deref().unwrap_or("");
        el = match key.as_str() {
            "focusable" => el.focusable(),
            "tabstop" => el.tab_stop(v != "0"),
            "tabindex" => el.tab_index(v.parse::<isize>().unwrap_or(0)),
            _ => el,
        };
    }
    el
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
        // gradient.rs paints a trapezoid with a gradient through
        // `PathBuilder` + `paint_path`. Reproduced here as a painter so the
        // example keeps its canvas.
        "prism" => {
            let band = 80.0_f32;
            // Upstream positions this from `bounds.size.width`, not
            // `bounds.origin.x` — reproduced as written.
            let ox = width * 0.5 - width * 0.8 * 0.5;
            let oy = y0;
            let horizontal = band;
            let vertical = 30.0_f32;
            let mut builder = gpui::PathBuilder::fill();
            builder.move_to(point(px(ox), px(oy + band)));
            builder.line_to(point(px(ox + horizontal), px(oy + vertical)));
            builder.line_to(point(px(ox + width * 0.8 - horizontal), px(oy + vertical)));
            builder.line_to(point(px(ox + width * 0.8), px(oy + band)));
            builder.line_to(point(px(ox), px(oy + band)));
            if let Ok(path) = builder.build() {
                window.paint_path(
                    path,
                    gpui::linear_gradient(
                        180.0,
                        gpui::linear_color_stop(style::colour("#ff0000"), 0.0),
                        gpui::linear_color_stop(style::colour("#0000ff"), 1.0),
                    ),
                );
            }
        }
        _ => {
            window.paint_quad(fill(bounds, style::colour("#45475a")));
        }
    }
}
