//! A text editor owned by the binding.
//!
//! `EntityInputHandler`, `InputHandler` and `Window::handle_input` are trait
//! callbacks the platform invokes *after* the binding call has returned, so
//! they cannot cross a call-scoped `cb_fn` and a Gossamer program cannot
//! implement them. The editor therefore lives here, and Gossamer reaches it as
//! `ui::editor(key, placeholder, style)`.
//!
//! It is a port of GPUI's own `examples/input.rs` (746 lines, shipped verbatim
//! in `tools/upstream/`). [`TextEditor`] is upstream's `TextInput`,
//! [`EditorElement`] is its `TextElement`, and the `EntityInputHandler` impl,
//! the `ElementInputHandler` + `window.handle_input` registration inside the
//! canvas paint callback, the shaped-line prepaint with its selection and
//! cursor quads, the mouse-selection handling and the UTF-8/UTF-16 offset
//! arithmetic are reproduced as written.
//!
//! # Deviations from upstream
//!
//! * **Keys are handled on the element, not by the application keymap.**
//!   Upstream binds thirteen actions with `cx.bind_keys` and handles them with
//!   `on_action`. `app.rs` owns the one application keymap in this crate and
//!   binds only Quit, so [`TextEditor::render`] installs a single `on_key_down`
//!   handler that matches `event.keystroke.key` and the modifiers itself:
//!   backspace, delete, left, right, shift-left, shift-right, cmd/ctrl-a,
//!   cmd/ctrl-c, cmd/ctrl-x, cmd/ctrl-v, home, end, and ctrl-cmd-space for the
//!   character palette. The guard on each arm mirrors upstream's binding
//!   string — a plain `left` only fires with no modifier at all, `shift-left`
//!   only with Shift — and a handled key stops propagation so it cannot also
//!   reach the window-level listener `gpui::on_keys` installs.
//!   **This widget adds the ones a document needs and upstream's one-line
//!   example had no use for** — enter (a break, or `\n` when Shift is held),
//!   up, down, pageup, pagedown, their shift-extended twins, and
//!   `cmd-left`/`cmd-right` for the start and end of the document — all of them
//!   arithmetic from [`crate::text`]. They are listed here rather than left to
//!   the code because the count is now larger than upstream's thirteen, and a
//!   reader comparing the two needs to know which extra keys are deliberate.
//! * **The text is published to the program.** Every content change writes
//!   `store::set_text(key, ..)` and raises `<key>|change`, so the Gosamer side
//!   reads the editor with `gpui::state_text(key, "")`, exactly as `ui::input`
//!   does. Upstream has no such channel; its `InputExample` only observes
//!   keystrokes.
//! * **The program may drive the editor too.** When a frame asks for an editor
//!   whose stored text differs from its own content — because the program
//!   called `gpui::set_text`/`gpui::forget` — the editor adopts the stored text,
//!   drops any marked (IME) range and keeps the caret inside the new text.
//!   Upstream's `TextInput` is the sole owner of its content.
//! * **The caret blinks.** `examples/input.rs` paints a static caret; zed's
//!   real editor blinks it with a timer on the background executor, which is
//!   what happens here. The task lives in the entity and is dropped — and so
//!   cancelled — while the editor is unfocused, then restarted on the next
//!   render after it regains focus.
//! * **Chrome and type belong to the container.** Upstream's example hard-codes
//!   a 30px line height, a 24px text size, a grey backdrop and a white padded
//!   box. Here the line is shaped from the inherited `window.text_style()` and
//!   the editor paints no background of its own, so `ui::editor(key, ..,
//!   "bg=… p=…")` decides how it looks.
//! * **The editor is multi-line, and every line is shaped on its own.** Upstream's
//!   example is a single shaped line with a newline flattened into a space on the
//!   way in — `TextSystem::shape_line` panics on `\n` in a debug build, and one
//!   line has nowhere to put a break. Here the content may hold breaks, the
//!   layout shapes one `ShapedLine` per line of [`crate::text::lines_of`], and the
//!   element is as tall as its lines. The caret's up/down, page moves, home and
//!   end are the arithmetic of `text.rs`, which was built and checked for this row
//!   (`tests/dw_text_model_test.gos`) and is now what the widget actually calls,
//!   so a program asking `gpui::text_line_move` and the editor moving the caret
//!   cannot disagree. Pasting keeps its newlines; a click lands on the line it is
//!   over rather than in one long run.
//! * **Caret and selection offsets are clamped to the text.** `move_to` and
//!   `select_to` floor an offset to a char boundary and clamp it to the content
//!   length, which upstream does not do. It is a no-op for every offset the
//!   editor computes itself; it guards the one place a stale offset can arrive
//!   — `index_for_mouse_position` answers from the last shaped line, and a
//!   mouse event, unlike a key event, does not force a repaint first — where a
//!   caret parked past the end of just-shortened text would panic on the next
//!   insertion.
//! * **The editor takes focus when it is created.** Upstream's `InputExample`
//!   focuses its text input the moment the window opens, with a `Window` in
//!   hand; a binding function cannot, so the editor does it once for itself.
//!   Afterwards clicking, tabbing away and returning to it behave normally —
//!   it is a tab stop for `gpui::focus_next()` like any `track_focus` element.
//! * Upstream's `InputExample` shell — the keystroke log, the Reset button, the
//!   `cmd-q` binding, `cx.observe_keystrokes` and `cx.on_keyboard_layout_change`
//!   — is the example's, not the widget's, and cannot be reached from a binding
//!   function.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId,
    Hsla, InspectorElementId, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, Task, TextRun,
    UTF16Selection, UnderlineStyle, Window, div, fill, hsla, point, px, relative, rgba, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::history;
use crate::store;
use crate::style;
use crate::view;
use crate::window;

/// How long the caret stays on (and off) while the editor is focused.
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// How wide the line-number gutter is when `gutter` carries no number of its own.
///
/// Enough for three digits and the space either side of them. `gutter=48` asks for a wider one;
/// the number is pixels, because the gutter is measured in the same unit as the text it sits beside.
const DEFAULT_GUTTER_WIDTH: f32 = 34.0;

