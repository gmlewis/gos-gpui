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
    ClickEvent, DispatchPhase, Div, ElementId, EntityId, FocusHandle, ImgResourceLoader,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Render, Resource, ScrollDelta, ScrollWheelEvent, SharedString, Svg,
    Window, LOADING_DELAY,
};

use gossamer_binding::{NativeDispatch, PersistentCallback, Value};

use crate::editor;
use crate::list;
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

thread_local! {
    /// The row builder `gpui::run_with_rows` was given, if the program used it.
    /// Like the view, it is valid for the whole app because `run` does not
    /// return while any frame is drawn.
    static ROW_BUILDER: RefCell<Option<PersistentCallback>> = const { RefCell::new(None) };
}

pub fn clear_dispatch() {
    DISPATCH.with(|cell| cell.set(None));
}

/// Forgets everything the frame machinery holds for this process.
///
/// Nothing here owns a registration in the runtime: the view callback is a
/// `PersistentCallback`, which frees nothing when it is dropped, so the
/// thread-exit panic this function guards against cannot happen. It is kept because
/// it is the one place that says what per-frame state is, and it still runs on the
/// way out (see `app::release_retained`).
pub fn release_retained() {
    GUARD.with(|h| h.borrow_mut().clear());
    ROW_BUILDER.with(|r| *r.borrow_mut() = None);
    TEXT.with(|t| t.borrow_mut().clear());
    FONTS.with(|f| *f.borrow_mut() = None);
    ARENAS.with(|a| a.borrow_mut().clear());
    CAPTURED.with(|c| c.borrow_mut().clear());
    ROOTS.with(|r| r.borrow_mut().clear());
    FRAMES.with(|f| f.borrow_mut().clear());
    FOCUS.with(|f| f.borrow_mut().clear());
    DECORATIONS.with(|d| d.borrow_mut().clear());
}

/// Records the program's row builder, from `gpui::run_with_rows`.
///
/// It is given to `run` rather than to `gpui::list` because `run` is the only
/// binding call that does not return: a callback handed to a binding function is
/// valid for exactly that call, and a compiled caller releases the registration
/// the moment the call returns.
pub fn set_row_builder(rows: PersistentCallback) {
    ROW_BUILDER.with(|r| *r.borrow_mut() = Some(rows));
}

/// The program's row builder, if it started with `gpui::run_with_rows`.
fn row_builder() -> Option<PersistentCallback> {
    ROW_BUILDER.with(|r| r.borrow().clone())
}

/// Calls back into the Gossamer view callback and answers what it answered.
///
/// This is the *only* callback this binding invokes across a call boundary, and
/// it is the one that survives: `gpui::run` does not return until the app exits
/// (`Application::run` blocks in the platform's run loop), so the caller's
/// `gos_rt_binding_callback_release` — the release a compiled caller emits
/// immediately after a binding fn returns — is never reached while any frame is
/// drawn. Every other callback shape — a row builder, a
/// window's view, a close guard, a window-closed handler — is expressed as state and
/// events instead: each would be released the moment its binding fn returned, and
/// therefore fail on the compiled tiers. That is why `cb_fn` appears exactly twice in
/// `lib.rs`, on `run` and `run_with`.
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

#[derive(Clone)]
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
    /// A raster image loaded from a filesystem path.
    ///
    /// `loading` and `fallback` are handles to the subtrees the element draws while the load
    /// is pending and after it has failed, or `0` for none — see `gpui::image_request`. They
    /// are not this element's children: they are drawn *instead of* the picture, so keeping
    /// them apart from `El::children` is what stops them being drawn beside it.
    Image { path: String, loading: i64, fallback: i64 },
    Icon(String),
    Divider,
    Spacer(f64),
    /// A `uniform_list`: a virtualised list of `count` rows of `row_height`
    /// each. Its rows are built by the program's row builder — the callback
    /// handed to `gpui::run_with_rows` — once per row on screen, during layout.
    ///
    /// The builder is not stored here, and that is the point: it is reachable
    /// for the whole app because `run` does not return, while a builder given to
    /// `gpui::list` was released as soon as `list` returned and failed on the
    /// compiled tiers.
    Virtual { count: usize, row_height: f64 },
    /// A `canvas` painted by one of the routines below.
    Canvas { painter: String, phase: f64 },
    /// A scrollbar for a scrolled element: a track, drawn as the element's own background, with a
    /// thumb that follows the element's offset and can be pulled to set it.
    Scrollbar { id: String, axis: String },
    /// A `canvas` that paints built paths, in order.
    Paths { paths: Vec<i64>, backgrounds: Vec<String> },
    /// A wrapper that reports pointer events for the element it holds.
    ///
    /// `kinds` are heard only while the pointer is over the element — gpui gates every
    /// element listener on `hitbox.is_hovered(window)`. `window_kinds` are heard wherever
    /// the pointer is, through `Window::on_mouse_event`; they are the same names, and the
    /// two may be mixed.
    Mouse { child: usize, id: String, kinds: String, window_kinds: String },
    /// Text with per-byte-range highlights.
    StyledText { content: String, highlights: Vec<String> },
    /// A binding-owned text editor, registered under `key`.
    Editor { key: String, placeholder: String },
    /// A tooltip: not a child of the element it is given to, but a subtree gpui draws over it
    /// while the pointer rests on it.
    ///
    /// `gpui::tooltip(style)` pushes one and `gpui::child(tooltip, …)` fills it. Whatever it is
    /// given to never lays it out beside its other children — `render` lifts it out and hands it
    /// to `Div::tooltip`, and gpui then lays it out as **its own root**, at the pointer, with
    /// `AvailableSpace::min_size()` (`gpui-0.2.2/src/window.rs:2102`), in the same window and the
    /// same paint pass. That last part is why a `bounds` token inside a tooltip reports a real
    /// box: the tooltip's subtree is prepainted by the ordinary path, keyed to the same window.
    Tooltip,
    /// An anchored element container: anchors its children relative to a corner and position.
    Anchored {
        corner: usize,
        x: f64,
        y: f64,
        offset_x: f64,
        offset_y: f64,
    },
    /// A deferred element container: delays rendering of children with a priority.
    Deferred {
        priority: usize,
    },
}

#[derive(Clone)]
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
    /// What has been noted per window, kept past the frame that noted it — see `note_error`.
    static NOTED: RefCell<HashMap<i64, Vec<String>>> = RefCell::new(HashMap::new());
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
    /// The window's *content* rectangle, which the platform insets from the outer
    /// one on Windows, Wayland and X11 — and which macOS reports as the outer one,
    /// because there is no such inset here.
    static INNER_RECT: RefCell<HashMap<i64, (f64, f64, f64, f64)>> = RefCell::new(HashMap::new());
    /// What `Window::window_bounds` says about the window: windowed, maximized or
    /// fullscreen, and — for the last two — the size it would go back to. gpui keeps
    /// that restore size inside the `WindowBounds` variant, which is the only place a
    /// program can read it.
    static BOUNDS_STATE: RefCell<HashMap<i64, (&'static str, f64, f64)>> = RefCell::new(HashMap::new());
    /// Who draws the window's decoration and how wide a strip the program draws it in,
    /// as of the start of this frame. Both answers are the platform's — `Window` forwards
    /// them to the window manager — and both are read here because a binding function has
    /// no `Window` to ask. The inset is the width this program *set*, which is the same
    /// number on every platform; the decorations are the platform's answer to a request,
    /// which is not, and which a window manager with no compositor answers with `server`
    /// however loudly it was asked for `client`.
    static DECORATIONS: RefCell<HashMap<i64, (&'static str, f64)>> = RefCell::new(HashMap::new());
    /// The platform colours the window's root asked to override, recorded as the frame
    /// that carried them was built. They are applied to the next frame's colours, because
    /// the colours are read at the *start* of a frame — before the view callback that
    /// describes the root — and a program's style string is only known once it has run.
    static THEME_OVERRIDES: RefCell<HashMap<i64, Vec<(String, gpui::Hsla)>>> =
        RefCell::new(HashMap::new());
    static DISPLAYS: RefCell<HashMap<i64, Vec<Display>>> = RefCell::new(HashMap::new());
    static KEYBOARD: RefCell<String> = const { RefCell::new(String::new()) };
    static WINDOW_MAXIMIZED: RefCell<HashMap<i64, bool>> = RefCell::new(HashMap::new());
    static WINDOW_MINIMIZED: RefCell<HashMap<i64, bool>> = RefCell::new(HashMap::new());
    static WINDOW_FULLSCREEN: RefCell<HashMap<i64, bool>> = RefCell::new(HashMap::new());
    static WINDOW_SCALE: RefCell<HashMap<i64, f64>> = RefCell::new(HashMap::new());
    static WINDOW_TITLE: RefCell<HashMap<i64, String>> = RefCell::new(HashMap::new());
    static WINDOW_BOUNDS: RefCell<HashMap<i64, (f64, f64, f64, f64)>> = RefCell::new(HashMap::new());
    static WINDOW_ACTIVE: RefCell<HashMap<i64, bool>> = RefCell::new(HashMap::new());
    static WINDOW_HOVERED: RefCell<HashMap<i64, bool>> = RefCell::new(HashMap::new());
    static CLIPBOARD_FALLBACK: RefCell<String> = const { RefCell::new(String::new()) };
    static ACTION_REGISTRY: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());

    /// What the frame shapes text with, per window: the window's own text
    /// system, and the font and size its text style resolves to.
    ///
    /// Captured in `begin_frame`, because measuring needs both and a binding
    /// function has no `Window` to ask.
    static TEXT: RefCell<HashMap<i64, TextMetrics>> = RefCell::new(HashMap::new());
}

/// The three things a measurement needs. `Arc`, not a pointer: the text system
/// is refcounted, so holding one past the frame it came from is ordinary
/// ownership rather than the erasure `DISPATCH` does.
struct TextMetrics {
    system: std::sync::Arc<gpui::WindowTextSystem>,
    font: gpui::Font,
    size: Pixels,
    line: Pixels,
    rem: Pixels,
    /// The window's appearance, `"light"` or `"dark"`, as of the start of the
    /// frame. What `gpui::default_color` is chosen from.
    appearance: String,
    /// What the system clipboard held at the start of the frame.
    clipboard: Clipboard,
    /// The inherited line height, which `lh=` and `lhpx=` override. A
    /// `DefiniteLength` rather than pixels, because a relative one has to be
    /// resolved against the size the style *ends up* asking for.
    line_length: gpui::DefiniteLength,
}

thread_local! {
    /// The application's text system, kept for the whole run.
    ///
    /// Separate from [`TEXT`], which is per window and per frame: the set of
    /// fonts a machine has is not a per-window question, and the answer must
    /// still be there when no frame is being built.
    static FONTS: RefCell<Option<std::sync::Arc<gpui::TextSystem>>> = const { RefCell::new(None) };
}

/// Keeps the application's text system, once, at the start of the run.
pub fn set_text_system(system: std::sync::Arc<gpui::TextSystem>) {
    FONTS.with(|f| *f.borrow_mut() = Some(system));
}

/// Every font family this machine offers, as the platform names them.
pub fn font_families() -> Vec<String> {
    FONTS.with(|f| f.borrow().as_ref().map(|s| s.all_font_names()).unwrap_or_default())
}

/// The window's line height, as of the start of this frame.
pub fn line_height() -> f64 {
    let win = window::current();
    TEXT.with(|t| t.borrow().get(&win).map(|m| f32::from(m.line) as f64)).unwrap_or(0.0)
}


/// `"start-end prop,prop"` over UTF-8 byte ranges, where the properties are the
/// per-range highlight vocabulary.
///
/// Typos in the property half are reported once, on screen, rather than silently
/// doing nothing — the same treatment the element style language gets. A
/// highlight that quietly drops `colur=#ff0000` is a syntax highlighter that
/// quietly stops colouring one kind of token.
fn parse_highlight(spec: &str) -> Option<(std::ops::Range<usize>, gpui::HighlightStyle)> {
    let (range, props) = spec.split_once(' ')?;
    let (start, end) = range.split_once('-')?;
    check_highlight(props);
    Some((start.parse().ok()?..end.parse().ok()?, highlight_style(props)))
}

/// Builds a [`gpui::HighlightStyle`] from the property half of a highlight.
///
/// All seven of its properties are reachable: `color`, `bg`,
/// `weight`/`bold`/`semibold`, `italic`, `underline`, `strike` and `fade`. The
/// three decorations carry detail as well — `undercolor`, `underthick`,
/// `underwavy`, `strikecolor`, `strikethick` — and each of those **implies the
/// decoration it belongs to**, so `underthick=3` is an underline.
fn highlight_style(props: &str) -> gpui::HighlightStyle {
    let mut style = gpui::HighlightStyle::default();
    for (key, value) in style::tokens(props) {
        let v = value.as_deref().unwrap_or("");
        match key.as_str() {
            "bold" => style.font_weight = Some(gpui::FontWeight::BOLD),
            "semibold" => style.font_weight = Some(gpui::FontWeight::SEMIBOLD),
            "weight" => {
                style.font_weight =
                    Some(gpui::FontWeight::from(v.parse::<f32>().unwrap_or(400.0)))
            }
            "italic" => style.font_style = Some(gpui::FontStyle::Italic),
            "color" => style.color = Some(style::colour(v)),
            "bg" => style.background_color = Some(style::colour(v)),
            "underline" => {
                style.underline.get_or_insert_with(gpui::UnderlineStyle::default);
            }
            "undercolor" => {
                style
                    .underline
                    .get_or_insert_with(gpui::UnderlineStyle::default)
                    .color = Some(style::colour(v));
            }
            "underthick" => {
                style
                    .underline
                    .get_or_insert_with(gpui::UnderlineStyle::default)
                    .thickness = px(v.parse::<f32>().unwrap_or(1.0));
            }
            "underwavy" => {
                style
                    .underline
                    .get_or_insert_with(gpui::UnderlineStyle::default)
                    .wavy = true;
            }
            "strike" => {
                style
                    .strikethrough
                    .get_or_insert_with(gpui::StrikethroughStyle::default);
            }
            "strikecolor" => {
                style
                    .strikethrough
                    .get_or_insert_with(gpui::StrikethroughStyle::default)
                    .color = Some(style::colour(v));
            }
            "strikethick" => {
                style
                    .strikethrough
                    .get_or_insert_with(gpui::StrikethroughStyle::default)
                    .thickness = px(v.parse::<f32>().unwrap_or(1.0));
            }
            // gpui clamps this when it paints; clamping here as well keeps the
            // value a program reads back the one it asked for.
            "fade" => style.fade_out = Some(v.parse::<f32>().unwrap_or(0.0).clamp(0.0, 1.0)),
            _ => {}
        }
    }
    style
}

/// Reports highlight tokens this module does not understand, once each.
fn check_highlight(props: &str) {
    let unknown = style::unknown_highlight_tokens(props);
    let fresh = unknown
        .into_iter()
        .filter(|token| REPORTED.with(|r| r.borrow_mut().insert(format!("highlight:{token}"))))
        .collect::<Vec<_>>();
    if !fresh.is_empty() {
        let message = format!("unknown highlight tokens: {}", fresh.join(", "));
        eprintln!("gpui: {message}");
        note_error(message);
    }
}

/// The `HighlightStyle` properties `props` sets, as a comma-separated list in a
/// fixed order: `color`, `background`, `weight`, `style`, `underline`,
/// `strikethrough`, `fade`.
///
/// The reader for a highlight spec — a program that generates one from a theme
/// can check what it produced instead of discovering it on screen.
pub fn highlight_properties(props: &str) -> String {
    let style = highlight_style(props);
    let mut names = Vec::new();
    if style.color.is_some() {
        names.push("color");
    }
    if style.background_color.is_some() {
        names.push("background");
    }
    if style.font_weight.is_some() {
        names.push("weight");
    }
    if style.font_style.is_some() {
        names.push("style");
    }
    if style.underline.is_some() {
        names.push("underline");
    }
    if style.strikethrough.is_some() {
        names.push("strikethrough");
    }
    if style.fade_out.is_some() {
        names.push("fade");
    }
    names.join(",")
}

/// What the system clipboard held when the frame began.
///
/// Read once per frame rather than on demand, because a binding function has no
/// `App` to read it with — the same reason the default colours and the pointer's
/// position are cached there.
#[derive(Clone)]
enum Clipboard {
    Empty,
    Text(String),
    /// Something that is not text. An image, in practice.
    Other,
}

/// Reads the clipboard.
fn read_clipboard(cx: &App) -> Clipboard {
    match cx.read_from_clipboard() {
        Some(item) => match item.text() {
            Some(text) => Clipboard::Text(text),
            None => Clipboard::Other,
        },
        None => Clipboard::Empty,
    }
}

/// The clipboard's text, or `""` when it holds none.
pub fn clipboard_text() -> String {
    let win = window::current();
    let text = TEXT.with(|t| {
        t.borrow()
            .get(&win)
            .map(|m| match &m.clipboard {
                Clipboard::Text(text) => text.clone(),
                _ => String::new(),
            })
            .unwrap_or_default()
    });
    if !text.is_empty() {
        text
    } else {
        CLIPBOARD_FALLBACK.with(|c| c.borrow().clone())
    }
}

pub fn set_clipboard_fallback(text: &str) {
    CLIPBOARD_FALLBACK.with(|c| *c.borrow_mut() = text.to_string());
}

/// What the clipboard holds: `"text"`, `"image"`, or `"empty"`.
///
/// `"image"` means the item is there and none of its entries is text, which is
/// what a screenshot or a copied picture looks like. The binding does not decode
/// it: writing an image needs gpui's own `Image` type, which nothing here
/// constructs. See [`crate::clipboard_text`].
pub fn clipboard_kind() -> String {
    let win = window::current();
    TEXT.with(|t| {
        t.borrow()
            .get(&win)
            .map(|m| match m.clipboard {
                Clipboard::Text(_) => "text",
                Clipboard::Other => "image",
                Clipboard::Empty => "empty",
            })
            .unwrap_or("empty")
            .to_string()
    })
}

