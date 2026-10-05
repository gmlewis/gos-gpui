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
//! * **The single-shaped-line invariant is enforced where text enters.** As
//!   upstream does for `paste`, a newline is flattened into a space by
//!   [`single_line`]; that is applied to adopted text and to every IME/input-
//!   handler replacement as well, because `TextSystem::shape_line` panics on
//!   `\n` in a debug build.
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
    AnyElement, App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId,
    InspectorElementId, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, Task, TextRun,
    UTF16Selection, UnderlineStyle, Window, div, fill, hsla, point, px, relative, rgba, size,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::store;
use crate::window;

/// How long the caret stays on (and off) while the editor is focused.
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);

thread_local! {
    /// One [`TextEditor`] per `ui::editor(key, ..)` call.
    ///
    /// The element tree is described from scratch every frame, so the entity —
    /// the thing that owns the text, the selection, the focus handle and the
    /// platform's input-handler registration — has to live outside it. Reusing
    /// the entity registered under `key` is what makes the editor's caret and
    /// selection survive from one frame to the next.
    static EDITORS: RefCell<HashMap<(i64, String), Entity<TextEditor>>> = RefCell::new(HashMap::new());
}

/// Renders the editor registered under `key`, creating it the first time.
///
/// Answers the [`TextEditor`] entity as an element: `Entity<V: Render>` is
/// itself an `IntoElement`, so GPUI draws the editor's own `render` tree.
pub fn editor_element(
    win: i64,
    key: &str,
    placeholder: &str,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (editor, created) = editor_for(win, key, placeholder, cx);

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

    editor.into_any_element()
}

/// The editor registered under `key`, creating and seeding it if this is the
/// first frame that asks for it. The flag answers whether it was created.
fn editor_for(win: i64, key: &str, placeholder: &str, cx: &mut App) -> (Entity<TextEditor>, bool) {
    let existing = EDITORS.with(|editors| editors.borrow().get(&(win, key.to_string())).cloned());
    if let Some(editor) = existing {
        let placeholder = SharedString::new(placeholder);
        editor.update(cx, |editor, _cx| {
            editor.placeholder = placeholder;
            editor.adopt_program_text();
        });
        return (editor, false);
    }

    let key = key.to_string();
    let content = window::with(win, || store::get_text(&key, ""));
    let placeholder = SharedString::new(placeholder);
    let registered = key.clone();
    let editor = cx.new(move |cx| TextEditor::new(win, key, placeholder, content, cx));
    EDITORS.with(|editors| {
        editors.borrow_mut().insert((win, registered), editor.clone())
    });
    (editor, true)
}

/// Upstream's editor shapes a single line, and `TextSystem::shape_line` panics
/// on a newline; upstream flattens one in `paste`, and the same is done to
/// everything else that reaches the content.
fn single_line(text: &str) -> String {
    if text.contains('\n') {
        text.replace('\n', " ")
    } else {
        text.to_string()
    }
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
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
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
        content: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let content = single_line(&content);
        // Upstream starts empty, so its caret starts at 0; a seeded editor
        // starts at the end of what it was seeded with.
        let caret = content.len();
        Self {
            win,
            key,
            focus_handle: cx.focus_handle(),
            content: content.into(),
            placeholder,
            selected_range: caret..caret,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
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

    fn home(&mut self, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, cx: &mut Context<Self>) {
        let end = self.content.len();
        self.move_to(end, cx);
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
            self.replace_text_in_range(None, &text.replace("\n", " "), window, cx);
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

    /// Upstream's `index_for_mouse_position`: a click above the line lands at
    /// the start, one below it at the end, and one inside it asks the shaped
    /// line which boundary is closest.
    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;

        if event.modifiers.shift {
            let offset = self.index_for_mouse_position(event.position);
            self.select_to(offset, cx);
        } else {
            let offset = self.index_for_mouse_position(event.position);
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
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let command = modifiers.platform || modifiers.control;
        let plain = !modifiers.shift && !modifiers.control && !modifiers.alt && !modifiers.platform;

        match keystroke.key.as_str() {
            "backspace" if plain => self.backspace(window, cx),
            "delete" if plain => self.delete(window, cx),
            "left" if modifiers.shift && !command && !modifiers.alt => self.select_left(cx),
            "right" if modifiers.shift && !command && !modifiers.alt => self.select_right(cx),
            "left" if plain => self.left(cx),
            "right" if plain => self.right(cx),
            "a" if command => self.select_all(cx),
            "c" if command => self.copy(cx),
            "x" if command => self.cut(window, cx),
            "v" if command => self.paste(window, cx),
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

        let text = single_line(&stored);
        if text != stored {
            // The program stored something the editor cannot hold — a newline,
            // in a single shaped line. Normalise it once rather than disagree
            // with the program on every frame.
            window::with(win, || store::set_text(&key, &text));
        }

        self.content = text.into();
        self.marked_range = None;
        // `last_layout` belongs to the old text: `character_index_for_point`
        // asserts the two agree, and the platform may ask before the next
        // paint rewrites it.
        self.last_layout = None;
        let len = self.content.len();
        let start = self.floor_boundary(self.selected_range.start.min(len));
        let end = self.floor_boundary(self.selected_range.end.min(len));
        self.selected_range = start.min(end)..start.max(end);
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
        let new_text = single_line(new_text);
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
        let new_text = single_line(new_text);
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
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
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
        let last_layout = self.last_layout.as_ref()?;

        assert_eq!(last_layout.text, self.content);
        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
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
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
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

        let (display_text, text_color) = if content.is_empty() {
            (placeholder, hsla(0., 0., 0., 0.2))
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    gpui::blue(),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0x3311ff30),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
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
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(bounds.origin, window.line_height(), window, cx)
            .unwrap();

        if focus_handle.is_focused(window)
            && cursor_visible
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
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