thread_local! {
    /// One [`TextEditor`] per `ui::editor(key, ..)` call.
    ///
    /// The element tree is described from scratch every frame, so the entity —
    /// the thing that owns the text, the selection, the focus handle and the
    /// platform's input-handler registration — has to live outside it. Reusing
    /// the entity registered under `key` is what makes the editor's caret and
    /// selection survive from one frame to the next.
    static EDITORS: RefCell<HashMap<(i64, String), Entity<TextEditor>>> = RefCell::new(HashMap::new());

    /// What each editor is holding, per `(window, key)`, written as each editor renders.
    ///
    /// A binding function is called between frames with no `App` and cannot read an entity, so this
    /// is where the widget's own content is left for [`editor_text`] to pick up. See its doc for why
    /// this reading exists beside the store's.
    static EDITOR_TEXT: RefCell<HashMap<(i64, String), String>> = RefCell::new(HashMap::new());

    /// What each editor is drawing its own chrome with, per `(window, key)`.
    ///
    /// The same reason as [`EDITOR_TEXT`] beside it: a binding function runs between frames with no
    /// `App` and cannot read an entity, so the widget leaves the chrome it resolved here for
    /// [`editor_chrome`] to pick up. It is written every frame, so a `theme:` value that changes with
    /// the platform's appearance is reported before a program can read a stale one.
    static EDITOR_CHROME: RefCell<HashMap<(i64, String), Chrome>> = RefCell::new(HashMap::new());

    /// What each editor has selected and where it was last drawn, per `(window, key)`.
    ///
    /// The same reason as the two above: a binding function cannot read an entity. This is what lets
    /// a program — and a check that drives a real mouse — ask what the editor has selected and where
    /// its box is on screen. The bounds are one frame behind, because they are the previous paint's.
    static EDITOR_STATE: RefCell<HashMap<(i64, String), (usize, usize, f64, f64, f64, f64)>> =
        RefCell::new(HashMap::new());
}

/// The text the editor on `key` in window `win` is holding, or `""` when that window has no editor
/// on that key.
///
/// **This is the widget's text; [`crate::store`]'s is the program's.** They agree once a frame has
/// passed in the editor's own window — the editor publishes every change and adopts every write — so
/// the difference is exactly one question, and it is the one this answers: *is this window's editor
/// mine, or is there none?* A window that draws no editor answers `""` however much text the program
/// has written under that key, because the text in the store is not being drawn by anything.
pub fn editor_text_in(win: i64, key: &str) -> String {
    EDITOR_TEXT.with(|texts| {
        texts
            .borrow()
            .get(&(win, key.to_string()))
            .cloned()
            .unwrap_or_default()
    })
}

/// The text this window's editor on `key` is holding.
pub fn editor_text(key: &str) -> String {
    editor_text_in(window::current(), key)
}

/// What the editor on `key` in window `win` is drawing its own chrome with, as `"<field>=<value>"`.
///
/// `field` is one of `caret`, `sel`, `gutter`, `gutterbg`, `gutterfg`, `wrap`; anything else answers
/// `""`, the same contract as [`crate::window::field`]. The colours are resolved to `#rrggbbaa` — a
/// `theme:` value is resolved against this window's table, not echoed — so a program can match its own
/// borders to the editor's caret without knowing which theme is in force.
///
/// This is the *effective* chrome, not the string the program wrote: the widget has already filled in
/// every value it did not set, and reading it back is how a program sees what a default actually is.
pub fn editor_chrome_in(win: i64, key: &str, field: &str) -> String {
    let chrome = EDITOR_CHROME.with(|all| {
        all.borrow()
            .get(&(win, key.to_string()))
            .cloned()
            .unwrap_or_default()
    });
    match field {
        "caret" => format!("{}", css(chrome.caret)),
        "sel" => format!("{}", css(chrome.selection)),
        "gutter" => format!("{}", f32::from(chrome.gutter).round() as i64),
        "gutterbg" => match chrome.gutter_bg {
            Some(colour) => format!("{}", css(colour)),
            None => "none".to_string(),
        },
        "gutterfg" => match chrome.gutter_fg {
            Some(colour) => format!("{}", css(colour)),
            None => "text".to_string(),
        },
        "wrap" => if chrome.softwrap { "soft".to_string() } else { "off".to_string() },
        _ => String::new(),
    }
}

/// What this window's editor on `key` is drawing its own chrome with.
pub fn editor_chrome(key: &str, field: &str) -> String {
    editor_chrome_in(window::current(), key, field)
}

/// What the editor on `key` in window `win` has selected, as `"<start>,<end>"` in bytes.
///
/// An empty selection — a caret with nothing highlighted — is `"<n>,<n>"`, which is the honest
/// reading rather than nothing: *something* is selected at every moment, and what is selected is
/// zero bytes of it. `""` means this window draws no editor on that key at all, the same contract as
/// [`editor_text_in`] and [`editor_chrome_in`].
///
/// The two offsets are always in order, so a selection dragged backwards reads the same as one
/// dragged forwards. Which end the caret is on is not reported: gpui draws the caret at the end the
/// mouse is at, and no reader of this crate has needed that yet.
pub fn editor_selection_in(win: i64, key: &str) -> String {
    editor_state(win, key).map_or(String::new(), |(start, end, ..)| format!("{start},{end}"))
}

/// What this window's editor on `key` has selected.
pub fn editor_selection(key: &str) -> String {
    editor_selection_in(window::current(), key)
}

/// Where the editor on `key` in window `win` was last drawn, in the window's own coordinates.
///
/// `field` is `x`, `y`, `w` or `h`; anything else answers `0.0`, the same contract as
/// [`crate::window::field`] and [`editor_chrome_in`]. These are the coordinates a pointer event
/// carries — the space `gpui::on_mouse` reports positions in — so a program that knows where its
/// editor is can aim something at it. A window drawing no editor on that key answers zeros, which is
/// the same reading the two readers above give: nothing is drawing it.
///
/// **One frame behind**, and only when the bounds changed: the box is recorded as the element is
/// built, before layout has run, so this is the rectangle of the *previous* paint.
pub fn editor_bounds_in(win: i64, key: &str, field: &str) -> f64 {
    let Some((_, _, x, y, w, h)) = editor_state(win, key) else {
        return 0.0;
    };
    match field {
        "x" => x,
        "y" => y,
        "w" => w,
        "h" => h,
        _ => 0.0,
    }
}

/// Where this window's editor on `key` was last drawn.
pub fn editor_bounds(key: &str, field: &str) -> f64 {
    editor_bounds_in(window::current(), key, field)
}

/// What the editor on `key` in window `win` left for the readers above to pick up.
fn editor_state(win: i64, key: &str) -> Option<(usize, usize, f64, f64, f64, f64)> {
    EDITOR_STATE.with(|all| all.borrow().get(&(win, key.to_string())).copied())
}

/// A colour as `#rrggbbaa`, the spelling [`crate::view::resolve_colour`] answers with.
fn css(colour: Hsla) -> String {
    let rgba = gpui::Rgba::from(colour);
    let byte = |channel: f32| (channel.clamp(0., 1.) * 255.).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}{:02x}",
        byte(rgba.r),
        byte(rgba.g),
        byte(rgba.b),
        byte(rgba.a)
    )
}

/// What an editor paints that is not a box: the caret, the selection highlight and the line-number
/// gutter.
///
/// The editor draws itself, and a box that draws itself cannot be reached by `bg=`/`color=` — those
/// colour the element the editor sits in, not the glyphs the editor puts on top of it. So these four
/// get a token vocabulary of their own, read from the same style string the element was given, and
/// every one of them defaults to the value upstream's `examples/input.rs` hard-codes: an editor that
/// sets nothing looks exactly as it did before this vocabulary existed.
///
/// Per `(window, key)` like the text itself: two windows may draw the same key in different colours.
#[derive(Clone)]
struct Chrome {
    /// The caret's colour.
    caret: Hsla,
    /// The selection highlight's colour.
    selection: Hsla,
    /// The width of the line-number gutter. Zero is no gutter, which is the default.
    gutter: Pixels,
    /// The gutter's own background, or `None` to let the editor's box show through.
    gutter_bg: Option<Hsla>,
    /// The line numbers' colour, or `None` for the text colour at a fraction of its alpha.
    gutter_fg: Option<Hsla>,
    /// Whether a line too long for the box wraps onto following rows instead of running past it.
    softwrap: bool,
}