/// Every [`gpui::colors::Colors`] field, named, as `#rrggbbaa`.
///
/// The eight names are `gpui::colour_names()`; there are exactly eight fields, so
/// this is the whole of what the platform offers rather than a selection from it.
fn named_colors(colors: &gpui::colors::Colors) -> HashMap<String, String> {
    colour_fields(colors)
        .into_iter()
        .map(|(name, colour)| (name.to_string(), hex(colour)))
        .collect()
}

/// The eight platform colours, each once, under the name it answers to.
///
/// One list, two renderings: [`named_colors`] turns it into the hex strings
/// `gpui::default_color` answers with, and [`colour_table`] into the `Hsla` values the
/// style language resolves `theme:<name>` against. Neither can gain a ninth without the
/// other, and `gpui::colour_names` is the same eight.
fn colour_fields(colors: &gpui::colors::Colors) -> [(&'static str, gpui::Rgba); 8] {
    [
        ("text", colors.text),
        ("selected_text", colors.selected_text),
        ("background", colors.background),
        ("disabled", colors.disabled),
        ("selected", colors.selected),
        ("border", colors.border),
        ("separator", colors.separator),
        ("container", colors.container),
    ]
}

/// `#rrggbbaa`, lower case: the form every colour reader in this binding answers in.
pub fn hex(c: gpui::Rgba) -> String {
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
        (c.a * 255.0).round() as u8,
    )
}

/// The frame's colours as the style language's vocabulary, for `theme:<name>` values
/// to resolve against.
fn colour_table(colors: &gpui::colors::Colors) -> HashMap<String, gpui::Hsla> {
    colour_fields(colors)
        .into_iter()
        .map(|(name, colour)| (name.to_string(), colour.into()))
        .collect()
}

/// What a style value resolves to right now, as `#rrggbbaa`.
///
/// Literals resolve to themselves; a `theme:<name>` value resolves against this window's
/// table — the platform's palette, the theme [`crate::view::theme_choice`] chose, and any
/// `theme.<field>=` overrides, in that order. The same call the element path makes, so a
/// program can see what it is about to draw with instead of guessing.
pub fn resolve_colour(spec: &str) -> String {
    hex(style::colour(spec).into())
}

/// gpui's two appearances, as one word each.
pub fn appearance_of(appearance: gpui::WindowAppearance) -> String {
    match appearance {
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark => "dark".into(),
        _ => "light".into(),
    }
}

/// The window's appearance, `"light"` or `"dark"`.
pub fn appearance() -> String {
    let win = window::current();
    TEXT.with(|t| {
        t.borrow()
            .get(&win)
            .map(|m| m.appearance.clone())
            .unwrap_or_default()
    })
}

/// The eight names [`default_color`] answers to.
pub fn colour_names() -> Vec<String> {
    let mut names: Vec<String> = named_colors(&gpui::colors::Colors::light())
        .into_keys()
        .collect();
    names.sort();
    names
}

/// What a default colour would be in a given appearance.
///
/// The other half of [`default_color`]: a program that wants a colour the window
/// is *not* using — a contrasting border, an overlay that reads on the other
/// theme — asks for it by name instead of hard-coding both sets.
pub fn default_color_for(appearance: &str, name: &str) -> String {
    let colors = match appearance {
        "dark" => gpui::colors::Colors::dark(),
        _ => gpui::colors::Colors::light(),
    };
    named_colors(&colors).get(name).cloned().unwrap_or_default()
}

/// The default colours, cached at frame start.
pub fn default_color(name: &str) -> String {
    let win = window::current();
    COLORS.with(|c| c.borrow().get(&win).and_then(|m| m.get(name)).cloned().unwrap_or_default())
}

/// The palette a program asked the named colour table to answer from, as
/// `"system"`, `"light"` or `"dark"`.
///
/// `"system"` — the default — means the platform's own choice of palette for this
/// window's appearance; the other two pin the table whatever the desktop is set to.
/// It selects the colours a *program* reads and draws with; it does not change what
/// [`appearance`] reports, which stays the operating system's answer, and it does not
/// repaint the platform's window chrome.
pub fn theme_choice(win: i64) -> String {
    store::get_text_in(win, "~theme", "system")
}

/// How far the scrolled element `id` has scrolled in this window, on `axis`.
///
/// gpui's sign, kept as it is: zero at the top, negative as the content scrolls
/// down. Answers `0.0` for an id no scrolled container has been drawn under —
/// which is also what a container that has not scrolled yet answers, so a
/// program that needs to tell the two apart asks for [`scroll_max_offset`],
/// which is positive only where a scrolled container exists.
pub fn scroll_offset(id: &str, axis: &str) -> f64 {
    match scrolled(window::current(), id) {
        Some(handle) => axis_value(handle.offset(), axis),
        None => 0.0,
    }
}

/// How much content the scrolled element `id` has to scroll past on `axis`: the
/// content size minus the container's, clamped at zero, and **positive**.
pub fn scroll_max_offset(id: &str, axis: &str) -> f64 {
    let Some(handle) = scrolled(window::current(), id) else {
        return 0.0;
    };
    let max = handle.max_offset();
    match axis {
        "x" => f32::from(max.width) as f64,
        _ => f32::from(max.height) as f64,
    }
}

/// Everything a measurement needs, with the style's overrides applied.
///
/// `spec` is the text half of the style language: `fs=17`, `weight=600`, `bold`,
/// `semibold`, `italic`. The family and the size come from the window's
/// inherited text style — which is what makes `font=2xl` on an ancestor count —
/// and `fs=` overrides the size. Anything else in `spec` is ignored.
fn text_metrics(
    spec: &str,
) -> Option<(
    std::sync::Arc<gpui::WindowTextSystem>,
    gpui::Font,
    Pixels,
    gpui::DefiniteLength,
)> {
    let win = window::current();
    let (system, mut font, mut size, rem, mut line_length) = TEXT.with(|t| {
        t.borrow()
            .get(&win)
            .map(|m| (m.system.clone(), m.font.clone(), m.size, m.rem, m.line_length))
    })?;
    let rem = f32::from(rem);
    let mut italic = false;
    for (key, value) in style::tokens(spec) {
        let v = value.as_deref().unwrap_or("");
        match key.as_str() {
            // The same four shapes an element takes: a size, a family, or both.
            // A family that is selected here is the family a *measurement* is
            // of, which is the whole point of the token.
            "font" => {
                let (family, font_size) = style::font_parts(v);
                // Same reading as the element path: after a comma the tail is a
                // size and nothing else, and a lone word is a family only when it
                // is neither a ramp name nor a number.
                match family {
                    Some(family) => {
                        if !family.is_empty() {
                            font.family = family.to_string().into();
                        }
                        if let Some(ramped) = style::ramp(font_size, rem) {
                            size = px(ramped);
                        } else if let Ok(explicit) = font_size.parse::<f32>() {
                            size = px(explicit);
                        }
                    }
                    None => {
                        if let Some(ramped) = style::ramp(font_size, rem) {
                            size = px(ramped);
                        } else if let Ok(explicit) = font_size.parse::<f32>() {
                            size = px(explicit);
                        } else if !font_size.is_empty() {
                            font.family = font_size.to_string().into();
                        }
                    }
                }
            }
            "fs" => size = px(v.parse::<f32>().unwrap_or(f32::from(size))),
            "lh" => line_length = relative(v.parse::<f32>().unwrap_or(1.0)),
            "lhpx" => line_length = px(v.parse::<f32>().unwrap_or(0.0)).into(),
            "weight" => font.weight = gpui::FontWeight::from(v.parse::<f32>().unwrap_or(400.0)),
            "bold" => font.weight = gpui::FontWeight::BOLD,
            "semibold" => font.weight = gpui::FontWeight::SEMIBOLD,
            "italic" => italic = true,
            _ => {}
        }
    }
    if italic {
        font.style = gpui::FontStyle::Italic;
    }
    Some((system, font, size, line_length))
}

