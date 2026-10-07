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
    ElementId, EntityId, FocusHandle, KeyDownEvent, KeyUpEvent, MouseButton, Pixels, SharedString,
    Svg, Window,
};

use gossamer_binding::{NativeDispatch, RetainedCallback, Value};

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

pub fn clear_dispatch() {
    DISPATCH.with(|cell| cell.set(None));
}

/// Releases every retained callback the frame machinery is still holding.
///
/// Called once the run loop is done, and it has to happen *there* rather than at
/// thread exit. The arena can hold a `uniform_list`'s retained row builder, and
/// the pending-window queue holds whole view callbacks; both are
/// `RetainedCallback`s, whose `Drop` calls back into the runtime to release the
/// registration. A thread-local dropped at thread exit runs that after the
/// runtime's own thread-locals are already gone, and the runtime panics at the
/// FFI entry:
///
/// ```text
/// thread '<unnamed>' panicked: cannot access a Thread Local Storage value
/// during or after destruction: AccessError
///   ... _gos_rt_binding_callback_release
/// ```
///
/// Releasing them here, while the app is still standing, is the fix. It is why
/// a compiled build of any program containing a `gpui::list` used to end in a
/// panic on stderr, which `scripts/tiers` is what caught. See the round log.
pub fn release_retained() {
    SHOULD_CLOSE.with(|h| h.borrow_mut().clear());
    SHOULD_CLOSE_PLATFORM.with(|h| h.borrow_mut().clear());
    TEXT.with(|t| t.borrow_mut().clear());
    FONTS.with(|f| *f.borrow_mut() = None);
    ARENAS.with(|a| a.borrow_mut().clear());
    ROOTS.with(|r| r.borrow_mut().clear());
    FRAMES.with(|f| f.borrow_mut().clear());
    FOCUS.with(|f| f.borrow_mut().clear());
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
    /// A scrollbar for a scrolled element: a track, drawn as the element's own background, with a
    /// thumb that follows the element's offset and can be pulled to set it.
    Scrollbar { id: String, axis: String },
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
    /// The platform colours the window's root asked to override, recorded as the frame
    /// that carried them was built. They are applied to the next frame's colours, because
    /// the colours are read at the *start* of a frame — before the view callback that
    /// describes the root — and a program's style string is only known once it has run.
    static THEME_OVERRIDES: RefCell<HashMap<i64, Vec<(String, gpui::Hsla)>>> =
        RefCell::new(HashMap::new());
    static DISPLAYS: RefCell<HashMap<i64, Vec<Display>>> = RefCell::new(HashMap::new());
    static KEYBOARD: RefCell<String> = const { RefCell::new(String::new()) };

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
    TEXT.with(|t| {
        t.borrow()
            .get(&win)
            .map(|m| match &m.clipboard {
                Clipboard::Text(text) => text.clone(),
                _ => String::new(),
            })
            .unwrap_or_default()
    })
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

/// The width `text` advances to, in pixels, as this window would draw it.
///
/// A newline is flattened into a space first, because gpui's shaper panics on
/// one — the same thing the editor does to everything that reaches it. Answers
/// `0.0` outside a frame, where there is no window's text style to measure
/// against.
pub fn text_width(text: &str, spec: &str) -> f64 {
    let Some((system, font, size, _)) = text_metrics(spec) else {
        return 0.0;
    };
    let text = text.replace('\n', " ");
    let run = gpui::TextRun {
        len: text.len(),
        font,
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = system.shape_line(text.into(), size, &[run], None);
    f32::from(line.width) as f64
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

thread_local! {
    /// Each window's should-close handler, if it has one.
    static SHOULD_CLOSE: RefCell<HashMap<i64, RetainedCallback>> = RefCell::new(HashMap::new());
    /// The windows whose handler has already been given to the platform. gpui's
    /// `on_window_should_close` replaces whatever was registered before it, so
    /// installing once per handler is enough and installing every frame would
    /// churn.
    static SHOULD_CLOSE_PLATFORM: RefCell<std::collections::HashSet<i64>> =
        RefCell::new(std::collections::HashSet::new());
}

/// Installs `handler` for the window this call is in.
pub fn set_should_close(handler: RetainedCallback) {
    let win = window::current();
    SHOULD_CLOSE.with(|h| {
        h.borrow_mut().insert(win, handler);
    });
    SHOULD_CLOSE_PLATFORM.with(|h| {
        h.borrow_mut().remove(&win);
    });
}

/// The handler for `win` if the platform has not been given it yet, marking it
/// as given.
pub fn take_should_close_for_platform(win: i64) -> Option<RetainedCallback> {
    let fresh = SHOULD_CLOSE_PLATFORM.with(|h| h.borrow_mut().insert(win));
    if !fresh {
        return None;
    }
    SHOULD_CLOSE.with(|h| h.borrow().get(&win).cloned())
}

thread_local! {
    /// The program's last-window-closed handler, if it installed one.
    static WINDOW_CLOSED: RefCell<Option<RetainedCallback>> = const { RefCell::new(None) };
}

/// Installs the handler that runs when the last window has closed.
pub fn set_window_closed(handler: RetainedCallback) {
    WINDOW_CLOSED.with(|h| *h.borrow_mut() = Some(handler));
}

/// Runs the program's last-window-closed handler, answering whether there was
/// one.
///
/// It runs in **window 0's namespace**, because there are no windows: whatever
/// state it reads and writes is the state a program has when it has no window,
/// which is the same namespace `gos test` runs in.
pub fn window_closed() -> bool {
    let handler = WINDOW_CLOSED.with(|h| h.borrow().clone());
    let Some(handler) = handler else {
        return false;
    };
    window::with(0, || {
        let _ = invoke(&handler, vec![]);
    });
    true
}

/// Asks an already-taken handler, for the platform's own close request.
pub fn ask_should_close(win: i64, handler: &RetainedCallback) -> bool {
    match window::with(win, || invoke(handler, vec![])) {
        Ok(Value::Bool(allow)) => allow,
        _ => true,
    }
}

/// Whether `win` may close: its handler's answer, or yes when it has none.
///
/// A handler that fails answers yes, which is what gpui does with its own
/// (`unwrap_or(true)`) — a broken guard must not trap a window forever.
pub fn may_close(win: i64) -> bool {
    let handler = SHOULD_CLOSE.with(|h| h.borrow().get(&win).cloned());
    let Some(handler) = handler else {
        return true;
    };
    match window::with(win, || invoke(&handler, vec![])) {
        Ok(Value::Bool(allow)) => allow,
        Ok(_) => true,
        Err(_) => true,
    }
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
/// declares no root behaving exactly as it did before: `img(PathBuf::from(path))` and
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
                Kind::Scrollbar { id, axis } => {
                    Kind::Scrollbar { id: id.clone(), axis: axis.clone() }
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
/// not only in the window's first two frames, which is what it used to do: a
/// program that turned the keyboard on at frame 40 got the listener and no focus,
/// and focus is what a key event needs to be delivered at all, so its shortcuts
/// were silently dead. A click into a text field still moves focus there and keeps
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
        .capture_key_down(move |event: &KeyDownEvent, _window, _cx| {
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
        // being held, and nothing used to listen for it at all.
        .capture_key_up(move |event: &KeyUpEvent, _window, _cx| {
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
                        win,
                        &el.id,
                        cx,
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
                        win,
                        &el.id,
                        cx,
                    );
                    let handle = scroll::handle(win, &el.id);
                    scrolled
                        .overflow_y_scroll()
                        .track_scroll(&handle)
                        .children(children)
                        .into_any_element()
                }
                // A plain box only becomes interactive when it asks to be, so
                // the common case keeps a stateless element. A `ctx=` counts as
                // asking: a key context lives on a dispatch-tree node, and a node
                // exists only for an element with an id.
                None if hover.is_some()
                    || active.is_some()
                    || focus.is_some()
                    || style::key_context(&el.style).is_some() =>
                {
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
            let focus_id = el.id.clone();
            let base = apply_interactions(base, &el.style);
            // A button is the element a person tabs to, and until round 60 it was the one element that
            // could not be focused: it never applied the focus tokens at all.
            let base = apply_focus_order(base, &el.style, win, &focus_id, cx);
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
        Kind::Image(path) => style::apply(img(PathBuf::from(asset_path(path))), &el.style)
            .id(ElementId::Name(el.id.clone().into()))
            .into_any_element(),

        Kind::Icon(path) => {
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
            // The list's own id, so the rows it builds can be recorded against
            // it while the closure has nothing else to reach for.
            let list_id = el.id.clone();
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
                        // The render has to run as this window too: `render`
                        // turns each child handle back into an element by
                        // reading the arena, and `arena()` keys on
                        // `window::current()`. A list is built during layout,
                        // after the frame that asked for it returned, so the
                        // window it was described in is no longer current —
                        // and without this the arena lookup misses and every
                        // container's children are dropped, leaving rows that
                        // paint their background and nothing else.
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
            // `on_mouse` reports for the element it was handed and adds no box
            // of its own, so whatever carries the handler has to *be* that
            // element. A bare `div()` wrapped around the receiver's render is
            // not: gpui makes every div a containing block (`Style::default`
            // is `Position::Relative`), so the wrapper became the containing
            // block of an out-of-flow receiver, and
            // `ui::rect("absolute right=4 top=0 w=16 h=full")` measured its
            // height against a wrapper that had none. The whole element, its
            // background and its children went to zero — which is what lost
            // 02_data_table's scrollbar strip and the thumb inside it — and
            // the box a click was tested against was that same empty wrapper.
            //
            // So a plain receiver is *folded*: its style and interactions
            // become the wrapper's, its children are rendered inside it, and a
            // child written after `.on_mouse(..)` is added beside them. That
            // is the receiver's own box in the receiver's own place, in flow
            // or out of it. (A receiver that scrolls would lose its handle
            // here; none does.)
            //
            // A text, button, image or list receiver keeps the older shape —
            // it renders itself and the handler goes on a wrapper around it.
            // No program chains a child after `.on_mouse` on one of those, so
            // that wrapper has never had to be the receiver's box.
            let receiver = snapshot(*child as i64);
            let folds = matches!(receiver.as_ref().map(|c| &c.kind), Some(Kind::Plain));
            let bounds_key = id.clone();
            let mut wrapper = match receiver {
                Some(receiver) if folds => {
                    let mut children: Vec<AnyElement> = Vec::new();
                    for handle in receiver.children.iter().chain(el.children.iter()) {
                        if let Some(child) = snapshot(*handle as i64) {
                            children.push(render(&child, window, cx, entity));
                        }
                    }
                    let base = style::apply(div(), &receiver.style)
                        .on_children_prepainted(move |bounds, _window, _cx| {
                            if let Some(first) = bounds.first() {
                                MOUSE_BOUNDS.with(|m| {
                                    m.borrow_mut().insert((win, bounds_key.clone()), *first)
                                });
                            }
                        })
                        .id(ElementId::Name(receiver.id.clone().into()));
                    let base = apply_interactions(base, &receiver.style);
                    apply_focus_order(base, &receiver.style, win, &receiver.id, cx)
                        .children(children)
                }
                receiver => {
                    let inner = receiver
                        .map(|mut c| {
                            c.children.extend(el.children.iter().copied());
                            window::with(win, || render(&c, window, cx, entity))
                        })
                        .unwrap_or_else(|| div().into_any_element());
                    style::apply(div(), &el.style)
                        .on_children_prepainted(move |bounds, _window, _cx| {
                            if let Some(first) = bounds.first() {
                                MOUSE_BOUNDS.with(|m| {
                                    m.borrow_mut().insert((win, bounds_key.clone()), *first)
                                });
                            }
                        })
                        .id(ElementId::Name(el.id.clone().into()))
                        .child(inner)
                }
            };

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

        Kind::Scrollbar { id, axis } => {
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
        }
    }
}

/// Applies the focus-order tokens to an already identified element.
fn apply_focus_order<T: StatefulInteractiveElement + 'static>(
    el: T,
    spec: &str,
    win: i64,
    id: &str,
    cx: &mut App,
) -> T {
    let mut el = el;
    for (key, value) in style::tokens(spec) {
        let v = value.as_deref().unwrap_or("");
        el = match key.as_str() {
            // **The binding's own handle, not gpui's.** `focusable()` only sets a flag on gpui's
            // internal interactivity, and gpui then makes a focus handle of its own — one
            // `gpui::is_focused` cannot see, because it reads back the handles *this* crate keeps.
            // Tracking our own handle is enough for gpui to use it: div.rs:1584 does exactly that
            // ("If there's an explicit focus handle we're tracking, use that. Otherwise create a new
            // handle"), so clicking still focuses the element and the tab map still gets an entry.
            //
            // **`focusable()` stays, and the handle is *added* to it** — round 65 thought it could
            // replace it, and round 69's probe disproved that: with the flag gone, `gpui::focus_next`
            // found no tab stop at all and `is_focused` still said no, so both halves of the row failed
            // together. gpui's own tab machinery keys on that flag (`div.rs:1584`), and the tracked
            // handle is what makes the element's focus *ours* rather than one gpui keeps to itself.
            "focusable" if v != "0" && v != "false" => el
                .focusable()
                .track_focus(&focus_for(win, id, cx)),
            "tabstop" => el.tab_stop(v != "0"),
            "tabindex" => el.tab_index(v.parse::<isize>().unwrap_or(0)),
            _ => el,
        };
    }
    el
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