impl Default for Chrome {
    fn default() -> Self {
        Self {
            caret: gpui::blue(),
            selection: rgba(0x3311ff30).into(),
            gutter: px(0.),
            gutter_bg: None,
            gutter_fg: None,
            softwrap: false,
        }
    }
}

impl Chrome {
    /// Reads the chrome tokens out of an editor's style string.
    ///
    /// A token that is absent keeps its default. `gutter` is a flag with an optional width:
    /// `gutter` alone is [`DEFAULT_GUTTER_WIDTH`] pixels, `gutter=48` is 48, and a value that is not
    /// a number is reported as an unknown token rather than quietly becoming no gutter.
    fn read(spec: &str) -> Self {
        let mut chrome = Chrome::default();
        if let Some(colour) = style::token_colour(spec, "caret") {
            chrome.caret = colour;
        }
        if let Some(colour) = style::token_colour(spec, "sel") {
            chrome.selection = colour;
        }
        if style::has_flag(spec, "gutter") {
            chrome.gutter = px(style::token_number(spec, "gutter").unwrap_or(DEFAULT_GUTTER_WIDTH));
        }
        chrome.gutter_bg = style::token_colour(spec, "gutterbg");
        chrome.gutter_fg = style::token_colour(spec, "gutterfg");
        chrome.softwrap = style::has_flag(spec, "softwrap");
        chrome
    }
}


/// The byte ranges a logical line is drawn on when its text has `width` to fit in.
///
/// One range per row. `None` — or a line that fits — answers the line's own range, so a caller that
/// always goes through here behaves identically whether wrapping is on or off; that is what lets the
/// widget keep one code path and keeps `DW1`'s caret arithmetic true in both modes.
///
/// The rule is greedy, and it is the one every editor uses: take the longest prefix that fits, back
/// up to the last space in it if there is one, and if even a single character does not fit then take
/// that character anyway — a row that cannot hold anything is a loop that never ends.
fn rows_for_line(
    measured: &ShapedLine,
    text: &str,
    line: (usize, usize),
    width: Option<Pixels>,
) -> Vec<(usize, usize)> {
    let (start, end) = line;
    let Some(width) = width else {
        return vec![line];
    };
    if width <= px(0.) || start >= end {
        return vec![line];
    }

    let mut rows = Vec::new();
    let mut at = start;
    while at < end {
        // `measured` is the *line*, shaped on its own, so it is indexed from the line's first
        // character — every offset into it is a document offset less `start`. Reading it with
        // document offsets measures the text from the wrong origin: the first row of a line is
        // right only because its `start` is zero, and every line after it wraps as if the box were
        // as narrow as the line is long. Measured, not reasoned: the capture that caught it showed
        // `one` broken into `on` and `e`.
        let from = measured.x_for_index(at - start);
        // The last character whose *far* edge still fits. A leading-edge test would let one glyph
        // hang over the box's edge, and `fit` never falls back to nothing: the first character goes
        // on the row even when it is too wide for it, so this always makes progress.
        let mut fit = at;
        for (offset, character) in text[at..end].char_indices() {
            // `measured` is indexed from the line's first character, so the offset into it is the
            // document offset less `start` — `offset` alone is right only on the row the line
            // begins on, which is every row when nothing wraps and wrong from the second row
            // onwards when something does.
            let far = measured.x_for_index(at - start + offset + character.len_utf8());
            if far - from <= width {
                fit = at + offset + character.len_utf8();
            } else if offset > 0 {
                break;
            } else {
                // Not even the first character fits; it goes on a row of its own.
                fit = at + character.len_utf8();
                break;
            }
        }
        let next = if fit >= end {
            // What is left of the line fits on this row, so it *is* the row. Backing up to the last
            // space here would cut a line that fits in two, which is the second thing the capture
            // caught: with `softwrap` on and a 406 px text area, a 376 px line came out as 361 px
            // and `in`. The word break is for the row that could not hold everything, not for
            // every row.
            end
        } else if fit > at {
            // Back up to the space that ends the last whole word of the row, when there is one and
            // it is not the row's first character — a long word with no space in it is broken
            // rather than pushed whole onto a row where it still would not fit.
            match text[at..fit].rfind(' ') {
                Some(space) if space > 0 => at + space + 1,
                _ => fit,
            }
        } else {
            at + text[at..].chars().next().map_or(1, char::len_utf8)
        };
        rows.push((at, next.min(end)));
        at = next.min(end);
    }
    rows
}

/// Which of `rows` the offset is on, and how far into it.
///
/// The row that *ends* at the offset wins over the one that begins there, which is what puts the
/// caret at the end of a wrapped row rather than at the start of the next one — the opposite of how
/// [`crate::text::index_of_line`] treats a break, and right for the same reason: a break belongs to
/// the line it closes, and a wrap belongs to the row it ends.
fn row_of(rows: &[(usize, usize)], offset: usize) -> (usize, usize) {
    for (index, (start, end)) in rows.iter().enumerate() {
        if offset <= *end || index + 1 == rows.len() {
            return (index, offset.saturating_sub(*start));
        }
    }
    (0, offset.saturating_sub(rows.first().map_or(0, |(start, _)| *start)))
}

/// The two offsets in a `"<start>,<end>"` range, as [`crate::text::word_at`] and
/// [`crate::text::line_bounds`] answer with.
///
/// A malformed range answers `0..0` rather than panicking: the only callers hand it a string this
/// crate has just built, so a panic here would be a claim about a value nothing can produce.
fn range_of(range: &str) -> (usize, usize) {
    let (start, end) = range.split_once(',').unwrap_or(("0", "0"));
    (start.parse().unwrap_or(0), end.parse().unwrap_or(0))
}