/// Shape `text` the way this window would draw it, with the style's overrides applied.
///
/// The one place this crate asks gpui's text system for a laid-out line, so every measurement
/// below answers from the same shaping and two of them cannot disagree. A newline is flattened
/// into a space first, because gpui's shaper panics on one — the same thing the editor does to
/// everything that reaches it. `None` outside a frame, where there is no window's text style to
/// measure against.
fn shape(text: &str, spec: &str) -> Option<gpui::ShapedLine> {
    let (system, font, size, _) = text_metrics(spec)?;
    let text = text.replace('\n', " ");
    let run = gpui::TextRun {
        len: text.len(),
        font,
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    Some(system.shape_line(text.into(), size, &[run], None))
}

/// The width `text` advances to, in pixels, as this window would draw it.
pub fn text_width(text: &str, spec: &str) -> f64 {
    match shape(text, spec) {
        Some(line) => f32::from(line.width) as f64,
        None => 0.0,
    }
}

/// Where character `index` begins, in pixels from the start of the run.
///
/// `index` is a **byte** offset into `text` — the unit the editor's own offsets and gpui's
/// `LineLayout::x_for_index` (`gpui-0.2.2/src/text_system/line_layout.rs:105`) both use. An index
/// past the end answers the run's whole width, and one that lands inside a character answers the
/// start of the glyph after it, which is the shaper's own rule and not a rounding of this
/// function's.
///
/// This is the number upstream's `input.rs` paints a caret and a selection with — `bounds.left() +
/// line.x_for_index(cursor)` (`tools/upstream/examples/input.rs:492`) — and until it existed no
/// Gosammer program could ask where a character it drew had ended up.
pub fn text_x_of_index(text: &str, index: i64, spec: &str) -> f64 {
    match shape(text, spec) {
        Some(line) => f32::from(line.x_for_index(index.max(0) as usize)) as f64,
        None => 0.0,
    }
}

/// Which character boundary lies nearest `x` — the inverse of [`text_x_of_index`].
///
/// The other half of a hit test: upstream asks it when a *click* has to become a caret position,
/// where this asks it when a caret position has to become a pixel. Answers a byte offset, `0` for
/// an `x` at or before the run's start and the run's whole length for one at or past its end.
pub fn text_index_at(text: &str, x: f64, spec: &str) -> i64 {
    match shape(text, spec) {
        Some(line) => line.closest_index_for_x(px(x as f32)) as i64,
        None => 0,
    }
}

/// The em width of the measured font, in pixels.
pub fn text_em_width(spec: &str) -> f64 {
    let Some((system, font, size, _)) = text_metrics(spec) else {
        return 0.0;
    };
    let id = system.resolve_font(&font);
    system.em_width(id, size).map(|w| f32::from(w) as f64).unwrap_or(0.0)
}

/// How far the first character of `ch` advances, in pixels.
pub fn text_advance(ch: &str, spec: &str) -> f64 {
    let Some(c) = ch.chars().next() else {
        return 0.0;
    };
    let Some((system, font, size, _)) = text_metrics(spec) else {
        return 0.0;
    };
    let id = system.resolve_font(&font);
    system.advance(id, size, c).map(|s| f32::from(s.width) as f64).unwrap_or(0.0)
}

/// The height one line occupies: what `lh=` or `lhpx=` asks for, or the font's
/// own ascent plus descent when the style names neither.
///
/// A relative `lh=` is resolved against the size the style *ends up* asking for,
/// so `fs=40 lh=2` is 80 pixels and not twice the window's inherited size.
pub fn text_line_height(spec: &str) -> f64 {
    let Some((system, font, size, line_length)) = text_metrics(spec) else {
        return 0.0;
    };
    let named = style::tokens(spec)
        .iter()
        .any(|(key, _)| key == "lh" || key == "lhpx");
    if named {
        let win = window::current();
        let rem = TEXT
            .with(|t| t.borrow().get(&win).map(|m| m.rem))
            .unwrap_or(size);
        return f32::from(line_length.to_pixels(gpui::AbsoluteLength::Pixels(size), rem)) as f64;
    }
    let id = system.resolve_font(&font);
    (f32::from(system.ascent(id, size)) + f32::from(system.descent(id, size))) as f64
}

/// The event a refused close raises in the window it was refused for.
pub const CLOSE_REQUESTED: &str = "close.requested";

thread_local! {
    /// Each window's close guard, if it has one.
    ///
    /// This is *state*, not a callback, and that is the whole point: a Gossamer
    /// closure stored by `gpui::on_window_should_close` is released by a compiled
    /// caller the moment that binding fn returns, so the guard would answer an
    /// error and map to "yes, close" — silently useless, which is the exact case
    /// CL1 exists to catch. The platform's own should-close closure is a *native*
    /// one, installed by this
    /// binding, and it reads this state; the program is asked through the event
    /// queue instead of through a callback, which works on every tier.
    static GUARD: RefCell<HashMap<i64, Guard>> = RefCell::new(HashMap::new());
}

/// One window's close guard.
#[derive(Clone, Copy, Default)]
struct Guard {
    /// The program asked to be consulted before this window closes.
    armed: bool,
    /// The program answered `gpui::allow_close()`: the close may go ahead.
    allow: bool,
}

/// Arms — or with `on = false` disarms — the close guard for this window.
///
/// Disarming forgets an `allow_close()` as well, so a program that re-arms a
/// guard for later does not inherit the old answer.
pub fn set_guard(on: bool) {
    let win = window::current();
    GUARD.with(|g| {
        let mut g = g.borrow_mut();
        let entry = g.entry(win).or_default();
        entry.armed = on;
        entry.allow = false;
    });
}

/// The program let this window close: clears the refusal the guard raised.
pub fn allow_close() {
    let win = window::current();
    GUARD.with(|g| {
        if let Some(entry) = g.borrow_mut().get_mut(&win) {
            entry.allow = true;
        }
    });
}

/// Whether `win` may close, raising `close.requested` in it when it may not.
///
/// Answers `true` for a window with no guard, an armed-but-allowed guard, and a
/// disarmed one — so an unguarded program never sees an event. It answers
/// `false` exactly when the program asked to be consulted and has not yet
/// answered, and raises the event on the way: the event is the question, and the
/// window stays open until the program answers with `gpui::allow_close()`.
pub fn may_close(win: i64) -> bool {
    let guard = GUARD.with(|g| g.borrow().get(&win).copied().unwrap_or_default());
    if !guard.armed || guard.allow {
        return true;
    }
    store::push_event_in(win, CLOSE_REQUESTED);
    false
}

/// The style tokens in `spec` this crate does not understand.
///
/// The reader behind the `unknown style tokens:` banner, for a program that
/// builds a style string from data and would rather check it than see it drawn.
pub fn unknown_style_tokens(spec: &str) -> Vec<String> {
    style::unknown_tokens(spec)
}

/// Where a program's assets live, as it declared it, or `""` when it declared nothing.
pub fn asset_root() -> String {
    store::get_text("~asset.root", "")
}

/// Resolves an asset name against the program's declared root.
///
/// An empty name, and an absolute path, come back unchanged — a program that knows where its
/// asset is is believed. Otherwise the declared root is tried first, and the name is handed back
/// as it was given when the root does not have it. That fallback is what keeps a program that
/// declares no root using a name exactly as written: `img(PathBuf::from(path))` and
/// `svg().path(..)` resolve against the process's working directory, which is where a
/// double-clicked program's working directory is *not*.
///
/// One resolver, three callers: the `image` element, the `icon` element, and `gpui::asset_path`,
/// so what a program checks is what it gets.
pub fn asset_path(name: &str) -> String {
    if name.is_empty() || std::path::Path::new(name).is_absolute() {
        return name.to_string();
    }
    let root = asset_root();
    if root.is_empty() {
        return name.to_string();
    }
    let candidate = std::path::Path::new(&root).join(name);
    if candidate.exists() {
        candidate.to_string_lossy().into_owned()
    } else {
        name.to_string()
    }
}

// ---------------------------------------------------------------------------
// what an image element drew, and the rule behind it
// ---------------------------------------------------------------------------

thread_local! {
    /// What each image element drew the last time it was rendered: `"image"`, `"loading"`,
    /// `"fallback"` or `"blank"`, keyed by `(window, element id)`.
    ///
    /// This is `gpui::image_state`'s reading, and it is the failure signal the asset row was
    /// missing: before it, a program could not tell an image that had loaded from one that
    /// had failed, because both were blank.
    static IMAGE_SHOWN: RefCell<HashMap<(i64, String), &'static str>> = RefCell::new(HashMap::new());
    /// When each image's load was first seen pending, keyed the same way.
    ///
    /// GPUI shows the loading subtree only once `LOADING_DELAY` has passed since the load
    /// started (`elements/img.rs:373-395`), so the moment the load *started* has to be kept
    /// across frames; a frame cannot tell a load that has just begun from one that has been
    /// running all along. Cleared the moment the load answers.
    static IMAGE_PENDING: RefCell<HashMap<(i64, String), std::time::Instant>> =
        RefCell::new(HashMap::new());
}

/// Whether the image named `name` would draw its fallback subtree, because there is no file
/// there to read.
///
/// This is the rule `gpui::image_request`'s fallback turns on, asked without a window. An image
/// element's source is always a filesystem path — [`asset_path`] resolves it and the loader reads
/// it with `fs::read` (`gpui-0.2.2 src/elements/img.rs:601`) — so for a source like that "the
/// loader will fail" and "there is no readable file at this path" are the same statement, and the
/// second can be answered by a `stat`. A directory is not a readable image and answers `true`:
/// the rule agrees with the loader rather than with "the path exists".
pub fn image_missing(name: &str) -> bool {
    let path = asset_path(name);
    !std::fs::metadata(&path).map(|meta| meta.is_file()).unwrap_or(false)
}

/// What the image element `id` drew the last time it was rendered, in this window.
///
/// `"image"`, `"loading"`, `"fallback"` or `"blank"` — and `""` for an image this window has not
/// rendered, which is also what a window that has rendered none answers. Like the other readers
/// here it is one frame behind: the tree is built by the view callback and the decision this
/// reports is taken when that tree is turned into elements, so a program reads the frame before.
pub fn image_state(win: i64, id: &str) -> String {
    IMAGE_SHOWN
        .with(|m| m.borrow().get(&(win, id.to_string())).copied())
        .unwrap_or("")
        .to_string()
}

/// Forgets every image's pending-since stamp, so a program that wants a load re-timed can say so.
pub fn forget_image_timings(win: i64) {
    IMAGE_PENDING.with(|m| m.borrow_mut().retain(|(w, _), _| *w != win));
}

/// The platform colours `spec` overrides, as `"selected,border"`.
pub fn theme_override(spec: &str) -> String {
    style::theme_overrides(spec)
        .into_iter()
        .map(|(field, _)| field)
        .collect::<Vec<_>>()
        .join(",")
}

/// The interaction refinements `spec` carries, as `"hover,active,focus"`.
pub fn style_refinements(spec: &str) -> String {
    let (hover, active, focus) = style::interaction(spec);
    let mut kinds = Vec::new();
    if hover.is_some() {
        kinds.push("hover");
    }
    if active.is_some() {
        kinds.push("active");
    }
    if focus.is_some() {
        kinds.push("focus");
    }
    kinds.join(",")
}

/// The scrolled element's viewport rectangle, in window pixels: `field` is `x`,
/// `y`, `w` or `h`.
///
/// This is the container's own box, not its content's — `h` is how much of the
/// scrolled thing is on screen, which is what a scrollbar needs to size its
/// thumb. Content extent is [`scroll_max_offset`]'s job.
pub fn scroll_bounds(id: &str, field: &str) -> f64 {
    let Some(handle) = scrolled(window::current(), id) else {
        return 0.0;
    };
    let bounds = handle.bounds();
    match field {
        "x" => f32::from(bounds.origin.x) as f64,
        "y" => f32::from(bounds.origin.y) as f64,
        "w" => f32::from(bounds.size.width) as f64,
        "h" => f32::from(bounds.size.height) as f64,
        _ => 0.0,
    }
}

/// Sets the offset of the scrolled element `id` in this window, in gpui's sign.
///
/// Answers `false` when no scrolled container has been drawn under that id, so a
/// program that misspells one is told rather than silently left at the top. The
/// offset is clamped to the scrolled range during the next prepaint, so setting
/// one past the end parks at the end.
pub fn scroll_to(id: &str, x: f64, y: f64) -> bool {
    let Some(handle) = scrolled(window::current(), id) else {
        return false;
    };
    handle.set_offset(gpui::point(px(x as f32), px(y as f32)));
    true
}

/// The scroll handle for `id` in `win` — a scrolled container's own, or a
/// virtual list's base handle, whichever the program drew under that name.
///
/// A program names the thing that scrolls, not the kind of thing it is; a
/// scrollbar written for one works beside the other.
fn scrolled(win: i64, id: &str) -> Option<gpui::ScrollHandle> {
    scroll::find(win, id).or_else(|| list::base_handle(win, id))
}

fn axis_value(point: gpui::Point<Pixels>, axis: &str) -> f64 {
    match axis {
        "x" => f32::from(point.x) as f64,
        _ => f32::from(point.y) as f64,
    }
}

/// The rows the virtual list `id` put on screen the last time it laid out,
/// inclusive, or `None` if it has never laid out.
pub fn list_range(id: &str) -> Option<(usize, usize)> {
    list::range(window::current(), id)
}

/// Scrolls the virtual list `id` so that row `index` is placed by `strategy`.
///
/// Answers `false` for a list nothing has drawn, or a strategy name that is not
/// one of `top`, `center` or `bottom` — a misspelling is reported rather than
/// silently treated as `top`.
pub fn list_scroll_to(id: &str, index: i64, strategy: &str) -> bool {
    use gpui::ScrollStrategy;
    let strategy = match strategy {
        "top" => ScrollStrategy::Top,
        "center" => ScrollStrategy::Center,
        "bottom" => ScrollStrategy::Bottom,
        _ => return false,
    };
    let Some(handle) = list::find(window::current(), id) else {
        return false;
    };
    handle.scroll_to_item(index.max(0) as usize, strategy);
    true
}

/// Reads what the frame will ask for while it still has the window: the
/// pointer's position, which focus handles are focused, and the default
/// colours. Called once, before the view callback runs.
pub fn begin_frame(window: &mut Window, cx: &App) {
    let win = window::current();
    // Once per window, and not every frame: the driver's name cannot change while a program runs,
    // and the call is a trip through the platform layer.
    crate::gpu::sample(win, window);
    // Likewise the title: sampled once here, and again by `Request::SetTitle` whenever a program
    // changes it. A program can read the title its window opened with, which is the platform's own
    // answer rather than a copy of the argument it passed to `gpui::run`.
    if !crate::options::title_current(win) {
        crate::options::observe_title(win, &window.window_title());
    }
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
    let inner = window.inner_window_bounds().get_bounds();
    INNER_RECT.with(|r| {
        r.borrow_mut().insert(
            win,
            (
                f32::from(inner.origin.x) as f64,
                f32::from(inner.origin.y) as f64,
                f32::from(inner.size.width) as f64,
                f32::from(inner.size.height) as f64,
            ),
        )
    });
    let state = match window.window_bounds() {
        gpui::WindowBounds::Windowed(_) => ("windowed", 0.0, 0.0),
        gpui::WindowBounds::Maximized(restore) => (
            "maximized",
            f32::from(restore.size.width) as f64,
            f32::from(restore.size.height) as f64,
        ),
        gpui::WindowBounds::Fullscreen(restore) => (
            "fullscreen",
            f32::from(restore.size.width) as f64,
            f32::from(restore.size.height) as f64,
        ),
    };
    BOUNDS_STATE.with(|b| {
        b.borrow_mut().insert(win, state);
    });
    // Who draws the window's decoration, and the strip this program draws its own in.
    // The platform's answer to a *request*, not a record of one: a window manager with
    // no compositor keeps `server` however loudly it was asked for `client`, which is
    // the whole reason a program reads this instead of assuming its request was taken.
    let decorations = match window.window_decorations() {
        gpui::Decorations::Server => "server",
        gpui::Decorations::Client { .. } => "client",
    };
    let inset = window.client_inset().map_or(0.0, |px| f32::from(px) as f64);
    DECORATIONS.with(|d| {
        d.borrow_mut().insert(win, (decorations, inset));
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
        // `get_global` hands back the shared `Arc`; this frame wants its own copy to
        // paint overrides onto, so the clone is of the `Colors` inside it.
        let mut colors = gpui::colors::Colors::clone(gpui::colors::Colors::get_global(cx));
        // The palette the program asked for, in place of the one the window's own
        // appearance installed. `"system"` — the default, and what a program that
        // never calls `set_theme` has — leaves the platform's choice alone.
        match theme_choice(win).as_str() {
            "light" => colors = gpui::colors::Colors::light(),
            "dark" => colors = gpui::colors::Colors::dark(),
            _ => {}
        }
        // What the root asked for last frame, on top of what the platform says.
        let overrides = THEME_OVERRIDES.with(|o| o.borrow().get(&win).cloned());
        if let Some(overrides) = overrides {
            for (field, colour) in overrides {
                let rgba: gpui::Rgba = colour.into();
                match field.as_str() {
                    "text" => colors.text = rgba,
                    "selected_text" => colors.selected_text = rgba,
                    "background" => colors.background = rgba,
                    "disabled" => colors.disabled = rgba,
                    "selected" => colors.selected = rgba,
                    "border" => colors.border = rgba,
                    "separator" => colors.separator = rgba,
                    "container" => colors.container = rgba,
                    _ => {}
                }
            }
        }
        Some(colors)
    } else {
        None
    };
    if let Some(colors) = colors {
        COLORS.with(|c| {
            c.borrow_mut().insert(win, named_colors(&colors));
        });
        // The same colours again as the style language's vocabulary, so `theme:<name>`
        // resolves against this window's frame and not against a second copy of the
        // platform defaults.
        style::set_colour_table(win, colour_table(&colors));
    }
    // What this window shapes text with: its own text system, and the font and
    // size its inherited text style resolves to — so a measurement is of the
    // text as this window would draw it, `font=` and all.
    {
        let text_style = window.text_style();
        let metrics = TextMetrics {
            system: window.text_system().clone(),
            font: text_style.font(),
            size: text_style.font_size.to_pixels(window.rem_size()),
            line: window.line_height(),
            rem: window.rem_size(),
            line_length: text_style.line_height,
            appearance: appearance_of(window.appearance()),
            clipboard: read_clipboard(cx),
        };
        TEXT.with(|t| {
            t.borrow_mut().insert(win, metrics);
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
    // What only the platform can answer about this window: how big a pixel is,
    // whether it is zoomed or fullscreen, and which display it landed on. Read
    // here because a binding function has no `Window` to ask.
    crate::options::observe(
        win,
        window.scale_factor() as f64,
        window.is_maximized(),
        window.is_fullscreen(),
        window
            .display(cx)
            .map(|display| i64::from(u32::from(display.id())))
            .unwrap_or(0),
    );
}

/// The window's rectangle, as of the start of this frame.
pub fn window_rect() -> (f64, f64, f64, f64) {
    let win = window::current();
    WINDOW_RECT.with(|r| r.borrow().get(&win).copied().unwrap_or((0.0, 0.0, 0.0, 0.0)))
}

/// The window's *content* rectangle, as of the start of this frame.
///
/// The same four numbers as [`window_rect`] on a platform with no window-manager
/// insets — which is this one — and smaller than them where a titlebar and a
/// border are drawn by the system.
pub fn inner_rect() -> (f64, f64, f64, f64) {
    let win = window::current();
    INNER_RECT.with(|r| r.borrow().get(&win).copied().unwrap_or((0.0, 0.0, 0.0, 0.0)))
}

/// Whether the window is `"windowed"`, `"maximized"` or `"fullscreen"`, and the
/// size it would go back to — zero while it is windowed.
pub fn bounds_state() -> (&'static str, f64, f64) {
    let win = window::current();
    BOUNDS_STATE.with(|b| b.borrow().get(&win).copied()).unwrap_or(("windowed", 0.0, 0.0))
}

/// Who draws the window's decoration, and the strip this program set aside for its own.
///
/// `"server"` means the platform draws a titlebar and a frame around the window;
/// `"client"` means the program does, and the platform has been told how wide the
/// strip is. Read at the start of every frame, because the answer is the platform's
/// and a window manager can change its mind — a window that becomes maximized is
/// no longer tiled — so it is a reading rather than a record of what was asked for.
pub fn decorations() -> (&'static str, f64) {
    let win = window::current();
    DECORATIONS.with(|d| d.borrow().get(&win).copied()).unwrap_or(("server", 0.0))
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

pub fn window_is_maximized() -> bool {
    let win = window::current();
    WINDOW_MAXIMIZED.with(|m| m.borrow().get(&win).copied().unwrap_or(false))
}

pub fn window_set_maximized(max: bool) {
    let win = window::current();
    WINDOW_MAXIMIZED.with(|m| m.borrow_mut().insert(win, max));
}

pub fn window_is_minimized() -> bool {
    let win = window::current();
    WINDOW_MINIMIZED.with(|m| m.borrow().get(&win).copied().unwrap_or(false))
}

pub fn window_set_minimized(min: bool) {
    let win = window::current();
    WINDOW_MINIMIZED.with(|m| m.borrow_mut().insert(win, min));
}

pub fn window_is_fullscreen() -> bool {
    let win = window::current();
    WINDOW_FULLSCREEN.with(|f| f.borrow().get(&win).copied().unwrap_or(false))
}

pub fn window_set_fullscreen(fs: bool) {
    let win = window::current();
    WINDOW_FULLSCREEN.with(|f| f.borrow_mut().insert(win, fs));
}

pub fn window_scale_factor() -> f64 {
    let win = window::current();
    WINDOW_SCALE.with(|s| s.borrow().get(&win).copied().unwrap_or(2.0))
}

pub fn window_bounds() -> (f64, f64, f64, f64) {
    let win = window::current();
    let custom = WINDOW_BOUNDS.with(|b| b.borrow().get(&win).copied());
    if let Some(rect) = custom {
        return rect;
    }
    let r = window_rect();
    if r.2 > 0.0 && r.3 > 0.0 {
        r
    } else {
        (0.0, 0.0, 800.0, 600.0)
    }
}

pub fn window_set_bounds(x: f64, y: f64, w: f64, h: f64) {
    let win = window::current();
    WINDOW_BOUNDS.with(|b| b.borrow_mut().insert(win, (x, y, w, h)));
    WINDOW_RECT.with(|r| r.borrow_mut().insert(win, (x, y, w, h)));
}

pub fn window_title() -> String {
    let win = window::current();
    WINDOW_TITLE.with(|t| t.borrow().get(&win).cloned().unwrap_or_default())
}

pub fn window_set_title(title: String) {
    let win = window::current();
    WINDOW_TITLE.with(|t| t.borrow_mut().insert(win, title));
}

pub fn window_is_active() -> bool {
    let win = window::current();
    WINDOW_ACTIVE.with(|a| a.borrow().get(&win).copied().unwrap_or(true))
}

pub fn window_is_hovered() -> bool {
    let win = window::current();
    WINDOW_HOVERED.with(|h| h.borrow().get(&win).copied().unwrap_or(true))
}

pub fn window_set_mouse_position(x: f64, y: f64) {
    let win = window::current();
    MOUSE_AT.with(|m| m.borrow_mut().insert(win, (x, y)));
}

pub fn all_displays() -> Vec<Display> {
    let list = displays();
    if !list.is_empty() {
        list
    } else {
        vec![Display {
            id: 1,
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        }]
    }
}

pub fn display_bounds(display_id: i64) -> (f64, f64, f64, f64) {
    for d in all_displays() {
        if d.id == display_id {
            return (d.x, d.y, d.w, d.h);
        }
    }
    (0.0, 0.0, 1920.0, 1080.0)
}

pub fn display_color_space(_display_id: i64) -> String {
    "Display P3".to_string()
}

pub fn bounds_contains(bx: f64, by: f64, bw: f64, bh: f64, px: f64, py: f64) -> bool {
    px >= bx && px <= bx + bw && py >= by && py <= by + bh
}

pub fn bounds_center(x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
    (x + w / 2.0, y + h / 2.0)
}

pub fn register_action(name: String, stroke: String) {
    ACTION_REGISTRY.with(|r| {
        r.borrow_mut().insert(stroke, name);
    });
}

pub fn resolve_action(stroke: &str) -> String {
    ACTION_REGISTRY.with(|r| {
        r.borrow().get(stroke).cloned().unwrap_or_default()
    })
}

pub fn dispatch_action(action: &str) -> bool {
    let known = ACTION_REGISTRY.with(|r| {
        r.borrow().values().any(|v| v == action)
    });
    if known {
        let win = window::current();
        crate::store::push_event_with_in(win, "action", action.to_string());
        true
    } else {
        false
    }
}

/// Whether any element that declared `group=<name>` is under the pointer right now.
///
/// The members are the boxes [`record_box`] wrote a `group.<name>.<id>` key for, and the pointer
/// is this frame's own reading of it ([`mouse_position`], taken from the window at the start of
/// the frame). So this answers about the frame being built rather than about the last event,
/// which is what a program asking "is the card under the pointer" wants: it is the same question
/// gpui's `group_hover` styles answer while painting.
///
/// **A member with no rectangle is not a member anything can answer about.** `group=` asks for
/// the rectangle the way `bounds` does (`style::wants_bounds`), and the element must carry an
/// `id=` for its key to be its own — a second member without one overwrites the first.
///
/// The reading is a rectangle test and not gpui's own hover test. The difference is z-order: a
/// member covered by something drawn over it still answers `true` here, where `hitbox.is_hovered`
/// would answer `false`. Nothing upstream asks the question that way, and a group is a box on
/// screen either way, but it is not the same function.
pub fn group_hovered(name: &str) -> bool {
    let win = window::current();
    let (x, y) = mouse_position();
    let point = gpui::point(px(x as f32), px(y as f32));
    let prefix = format!("group.{name}.");
    MOUSE_BOUNDS.with(|m| {
        m.borrow()
            .iter()
            .any(|((w, key), bounds)| *w == win && key.starts_with(&prefix) && bounds.contains(&point))
    })
}

/// Whether a registered focus handle is the focused one this frame.
/// The id of the element that currently holds focus, or `""` when none does.
///
/// **One id, because a program can act on one.** More than one handle can report focus at once — a
/// focused element inside a focused container — and a set has no order, so this answers the
/// lexicographically first and says so rather than pretending the choice is meaningful. A program that
/// needs to know about a *particular* element asks [`is_focused`].
///
/// It is also the instrument for a question this crate could not answer about itself: `MR5` changed how the
/// `focusable` token builds its handle, and `gpui::focus_next` then appeared to focus nothing at all.
/// `is_focused` can only be asked about ids a program already knows, so it could not say whether the tab
/// order was empty or whether focus had arrived somewhere `is_focused` failed to report. This can.
/// The focus handle for `id` in `win`, if the element ever asked for one.
///
/// [`focus_for`] creates one; this only looks. It is what lets a request focus an element by name without
/// inventing a handle for an element that is not drawn.
pub fn handle_for(win: i64, id: &str) -> Option<FocusHandle> {
    FOCUS.with(|f| f.borrow().get(&(win, id.to_string())).cloned())
}

pub fn focused() -> String {
    let win = window::current();
    FOCUSED_NOW.with(|set| {
        let set = set.borrow();
        let Some(focused) = set.get(&win) else {
            return String::new();
        };
        let mut ids: Vec<&String> = focused.iter().collect();
        ids.sort();
        ids.first().map(|id| id.to_string()).unwrap_or_default()
    })
}

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

pub fn anchored(corner: i64, x: f64, y: f64, offset_x: f64, offset_y: f64, style: &str) -> i64 {
    push(
        Kind::Anchored {
            corner: corner as usize,
            x,
            y,
            offset_x,
            offset_y,
        },
        style.to_string(),
        String::new(),
    )
}

pub fn deferred(priority: i64, style: &str) -> i64 {
    push(
        Kind::Deferred {
            priority: priority as usize,
        },
        style.to_string(),
        String::new(),
    )
}

/// Appends style tokens to an element in the arena.
pub fn append_style(handle: i64, token: &str) -> i64 {
    arena(|a| {
        let last = a.els.len().saturating_sub(1);
        let target = usize::try_from(handle)
            .ok()
            .filter(|p| *p > 0 && *p <= last)
            .unwrap_or(last);
        if let Some(el) = a.els.get_mut(target) {
            if el.style.is_empty() {
                el.style = token.to_string();
            } else {
                el.style.push(' ');
                el.style.push_str(token);
            }
        }
    });
    handle
}

/// Reads the accumulated style tokens from an element in the arena.
pub fn get_style(handle: i64) -> String {
    arena(|a| {
        let index = usize::try_from(handle).ok().unwrap_or(0);
        a.els.get(index).map(|el| el.style.clone()).unwrap_or_default()
    })
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
///
/// A row builder runs *during layout*, after the frame that described the list
/// has been built, and it appends to that same arena. Its elements are needed
/// only until the row is a real GPUI element, so the arena is handed back
/// straight afterwards rather than growing with every row of every scroll.
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

/// Notes a problem for the person to see, on screen and — if asked — in the log.
///
/// The arena's list is what the banner draws from, and it is consumed by the frame that noted it. A check
/// that only looks at stdout therefore cannot see any of this, which is how an example that drew nothing
/// passed every silence check for days. So a copy is kept per window for the program to ask about, and
/// printed to stderr when `GOSSAMER_NOTED_ERRORS=1` is in the environment — opt-in, so an example run for
/// its own sake stays silent.
pub fn note_error(message: impl Into<String>) {
    let message = message.into();
    if std::env::var_os("GOSSAMER_NOTED_ERRORS").is_some() {
        eprintln!("gpui: noted: {message}");
    }
    let win = window::current();
    NOTED.with(|n| {
        let mut all = n.borrow_mut();
        let kept = all.entry(win).or_default();
        // Capped: a program that never asks must not grow this without bound. A row builder that fails
        // fails for every row, so sixty-four is far more than enough to see what is wrong.
        if kept.len() < 64 {
            kept.push(message.clone());
        }
    });
    arena(|a| a.errors.push(message));
}

/// What has been noted for the current window and not yet asked about.
///
/// Non-destructive and capped at 64 entries. This is the program's view of the banner: the same messages,
/// for a program that wants to react to them rather than a person who wants to read them.
pub fn noted_errors() -> Vec<String> {
    let win = window::current();
    NOTED.with(|n| n.borrow().get(&win).cloned().unwrap_or_default())
}

/// Forgets everything noted for the current window.
pub fn clear_noted_errors() {
    let win = window::current();
    NOTED.with(|n| {
        n.borrow_mut().remove(&win);
    });
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
///
/// While a tooltip is being drawn there is no arena to read: the element comes from the tooltip's
/// own copy of its subtree — see `CAPTURED`.
fn snapshot(handle: i64) -> Option<El> {
    let index = usize::try_from(handle).ok()?;
    let captured = CAPTURED.with(|c| c.borrow().last().cloned());
    if let Some(els) = captured {
        return els.get(index).cloned();
    }
    arena(|arena| {
        let el = arena.els.get(index)?;
        // `Kind` is `Clone`, so the copy is the derive's, not a match with an arm
        // per variant — the shape that quietly stops compiling the day a variant is
        // added.
        Some(El {
            kind: el.kind.clone(),
            style: el.style.clone(),
            id: el.id.clone(),
            children: el.children.clone(),
        })
    })
}

thread_local! {
    /// The subtree a tooltip's view is drawing from, while it is drawing it.
    ///
    /// A tooltip is the one thing gpui draws that the frame did not hand it. Its builder runs on
    /// gpui's own half-second clock, in a task (`elements/div.rs:2751`), and the view it answers
    /// is laid out inside a *later* frame's `Window::draw` (`window.rs:2088`) — so by the time a
    /// child handle inside it is read, the arena that described it has been reset and refilled,
    /// and the handle names whatever happens to sit at that position in the newer frame. In the
    /// frame the program quits on it names nothing at all: `release_retained` has already dropped
    /// the arena, and the tooltip draws once as an empty box. `capture` copies the subtree out at
    /// build time, and `snapshot` reads that copy while the tooltip is drawn.
    static CAPTURED: RefCell<Vec<std::rc::Rc<Vec<El>>>> = const { RefCell::new(Vec::new()) };
}

/// Copies a whole subtree out of this frame's arena, so it can be drawn in a later one.
///
/// The copy has to stand on its own: every handle inside it is rewritten to name a position in
/// the copy rather than in the arena it came from, whether it is a child's, an image's `loading`
/// and `fallback` subtrees, or a `mouse` wrapper's element.
fn capture(el: &El) -> std::rc::Rc<Vec<El>> {
    let mut els: Vec<El> = vec![el.clone()];
    let mut seen: HashMap<usize, usize> = HashMap::new();
    let kids = el
        .children
        .clone()
        .into_iter()
        .filter_map(|h| copy(h, &mut els, &mut seen))
        .collect();
    els[0].children = kids;
    std::rc::Rc::new(els)
}

/// Copies `handle` and everything under it into `els`, answering where it landed.
///
/// `seen` is what makes the copy a copy of the *thing* and not of the walk: an element given to
/// two parents is copied once, so a subtree cannot be duplicated, and the walk cannot double back
/// on itself.
fn copy(handle: usize, els: &mut Vec<El>, seen: &mut HashMap<usize, usize>) -> Option<usize> {
    if let Some(&at) = seen.get(&handle) {
        return Some(at);
    }
    let el = snapshot(handle as i64)?;
    let at = els.len();
    seen.insert(handle, at);
    els.push(el);

    let kids: Vec<usize> = els[at]
        .children
        .clone()
        .into_iter()
        .filter_map(|h| copy(h, els, seen))
        .collect();
    els[at].children = kids;

    // Two variants hold a handle of their own, outside `children`: an image's `loading` and
    // `fallback` subtrees, which it draws *instead of* the picture, and a `mouse` wrapper's
    // element, which it hands straight to gpui. A handle that names nothing becomes `0`, which
    // both of them already read as "none".
    let image = match &els[at].kind {
        Kind::Image { path, loading, fallback } => Some((path.clone(), *loading, *fallback)),
        _ => None,
    };
    if let Some((path, loading, fallback)) = image {
        let loading = remap(loading, els, seen);
        let fallback = remap(fallback, els, seen);
        els[at].kind = Kind::Image { path, loading, fallback };
    }
    let mouse = match &els[at].kind {
        Kind::Mouse { child, id, kinds, window_kinds } => {
            Some((*child as i64, id.clone(), kinds.clone(), window_kinds.clone()))
        }
        _ => None,
    };
    if let Some((child, id, kinds, window_kinds)) = mouse {
        let child = remap(child, els, seen).max(0) as usize;
        els[at].kind = Kind::Mouse { child, id, kinds, window_kinds };
    }
    Some(at)
}

/// Where `handle` lands in the copy, or `0` when it names no element at all.
fn remap(handle: i64, els: &mut Vec<El>, seen: &mut HashMap<usize, usize>) -> i64 {
    if handle <= 0 {
        return 0;
    }
    copy(handle as usize, els, seen).map_or(0, |at| at as i64)
}

/// Turns the frame the view callback described into GPUI elements.
pub fn finish(root: Option<i64>, window: &mut Window, cx: &mut App, entity: EntityId) -> AnyElement {
    let errors = arena(|a| std::mem::take(&mut a.errors));

    // A clean frame is handed straight to GPUI: the element the program
    // described *is* the window's root, with nothing wrapped around it. The
    // column only appears when there is something to report.
    if errors.is_empty() {
        let element = match root.and_then(snapshot) {
            Some(el) => {
                // The root's `theme.` tokens are the window's, and they are remembered
                // for the frame after this one — see `THEME_OVERRIDES`.
                let overrides = style::theme_overrides(&el.style);
                let win = window::current();
                THEME_OVERRIDES.with(|o| {
                    o.borrow_mut().insert(win, overrides);
                });
                render(&el, window, cx, entity)
            }
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

thread_local! {
    /// The windows whose `gpui::on_keys(true)` the root has already taken focus
    /// for. The flag is set by the program and read every frame, so "the frame it
    /// turned on" is not a frame number — it is the transition from not being in
    /// this set to being in it. `on_keys(false)` takes the window back out, so a
    /// program that wants the keyboard again can ask again.
    static KEYS_ARMED: RefCell<std::collections::HashSet<i64>> =
        RefCell::new(std::collections::HashSet::new());
}

/// Disarms the window's keyboard, so that enabling it again takes focus again.
pub fn disarm_keys(win: i64) {
    KEYS_ARMED.with(|a| {
        a.borrow_mut().remove(&win);
    });
}

/// Records one keystroke's fields for the program, under `prefix` — `~key` for a
/// press and `~keyup` for a release — and answers the stroke an event payload
/// should carry.
///
/// The point of the prefix pair is that a program can tell a press from a release
/// and can tell one keystroke from another: `unparse` is gpui's own spelling of
/// the whole stroke (`"shift-tab"`, `"cmd-c"`, `"a"`), while `key` is only the
/// character printed on the key — and gpui maps **both** Tab and Shift-Tab to the
/// key `"tab"`, so a program reading `key` alone cannot tell them apart. The five
/// modifier flags are the state that distinguishes them, and `char` is what would
/// have been typed (`"A"` under shift, nothing under cmd).
fn record_keystroke(prefix: &str, keystroke: &gpui::Keystroke) -> String {
    let unparsed = keystroke.unparse();
    store::set_text(&format!("{prefix}.unparse"), &unparsed);
    store::set_text(&format!("{prefix}.key"), &keystroke.key);
    store::set_text(
        &format!("{prefix}.char"),
        keystroke.key_char.as_deref().unwrap_or(""),
    );
    let m = keystroke.modifiers;
    for (name, on) in [
        ("shift", m.shift),
        ("control", m.control),
        ("alt", m.alt),
        ("platform", m.platform),
        ("function", m.function),
    ] {
        store::set_flag(&format!("{prefix}.{name}"), on);
    }
    // The modifier half of `unparsed`, in gpui's own order and spelling, joined
    // with `+` — `"shift"`, `"ctrl-alt"`, `""` when none is held. The flags above
    // are the same information one bit at a time; this is the one glance.
    store::set_text(&format!("{prefix}.mods"), &modifier_words(&m));
    unparsed
}

/// The held modifiers as one word, in `gpui::Keystroke::unparse`'s order: the
/// function key, control, alt, the platform key, then shift.
fn modifier_words(m: &gpui::Modifiers) -> String {
    let mut words: Vec<&str> = Vec::new();
    if m.function {
        words.push("fn");
    }
    if m.control {
        words.push("ctrl");
    }
    if m.alt {
        words.push("alt");
    }
    if m.platform {
        words.push("platform");
    }
    if m.shift {
        words.push("shift");
    }
    words.join("+")
}

/// After `gpui::on_keys(true)`, the window itself listens for keystrokes and
/// raises `"key|<name>"`, so a program can use the keyboard without owning a
/// GPUI element.
///
/// The root takes focus **once, the frame the program enables the keyboard** —
/// not only in the window's first two frames: a program that turns the keyboard on at
/// frame 40 would otherwise get the listener and no focus, and focus is what a key
/// event needs to be delivered at all, so its shortcuts would be silently dead. A click into a text field still moves focus there and keeps
/// it: this runs on the frame the flag turns on, or when nothing at all is focused,
/// and never steals focus back afterwards.
///
/// The listener is registered in the **capture** phase, and that is the whole of
/// KM5: gpui dispatches a key to the path from the root down to the focused element
/// and then back up (`window.rs:3878-3906`), and `cx.stop_propagation()` in an
/// element's handler ends the *bubble* half — taking the window's
/// `cx.observe_keystrokes` observers with it (`window.rs:3861-3869`). The text editor
/// stops propagation for every key it handles, so those keys never reached a program
/// that was watching. A capture listener on the root runs before any of that and
/// cannot be stopped by a handler below it.
fn listen_for_keys(element: AnyElement, window: &mut Window, cx: &mut App) -> AnyElement {
    let win = window::current();
    let keys = store::get_flag("~keys", false);
    let observers = key_observers(win);
    if !keys && observers.is_empty() {
        return element;
    }

    let handle = focus_for(win, "~root", cx);
    if keys {
        let armed = KEYS_ARMED.with(|a| a.borrow().contains(&win));
        if !handle.is_focused(window) && (!armed || window.focused(cx).is_none()) {
            window.focus(&handle);
        }
        KEYS_ARMED.with(|a| {
            a.borrow_mut().insert(win);
        });
    }

    div()
        .size_full()
        .id(ElementId::Name("~root".into()))
        .track_focus(&handle)
        .capture_key_down(move |event: &KeyDownEvent, window, _cx| {
            window.refresh();
            window::with(win, || {
                let unparsed = record_keystroke("~key", &event.keystroke);
                store::set_flag("~key.held", event.is_held);
                if keys {
                    // The whole stroke, not the key's name: `"shift-tab"` and
                    // `"tab"` are the same `key` in gpui, so reporting `key` here
                    // left a program unable to tell the two apart.
                    store::push_event_with("key", &unparsed);
                }
                // The program's own observers, whether or not an element handled
                // the key this frame.
                for id in &observers {
                    store::push_event_with(id, &unparsed);
                }
            });
        })
        // The release, on the same element and under the same flag: a program
        // driving movement from the keyboard needs to know when the key stopped
        // being held, and this is the only thing that listens for it.
        .capture_key_up(move |event: &KeyUpEvent, window, _cx| {
            window.refresh();
            window::with(win, || {
                let unparsed = record_keystroke("~keyup", &event.keystroke);
                if keys {
                    store::push_event_with("keyup", &unparsed);
                }
            });
        })
        .child(element)
        .into_any_element()
}

thread_local! {
    /// The keystroke observers a program has asked for, per window: the ids
    /// [`add_key_observer`] recorded, in the order they were asked for.
    static KEY_OBSERVERS: RefCell<HashMap<i64, Vec<String>>> = RefCell::new(HashMap::new());
}

/// Records that window `win` should raise `id` for every keystroke it receives.
///
/// Asked for once; asking twice with the same id is ignored, because the listener
/// this feeds is rebuilt every frame and would otherwise raise the event twice per
/// frame it was asked for again.
pub fn add_key_observer(win: i64, id: String) {
    KEY_OBSERVERS.with(|o| {
        let mut observers = o.borrow_mut();
        let list = observers.entry(win).or_default();
        if !list.contains(&id) {
            list.push(id);
        }
    });
}

/// The keystroke observer ids recorded for `win`.
fn key_observers(win: i64) -> Vec<String> {
    KEY_OBSERVERS.with(|o| o.borrow().get(&win).cloned().unwrap_or_default())
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
    /// The last point the pointer was seen at for an element, in the window's space, with the
    /// modifiers that were held then.
    ///
    /// Kept for one caller: [`gpui::on_hover`]'s transition, which reports that the pointer
    /// *left* an element and is handed no event to say from where. `elements/div.rs:2270` fires
    /// it from a comparison of two booleans on a mouse move, so the point it should report is
    /// the one the pointer was last at — which is what this holds. Nothing else reads it.
    static ELEMENT_AT: RefCell<HashMap<(i64, String), (f64, f64, gpui::Modifiers)>> =
        RefCell::new(HashMap::new());
    /// Whether an element carrying the `over` kind had the pointer inside its rectangle at the
    /// last move of the drag in flight.
    ///
    /// `over` is a **transition**, like the element-level `exit`, and gpui hands the listener only
    /// the present: `DragMoveEvent` says where the pointer is and what this element's rectangle is
    /// (`elements/div.rs:62-70`) and nothing about the move before it. So the previous answer has
    /// to be kept somewhere, and it is kept per `(window, element)` for the same reason
    /// `MOUSE_BOUNDS` is: a drag belongs to one window, and the same id in another window is a
    /// different element.
    ///
    /// Cleared when the drop listener fires, so that the next drag over the same element reports
    /// its entry again. Without that, an element a drag ended on would answer "already inside"
    /// forever, because the moves that carried the pointer away from it happened before the flag
    /// was ever read again.
    static DRAG_OVER: RefCell<HashMap<(i64, String), bool>> = RefCell::new(HashMap::new());
}

/// Record one element's own rectangle, and the same rectangle a second time under the group it
/// declared.
///
/// The map is keyed by `(window, key)`, and a group member's second key is
/// `group.<name>.<id>` — so one box is reachable as itself, through [`element_bounds`], and as a
/// member of its group, through [`group_hovered`], and the two can never disagree about where the
/// box was. This is the only writer of `MOUSE_BOUNDS`.
fn record_box(win: i64, key: &str, group: Option<&str>, bounds: Bounds<Pixels>) {
    MOUSE_BOUNDS.with(|m| {
        let mut m = m.borrow_mut();
        m.insert((win, key.to_string()), bounds);
        if let Some(group) = group {
            m.insert((win, format!("group.{group}.{key}")), bounds);
        }
    });
}

/// The strip that stands in for a div's own box: out of flow, unpainted, the full size of its
/// parent.
///
/// gpui hands a div the bounds of its children and never its own
/// (`Div::on_children_prepainted`, `gpui-0.2.2/src/elements/div.rs:1243`), so the box is read from
/// a child that is `absolute` — out of flow, so it moves nothing — `size_full()`, so it is
/// exactly the parent's rectangle, and has no hitbox. Being **first** is what makes
/// `bounds.first()` it. This backs the pointer rows; `bounds` and `group=` reuse it
/// verbatim, which is why the three agree about where an element is.
fn bounds_strip() -> AnyElement {
    div()
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .size_full()
        .into_any_element()
}

/// Put [`bounds_strip`] and the listener that reads it onto a **plain** `Div`.
///
/// `on_children_prepainted` is an inherent method of the stateless `Div` and of nothing else, so
/// this has to run before the element is given an id — which is also the only order in which it
/// can run. `Stateful<Div>` is a newtype over a private field with no `Deref`
/// (`elements/div.rs:2903`), so an element that is already interactive can never be given this,
/// and that is why an interactive box that wants its rectangle is written as "strip and listener
/// first, id after".
fn with_bounds_strip(box_: Div, win: i64, key: String, group: Option<String>) -> Div {
    box_
        .child(bounds_strip())
        .on_children_prepainted(move |bounds, _window, _cx| {
            if let Some(first) = bounds.first() {
                record_box(win, &key, group.as_deref(), *first);
            }
        })
}

/// Give `box_` a way to answer for **its own** box, and put `children` back inside it.
///
/// The `tooltip` argument is the third thing a box can want at once. It is applied **here** and
/// not at the call site because `on_children_prepainted` belongs to the stateless `Div` and
/// `tooltip` to its *stateful* half, and there is no way back from `Stateful<Div>` to `Div`
/// (`elements/div.rs:2903` is a newtype over a private field, with no `Deref`). So the id that
/// `tooltip` needs is given last, after the prepaint listener is already on. A box that wants no
/// tooltip keeps the stateless element it has always been.
fn report_bounds(
    box_: Div,
    win: i64,
    key: String,
    children: Vec<AnyElement>,
    tip: Option<std::rc::Rc<El>>,
    entity: EntityId,
    group: Option<String>,
) -> AnyElement {
    let id = key.clone();
    let stripped = with_bounds_strip(box_, win, key, group).children(children);
    with_tooltip(stripped.id(ElementId::Name(id.into())), tip, win, entity).into_any_element()
}

/// Hangs `tip` on an element that has already been given an id, if the box was given one.
///
/// `Div::tooltip` lives on a div's stateful half, so a box that wants one has to be id'd — which
/// is the same thing every interactive branch already does, and which `should_insert_hitbox`
/// (`elements/div.rs:1710`) then turns into the very hitbox whose hover raises the tooltip.
///
/// The subtree is copied **here**, while the frame that described it is still the frame: gpui
/// runs the closure below 500 ms later (`TOOLTIP_SHOW_DELAY`, `elements/div.rs:48`), off the end
/// of the frame and off the end of the arena, and the copy is the only thing that lasts that
/// long. `capture` says what it is for.
///
/// `win` and `entity` are captured here for the same reason, and they are the reason this
/// argument list is longer than it looks like it needs to be. gpui lays a tooltip out inside
/// `Window::draw`, long after `Root::render` — the one place `window::with` points the thread at
/// a window — has returned. So during a tooltip's own render, `window::current()` is `0`, and
/// every per-window thing the binding reads out of it — the `bounds` registry a child reports
/// into, the window a notification is asked of — would be another window's. `TooltipView` puts
/// the window back (`window::with`), which is what makes a `bounds` token inside a tooltip name
/// the same window as the box it is a tooltip *for*.
fn with_tooltip<E>(element: E, tip: Option<std::rc::Rc<El>>, win: i64, entity: EntityId) -> E
where
    E: gpui::StatefulInteractiveElement,
{
    match tip {
        None => element,
        Some(tip) => {
            let els = capture(&tip);
            element.tooltip(move |_window, cx| {
                cx.new(|_cx| TooltipView {
                    els: els.clone(),
                    entity,
                    win,
                })
                .into()
            })
        }
    }
}

/// The separator inside a pointer event's payload.
///
/// `\u{1}` — ASCII's start-of-heading, which gpui itself uses between the fields of an
/// unparsed keystroke. It was chosen because nothing a program can put in a payload
/// contains it: not a keystroke name, not a label, not what `gpui::emit` is handed. So a
/// payload can be cut at it without a payload that merely *looks* like a pointer event
/// being cut by mistake — which a space or a comma would do.
pub(crate) const POINTER_SEP: char = '\u{1}';

/// The three decimal places a pointer coordinate is kept to.
///
/// A `Pixels` is `f32`; printed whole, 141 becomes `141.0021514892578`. A thousandth of a
/// pixel is far below anything a person or a layout can see, and rounding here means the
/// payload a program reads and the `<id>.x` the store holds are the *same number* rather
/// than two spellings of it.
fn round3(v: f32) -> f64 {
    ((v as f64) * 1000.0).round() / 1000.0
}

/// Writes a pointer event into the state store and raises `"<id>|<payload>"`.
///
/// Coordinates are element-relative, so a program never has to know where its
/// element sits — the same thing the slider does.
///
/// **The payload carries its own copy of the position**, in *two* spaces.
///
/// The store has one slot per key, so it can only ever hold the *latest* position: when
/// several moves coalesce into one frame — which they do, this was measured, four `xdotool`
/// moves posted back to back arrived as four events in one frame — every one of the four
/// events reads back the last of them and the points in between are gone. A painter drawing
/// a stroke from those events draws one dab and a jump. The payload is
/// `"<kind>\u{1}<window x>\u{1}<window y>\u{1}<mods>"`, so each event answers for itself
/// through [`crate::pointer_xy`] and [`crate::pointer_mods`].
///
/// **The payload's position is the one in the window**, which is what upstream's
/// `MouseDownEvent::position` holds — gpui's own doc comment on that field is "The position
/// of the mouse on the window", the platform backends put the raw pointer position in it
/// (`platform/linux/x11/client.rs:1121`, `platform/mac/events.rs:151`), and gpui compares it
/// against hitbox origins, which are window coordinates
/// (`elements/div.rs:1966`). A port that paints — as `13_painting` does, through
/// `Window::paint_path`, which applies no element offset of its own
/// (`window.rs:2860`) — needs exactly that number. **The store's `<id>.x` / `<id>.y` stay
/// element-relative**, which is the convenience they have always been and what a widget
/// measuring a position inside itself wants; [`crate::pointer_xy`] documents the difference.
/// Registers `id`'s **window-level** listeners on `window` for the events named in `kinds`.
///
/// The same events the element-level handlers report, plus two the element cannot hear at all —
/// `exit`, which no element-level listener type-matches, and `scroll`, which gpui gates on
/// `should_handle_scroll` rather than on hover — through the same writer
/// ([`report_pointer`]), registered on the window instead of on the element. gpui dispatches
/// window listeners for every mouse event wherever the pointer is
/// (`gpui-0.2.2/src/window.rs:3677-3714`), where an element's own listener is gated on the
/// element's hitbox being hovered (`elements/div.rs:128`, `:147`) — which is what keeps a
/// drag alive once the pointer has left the element it began on.
///
/// **Only legal from a paint callback.** `on_mouse_event` opens with `debug_assert_paint`
/// (`window.rs:3425`), and the view's `Render::render` runs in the *prepaint* phase; calling
/// it from there panics with `this method can only be called during paint`. The caller is
/// the paint closure of a `canvas` the mouse element carries for exactly this.
///
/// `kinds` is read as **words** — `"down up move"` names the same three as `"down,up,move"` —
/// because a program writes it the way `gpui::on_mouse`'s `kinds` is written, and that one is
/// read with a `contains`. A `split(',')` alone made `"down up move"` one word, matched none
/// of the three and registered nothing at all.
fn register_window_mouse(
    window: &mut Window,
    cx: &mut App,
    entity: EntityId,
    win: i64,
    id: &str,
    kinds: &str,
) {
    // The kind words are read the same way the element-level half reads them, including the `!`
    // suffix that asks for the event to stop where it landed — see [`render`]'s mouse arm for what
    // a stop means and why it is on the kind. Here the stop lands among the window's own
    // listeners, which gpui dispatches in the bubble pass over `Window::mouse_listeners`
    // (`window.rs:3695-3714`), so it withholds the event from the window listeners registered
    // **before** this one and does not touch an element's listener at all. A `!` that did not also
    // register the plain kind would be the worst of both — the modifier would silently disable
    // the listener — so the bang is stripped for the match and read again for the stop.
    let wants = |want: &str| {
        kinds
            .split(|c: char| c == ',' || c.is_whitespace())
            .any(|word| word.trim_end_matches('!') == want)
    };
    let consumes = |kind: &str| {
        let wanted = format!("{kind}!");
        kinds
            .split(|c: char| c == ',' || c.is_whitespace())
            .any(|word| word == wanted)
    };

    if wants("down") {
        let key = id.to_string();
        let stops = consumes("down");
        window.on_mouse_event(move |event: &MouseDownEvent, phase, _w, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                return;
            }
            window::with(win, || {
                report_pointer(&key, "down", event.position, event.modifiers, Some(event.click_count))
            });
            if stops {
                cx.stop_propagation();
            }
            cx.notify(entity);
        });
    }
    if wants("up") {
        let key = id.to_string();
        let stops = consumes("up");
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _w, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                return;
            }
            window::with(win, || {
                report_pointer(&key, "up", event.position, event.modifiers, Some(event.click_count))
            });
            if stops {
                cx.stop_propagation();
            }
            cx.notify(entity);
        });
    }
    if wants("move") {
        let key = id.to_string();
        let stops = consumes("move");
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _w, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            window::with(win, || {
                report_pointer(&key, "move", event.position, event.modifiers, None)
            });
            if stops {
                cx.stop_propagation();
            }
            cx.notify(entity);
        });
    }
    // The window's own exit: the pointer has left the window altogether.
    //
    // This is a different thing from the element-level `exit` and the difference is worth the two
    // registrations. `MouseExitEvent` is generated by the platform when the pointer leaves the
    // *window* (`x11/client.rs:1233`), it is dispatched to window listeners directly, and it
    // carries its own position — so it arrives whether or not the pointer moves again, which the
    // element-level one does not. It also never reaches an element listener at all: those are
    // registered with a concrete event type and this is not one they name.
    if wants("exit") {
        let key = id.to_string();
        let stops = consumes("exit");
        window.on_mouse_event(move |event: &MouseExitEvent, phase, _w, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            window::with(win, || {
                report_pointer(&key, "exit", event.position, event.modifiers, None)
            });
            if stops {
                cx.stop_propagation();
            }
            cx.notify(entity);
        });
    }
    // The wheel, where the pointer is rather than where this element is: `down`, `up` and `move`
    // are gated on the element by nothing at all here, and neither is this.
    if wants("scroll") {
        let key = id.to_string();
        let stops = consumes("scroll");
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _w, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            window::with(win, || report_scroll(&key, event));
            if stops {
                cx.stop_propagation();
            }
            cx.notify(entity);
        });
    }
}

fn report_pointer(
    id: &str,
    kind: &str,
    position: gpui::Point<Pixels>,
    modifiers: gpui::Modifiers,
    count: Option<usize>,
) {
    report_pointer_at(
        id,
        kind,
        round3(f32::from(position.x)),
        round3(f32::from(position.y)),
        modifiers,
        count,
    );
}

/// [`report_pointer`] with the point already in the window's own numbers.
///
/// Split out for the one reporter that has no event of its own to read a point from: an `exit`
/// arrives as a boolean ([`report_exit`]) and reports from the point the pointer was last seen
/// at, which is a pair of `f64`s already — there is no `Pixels` to convert, and putting one back
/// together only to take it apart again would round the number twice.
fn report_pointer_at(
    id: &str,
    kind: &str,
    wx: f64,
    wy: f64,
    modifiers: gpui::Modifiers,
    count: Option<usize>,
) {
    let win = window::current();
    ELEMENT_AT.with(|m| {
        m.borrow_mut()
            .insert((win, id.to_string()), (wx, wy, modifiers))
    });
    let bounds = MOUSE_BOUNDS.with(|m| m.borrow().get(&(win, id.to_string())).copied());
    let (x, y) = match bounds {
        Some(bounds) => (
            round3((wx as f32) - f32::from(bounds.origin.x)),
            round3((wy as f32) - f32::from(bounds.origin.y)),
        ),
        None => (wx, wy),
    };
    store::set_float(&format!("{id}.x"), x);
    store::set_float(&format!("{id}.y"), y);
    // The click count the *platform* kept, on the events that carry one. `None` leaves the slot
    // alone rather than writing a zero: a move between two clicks of a double click must not
    // reset the count the second click reports, and a program that reads `<id>.count` after a
    // move wants the count of the last click, not of the move that followed it.
    if let Some(count) = count {
        store::set_int(&format!("{id}.count"), count as i64);
    }
    store::set_flag(&format!("{id}.dragging"), kind == "move");
    store::set_flag(&format!("{id}.shift"), modifiers.shift);
    store::set_flag(&format!("{id}.control"), modifiers.control);
    store::set_flag(&format!("{id}.alt"), modifiers.alt);
    store::set_flag(&format!("{id}.platform"), modifiers.platform);
    let mods = modifier_words(&modifiers);
    store::push_event_with(
        id,
        &format!("{kind}{POINTER_SEP}{wx}{POINTER_SEP}{wy}{POINTER_SEP}{mods}"),
    );
}

/// The platform's own click counter for a click event, or `None` for a click that had none.
///
/// `ClickEvent` is either a mouse click or a keyboard one — Enter or Space on a focused element
/// (`elements/div.rs:2179-2210`) — and only the mouse half has a counter to report. gpui keeps
/// that counter in the platform backend and hands it in `MouseDownEvent::click_count`, which is
/// what makes a second press a double click; a program cannot compute it, because the interval
/// is the platform's (`x11/client.rs:1103`, `DOUBLE_CLICK_INTERVAL`).
fn click_count(event: &ClickEvent) -> Option<usize> {
    match event {
        ClickEvent::Mouse(mouse) => Some(mouse.up.click_count),
        ClickEvent::Keyboard(_) => None,
    }
}

/// Writes a scroll wheel event the way `report_pointer` writes the rest: the point and the
/// modifiers through the same writer, and the *delta* — which no other pointer event has — into
/// `<id>.dx` / `<id>.dy`.
///
/// The delta is **not** in the payload, and that is deliberate. `pointer_fields` reads a payload
/// of exactly four `\u{1}`-separated fields and answers `None` for anything else, so a program's
/// scroll handling is the same code as its click handling and only the two store slots differ.
///
/// gpui spells a delta two ways, and they are **not the same unit**:
/// `ScrollDelta::Pixels` is a precise device's distance in pixels, `ScrollDelta::Lines` is a
/// notched wheel's count of lines (`interactive.rs:387-392`). Both would land in one slot, and
/// a program reading `8.0` could not tell eight pixels from eight lines. So the unit is written
/// beside the number: `<id>.scroll_lines` is `true` when the two slots hold lines. A program
/// that wants pixels and has lines multiplies by whatever its own line height is, which is a
/// decision only the program can make.
fn report_scroll(id: &str, event: &ScrollWheelEvent) {
    let (dx, dy, lines) = match event.delta {
        ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y), false),
        ScrollDelta::Lines(delta) => (delta.x, delta.y, true),
    };
    store::set_float(&format!("{id}.dx"), round3(dx));
    store::set_float(&format!("{id}.dy"), round3(dy));
    store::set_flag(&format!("{id}.scroll_lines"), lines);
    report_pointer(id, "scroll", event.position, event.modifiers, None);
}

/// Raise `"<id>|exit"`: the pointer is no longer over this element.
///
/// Reported against the point the pointer was last seen at, which is the point it left from.
/// There is nothing else to report it from: gpui's element-level exit is a **boolean**, handed to
/// the hover listener on a *mouse move* once the hit test no longer covers the element
/// (`elements/div.rs:2270`, `if is_hovered != *was_hovered`), and it carries no event. The
/// window-level exit is a different thing and does carry one — see `register_window_mouse`,
/// whose `exit` kind reports `MouseExitEvent`'s own position.
///
/// **This is why an element-level `exit` needs a mouse move to arrive.** A pointer that simply
/// stops moving, or one that leaves the window altogether, produces no further `MouseMoveEvent`
/// and so no element-level exit at all; the window's own `exit` is what covers that case, and it
/// arrives directly. A program that wants to know the pointer has gone should listen for both.
///
/// An element the pointer was never seen over answers no point — `pointer_xy` on this event gives
/// `(-1.0, -1.0)`, the same "there is no position here" the reader gives anything that is not a
/// pointer event. That can only happen for an element that listens for `exit` and for none of the
/// events that would have recorded where the pointer was.
fn report_exit(id: &str) {
    let win = window::current();
    let last = ELEMENT_AT
        .with(|m| m.borrow().get(&(win, id.to_string())).copied());
    let (wx, wy, modifiers) = last.unwrap_or((-1.0, -1.0, gpui::Modifiers::default()));
    report_pointer_at(id, "exit", wx, wy, modifiers, None);
}