/// Renders the editor registered under `key`, creating it the first time.
///
/// Answers the [`TextEditor`] entity as an element: `Entity<V: Render>` is
/// itself an `IntoElement`, so GPUI draws the editor's own `render` tree.
///
/// `spec` is the element's style string, which is both applied to the box the editor sits in and
/// read for the editor's own chrome tokens (`caret=`, `sel=`, `gutter=`, `gutterbg=`, `gutterfg=`,
/// `softwrap`) — see [`Chrome`].
pub fn editor_element(
    win: i64,
    key: &str,
    placeholder: &str,
    spec: &str,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (editor, created) = editor_for(win, key, placeholder, spec, cx);

    // Upstream's `InputExample` focuses its text input the moment the window
    // opens (`window.focus(&view.text_input.focus_handle(cx))`). A binding
    // function cannot do that on the program's behalf, so the editor takes
    // focus once, when it is created; clicking, tabbing away and back all
    // behave normally afterwards.
    if created {
        let handle = editor.read(cx).focus_handle.clone();
        if !handle.is_focused(window) {
            window.focus(&handle);
        }
    }

    // Make the editor visible to `gpui::is_focused`/`gpui::focused`/`gpui::focus`, which read the
    // binding's own handle registry and cannot see a handle it did not build. The editor *has* a
    // handle — the one it tracks on its element — so this records that one rather than making a
    // second. Without it both docs lied: they name an `editor` among the elements focus reaches.
    let handle = editor.read(cx).focus_handle.clone();
    view::register_focus(win, key, handle);

    // Leave what the widget is holding where a binding function can read it. Every frame, because the
    // content can change under the editor as well as in it — a program's `set_text` is adopted on the
    // next frame — and a stale reading would be worse than none.
    let held = editor.read(cx).content.to_string();
    EDITOR_TEXT.with(|texts| {
        texts.borrow_mut().insert((win, key.to_string()), held);
    });

    // The chrome the widget resolved, for the same reason and on the same beat: a `theme:` value is
    // resolved against the window's table, which the platform can change under it, so this is
    // rewritten every frame rather than only when the style string changes.
    let chrome = editor.read(cx).chrome.clone();
    EDITOR_CHROME.with(|all| {
        all.borrow_mut().insert((win, key.to_string()), chrome);
    });

    // The selection and the box, on the same beat and for the same reason. The bounds are the ones
    // the last paint used — this runs before the element is laid out — so they lag a frame, which is
    // the same lag `index_for_mouse_position` already lives with.
    let state = {
        let editor = editor.read(cx);
        let rect = editor.last_bounds.map_or((0.0, 0.0, 0.0, 0.0), |bounds| {
            (
                f64::from(f32::from(bounds.origin.x)),
                f64::from(f32::from(bounds.origin.y)),
                f64::from(f32::from(bounds.size.width)),
                f64::from(f32::from(bounds.size.height)),
            )
        });
        (editor.selected_range.start, editor.selected_range.end, rect.0, rect.1, rect.2, rect.3)
    };
    EDITOR_STATE.with(|all| {
        all.borrow_mut().insert((win, key.to_string()), state);
    });

    editor.into_any_element()
}

/// The editor registered under `key`, creating and seeding it if this is the
/// first frame that asks for it. The flag answers whether it was created.
fn editor_for(
    win: i64,
    key: &str,
    placeholder: &str,
    spec: &str,
    cx: &mut App,
) -> (Entity<TextEditor>, bool) {
    let existing = EDITORS.with(|editors| editors.borrow().get(&(win, key.to_string())).cloned());
    if let Some(editor) = existing {
        let placeholder = SharedString::new(placeholder);
        let chrome = Chrome::read(spec);
        editor.update(cx, |editor, _cx| {
            editor.placeholder = placeholder;
            editor.chrome = chrome;
            editor.adopt_program_text();
        });
        return (editor, false);
    }

    let key = key.to_string();
    let content = window::with(win, || store::get_text(&key, ""));
    let placeholder = SharedString::new(placeholder);
    let chrome = Chrome::read(spec);
    let registered = key.clone();
    let editor = cx.new(move |cx| TextEditor::new(win, key, placeholder, chrome, content, cx));
    EDITORS.with(|editors| {
        editors.borrow_mut().insert((win, registered), editor.clone())
    });
    (editor, true)
}

// ---------------------------------------------------------------------------
// the editor entity
// ---------------------------------------------------------------------------

struct TextEditor {
    /// The window this editor belongs to. Its text is shared with the program
    /// under `key` *in that window*: two windows can each have an editor on
    /// the same key and not see each other.
    win: i64,
    /// The state key the text is shared under.
    key: String,
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    /// What the editor paints its own chrome with, refreshed from the element's style string every
    /// frame the element is drawn — so a program can change the caret's colour by changing the style
    /// string it passes, with no second call anywhere.
    chrome: Chrome,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// One shaped line per line of the content, in order.
    last_layout: Vec<ShapedLine>,
    /// The byte range of each of those lines, excluding its break — the same
    /// ranges [`crate::text::lines_of`] answers with, kept so a point can be
    /// turned into an offset without re-splitting the text.
    last_lines: Vec<(usize, usize)>,
    /// The byte range of each *drawn row*, which is a line when the editor does not wrap and a
    /// piece of one when it does. With wrapping off this is [`Self::last_lines`] and every offset
    /// arithmetic below behaves exactly as it did before wrapping existed.
    last_rows: Vec<(usize, usize)>,
    /// The height one line was laid out at, so a click can be turned into a line
    /// number without a window to ask.
    last_line_height: Pixels,
    /// How many lines the element had room for at the last paint: the size of a
    /// page move.
    last_page: i64,
    last_bounds: Option<Bounds<Pixels>>,
    /// The byte column a run of up/down is aiming for, kept while the caret is
    /// moving vertically and dropped by anything else. This is what makes a trip
    /// down through a short line and back land where it started.
    desired_column: Option<i64>,
    is_selecting: bool,
    /// Whether the editor held focus at the last render.
    focused: bool,
    /// The caret's blink phase; false while the caret is hidden.
    cursor_visible: bool,
    /// The blink loop. Dropping the task cancels it.
    blink_task: Option<Task<()>>,
}

impl TextEditor {
    fn new(
        win: i64,
        key: String,
        placeholder: SharedString,
        chrome: Chrome,
        content: String,
        cx: &mut Context<Self>,
    ) -> Self {
        // Upstream starts empty, so its caret starts at 0; a seeded editor
        // starts at the end of what it was seeded with.
        let caret = content.len();
        Self {
            win,
            key,
            focus_handle: cx.focus_handle(),
            content: content.into(),
            placeholder,
            chrome,
            selected_range: caret..caret,
            selection_reversed: false,
            marked_range: None,
            last_layout: Vec::new(),
            last_lines: Vec::new(),
            last_rows: Vec::new(),
            last_line_height: px(0.0),
            last_page: 1,
            last_bounds: None,
            desired_column: None,
            is_selecting: false,
            focused: false,
            cursor_visible: true,
            blink_task: None,
        }
    }

    // -- editing commands, ported from upstream's action handlers -----------