/// Raise one of the four drag states: `"<id>|drag"` when this element's drag is armed,
/// `"<id>|over"` and `"<id>|out"` as a drag in flight crosses this element's rectangle, and
/// `"<id>|drop"` when the release lands inside it.
///
/// Four names and not one with a phase beside it, because they answer two different questions and
/// are heard by two different ends of the gesture. `drag` is the **source** saying it has been
/// picked up; `over`, `out` and `drop` are a **target** saying what a drag in flight is doing to it.
/// An element that is both hears all four, and the `kinds` string is what decides.
///
/// The payload — the value the program gave the drag when it started — is written to
/// **`<id>.drag_value`** on every one of them, so a target knows *what* is over it and not only
/// that something is. It is written with `set_text_quiet` and not `set_text`: this is a slot a
/// drag fills and empties several times a gesture, and routing it through the undo history would
/// put a drag in the middle of a program's edit history.
///
/// `over` is the only one that also writes a flag, **`<id>.over`** — `true` while the pointer is
/// inside the element with a drag in flight and `false` once it has left. It is a flag and not a
/// fifth event because it is the *state* the row is about: a drop target that paints itself
/// differently while a payload is over it reads `over` once a frame, and a program that wants the
/// two transitions instead reads the two events.
fn report_drag(id: &str, kind: &str, position: gpui::Point<Pixels>, modifiers: gpui::Modifiers, value: &str) {
    if kind == "over" || kind == "out" {
        store::set_flag(&format!("{id}.over"), kind == "over");
    }
    if !value.is_empty() {
        store::set_text_quiet(&format!("{id}.drag_value"), value);
    }
    report_pointer(id, kind, position, modifiers, None);
}

/// Raise `"<id>|drop"` and clear this element's `over` state.
///
/// Separate from [`report_drag`] for the two things a drop does not have. It is handed **no
/// event** — `Interactivity::on_drop`'s listener takes the payload, a window and an app
/// (`elements/div.rs:462`) — so its point comes from `Window::mouse_position()`, which is where the
/// pointer is at that moment, and its modifiers from the last move this element was reported at,
/// since a release carries none of its own. And it clears `DRAG_OVER`: a drag that ended on an
/// element must not leave it answering "already inside" to the next one.
///
/// **The point is passed in and not read here**, because the caller is the one holding the
/// captured window: `window::current()` is `0` inside a listener gpui calls after the frame that
/// registered it, so a reporter that reaches for it there writes into no window at all. See
/// `Root::render`.
fn report_drop(id: &str, position: gpui::Point<Pixels>, value: &str) {
    let win = window::current();
    let modifiers = ELEMENT_AT
        .with(|m| m.borrow().get(&(win, id.to_string())).map(|(_, _, m)| *m))
        .unwrap_or_default();
    DRAG_OVER.with(|m| m.borrow_mut().insert((win, id.to_string()), false));
    // The `over` flag goes with it: the pointer is still inside the element after a release, but
    // there is no longer a drag over it, and a target that paints itself for an incoming payload
    // must stop when the payload has landed.
    store::set_flag(&format!("{id}.over"), false);
    report_drag(id, "drop", position, modifiers, value);
}

/// The event's own position in the window and its modifiers, or `None` when it is not a
/// pointer event.
///
/// Reads the payload `report_pointer` wrote. A payload that is not four `\u{1}`-separated
/// fields — anything `gpui::emit` raised, anything with no payload at all — is not a
/// pointer event and gets `None` rather than a position invented out of whatever numbers
/// happened to be in it.
pub(crate) fn pointer_fields(event: &str) -> Option<(f64, f64, String)> {
    let (_, payload) = event.split_once('|')?;
    let mut fields = payload.split(POINTER_SEP);
    let _kind = fields.next()?;
    let x = fields.next()?.parse::<f64>().ok()?;
    let y = fields.next()?.parse::<f64>().ok()?;
    let mods = fields.next()?;
    // A fifth field means this is not a payload `report_pointer` wrote.
    if fields.next().is_some() {
        return None;
    }
    Some((x, y, mods.to_string()))
}

/// One field of an element's rectangle, in **window** coordinates, or `0.0` when there is
/// no rectangle to report.
///
/// The rectangle gpui laid the element out in during the last frame it was drawn, read
/// back from the same per-`(window, element)` map `report_pointer` measures the
/// element-relative `<id>.x` against — so a program that has one has the other, and they
/// agree.
///
/// **Only an element that carries an interaction has one.** gpui hands the binding the
/// bounds of an element's box while it is being painted, which the binding keeps for the
/// elements a pointer handler is attached to and for sliders — nothing else. An element
/// with a name and no listener reports `0.0`, and so does a name that is not in this
/// window. A caller that needs a rectangle to read back must give the element something to
/// listen with, which is exactly what upstream's `thumb_bounds.contains(&ev.position)`
/// needs to gate a drag on.
///
/// **A rectangle stays after the element stops being drawn**, because nothing clears the
/// map — it is the last frame's answer, not this frame's. The two cases are the same one
/// while a program rebuilds its tree every frame, which is what a view callback does.
pub fn element_bounds(id: &str, field: &str) -> f64 {
    let win = window::current();
    let bounds = MOUSE_BOUNDS.with(|m| m.borrow().get(&(win, id.to_string())).copied());
    let Some(bounds) = bounds else {
        return 0.0;
    };
    match field {
        "x" => round3(f32::from(bounds.origin.x)),
        "y" => round3(f32::from(bounds.origin.y)),
        "w" => round3(f32::from(bounds.size.width)),
        "h" => round3(f32::from(bounds.size.height)),
        _ => 0.0,
    }
}

/// The payload of an event, cut at [`POINTER_SEP`].
///
/// This is what [`crate::event_parts`] hands back, so `kind == "move"` keeps meaning what it
/// meant before the pointer payload grew a tail.
pub(crate) fn payload_kind(payload: &str) -> String {
    match payload.split_once(POINTER_SEP) {
        Some((kind, _)) => kind.to_string(),
        None => payload.to_string(),
    }
}

fn focus_for(win: i64, id: &str, cx: &mut App) -> FocusHandle {
    FOCUS.with(|f| {
        f.borrow_mut()
            .entry((win, id.to_string()))
            .or_insert_with(|| cx.focus_handle())
            .clone()
    })
}

/// Records a handle the caller already owns under `(win, id)`, so [`is_focused`] and [`focused`] can
/// see it and a [`crate::store::Request::Focus`] naming `id` can reach it.
///
/// [`focus_for`] *builds* a handle for the elements that have none of their own; this is for the one
/// element that does — the editor owns an entity with its own `cx.focus_handle()`, and registering a
/// second, unrelated handle for it would report focus that the widget never has.
pub(crate) fn register_focus(win: i64, id: &str, handle: FocusHandle) {
    FOCUS.with(|f| {
        f.borrow_mut().insert((win, id.to_string()), handle);
    });
}

/// A tooltip's subtree, as a view of its own — which is the only shape gpui accepts for one.
///
/// `Div::tooltip`'s builder answers an `AnyView`, and gpui lays that view out as a **root**
/// (`gpui-0.2.2/src/window.rs:2100-2102`), so a described subtree has to arrive as one. This is
/// that wrapper and nothing else: the tooltip's own style string is the style of the box it
/// draws and its children are its content, so it goes through the ordinary `Kind::Plain` path —
/// padding, background, `w=auto` and the `bounds` token included. Nothing about a tooltip's
/// layout is special; only *when* it happens is.
struct TooltipView {
    /// The tooltip's whole subtree, copied out of the frame that described it and standing on
    /// its own: the tooltip is laid out in a later frame, where the arena it was built in is a
    /// different tree wearing the same numbering. See `capture`.
    els: std::rc::Rc<Vec<El>>,
    /// The entity to notify when something inside the tooltip asks for a redraw. The tooltip is
    /// a view of its own, so it cannot notify the frame that built it directly; the frame's id is
    /// carried here for the one class of element that notifies at all (a pointer's, an editor's).
    entity: EntityId,
    /// The window this tooltip belongs to, captured when it was given to its target.
    ///
    /// gpui lays a tooltip out inside `Window::draw`, which is after `Root::render` — the one
    /// place that points the thread at a window — has returned. Without this the tooltip's own
    /// subtree would be rendered as part of window `0`: a `bounds` token inside it would report
    /// into the wrong window's registry, and anything it asked to be redrawn would be asked of
    /// nothing.
    win: i64,
}

impl Render for TooltipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Everything the subtree below reads — its children, its image subtrees — comes from the
        // copy, so it does not matter what this window's arena holds by now, or whether it holds
        // anything at all.
        CAPTURED.with(|c| c.borrow_mut().push(self.els.clone()));
        let element = window::with(self.win, || render(&self.els[0], window, cx, self.entity));
        CAPTURED.with(|c| c.borrow_mut().pop());
        element
    }
}

/// The ghost a drag draws while it is in flight: the element being dragged, drawn under the
/// pointer that picked it up.
///
/// It is [`TooltipView`]'s situation exactly. gpui takes the view `on_drag` was handed, prepaints
/// it as a **root of its own** at `mouse_position - cursor_offset` with `AvailableSpace::min_size()`
/// (`gpui-0.2.2/src/window.rs:2055-2060` — and takes it in preference to a tooltip, so a drag hides
/// one), and paints it from inside `Window::draw`, after `Root::render` has returned. So the
/// subtree is copied out of the frame that described it by `capture`, and put back into the window
/// that drew it by `window::with` — the same two moves, for the same two reasons.
///
/// **What it holds is the dragged element's own subtree and not the `mouse` wrapper around it.**
/// The wrapper is what arms the drag in the first place, so drawing it here would register a
/// second `on_drag` for the same id from inside the overlay's own paint — which gpui refuses
/// (`elements/div.rs:513` asserts the listener is not already set), one frame into a drag.
struct DragPreview {
    els: std::rc::Rc<Vec<El>>,
    entity: EntityId,
    win: i64,
}

impl Render for DragPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        CAPTURED.with(|c| c.borrow_mut().push(self.els.clone()));
        let element = window::with(self.win, || render(&self.els[0], window, cx, self.entity));
        CAPTURED.with(|c| c.borrow_mut().pop());
        element
    }
}