    fn left(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let offset = self.previous_boundary(self.cursor_offset());
            self.move_to(offset, cx);
        } else {
            let start = self.selected_range.start;
            self.move_to(start, cx)
        }
    }

    fn right(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let offset = self.next_boundary(self.selected_range.end);
            self.move_to(offset, cx);
        } else {
            let end = self.selected_range.end;
            self.move_to(end, cx)
        }
    }

    fn select_left(&mut self, cx: &mut Context<Self>) {
        let offset = self.previous_boundary(self.cursor_offset());
        self.select_to(offset, cx);
    }

    fn select_right(&mut self, cx: &mut Context<Self>) {
        let offset = self.next_boundary(self.cursor_offset());
        self.select_to(offset, cx);
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        let end = self.content.len();
        self.select_to(end, cx)
    }

    /// Home goes to the start of **the line the caret is on**, and end to its end.
    ///
    /// On a single-line editor these were the start and end of the document, and
    /// the two are the same thing there. They are not the same here, and the
    /// line-relative reading is the one every editor uses; the document-level
    /// moves are `cmd-left` and `cmd-right`, which is also what macOS does.
    fn home(&mut self, cx: &mut Context<Self>) {
        let offset = self.line_start(self.cursor_offset());
        self.move_to(offset, cx);
    }

    fn end(&mut self, cx: &mut Context<Self>) {
        let offset = self.line_end(self.cursor_offset());
        self.move_to(offset, cx);
    }

    fn select_home(&mut self, cx: &mut Context<Self>) {
        let offset = self.line_start(self.cursor_offset());
        self.select_to(offset, cx);
    }

    fn select_end(&mut self, cx: &mut Context<Self>) {
        let offset = self.line_end(self.cursor_offset());
        self.select_to(offset, cx);
    }

    /// The first byte of the line `offset` is on.
    fn line_start(&self, offset: usize) -> usize {
        crate::text::lines_of(&self.content)[crate::text::index_of_line(&self.content, offset)].0
    }

    /// The last byte of the line `offset` is on, **excluding** its break — so the
    /// caret stops before the newline rather than after it.
    fn line_end(&self, offset: usize) -> usize {
        crate::text::lines_of(&self.content)[crate::text::index_of_line(&self.content, offset)].1
    }

    /// How many bytes into its line the caret is. This is the column a vertical
    /// move aims to keep.
    fn column(&self) -> i64 {
        let cursor = self.cursor_offset();
        (cursor - self.line_start(cursor)) as i64
    }

    /// Moves the caret `lines` lines down (positive) or up (negative), keeping the
    /// column it started the run from.
    ///
    /// The arithmetic is [`crate::text::line_move`] — the same function a program
    /// calls as `gpui::text_line_move` — so the editor's caret and a program's
    /// own reading of where it would land are the same number by construction.
    /// The column is remembered across a run of moves and dropped by anything
    /// else, which is what makes the caret come back to where it left rather than
    /// creeping left along each short line it passes.
    fn move_line(&mut self, lines: i64, extend: bool, cx: &mut Context<Self>) {
        let column = self.desired_column.unwrap_or_else(|| self.column());
        let cursor = self.cursor_offset();
        let offset = crate::text::line_move(&self.content, cursor, lines, column) as usize;
        if extend {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
        // After the move, not before: `move_to`/`select_to` are the two places
        // that drop the column, and they are doing so because *they* were called
        // directly — this call is the one that means to keep it.
        self.desired_column = Some(column);
    }

    /// A page is however many lines the element had room for at its last paint,
    /// less one for continuity — the one-line overlap every editor leaves.
    fn move_page(&mut self, pages: i64, extend: bool, cx: &mut Context<Self>) {
        let step = (self.last_page - 1).max(1) * pages;
        self.move_line(step, extend, cx);
    }

    fn backspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let offset = self.previous_boundary(self.cursor_offset());
            self.select_to(offset, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let offset = self.next_boundary(self.cursor_offset());
            self.select_to(offset, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // Kept verbatim, breaks and all. Upstream flattens the newline here because its
            // editor has one line to put it on; this one has as many as the text needs.
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    /// Upstream's editor binds `ctrl-cmd-space` to `window.show_character_palette`
    /// as well; it is here so the macOS emoji picker still opens from the
    /// editor rather than from the app keymap.
    fn show_character_palette(&mut self, window: &mut Window) {
        window.show_character_palette();
    }

    // -- selection ----------------------------------------------------------

    /// Puts the caret at `offset`, floored to a char boundary and clamped to
    /// the text.
    ///
    /// The clamp is a no-op for every offset the editor computes itself: it is
    /// there because a mouse event does not force a repaint the way a key event
    /// does, so `index_for_mouse_position` can answer from the previous frame's
    /// shaped line — an offset that may sit past the end of text that has since
    /// been shortened.
    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.floor_boundary(offset);
        self.selected_range = offset..offset;
        // Anything that is not a vertical move drops the column it was aiming for:
        // a `right` after a `down` should go one character from where the caret
        // *is*, not back to the column the caret came down from.
        self.desired_column = None;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.floor_boundary(offset);
        self.desired_column = None;
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    /// Upstream's `index_for_mouse_position`, made aware that there are lines.
    ///
    /// A click above the text lands at the start and one below it at the end, as
    /// upstream does. Between those, the vertical position picks a line — by dividing
    /// by the height the lines were laid out at, which is the number the paint step
    /// remembered — and the horizontal position asks *that* line which boundary is
    /// closest. A click past the end of a line's text lands at the end of that line
    /// rather than at the end of the document, which is what a click in the margin
    /// beside a short line means.
    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        if self.last_layout.is_empty() {
            return 0;
        }

        let line_height = self.last_line_height;
        if line_height <= px(0.0) {
            return 0;
        }
        let rows = if self.last_rows.is_empty() {
            &self.last_lines
        } else {
            &self.last_rows
        };
        let count = self.last_layout.len();

        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }

        let row = ((position.y - bounds.top()) / line_height) as f32;
        let index = (row.floor() as i64).clamp(0, count as i64 - 1) as usize;
        let (start, end) = rows[index];
        // The text is drawn past the gutter, so the point is measured from the same edge it was
        // drawn from — otherwise every click lands a gutter's width to the right of where it aimed.
        let within =
            self.last_layout[index].closest_index_for_x(position.x - bounds.left() - self.chrome.gutter);
        (start + within).min(end)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;

        let offset = self.index_for_mouse_position(event.position);
        // The click count is the platform's, not the widget's: macOS counts clicks that land close
        // together in time and place and hands over the running total. Upstream's `TextInput`
        // ignores it, and an editor that ignores it cannot select a word or a line — so the two
        // counts above a single click are taken here, the word and the line coming from
        // [`crate::text`], which is the same arithmetic [`crate::text::word_at`] and
        // [`crate::text::line_bounds`] answer a program with.
        let range = match event.click_count {
            2 => Some(crate::text::word_at(&self.content, offset)),
            3 => Some(crate::text::line_bounds(&self.content, offset)),
            _ => None,
        };
        if let Some(range) = range {
            let (start, end) = range_of(&range);
            self.move_to(start, cx);
            self.select_to(end, cx);
        } else if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            let offset = self.index_for_mouse_position(event.position);
            self.select_to(offset, cx);
        }
    }

    // -- keys ---------------------------------------------------------------

    /// Stands in for upstream's thirteen `on_action` handlers.
    ///
    /// The guard on each arm reproduces the modifier set of upstream's binding
    /// string: `"left"` fires only with no modifier at all, `"shift-left"` only
    /// with Shift, and `"cmd-a"` on `cmd` (macOS) or `ctrl` elsewhere.
    /// Steps the document back one step.
    ///
    /// The text comes back through the store, which is the document both sides write to — so
    /// `adopt_program_text` picks it up on the next frame exactly as it picks up a program's own
    /// write. Nothing here touches `self.content` directly, because there is one writer.
    fn undo(&mut self, cx: &mut Context<Self>) {
        let (win, key) = (self.win, self.key.clone());
        if history::step(win, &key, false) {
            cx.notify();
        }
    }

    /// Steps it forward again, over anything that was undone.
    fn redo(&mut self, cx: &mut Context<Self>) {
        let (win, key) = (self.win, self.key.clone());
        if history::step(win, &key, true) {
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let command = modifiers.platform || modifiers.control;
        let plain = !modifiers.shift && !modifiers.control && !modifiers.alt && !modifiers.platform;

        match keystroke.key.as_str() {
            "backspace" if plain => self.backspace(window, cx),
            "delete" if plain => self.delete(window, cx),
            // A break, which is the one key a single-line editor could not honour and this one
            // exists to. It goes through the same replacement path as a typed character, so it is
            // published to the program and lands in the undo history like anything else.
            "enter" if plain || modifiers.shift => {
                self.replace_text_in_range(None, "\n", window, cx)
            }
            "left" if modifiers.shift && !command && !modifiers.alt => self.select_left(cx),
            "right" if modifiers.shift && !command && !modifiers.alt => self.select_right(cx),
            "left" if plain => self.left(cx),
            "right" if plain => self.right(cx),
            // The document-level moves, which `home`/`end` were before the editor grew lines.
            "left" if command => self.move_to(0, cx),
            "right" if command => {
                let end = self.content.len();
                self.move_to(end, cx)
            }
            "up" if modifiers.shift && !command && !modifiers.alt => self.move_line(-1, true, cx),
            "down" if modifiers.shift && !command && !modifiers.alt => self.move_line(1, true, cx),
            "up" if plain => self.move_line(-1, false, cx),
            "down" if plain => self.move_line(1, false, cx),
            "pageup" if modifiers.shift && !command && !modifiers.alt => self.move_page(-1, true, cx),
            "pagedown" if modifiers.shift && !command && !modifiers.alt => {
                self.move_page(1, true, cx)
            }
            "pageup" if plain => self.move_page(-1, false, cx),
            "pagedown" if plain => self.move_page(1, false, cx),
            "a" if command => self.select_all(cx),
            "c" if command => self.copy(cx),
            "x" if command => self.cut(window, cx),
            "v" if command => self.paste(window, cx),
            // The two the row is about: they used to fall through every arm below and reach
            // `gpui::on_keys`, so the editor had no undo at all.
            "z" if command && modifiers.shift => self.redo(cx),
            "z" if command => self.undo(cx),
            "home" if modifiers.shift && !command => self.select_home(cx),
            "end" if modifiers.shift && !command => self.select_end(cx),
            "home" if plain => self.home(cx),
            "end" if plain => self.end(cx),
            "space" if modifiers.platform && modifiers.control => {
                self.show_character_palette(window)
            }
            // Not one of the editor's keys: let it keep bubbling, so
            // `gpui::on_keys` still reports it.
            _ => return,
        }

        cx.stop_propagation();
    }

    // -- utf-8 / utf-16 -----------------------------------------------------

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    /// The largest char boundary at or below `offset`, so a caret can never
    /// split a character.
    fn floor_boundary(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.content.len());
        while !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    // -- state --------------------------------------------------------------

    /// Publishes new content to the program and asks for a repaint.
    fn changed(&mut self, cx: &mut Context<Self>) {
        // The content is not what it was, so a run of vertical moves is over: the next `down`
        // aims from where the caret *is* now, exactly as `move_to` says anything but a vertical
        // move drops the column. Every content change comes through here — a typed character, a
        // paste, a cut, a backspace — so this is the one place that has to say it.
        self.desired_column = None;
        // A key press arrives after the frame that built this element has been
        // painted, so the window has to be named again before the store is
        // touched.
        let (win, key, content) = (self.win, self.key.clone(), self.content.clone());
        window::with(win, || {
            store::set_text(&key, &content);
            store::push_event_with(&key, "change");
        });
        cx.notify();
    }

    /// Takes on text the program wrote under `key`.
    ///
    /// The editor is the owner of its content while it is being edited, so this
    /// only does anything when the stored text differs from what the editor
    /// last published — that is, when the program called `gpui::set_text`
    /// (or `gpui::forget`) behind it.
    fn adopt_program_text(&mut self) {
        let (win, key) = (self.win, self.key.clone());
        let stored = window::with(win, || store::get_text(&key, ""));
        if stored == self.content.as_ref() {
            return;
        }

        self.content = stored.into();
        self.marked_range = None;
        // The document is not the one the caret was moving through, so the column a run of
        // vertical moves was aiming for does not carry over — the same rule as `changed()`.
        self.desired_column = None;
        // The per-line layouts belong to the old text: `character_index_for_point`
        // asserts the two agree, and the platform may ask before the next paint
        // rewrites them.
        self.last_layout = Vec::new();
        self.last_lines = Vec::new();
        let len = self.content.len();
        let start = self.floor_boundary(self.selected_range.start.min(len));
        let end = self.floor_boundary(self.selected_range.end.min(len));
        self.selected_range = start.min(end)..start.max(end);
    }

    /// Which drawn row an offset is on, clamped into the layout that exists.
    ///
    /// With wrapping off this is the line the offset is on, because a row *is* a line; with it on it
    /// is the row of the line. Everything that has to name a row in `last_layout` goes through
    /// here, so the two modes cannot index it differently.
    fn row_index_for(&self, offset: usize) -> usize {
        if self.last_rows.is_empty() {
            return 0;
        }
        row_of(&self.last_rows, offset).0
    }

    /// Starts the caret blinking, as zed's own editor does.
    ///
    /// The task is stored on the entity, so dropping it cancels it; the entity
    /// holds nothing else that keeps it alive, which is why the loop also stops
    /// when the entity is released.
    fn start_blinking(&mut self, cx: &mut Context<Self>) {
        self.blink_task = Some(cx.spawn(async move |editor, cx| {
            loop {
                cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
                let alive = editor.update(cx, |editor, cx| {
                    if editor.focused {
                        editor.cursor_visible = !editor.cursor_visible;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }
}

impl EntityInputHandler for TextEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if self.last_layout.is_empty() {
            return None;
        }
        let range = self.range_from_utf16(&range_utf16);
        let last = self.last_layout.len() - 1;
        let first_row = self.row_index_for(range.start).min(last);
        let last_row = self.row_index_for(range.end).min(last);
        let line_height = self.last_line_height;

        // A range that spans a break gets a box over every row it touches — which is what
        // the platform wants, since this is where it puts an IME candidate window.
        let start_at = self.last_rows[first_row].0;
        let end_at = self.last_rows[last_row].0;
        // Past the gutter, like the glyphs themselves: an IME candidate window is placed against
        // this box, and a box that ignored the gutter would point at the line numbers.
        let text_left = bounds.left() + self.chrome.gutter;
        let left = text_left
            + self.last_layout[first_row]
                .x_for_index((range.start - start_at).min(self.last_layout[first_row].len()));
        let right = text_left
            + self.last_layout[last_row]
                .x_for_index((range.end - end_at).min(self.last_layout[last_row].len()));

        Some(Bounds::from_corners(
            point(left, bounds.top() + line_height * first_row as f32),
            point(
                right.max(left),
                bounds.top() + line_height * (last_row + 1) as f32,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        if self.last_layout.is_empty() {
            return None;
        }
        // The layout has to belong to the text: the platform can ask between a write and
        // the paint that lays the new text out, and an answer from the old lines would be
        // an offset into a document that is no longer there.
        assert_eq!(
            self.last_lines.len(),
            crate::text::lines_of(&self.content).len()
        );

        let line_height = self.last_line_height;
        let index = if line_height > px(0.0) {
            (((point.y - line_point.y) / line_height) as f32).floor() as i64
        } else {
            0
        };
        let index = index.clamp(0, self.last_layout.len() as i64 - 1) as usize;

        let rows = if self.last_rows.is_empty() {
            &self.last_lines
        } else {
            &self.last_rows
        };
        let (start, end) = rows[index];
        let within =
            self.last_layout[index].index_for_x(point.x - line_point.x - self.chrome.gutter)?;
        Some(self.offset_to_utf16((start + within).min(end)))
    }
}

impl Focusable for TextEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------------------
// the element that shapes, paints and registers the input handler
// ---------------------------------------------------------------------------

struct EditorElement {
    input: Entity<TextEditor>,
}

struct PrepaintState {
    /// One shaped line per line of the content, in order.
    lines: Vec<ShapedLine>,
    /// A highlight for every line the selection touches — several for a selection
    /// that spans lines, where the single-line editor had at most one.
    selection: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    /// The gutter's background, when one is asked for and given a colour of its own.
    gutter_background: Option<PaintQuad>,
    /// One shaped line number per line, paired with where it is painted. Empty when the editor
    /// draws no gutter, which is the default.
    gutter_numbers: Vec<(ShapedLine, Point<Pixels>)>,
    /// How much of the element's left edge the gutter takes. The text is drawn to the right of it,
    /// and a point is turned into an offset against the same edge — so the two cannot disagree.
    gutter_width: Pixels,
    /// The byte range of each drawn row, so the click arithmetic can be done after the fact without
    /// re-measuring the text — measurement needs a window and a mouse event does not force a repaint.
    rows: Vec<(usize, usize)>,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let (count, softwrap, gutter) = {
            let input = self.input.read(cx);
            (
                crate::text::lines_of(&input.content).len().max(1),
                input.chrome.softwrap,
                input.chrome.gutter,
            )
        };

        // As tall as its lines. The count comes from the same function the program's
        // `gpui::text_lines` answers from, so the box the editor asks for and the number a
        // program reads are one number.
        let mut style = Style::default();
        style.size.width = relative(1.).into();

        if !softwrap {
            style.size.height = (window.line_height() * count as f32).into();
            return (window.request_layout(style, [], cx), ());
        }

        // Wrapping makes the height a question about the *width*, and the width is not known until
        // layout runs — so this asks taffy the question the other way round: hand it a measure
        // callback and let it call back with the width it settled on. It is the same mechanism
        // gpui's own text element uses, and the same arithmetic `prepaint` uses, so the box the
        // element asks for is the box the rows fill.
        let input = self.input.clone();
        let layout = window.request_measured_layout(style, move |known, available, window, cx| {
            let (content, softwrap) = {
                let editor = input.read(cx);
                (editor.content.clone(), editor.chrome.softwrap)
            };
            let line_height = window.line_height();
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
            });
            if !softwrap {
                let count = crate::text::lines_of(&content).len().max(1);
                return size(px(0.), line_height * count as f32);
            }
            let Some(width) = width else {
                let count = crate::text::lines_of(&content).len().max(1);
                return size(px(0.), line_height * count as f32);
            };
            let text_width = (width - gutter).max(px(1.));
            let style = window.text_style();
            let font_size = style.font_size.to_pixels(window.rem_size());
            let mut rows = 0usize;
            for line in crate::text::lines_of(&content) {
                if line.0 == line.1 {
                    rows += 1;
                    continue;
                }
                let whole = content[line.0..line.1].to_string();
                let probe = window.text_system().shape_line(
                    SharedString::from(whole.clone()),
                    font_size,
                    &[TextRun {
                        len: whole.len(),
                        font: style.font(),
                        color: style.color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                rows += rows_for_line(&probe, &content, line, Some(text_width)).len();
            }
            size(px(0.), line_height * rows.max(1) as f32)
        });
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let marked_range = input.marked_range.clone();
        let placeholder = input.placeholder.clone();
        let style = window.text_style();

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let ranges = crate::text::lines_of(&content);
        let empty = content.is_empty();

        // The gutter takes the element's left edge and the text starts past it. `text_left` is the
        // one number every other part of this widget measures from — the lines, the selection, the
        // caret and the point-to-offset arithmetic — so a click lands where the glyphs are.
        let chrome = input.chrome.clone();
        let gutter_width = chrome.gutter;
        let text_left = bounds.left() + gutter_width;

        // The rows the text is drawn on. With wrapping off they are the lines themselves, one row
        // each; with it on, a line too long for the box is measured and cut into pieces that fit, and
        // the element is as tall as the pieces. Everything below — the highlight, the caret, the
        // gutter numbers and the click arithmetic — works in rows, so a wrapped line's second row is
        // a row like any other and no part of the widget has to know which mode it is in.
        let text_width = if chrome.softwrap {
            Some((bounds.size.width - gutter_width).max(px(1.)))
        } else {
            None
        };
        let mut rows: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
        // The row each logical line begins on, so a gutter number stands against the line it
        // numbers rather than against a wrapped continuation of the line above it.
        let mut line_rows: Vec<usize> = Vec::with_capacity(ranges.len());
        for line in &ranges {
            line_rows.push(rows.len());
            if text_width.is_none() || line.0 == line.1 {
                rows.push(*line);
                continue;
            }
            // Measured unwrapped first: the wrap points are read off the line's own advances, so
            // the same glyphs decide where it breaks and where the caret sits inside it.
            let whole = content[line.0..line.1].to_string();
            let probe = window.text_system().shape_line(
                SharedString::from(whole.clone()),
                font_size,
                &[TextRun {
                    len: whole.len(),
                    font: style.font(),
                    color: style.color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            rows.extend(rows_for_line(&probe, &content, *line, text_width));
        }

        let mut lines = Vec::with_capacity(rows.len());
        for (start, end) in &rows {
            // The placeholder is what an *empty document* shows, not what an empty line
            // shows: a blank line in the middle of a text is a blank line, and drawing
            // "type here" on it would be a lie about what the editor holds.
            let (display, text_color) = if empty {
                (placeholder.clone(), hsla(0., 0., 0., 0.2))
            } else {
                (
                    SharedString::from(content[*start..*end].to_string()),
                    style.color,
                )
            };

            let run = TextRun {
                len: display.len(),
                font: style.font(),
                color: text_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            // The marked (IME) range is clipped to this line, and only the piece on this
            // line is underlined — a composition that spans a break would otherwise mark
            // the whole rest of the document.
            let runs = match marked_range.as_ref() {
                Some(marked) => {
                    let marked_start = marked.start.clamp(*start, *end) - *start;
                    let marked_end = marked.end.clamp(*start, *end) - *start;
                    vec![
                        TextRun {
                            len: marked_start,
                            ..run.clone()
                        },
                        TextRun {
                            len: marked_end - marked_start,
                            underline: Some(UnderlineStyle {
                                color: Some(run.color),
                                thickness: px(1.0),
                                wavy: false,
                            }),
                            ..run.clone()
                        },
                        TextRun {
                            len: display.len() - marked_end,
                            ..run
                        },
                    ]
                    .into_iter()
                    .filter(|run| run.len > 0)
                    .collect()
                }
                None => vec![run],
            };

            lines.push(
                window
                    .text_system()
                    .shape_line(display, font_size, &runs, None),
            );
        }

        // The highlight, row by row. A row is touched when the selection reaches into
        // it, and the part it covers is the selection clipped to the row — plus, for a
        // selection that runs past this row's end, everything up to where the next row
        // starts, so a multi-line selection is one solid block rather than a ragged one.
        let mut selection = Vec::new();
        if !selected_range.is_empty() {
            for (index, (start, end)) in rows.iter().enumerate() {
                // Where this line ends *including* its break, which is one byte past
                // `end` when a break follows.
                let line_end = if *end < content.len() { *end + 1 } else { *end };
                if selected_range.start >= line_end || selected_range.end <= *start {
                    continue;
                }
                let from = selected_range.start.clamp(*start, *end) - *start;
                let to = selected_range.end.clamp(*start, *end) - *start;
                let top = bounds.top() + line_height * index as f32;
                // A selection that starts before this line reaches back to its left edge,
                // and one that runs past it reaches out to the right edge: the highlight
                // covers the whole of the lines it passes through.
                let left = if selected_range.start < *start {
                    text_left
                } else {
                    text_left + lines[index].x_for_index(from)
                };
                let right = if selected_range.end > *end {
                    bounds.right()
                } else {
                    text_left + lines[index].x_for_index(to)
                };
                selection.push(fill(
                    Bounds::from_corners(
                        point(left, top),
                        point(right.max(left), top + line_height),
                    ),
                    chrome.selection,
                ));
            }
        }

        // The caret, on the row it is on — clamped to a row that exists, because a
        // caret one byte past a final break belongs on the empty row after it.
        let cursor_quad = {
            let (index, within) = row_of(&rows, cursor);
            let index = index.min(lines.len().saturating_sub(1));
            let within = within.min(lines[index].len());
            Some(fill(
                Bounds::new(
                    point(
                        text_left + lines[index].x_for_index(within),
                        bounds.top() + line_height * index as f32,
                    ),
                    size(px(2.), line_height),
                ),
                chrome.caret,
            ))
        };

        // The gutter, when the element asked for one. It is the numbers on the lines the text is
        // on, one per line, right-aligned against a margin — the arithmetic is `lines_of`'s, the
        // same list the text is shaped from, so a line and its number cannot come apart.
        let (gutter_background, gutter_numbers) = if gutter_width > px(0.) {
            let background = chrome.gutter_bg.map(|colour| {
                fill(
                    Bounds::from_corners(
                        bounds.origin,
                        point(bounds.left() + gutter_width, bounds.bottom()),
                    ),
                    colour,
                )
            });
            let number_style = window.text_style();
            let number_colour = chrome.gutter_fg.unwrap_or(hsla(
                number_style.color.h,
                number_style.color.s,
                number_style.color.l,
                number_style.color.a * 0.4,
            ));
            // Room for the digits against the right edge of the gutter, so the numbers form a
            // column rather than a ragged edge: the widest one decides how far in they all sit.
            let widest = ranges.len().to_string().len();
            let mut numbers = Vec::with_capacity(ranges.len());
            for (index, row) in line_rows.iter().enumerate() {
                let digits = SharedString::from(format!("{:>width$}", index + 1, width = widest));
                let run = TextRun {
                    len: digits.len(),
                    font: number_style.font(),
                    color: number_colour,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = window
                    .text_system()
                    .shape_line(digits, font_size, &[run], None);
                let inset = px(6.);
                let x = bounds.left() + gutter_width - inset - shaped.width;
                numbers.push((shaped, point(x, bounds.top() + line_height * *row as f32)));
            }
            (background, numbers)
        } else {
            (None, Vec::new())
        };

        PrepaintState {
            lines,
            selection,
            cursor: cursor_quad,
            gutter_background,
            gutter_numbers,
            gutter_width,
            rows,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (focus_handle, cursor_visible) = {
            let input = self.input.read(cx);
            (input.focus_handle.clone(), input.cursor_visible)
        };
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad)
        }
        // The gutter goes under the text: its background first, then the numbers, so a line
        // scrolled against it cannot be covered by the rule.
        if let Some(background) = prepaint.gutter_background.take() {
            window.paint_quad(background);
        }
        let line_height = window.line_height();
        for (number, origin) in prepaint.gutter_numbers.drain(..) {
            number.paint(origin, line_height, window, cx).unwrap();
        }
        let rows = prepaint.rows.clone();
        let text_left = bounds.left() + prepaint.gutter_width;
        let mut painted = Vec::with_capacity(prepaint.lines.len());
        for (index, line) in prepaint.lines.drain(..).enumerate() {
            line.paint(
                point(text_left, bounds.top() + line_height * index as f32),
                line_height,
                window,
                cx,
            )
            .unwrap();
            painted.push(line);
        }

        if focus_handle.is_focused(window)
            && cursor_visible
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        // The page size a `pageup`/`pagedown` moves by: how many *rows* this element
        // actually had room for, which is its lines when it does not wrap.
        let page = (bounds.size.height / line_height).floor().max(1.0) as i64;
        let ranges = {
            let input = self.input.read(cx);
            crate::text::lines_of(&input.content)
        };
        self.input.update(cx, |input, _cx| {
            input.last_layout = painted;
            input.last_lines = ranges;
            input.last_rows = rows;
            input.last_line_height = line_height;
            input.last_page = page;
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        if focused != self.focused {
            self.focused = focused;
            self.cursor_visible = true;
            // Dropping the task cancels it, so the caret stops blinking the
            // moment the editor loses focus and starts again when it comes
            // back.
            self.blink_task = None;
        }
        if self.focused && self.blink_task.is_none() {
            self.start_blinking(cx);
        }

        let element = EditorElement { input: cx.entity() };
        div()
            .id(ElementId::Name(format!("ui-editor-{}", self.key).into()))
            .flex()
            .w_full()
            .key_context("TextEditor")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(element)
    }
}