pub fn render(el: &El, window: &mut Window, cx: &mut App, entity: EntityId) -> AnyElement {
    // Every callback built below outlives this frame. Each one re-enters
    // through `window::with(win, ..)` so that the state, events and requests it
    // touches belong to the window it was drawn in.
    let win = window::current();
    match &el.kind {
        Kind::Plain => own_frame(|| {
            // A `gpui::tooltip` among the children is lifted out instead of laid out beside
            // them, because gpui does not draw it there: it draws it *over* this element, at
            // the pointer, while the pointer rests on it. Everything else is a child as before.
            let mut children: Vec<AnyElement> = Vec::new();
            let mut tip: Option<El> = None;
            for h in &el.children {
                let child = snapshot(*h as i64);
                if child.is_none() {
                    // A parent named an element that is not in this window's
                    // arena, so it will not be drawn and nothing else will say
                    // so: the parent still paints its own background, and an
                    // empty box is silent. This is the exact shape of the fault
                    // that drew 02_data_table's rows as 42 tinted bars for days
                    // — `arena()` keys on `window::current()`, and the list
                    // rendered its rows outside the frame's window, so every
                    // child handle missed. Naming it here is what makes that
                    // class of fault visible to the suites.
                    let have = arena(|a| a.els.len());
                    note_error(format!(
                        "child handle {h} names no element in this window (window {win} holds {have})"
                    ));
                    continue;
                }
                let child = child.unwrap();
                if matches!(child.kind, Kind::Tooltip) {
                    tip = Some(child);
                    continue;
                }
                children.push(render(&child, window, cx, entity));
            }
            // `tip` becomes an `Rc` once, here: `with_tooltip` moves it into a closure that gpui
            // may call many times over many frames, and each call has to be able to take its own
            // copy of the descriptor.
            let tip = tip.map(std::rc::Rc::new);
            let base = style::apply(div(), &el.style);
            let (hover, active, focus) = style::interaction(&el.style);
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
                        win,
                        &el.id,
                        cx,
                    );
                    let handle = scroll::handle(win, &el.id);
                    with_tooltip(
                        scrolled
                            .overflow_x_scroll()
                            .track_scroll(&handle)
                            .children(children),
                        tip,
                        win,
                        entity,
                    )
                    .into_any_element()
                }
                Some("both") => {
                    let scrolled = apply_focus_order(
                        apply_interactions(
                            base.id(ElementId::Name(el.id.clone().into())),
                            &el.style,
                        ),
                        &el.style,
                        win,
                        &el.id,
                        cx,
                    );
                    let handle = scroll::handle(win, &el.id);
                    with_tooltip(
                        scrolled.overflow_scroll().track_scroll(&handle).children(children),
                        tip,
                        win,
                        entity,
                    )
                    .into_any_element()
                }
                Some(_) => {
                    let scrolled = apply_focus_order(
                        apply_interactions(
                            base.id(ElementId::Name(el.id.clone().into())),
                            &el.style,
                        ),
                        &el.style,
                        win,
                        &el.id,
                        cx,
                    );
                    let handle = scroll::handle(win, &el.id);
                    with_tooltip(
                        scrolled
                            .overflow_y_scroll()
                            .track_scroll(&handle)
                            .children(children),
                        tip,
                        win,
                        entity,
                    )
                    .into_any_element()
                }
                // A plain box only becomes interactive when it asks to be, so
                // the common case keeps a stateless element. A `ctx=` counts as
                // asking: a key context lives on a dispatch-tree node, and a node
                // exists only for an element with an id. So does any of the focus
                // tokens — `focusable`, `tabstop=`, `tabindex=` — because those are
                // applied to interactive elements only, and a box that asks to be
                // tabbed to and gets nothing is the fault `19_tab_stop` names: "a
                // plain element only receives focus tokens when
                // its style also has `hover=` or `active=`".
                None if hover.is_some()
                    || active.is_some()
                    || focus.is_some()
                    || style::key_context(&el.style).is_some()
                    || style::wants_focus(&el.style) =>
                {
                    // A box that asked for its rectangle gets it recorded even when it is
                    // interactive. The `bounds` arm below is only reached by a box that is *not*
                    // — it is matched after this one — so a container carrying both an
                    // interaction token and `bounds` was answered with nothing at all, which is
                    // the same silence a missing slot gives everywhere else. `group=` is a second
                    // reason to need that rectangle, and the reason it was found.
                    //
                    // The strip goes on **before** the id, because `on_children_prepainted` is
                    // the stateless `Div`'s own method and `Stateful<Div>` has no way back to it
                    // — see [`with_bounds_strip`]. The id is applied in the same call here that
                    // already applied it, so an interactive box that wants its box is the same
                    // dispatch-tree node it was before, plus one inert child.
                    let wants = style::wants_bounds(&el.style) && !el.id.is_empty();
                    let group = style::group(&el.style);
                    let base = if wants {
                        with_bounds_strip(base, win, el.id.clone(), group)
                    } else {
                        base
                    };
                    let interactive = apply_focus_order(
                        apply_interactions(
                            base.id(ElementId::Name(el.id.clone().into())),
                            &el.style,
                        ),
                        &el.style,
                        win,
                        &el.id,
                        cx,
                    );
                    let interactive = if wants {
                        interactive.children(std::iter::once(bounds_strip()).chain(children))
                    } else {
                        interactive.children(children)
                    };
                    with_tooltip(interactive, tip, win, entity).into_any_element()
                }
                // `bounds` — the element asks for its own box to be readable. A box that
                // listens to nothing is otherwise unreachable by `gpui::element_bounds`, and
                // "a box sized to its content, read back" is exactly that shape.
                None if style::wants_bounds(&el.style) && !el.id.is_empty() => {
                    report_bounds(base, win, el.id.clone(), children, tip, entity, style::group(&el.style))
                }
                // A box that was given a tooltip has to be id'd even when it asks for nothing
                // else, because that is where gpui hangs the tooltip. One that asks for
                // nothing and was given nothing stays the stateless element it has always
                // been.
                None if tip.is_some() => with_tooltip(
                    base.id(ElementId::Name(el.id.clone().into())).children(children),
                    tip,
                    win,
                    entity,
                )
                .into_any_element(),
                None => base.children(children).into_any_element(),
            }
        }),

        // A tooltip that no target ever took. `Kind::Plain` lifts one out of its children and
        // hands it to `Div::tooltip`; reaching here means this one was put somewhere that
        // lifts nothing — a root, a row — and gpui draws no tooltip without a hitbox under it,
        // so there is no "correct" rendering to give it. It is drawn as the box it is: a stray
        // `gpui::tooltip` shows up where the program put it instead of vanishing or panicking.
        Kind::Tooltip => own_frame(|| {
            let plain = El {
                kind: Kind::Plain,
                style: el.style.clone(),
                id: el.id.clone(),
                children: el.children.clone(),
            };
            render(&plain, window, cx, entity)
        }),

        Kind::Text(text) => own_frame(|| {
            let text = text.clone();
            let box_ = style::apply(div(), &el.style);
            // A text element can be asked for its own box too: the run's laid-out width is what
            // "a box sized to its content" is checked against, and it is the box a marker placed
            // at a character is measured inside of.
            if style::wants_bounds(&el.style) && !el.id.is_empty() {
                // A run of text is given no tooltip: gpui raises one from a hitbox, and a text
                // element is not where a program would hang one — the box around it is.
                report_bounds(box_, win, el.id.clone(), vec![text.into_any_element()], None, entity, style::group(&el.style))
            } else {
                box_.child(text).into_any_element()
            }
        }),

        Kind::Button(label) => own_frame(|| {
            let id = el.id.clone();
            let click_id = id.clone();
            let base = style::apply(
                div().flex().items_center().justify_center().cursor_pointer(),
                &el.style,
            )
            .id(ElementId::Name(id.into()));
            let focus_id = el.id.clone();
            let base = apply_interactions(base, &el.style);
            // A button is the element a person tabs to, so it applies the focus tokens like
            // any other interactive element: a button that did not could not be focused at all.
            let base = apply_focus_order(base, &el.style, win, &focus_id, cx);
            base.child(label.clone())
                .on_click(move |_event, _window, cx| {
                    window::with(win, || store::push_event(click_id.clone()));
                    cx.notify(entity);
                })
                .into_any_element()
        }),

        Kind::Checkbox { label, checked } => own_frame(|| {
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
                        let on = !store::get_flag(&click_id, checked);
                        store::set_flag(&click_id, on);
                        store::push_event_with(&click_id, if on { "on" } else { "off" });
                    });
                    cx.notify(entity);
                })
                .into_any_element()
        }),

        Kind::Input { placeholder } => own_frame(|| {
            // A single-line field is the editor's engine with a flag, not a second widget. What that
            // buys is everything a hand-rolled key loop never had: the caret, the selection and the
            // clipboard, the undo history, and — the reason the row exists — a **marked range**, so
            // an input method has somewhere to put the word it is composing. The keys this field
            // answers to are the engine's own, in `editor::TextEditor::on_key_down`: `enter` submits
            // and `escape` cancels, and a vertical move goes on bubbling.
            let key = el.id.clone();
            let field = editor::editor_element(win, &el.id, placeholder, &el.style, true, window, cx);
            let base = style::apply(
                div().flex().flex_row().items_center().min_h(px(28.0)).cursor_text(),
                &el.style,
            );
            base.id(ElementId::Name(el.id.clone().into()))
                // `<key>|focus`, on the way down rather than on a click that completed: this is the
                // event the widget has always raised, and a program that swaps a row of buttons for
                // a search field when it fires wants it when the field is entered, not when the
                // button came back up. The field's own `on_mouse_down` does not stop propagation, so
                // the caret is placed by the engine and this runs after it.
                .on_mouse_down(MouseButton::Left, move |_event, _window, _cx| {
                    let key = key.clone();
                    window::with(win, || store::push_event_with(&key, "focus"));
                })
                .child(field)
                .into_any_element()
        }),

        Kind::Slider { min, max } => own_frame(|| {
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
        }),

        Kind::Progress(value) => own_frame(|| {
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
        }),

        // The element id matters here beyond hit-testing: GPUI keeps an
        // image's animation cursor (`ImgState`) keyed by its global element id,
        // so without `.id(..)` an animated GIF never advances a frame.
        Kind::Image { path, loading, fallback } => own_frame(|| {
            render_image(el, path, *loading, *fallback, win, window, cx, entity)
        }),

        Kind::Icon(path) => own_frame(|| {
            let icon: Svg = svg().path(SharedString::from(asset_path(path)));
            let icon = style::apply(icon, &el.style);
            match style::rotation(&el.style) {
                Some(degrees) => icon
                    .with_transformation(gpui::Transformation::rotate(gpui::radians(
                        degrees.to_radians(),
                    )))
                    .into_any_element(),
                None => icon.into_any_element(),
            }
        }),

        Kind::Divider => own_frame(|| {
            style::apply(div().h(px(1.0)).w_full().bg(style::colour("#313244")), &el.style)
                .into_any_element()
        }),

        Kind::Spacer(size) => own_frame(|| {
            style::apply(div().h(px(*size as f32)).w_full(), &el.style).into_any_element()
        }),

        Kind::Virtual { count, row_height } => own_frame(|| {
            let count = *count;
            let row_height = *row_height;
            // The list's own id, so the rows it builds can be recorded against
            // it while the closure has nothing else to reach for, and so the row
            // builder — one per program — knows which list is asking.
            let list_id = el.id.clone();
            // The builder was handed to `gpui::run_with_rows`, and is invoked
            // here, during layout. That is exactly what a *stored* callback
            // cannot be: `gpui::list` returns before the list lays out, so a
            // builder given to it was released by its caller long before this
            // point, and on the compiled tiers every row answered an error.
            let rows_builder = row_builder();
            let list = uniform_list(ElementId::Name(el.id.clone().into()), count, {
                move |range, window, cx| {
                    // `range` is the rows gpui is laying out — the rows on
                    // screen. It is the only reliable statement of the visible
                    // range (`logical_scroll_top_index` is `cfg(test)`), so it
                    // is recorded here before the loop consumes it.
                    let (first, last) = (range.start, range.end.saturating_sub(1));
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
                        let row = match rows_builder.as_ref() {
                            None => {
                                window::with(win, || {
                                    note_error(
                                        "this program draws a list but was started with gpui::run, \
                                         which carries no row builder; start it with \
                                         gpui::run_with_rows instead".to_string(),
                                    )
                                });
                                None
                            }
                            Some(build) => {
                                // The id first, then the index: one builder
                                // serves every list a program draws.
                                let args = vec![
                                    Value::String(list_id.clone().into()),
                                    Value::Int(index as i64),
                                ];
                                match window::with(win, || invoke(build, args)) {
                                    Ok(Value::Int(handle)) if handle > 0 => {
                                        window::with(win, || snapshot(handle))
                                    }
                                    Ok(Value::Struct(s)) => {
                                        let handle = s
                                            .fields
                                            .iter()
                                            .find_map(|(name, val)| {
                                                if *name == "handle" {
                                                    if let Value::Int(h) = val {
                                                        return Some(*h);
                                                    }
                                                }
                                                None
                                            })
                                            .or_else(|| {
                                                s.fields.get(0).and_then(|(_, val)| match val {
                                                    Value::Int(h) if *h > 0 => Some(*h),
                                                    _ => None,
                                                })
                                            });
                                        match handle {
                                            Some(h) if h > 0 => window::with(win, || snapshot(h)),
                                            _ => {
                                                window::with(win, || {
                                                    note_error(format!(
                                                        "the row builder for row {index} answered a struct without a valid element handle"
                                                    ))
                                                });
                                                None
                                            }
                                        }
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
                                }
                            }
                        };
                        // The render has to run as this window too: `render`
                        // turns each child handle back into an element by
                        // reading the arena, and `arena()` keys on
                        // `window::current()`. A list is built during layout,
                        // after the frame that asked for it returned, so the
                        // window it was described in is no longer current —
                        // and without this the arena lookup misses and every
                        // container's children are dropped, leaving rows that
                        // paint their background and nothing else. That is the
                        // fault that draws 02_data_table's rows as 42 tinted bars.
                        let element = match row {
                            Some(row) => window::with(win, || render(&row, window, cx, entity)),
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
                    if !rows.is_empty() {
                        window::with(win, || list::set_range(win, &list_id, first, last));
                    }
                    rows
                }
            });
            let handle = list::handle(win, &el.id);
            style::apply(list, &el.style).track_scroll(handle).into_any_element()
        }),

        Kind::Paths { paths, backgrounds } => own_frame(|| {
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
        }),

        Kind::Mouse { child, id, kinds, window_kinds } => own_frame(|| {
            // `on_mouse` reports for the element it was handed and adds no box
            // of its own, so whatever carries the handler has to *be* that
            // element. A bare `div()` wrapped around the receiver's render is
            // not: gpui makes every div a containing block (`Style::default`
            // is `Position::Relative`), so the wrapper becomes the containing
            // block of an out-of-flow receiver, and
            // `ui::rect("absolute right=4 top=0 w=16 h=full")` measures its
            // height against a wrapper with none. The whole element, its
            // background and its children go to zero — 02_data_table's scrollbar
            // strip and the thumb inside it — and
            // the box a click is tested against is that same empty wrapper.
            //
            // So a plain receiver is *folded*: its style and interactions
            // become the wrapper's, its children are rendered inside it, and a
            // child written after `.on_mouse(..)` is added beside them. That
            // is the receiver's own box in the receiver's own place, in flow
            // or out of it. (A receiver that scrolls would lose its handle
            // here; none does.)
            //
            // A text, button, image or list receiver takes the other shape —
            // it renders itself and the handler goes on a wrapper around it.
            // No program chains a child after `.on_mouse` on one of those, so
            // that wrapper has never had to be the receiver's box.
            //
            // A div can report only its *children's* bounds, so the box the
            // pointer is measured against has to be one of them. The folded
            // wrapper therefore opens with the same child the slider opens
            // with: an out-of-flow, unpainted, full-size strip that *is* the
            // element's box. `report_pointer` subtracts its origin, and without
            // it the first child here would be the first thing *inside* the
            // element — 02's scrollbar thumb — so `thumb.y` would arrive
            // relative to the thumb rather than to the strip it slides on, and
            // an element with no children at all (13's canvas, 26's drag box)
            // would report window coordinates. It moves nothing, paints
            // nothing, and takes no click: a div with no listener inserts no
            // hitbox.
            let receiver = snapshot(*child as i64);
            let folds = matches!(receiver.as_ref().map(|c| &c.kind), Some(Kind::Plain));
            let bounds_key = id.clone();
            let mut wrapper = match receiver {
                Some(receiver) if folds => {
                    let bounds_group = style::group(&receiver.style);
                    let mut children: Vec<AnyElement> = vec![div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .size_full()
                        .into_any_element()];
                    for handle in receiver.children.iter().chain(el.children.iter()) {
                        match snapshot(*handle as i64) {
                            Some(child) => children.push(render(&child, window, cx, entity)),
                            None => note_error(format!(
                                "child handle {handle} names no element in this window"
                            )),
                        }
                    }
                    let base = style::apply(div(), &receiver.style)
                        .on_children_prepainted(move |bounds, _window, _cx| {
                            if let Some(first) = bounds.first() {
                                record_box(win, &bounds_key, bounds_group.as_deref(), *first);
                            }
                        })
                        .id(ElementId::Name(receiver.id.clone().into()));
                    let base = apply_interactions(base, &receiver.style);
                    apply_focus_order(base, &receiver.style, win, &receiver.id, cx)
                        .children(children)
                }
                receiver => {
                    let bounds_group = style::group(&el.style);
                    let inner = receiver
                        .map(|mut c| {
                            c.children.extend(el.children.iter().copied());
                            window::with(win, || render(&c, window, cx, entity))
                        })
                        .unwrap_or_else(|| div().into_any_element());
                    style::apply(div(), &el.style)
                        .on_children_prepainted(move |bounds, _window, _cx| {
                            if let Some(first) = bounds.first() {
                                record_box(win, &bounds_key, bounds_group.as_deref(), *first);
                            }
                        })
                        .id(ElementId::Name(el.id.clone().into()))
                        .child(inner)
                }
            };

            // `"down! up"` asks for the same report as `"down up"` **and** for the press to stop
            // where it landed: the listener calls `cx.stop_propagation()` before it returns, so
            // no listener registered earlier hears the event. gpui's dispatch runs the capture
            // pass in registration order and then the bubble pass over the same list `rev()`d,
            // breaking out of each the moment `cx.propagate_event` goes false
            // (`window.rs:3695-3714`); elements register in paint order, parents before
            // children, so the reverse order is child-first and what a stop actually withholds
            // the event from is the **ancestors**. `cx.propagate_event` is reset for every event
            // (`window.rs:3584`), so a stop is about the one event and not about the next.
            //
            // The `!` is on the kind rather than on the element because those are two different
            // questions: an element may want its own press reported and the presses inside it
            // left alone, and there is no way to write "everything except this one" in a list of
            // kinds. `down!` is what `down` is — `kinds.contains("down")` is still true of it —
            // with one listener option set.
            let consumes = |kind: &str| {
                let wanted = format!("{kind}!");
                kinds
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .any(|word| word == wanted)
            };

            if kinds.contains("down") {
                let key = id.clone();
                let stops = consumes("down");
                wrapper = wrapper.on_mouse_down(MouseButton::Left, move |event, _w, cx| {
                    window::with(win, || {
                        report_pointer(&key, "down", event.position, event.modifiers, Some(event.click_count))
                    });
                    if stops {
                        cx.stop_propagation();
                    }
                    cx.notify(entity);
                });
            }
            if kinds.contains("up") {
                let key = id.clone();
                let stops = consumes("up");
                wrapper = wrapper.on_mouse_up(MouseButton::Left, move |event, _w, cx| {
                    window::with(win, || {
                        report_pointer(&key, "up", event.position, event.modifiers, Some(event.click_count))
                    });
                    if stops {
                        cx.stop_propagation();
                    }
                    cx.notify(entity);
                });
            }
            if kinds.contains("click") {
                let key = id.clone();
                let stops = consumes("click");
                wrapper = wrapper.on_click(move |event, _w, cx| {
                    let count = click_count(event);
                    window::with(win, || {
                        report_pointer(&key, "click", event.position(), event.modifiers(), count)
                    });
                    if stops {
                        cx.stop_propagation();
                    }
                    cx.notify(entity);
                });
            }
            if kinds.contains("right") {
                let key = id.clone();
                let stops = consumes("right");
                wrapper = wrapper.on_mouse_down(MouseButton::Right, move |event, _w, cx| {
                    window::with(win, || {
                        report_pointer(&key, "right", event.position, event.modifiers, Some(event.click_count))
                    });
                    if stops {
                        cx.stop_propagation();
                    }
                    cx.notify(entity);
                });
            }
            if kinds.contains("move") {
                let key = id.clone();
                let stops = consumes("move");
                wrapper = wrapper.on_mouse_move(move |event, _w, cx| {
                    if event.dragging() {
                        window::with(win, || {
                            report_pointer(&key, "move", event.position, event.modifiers, None)
                        });
                        if stops {
                            cx.stop_propagation();
                        }
                        cx.notify(entity);
                    }
                });
            }
            // The wheel. The gate is not the same one the handlers above are under, and the
            // difference is gpui's: `on_scroll_wheel` asks `hitbox.should_handle_scroll(window)`
            // (`elements/div.rs:318`), which tests the whole hit-test list, where `on_mouse_down`
            // and its neighbours ask `hitbox.is_hovered(window)` (`:128`), which tests only the
            // hovered prefix (`window.rs:500-516`). So the wheel reaches an element that has
            // something on top of it and the presses do not — which is what a scrolling
            // container wants, and is why this is registered here rather than folded into the
            // move handler.
            if kinds.contains("scroll") {
                let key = id.clone();
                let stops = consumes("scroll");
                wrapper = wrapper.on_scroll_wheel(move |event, _w, cx| {
                    window::with(win, || report_scroll(&key, event));
                    if stops {
                        cx.stop_propagation();
                    }
                    cx.notify(entity);
                });
            }
            // The pointer leaving. gpui gives an element no exit event — element listeners are
            // matched on a concrete event type, and `MouseExitEvent` is not one any of them names
            // (`elements/div.rs:2042-2059` registers them all as `on_mouse_event::<MouseDownEvent>`
            // and its neighbours), so a window-level exit listener is the only one that ever sees
            // it. What an element does have is the **transition** of its own hover, delivered to
            // the hover listener as a bool, and that is what this reports. See `report_exit` for
            // what that costs — a move, and a point borrowed from the last one.
            if kinds.contains("exit") {
                let key = id.clone();
                wrapper = wrapper.on_hover(move |hovered, _w, cx| {
                    if !*hovered {
                        window::with(win, || report_exit(&key));
                        cx.notify(entity);
                    }
                });
            }

            // ---- the drag states ---------------------------------------------------------------
            //
            // gpui's own drag protocol, reached from the binding rather than rebuilt. `DD2`'s row
            // said the binding could not: "`InteractiveElement::on_drag_move` (`elements/div.rs:282`)
            // is a method on `Interactivity`, gpui's internal per-element state, so the binding
            // cannot register one through the element vocabulary it builds." **It can.**
            // `Interactivity` is a `pub` struct (`:1478`), `InteractiveElement` is a `pub` trait
            // whose three drag methods are public (`:818`, `:976`, `:1132`), and the one thing that
            // stood in the way — `mod elements` private (`gpui.rs:21`) — is undone by the line
            // after it, `pub use elements::*` (`:77`). So `gpui::Interactivity` is a nameable path
            // and these are callable from here.
            //
            // What was true is that the binding could not make one **fire**. `on_drag_move`'s
            // listener opens with `phase == DispatchPhase::Capture && let Some(drag) = &cx.active_drag`
            // and `drag.value.type_id() == TypeId::of::<T>()` (`:282-296`), and `cx.active_drag` is
            // `pub(crate)` (`app.rs:545`) with exactly two writers in the whole crate: the
            // platform's file-drop translation, and the pending-mouse-down listener that arms a
            // drag — which only arms when `drag_listener.take()` is `Some` (`:2158-2174`), and only
            // `on_drag` ever puts one there (`:499`). So the arming half is not optional and the
            // payload comes with it.
            //
            // That payload is this element's own id, a `String` the program chose, and it is what
            // travels: gpui carries it in `cx.active_drag` and hands it to whichever drop listener
            // matches its `TypeId`. It is written back to a target as `<id>.drag_value`, and read
            // there by `DragMoveEvent::drag` (`:70`) — the drop half gets it as an argument.
            if kinds.contains("drag") {
                let key = id.clone();
                // The **dragged element's own subtree**, not the wrapper around it: the wrapper is
                // what arms this drag, so drawing it in the overlay would register a second
                // `on_drag` for one id, which gpui's own assertion refuses.
                let ghost = snapshot(*child as i64)
                    .map(|receiver| capture(&receiver))
                    .unwrap_or_else(|| std::rc::Rc::new(Vec::new()));
                wrapper = wrapper.on_drag::<String, DragPreview>(
                    id.clone(),
                    move |value, _offset, window, cx| {
                        let point = window.mouse_position();
                        let modifiers = ELEMENT_AT
                            .with(|m| m.borrow().get(&(win, key.clone())).map(|(_, _, m)| *m))
                            .unwrap_or_default();
                        window::with(win, || report_drag(&key, "drag", point, modifiers, value));
                        cx.notify(entity);
                        cx.new(|_cx| DragPreview {
                            els: ghost.clone(),
                            entity,
                            win,
                        })
                    },
                );
            }
            if kinds.contains("over") {
                let key = id.clone();
                // `win` is the **captured** window and not `window::current()`, which is `0` here:
                // gpui calls this listener from its own dispatch, long after the frame that
                // registered it has returned, and only that frame ran inside `window::with`.
                wrapper = wrapper.on_drag_move::<String>(move |event, _window, cx| {
                    let inside = event.bounds.contains(&event.event.position);
                    let was = DRAG_OVER
                        .with(|m| m.borrow().get(&(win, key.clone())).copied())
                        .unwrap_or(false);
                    if inside != was {
                        DRAG_OVER.with(|m| m.borrow_mut().insert((win, key.clone()), inside));
                        let kind = if inside { "over" } else { "out" };
                        let payload = event.drag(cx).clone();
                        window::with(win, || {
                            report_drag(&key, kind, event.event.position, event.event.modifiers, &payload)
                        });
                        cx.notify(entity);
                    }
                });
            }
            if kinds.contains("drop") {
                let key = id.clone();
                wrapper = wrapper.on_drop::<String>(move |value, window, cx| {
                    let point = window.mouse_position();
                    window::with(win, || report_drop(&key, point, value));
                    cx.notify(entity);
                });
            }

            // The window-level half: the same events, heard wherever the pointer is.
            //
            // Every handler above but the wheel is gated by gpui on
            // `hitbox.is_hovered(window)` (`gpui-0.2.2/src/elements/div.rs:128`, `:147`), so a
            // drag that leaves the element is heard by nobody: the moves stop arriving and the
            // release, which can only happen outside it, is missed altogether — and the wheel
            // is gated on `should_handle_scroll`, which is a different question again and
            // still not this one. `Window::on_mouse_event`
            // (`:3421`) is the window's own listener — dispatched for every mouse event
            // wherever the pointer is (`window.rs:3677-3714`) — and is what upstream reaches
            // for to keep a drag alive: `data_table.rs` hangs its scrollbar's three listeners
            // on a `canvas` inside the thumb (`:320-364`).
            //
            // **A `canvas`, and not this element's own render, because gpui refuses the call
            // anywhere else**: `on_mouse_event` opens with `debug_assert_paint`
            // (`window.rs:3425`), and a view's `Render::render` — which is where every
            // handler above is written — runs in the *prepaint* phase. Registering from there
            // panics with `this method can only be called during paint`
            // at `window.rs:3425`. Upstream is in a paint callback for the same reason, and the
            // canvas is how a program that builds its tree in `render` reaches one.
            //
            // Registered every frame the element is drawn, and that is not an install:
            // gpui clears the list at the end of each frame (`window.rs:743`), so an element
            // a program stops drawing stops hearing anything. `report_pointer` is the same
            // writer the element-level handlers use, so the payload, the store and the
            // `<id>.x`/`<id>.y` the program reads are the same either way; the only
            // difference is where the pointer had to be.
            if !window_kinds.is_empty() {
                let key = id.clone();
                let heard = window_kinds.clone();
                // Out of flow and full size, so it cannot move anything: the first child of
                // this element is still the strip `MOUSE_BOUNDS` is read from, and this is
                // appended after it and after everything else.
                let registrar = canvas(
                    move |_bounds, _window, _cx| (),
                    move |_bounds, _state, window, cx| {
                        register_window_mouse(window, cx, entity, win, &key, &heard)
                    },
                )
                .absolute()
                .size_full();
                wrapper = wrapper.child(registrar);
            }
            wrapper.into_any_element()
        }),

        Kind::Editor { key, placeholder } => own_frame(|| {
            // The style string goes both ways: it is applied to the box the editor sits in, as it
            // always was, and it is handed to the widget for the chrome tokens that colour what the
            // widget paints itself — the caret, the selection, and the line-number gutter.
            let field = editor::editor_element(win, key, placeholder, &el.style, false, window, cx);
            style::apply(div(), &el.style).child(field).into_any_element()
        }),

        Kind::StyledText { content, highlights } => own_frame(|| {
            let ranges: Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> = highlights
                .iter()
                .filter_map(|spec| parse_highlight(spec))
                .collect();
            let text = gpui::StyledText::new(content.clone()).with_highlights(ranges);
            style::apply(div(), &el.style).child(text).into_any_element()
        }),

        Kind::Canvas { painter, phase } => own_frame(|| {
            let painter = painter.clone();
            let phase = *phase;
            let element = canvas(
                move |_bounds, _window, _cx| (),
                move |bounds, _state, window, _cx| paint(&painter, bounds, phase, window),
            );
            style::apply(element, &el.style).into_any_element()
        }),

        Kind::Scrollbar { id, axis } => own_frame(|| {
            let painting_id = id.clone();
            let painting_axis = axis.clone();
            let down_id = id.clone();
            let down_axis = axis.clone();
            let move_id = id.clone();
            let move_axis = axis.clone();
            let colour = style::thumb_colour(&el.style).unwrap_or_else(|| style::colour("#585b70"));

            // The track is the element's own background and the thumb is painted over it, so the
            // bar is `canvas` inside a `div` that carries the style and the handlers — a canvas on
            // its own is not an element anything can be pressed on.
            let bar = canvas(
                move |_bounds, _window, _cx| (),
                move |bounds, _state, window, _cx| {
                    scroll::paint(&painting_id, &painting_axis, bounds, colour, window)
                },
            );

            // Pressing or dragging sets the offset — the half of a scrollbar the inert thumb in
            // `02_data_table` does not have. Both are the same rule, so there is no drag state to
            // lose when the pointer leaves the track.
            style::apply(div().child(bar), &el.style)
                .id(ElementId::Name(format!("~scrollbar|{id}|{axis}").into()))
                .on_mouse_down(MouseButton::Left, move |event, _window, _cx| {
                    scroll::jump(&down_id, &down_axis, event.position);
                })
                .on_mouse_move(move |event, _window, _cx| {
                    if event.dragging() {
                        scroll::jump(&move_id, &move_axis, event.position);
                    }
                })
                .into_any_element()
        }),

        Kind::Anchored { corner, x, y, offset_x, offset_y } => own_frame(|| {
            let mut a = gpui::anchored();
            a = match corner {
                1 => a.anchor(gpui::Corner::TopRight),
                2 => a.anchor(gpui::Corner::BottomLeft),
                3 => a.anchor(gpui::Corner::BottomRight),
                _ => a.anchor(gpui::Corner::TopLeft),
            };
            if *x != 0.0 || *y != 0.0 {
                a = a.position(gpui::point(gpui::px(*x as f32), gpui::px(*y as f32)));
            }
            if *offset_x != 0.0 || *offset_y != 0.0 {
                a = a.offset(gpui::point(gpui::px(*offset_x as f32), gpui::px(*offset_y as f32)));
            }
            for h in &el.children {
                if let Some(child) = snapshot(*h as i64) {
                    a = a.child(render(&child, window, cx, entity));
                }
            }
            a.into_any_element()
        }),

        Kind::Deferred { priority } => own_frame(|| {
            let mut container = div();
            for h in &el.children {
                if let Some(child) = snapshot(*h as i64) {
                    container = container.child(render(&child, window, cx, entity));
                }
            }
            gpui::deferred(style::apply(container, &el.style))
                .with_priority(*priority)
                .into_any_element()
        })
    }
}

/// Builds one element on a stack frame of its own, rather than inside `render`'s.
///
/// `render` is the recursive function — it calls itself once per level of the
/// element tree — so everything reserved in its frame is paid again at every
/// level. Measured before this split, one `render` frame was **202,704 bytes**,
/// which put the ceiling near 40 levels on a thread's 8 MiB stack; after it,
/// **41,824**. `23_tree` ported upstream's `tree` example, which builds 50, and
/// faulted rather than stopping, because the ceiling had moved since the number
/// was written down.
///
/// The frame was not one arm's. The `match` in `render` has nineteen arms and
/// LLVM lays their locals out side by side instead of sharing one slot between
/// arms that can never both be live, so what `render` reserved was roughly their
/// *sum* — the scrollbar's `Stateful<Div>` and the list's `UniformList` and the
/// mouse wrapper's `Div` all counted at once, though a given call takes exactly
/// one.
///
/// This helper is generic over the arm's own closure, so each arm is
/// monomorphised on its own and reserves only its own locals, and
/// `#[inline(never)]` keeps that frame out of `render`'s. Each arm then costs
/// what its element is worth: a `Div` is a `Div`, not nineteen of them.
///
/// The same trick upstream gets for free, and the reason `tree.rs` can nest 50
/// divs where this could nest 40: upstream writes `div().child(inner)` in the
/// program, where each level is its own call and the compiler never has to
/// choose between nineteen siblings' stack slots. A tree described *as data*
/// and rendered by one function is what puts them in one frame.
#[inline(never)]
fn own_frame(f: impl FnOnce() -> AnyElement) -> AnyElement {
    f()
}

/// One image element, drawn in whichever of its four states its load is in.
///
/// The rule is GPUI's own, read off the loader `Img` also reads (`gpui-0.2.2
/// src/elements/img.rs:366-401`): `Some(Ok(..))` is the picture, `Some(Err(..))` is the fallback
/// subtree, and `None` is a load still in flight — the loading subtree once `LOADING_DELAY` has
/// passed since the load started, and the bare element before that.
///
/// Upstream reaches that arm by handing `Img` two closures, `with_loading` and `with_fallback`
/// (`:164-173`). The binding cannot: those closures are `Fn() -> AnyElement` and take no window,
/// and a subtree this crate described is turned into elements only with a `&mut Window` in hand.
/// So the same decision is made here, where the window and the app *are* in hand, and the subtree
/// the program described is drawn in the picture's place. GPUI lays a replacement out inside the
/// image's own style, so the wrapper carries that style.
///
/// `image_state` is what this records: it is the failure signal a program had no way to get, and
/// the probe's reader.
fn render_image(
    el: &El,
    path: &str,
    loading: i64,
    fallback: i64,
    win: i64,
    window: &mut Window,
    cx: &mut App,
    entity: EntityId,
) -> AnyElement {
    let resolved = PathBuf::from(asset_path(path));
    let resource: Resource = resolved.clone().into();
    // The same call `Img` makes, and the same cache: the binding never pushes an image cache of
    // its own, so `window.image_cache_stack` is empty and `Img::paint` falls back to
    // `window.use_asset::<ImgResourceLoader>` (`img.rs:519-525`, `window.rs:843`).
    let state = window.use_asset::<ImgResourceLoader>(&resource, cx);
    let key = (win, el.id.clone());
    let now = std::time::Instant::now();

    let (shown, subtree) = match state {
        Some(Ok(_)) => {
            IMAGE_PENDING.with(|m| m.borrow_mut().remove(&key));
            ("image", None)
        }
        Some(Err(_)) => {
            IMAGE_PENDING.with(|m| m.borrow_mut().remove(&key));
            // A failed load with no fallback subtree draws nothing, which is what a plain
            // `gpui::image` does today and what upstream's cell 4 would do without its
            // `with_fallback` — not an invented empty box.
            if fallback > 0 {
                ("fallback", Some(fallback))
            } else {
                ("blank", None)
            }
        }
        None => {
            let (first, started) = IMAGE_PENDING.with(|m| {
                let mut m = m.borrow_mut();
                let first = !m.contains_key(&key);
                let started = *m.entry(key.clone()).or_insert(now);
                (first, started)
            });
            if loading > 0 && now.duration_since(started) > LOADING_DELAY {
                ("loading", Some(loading))
            } else {
                if first {
                    // A load that outlasts `LOADING_DELAY` is due a repaint at that moment and
                    // not only when the load answers: `use_asset` asks for the frame after the
                    // *load*, which is a different frame, and a program sitting still would
                    // otherwise never draw its loading subtree at all.
                    window
                        .spawn(&*cx, async move |cx| {
                            cx.background_executor().timer(LOADING_DELAY).await;
                            cx.update(move |_window, cx| cx.notify(entity)).ok();
                        })
                        .detach();
                }
                ("blank", None)
            }
        }
    };

    IMAGE_SHOWN.with(|m| m.borrow_mut().insert(key, shown));

    match subtree {
        Some(handle) => {
            let base = style::apply(div(), &el.style).id(ElementId::Name(el.id.clone().into()));
            match snapshot(handle as i64) {
                Some(child) => base.child(render(&child, window, cx, entity)).into_any_element(),
                None => {
                    note_error(format!(
                        "image `{}` names a subtree handle {handle} that is not in this window",
                        el.id
                    ));
                    base.into_any_element()
                }
            }
        }
        None => style::apply(img(resolved), &el.style)
            .id(ElementId::Name(el.id.clone().into()))
            .into_any_element(),
    }
}

/// Applies the focus-order tokens — `focusable`, `tabstop=` and `tabindex=` — to an already
/// identified element.
///
/// **`tab_stop` and `tab_index` are written onto the focus handle, and that is not a detail.**
/// GPUI reads them off the *element* only on the path this crate does not take:
/// `gpui-0.2.2/src/elements/div.rs:1584` copies `tab_stop` and
/// `tab_index` onto a handle **only when there is no tracked handle and it makes one itself**
/// ("If there's an explicit focus handle we're tracking, use that. Otherwise create a new handle").
/// A handle this crate makes starts at `tab_stop: false` (`window.rs:277`, `FocusHandle::new`), so
/// with a tracked handle supplied the element's own `tab_stop`/`tab_index` go nowhere, every
/// focusable element enters gpui's tab map as a non-stop, and `Window::focus_next`
/// (`window.rs:1413-1422`, `TabStopMap::next`) finds nothing to move to — which is what
/// `tests/gui/fo1_tab_order.gos` checks: six presses, focus never leaving `~root`. Writing the
/// two through the handle — `FocusHandle::tab_stop`/`tab_index` write to the shared `FocusRef`, so
/// the handle the registry already holds is updated too — is what makes the tab order exist.
///
/// The tokens are gathered before anything is applied, because their order in the string is the
/// program's business and `tabindex=` may precede `focusable=`.
fn apply_focus_order<T: StatefulInteractiveElement + 'static>(
    el: T,
    spec: &str,
    win: i64,
    id: &str,
    cx: &mut App,
) -> T {
    let mut el = el;
    let mut focusable = false;
    let mut tab_stop = None;
    let mut tab_index = None;
    for (key, value) in style::tokens(spec) {
        let v = value.as_deref().unwrap_or("");
        match key.as_str() {
            "focusable" => focusable = focusable || (v != "0" && v != "false"),
            "tabstop" => tab_stop = Some(v != "0"),
            "tabindex" => tab_index = Some(v.parse::<isize>().unwrap_or(0)),
            _ => {}
        }
    }
    if let Some(stop) = tab_stop {
        el = el.tab_stop(stop);
    }
    if let Some(index) = tab_index {
        el = el.tab_index(index);
    }
    if !focusable {
        // Nothing to track and nothing to be tracked by: gpui's tab machinery keys on the
        // `focusable` flag, and a `tabindex=` on an element nothing can focus is inert in gpui too.
        return el;
    }
    // **The binding's own handle, not gpui's.** `focusable()` only sets a flag on gpui's internal
    // interactivity, and gpui then makes a focus handle of its own — one `gpui::is_focused` cannot
    // see, because it reads back the handles *this* crate keeps. Tracking our own handle is enough
    // for gpui to use it: div.rs:1584 does exactly that, so clicking still focuses the element and
    // the tab map still gets an entry.
    //
    // **`focusable()` stays, and the handle is *added* to it.** Replacing the flag with
    // the handle does not work: with the flag gone, `gpui::focus_next` finds no tab
    // stop at all and `is_focused` still says no, so both halves of the row fail together. gpui's
    // own tab machinery keys on that flag, and the tracked handle is what makes the element's focus
    // *ours* rather than one gpui keeps to itself.
    let mut handle = focus_for(win, id, cx);
    if let Some(stop) = tab_stop {
        handle = handle.tab_stop(stop);
    }
    if let Some(index) = tab_index {
        handle = handle.tab_index(index);
    }
    el.focusable().track_focus(&handle)
}

/// Applies the `hover=` / `active=` / `focus=` refinements to an already identified
/// element.
///
/// `focus=` is gpui's `InteractiveElement::focus`, which needs the element to be
/// focusable to mean anything — `track_focus` for a focus handle of its own, or
/// `focusable`/`tabindex=`/`tabstop` for the framework's. A program that asks for a focus
/// refinement on an element nothing can focus has asked for something that will not
/// happen; that is the style language's usual bargain, and `gpui::style_refinements`
/// exists so it can be checked rather than guessed at.
fn apply_interactions<T: StatefulInteractiveElement + 'static>(el: T, spec: &str) -> T {
    let (hover, active, focus) = style::interaction(spec);
    let mut el = el;
    if let Some(hover) = hover {
        el = el.hover(move |r| style::refine(r, &hover));
    }
    if let Some(active) = active {
        el = el.active(move |r| style::refine(r, &active));
    }
    if let Some(focus) = focus {
        el = el.focus(move |r| style::refine(r, &focus));
    }
    // The element's place on the focused node's context stack, which is what a
    // keymap binding's context predicate is evaluated against. Passed through
    // verbatim: gpui's grammar is wider than a name and it logs what it cannot
    // parse.
    if let Some(context) = style::key_context(spec) {
        // `&str`, because that is the `TryInto<KeyContext>` gpui implements; the
        // context owns its own entries once built.
        el = el.key_context(context.as_str());
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
