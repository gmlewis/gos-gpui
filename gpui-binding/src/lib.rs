//! `gpui` — Zed's GPU-accelerated UI framework, exposed to Gossamer.
//!
//! `register_module!` walks its function list with a tt-muncher, so the crate
//! needs a deeper macro recursion limit than the default once the surface
//! passes roughly sixty bindings.
#![recursion_limit = "512"]

//!
//! A Gossamer program describes its window once per frame, and the view
//! callback *answers* the element it wants as the root:
//!
//! ```gossamer
//! use gpui
//! use ui
//!
//! fn main() {
//!     gpui::run("Counter", 420.0, 260.0, || {
//!         for event in gpui::take_events() {
//!             let id, _data = gpui::event_parts(event)
//!             if id == "inc" {
//!                 gpui::set_int("count", gpui::state_int("count", 0) + 1)
//!             }
//!         }
//!         ui::column("gap=10 p=20 bg=#181825 w=full h=full")
//!             .text(f"count = {gpui::state_int(\"count\", 0)}", "font=2xl color=#cdd6f4")
//!             .button("inc", "+1", "px=14 py=8 rounded=8 bg=#89b4fa hover=bg:#a6e3fa")
//!     })
//! }
//! ```
//!
//! `run` blocks for the life of the program: GPUI calls the view callback once
//! per frame, and the callback stays valid throughout, which is exactly the
//! lifetime a Gossamer callback has.
//!
//! Each constructor here answers an `i64` *handle* into a per-frame arena.
//! Handles are plain integers in Gossamer, so the `.child(..)` chaining that
//! makes these examples read well lives on the Gossamer side, in `ui`, built
//! on `gpui::child`.

// Imports must live outside the `register_module!` block: the macro lifts the
// binding bodies into a generated module that reaches the surrounding scope
// with `use super::*;`.
use gossamer_binding::PersistentCallback;

// Window and display geometry is returned as scalars rather than a struct.
// A binding-returned `GosStruct` is an opaque type to the compiled tier, which
// lowers a field read only when the *program* declares a matching Gossamer
// struct — so `window_rect().x` would fail to lower in a program that did not
// happen to redeclare `Rect`. Scalars cross every tier unchanged.

mod app;
mod editor;
mod list;
mod path;
mod scroll;
mod store;
mod style;
mod view;
mod window;

use view::Kind;

gossamer_binding::register_module!(
    name: gpui,
    doc: "Zed's gpui UI framework: windows, flexbox layout, styling, interaction, animation and custom painting.",

    // -----------------------------------------------------------------
    // running
    // -----------------------------------------------------------------

    /// Open a window and run the event loop until the program quits.
    ///
    /// `view` is called once per frame and must *answer* the window's root
    /// element — end it with `ui::column(..)` or another element, not with a
    /// bare statement.
    cb_fn run(d, title: String, width: f64, height: f64, view: PersistentCallback) -> Result<(), String> {
        // Retained: the view is invoked once per frame, for the whole life of
        // the app, which outlasts the call that handed it over.
        app::run_app(d, title, width, height, "", view.retain())
    }

    /// `run`, with window options: `"resizable=false titlebar=none float=true background=blur"`.
    cb_fn run_with(d, title: String, width: f64, height: f64, options: String, view: PersistentCallback) -> Result<(), String> {
        app::run_app(d, title, width, height, &options, view.retain())
    }

    /// Close the window and end the program.
    fn quit() {
        store::request(store::Request::Quit)
    }

    /// Keep painting: ask for another frame as soon as this one is on screen.
    fn animate() {
        store::request(store::Request::Animate)
    }

    /// Raise the event `id` every `ms` milliseconds.
    fn tick(id: String, ms: i64) {
        store::request(store::Request::Tick { id, ms: ms.max(1) as u64 })
    }

    /// Do slow work off the UI thread, then raise the event `id` and store a
    /// human-readable summary under `<id>.result`.
    fn task(id: String, ms: i64) {
        store::request(store::Request::Task { id, ms: ms.max(1) as u64 })
    }

    /// The number of frames this window has painted.
    fn frame() -> i64 {
        view::frame_number() as i64
    }

    /// The window whose frame — or whose callback — is running.
    ///
    /// Non-zero, and the same for every frame of a given window and every
    /// callback it built. Window state, events and requests are all keyed by
    /// it: `gpui::window_id()` inside a view callback names the window that
    /// callback is drawing, and inside a click handler the window the click
    /// happened in. `0` is "no window", which is what a program running
    /// outside a frame — `gos test` — sees.
    fn window_id() -> i64 {
        window::current()
    }

    /// A state key, addressed to a window that is not the current one.
    ///
    /// Every accessor below has an `_in` twin: `state_int(key, d)` reads the
    /// current window, `state_int_in(win, key, d)` reads the window named. A
    /// helper that serves several windows at once wants the second form;
    /// everything else wants the first.
    fn state_int_in(win: i64, key: String, default: i64) -> i64 {
        store::get_int_in(win, &key, default)
    }

    /// Write an integer to another window's state.
    fn set_int_in(win: i64, key: String, value: i64) {
        store::set_int_in(win, &key, value)
    }

    /// Read a float from another window's state.
    fn state_float_in(win: i64, key: String, default: f64) -> f64 {
        store::get_float_in(win, &key, default)
    }

    /// Write a float to another window's state.
    fn set_float_in(win: i64, key: String, value: f64) {
        store::set_float_in(win, &key, value)
    }

    /// Read a flag from another window's state.
    fn state_bool_in(win: i64, key: String, default: bool) -> bool {
        store::get_flag_in(win, &key, default)
    }

    /// Write a flag to another window's state.
    fn set_bool_in(win: i64, key: String, value: bool) {
        store::set_flag_in(win, &key, value)
    }

    /// Read a string from another window's state.
    fn state_text_in(win: i64, key: String, default: String) -> String {
        store::get_text_in(win, &key, &default)
    }

    /// Write a string to another window's state.
    fn set_text_in(win: i64, key: String, value: String) {
        store::set_text_in(win, &key, &value)
    }

    /// Forget one key in another window's state.
    fn forget_in(win: i64, key: String) {
        store::forget_in(win, &key)
    }

    /// Seconds since the program started, as a float — handy for animation.
    fn now() -> f64 {
        view::seconds_since_start()
    }

    // -----------------------------------------------------------------
    // events and state
    // -----------------------------------------------------------------

    /// Every event raised since the previous frame, drained.
    ///
    /// An entry is `"id"` or `"id|payload"`; use [`gpui::event_parts`] to split
    /// one, or [`gpui::event_id`] when only the id is wanted.
    fn take_events() -> Vec<String> {
        store::take_events()
    }

    /// The event's id: everything before the first `|`.
    ///
    /// Consumes the event, so reach for [`gpui::event_parts`] when both halves
    /// are wanted.
    fn event_id(event: String) -> String {
        match event.split_once('|') {
            Some((id, _)) => id.to_string(),
            None => event,
        }
    }

    /// Raise an event from inside a frame, as `"<id>|<payload>"`.
    ///
    /// Gossamer has no observable-entity layer to emit through; this is the
    /// binding's equivalent, and it is what a port uses where upstream would
    /// call `cx.emit(..)`.
    fn emit(id: String, payload: String) {
        store::push_event_with(&id, payload)
    }

    /// Split an event into `(id, payload)` in one step.
    fn event_parts(event: String) -> (String, String) {
        match event.split_once('|') {
            Some((id, data)) => (id.to_string(), data.to_string()),
            None => (event, String::new()),
        }
    }

    /// Read an integer from the app state.
    fn state_int(key: String, default: i64) -> i64 {
        store::get_int(&key, default)
    }

    /// Write an integer to the app state.
    fn set_int(key: String, value: i64) {
        store::set_int(&key, value)
    }

    /// Read a float from the app state.
    fn state_float(key: String, default: f64) -> f64 {
        store::get_float(&key, default)
    }

    /// Write a float to the app state.
    fn set_float(key: String, value: f64) {
        store::set_float(&key, value)
    }

    /// Read a flag from the app state.
    fn state_bool(key: String, default: bool) -> bool {
        store::get_flag(&key, default)
    }

    /// Write a flag to the app state.
    fn set_bool(key: String, value: bool) {
        store::set_flag(&key, value)
    }

    /// Read a string from the app state.
    fn state_text(key: String, default: String) -> String {
        store::get_text(&key, &default)
    }

    /// Write a string to the app state.
    fn set_text(key: String, value: String) {
        store::set_text(&key, &value)
    }

    /// Forget one key, whatever type it holds.
    fn forget(key: String) {
        store::forget(&key)
    }

    // ---- list-valued state ------------------------------------------

    /// Append to the list stored under `key`.
    fn list_push(key: String, value: String) {
        store::push_text(&key, &value)
    }

    /// How many items the list under `key` holds.
    fn list_len(key: String) -> i64 {
        store::text_len(&key)
    }

    /// The item at `index`, or `""`.
    fn list_get(key: String, index: i64) -> String {
        store::text_at(&key, index)
    }

    /// Replace the item at `index`.
    fn list_set(key: String, index: i64, value: String) {
        store::text_remove(&key, index);
        store::text_insert(&key, index, &value)
    }

    /// Remove the item at `index`.
    fn list_remove(key: String, index: i64) {
        store::text_remove(&key, index)
    }

    /// Insert `value` at `index`.
    fn list_insert(key: String, index: i64, value: String) {
        store::text_insert(&key, index, &value)
    }

    /// The whole list, as a copy.
    fn list_all(key: String) -> Vec<String> {
        store::get_texts(&key)
    }

    /// Replace the whole list.
    fn list_replace(key: String, values: Vec<String>) {
        store::set_texts(&key, values)
    }

    /// Empty the list.
    fn list_clear(key: String) {
        store::set_texts(&key, Vec::new())
    }

    // -----------------------------------------------------------------
    // containers
    // -----------------------------------------------------------------

    /// A column container.
    fn column(style: String) -> i64 {
        view::push(Kind::Plain, format!("flex col {}", style), String::new())
    }

    /// A row container.
    fn row(style: String) -> i64 {
        view::push(Kind::Plain, format!("flex row {}", style), String::new())
    }

    /// A container with no flex direction of its own.
    fn panel(style: String) -> i64 {
        view::push(Kind::Plain, style, String::new())
    }

    /// A container whose children stack on top of one another; position them
    /// with `absolute top=… left=…`.
    fn stack(style: String) -> i64 {
        view::push(Kind::Plain, format!("relative {}", style), String::new())
    }

    /// A vertically scrolling area.
    fn scroll(style: String) -> i64 {
        view::push(Kind::Plain, format!("flex col overflow=y {}", style), String::new())
    }

    // -----------------------------------------------------------------
    // measuring text
    // -----------------------------------------------------------------

    /// How wide `text` is, in pixels, as this window would draw it.
    ///
    /// The measurement a layout needs and a style string cannot give: "will this
    /// label fit?", "how tall is this paragraph?", "where does the caret go?".
    ///
    /// `style` is the text half of the style language — `fs=17`, `weight=600`,
    /// `bold`, `semibold`, `italic`. The **family and the size come from the
    /// window's inherited text style**, so a container styled `font=2xl` is
    /// inherited by everything measured inside it; `fs=` overrides the size, and
    /// nothing else in `style` is read. A newline is flattened into a space first,
    /// because gpui's shaper panics on one.
    fn text_width(text: String, style: String) -> f64 {
        view::text_width(&text, &style)
    }

    /// The em width of the font `style` selects, in pixels — the width of a
    /// capital `M`'s box, and the unit a `1em` length means.
    fn text_em_width(style: String) -> f64 {
        view::text_em_width(&style)
    }

    /// How far the first character of `ch`, in pixels.
    ///
    /// Per character, so a program can lay out a monospaced column from the
    /// widest digit rather than from a guess. Answers `0.0` for an empty string.
    fn text_advance(ch: String, style: String) -> f64 {
        view::text_advance(&ch, &style)
    }

    /// The `gpui::HighlightStyle` properties a highlight's property half sets,
    /// as a comma-separated list in a fixed order: `color`, `background`,
    /// `weight`, `style`, `underline`, `strikethrough`, `fade`.
    ///
    /// The reader for a spec — check what a generated highlight actually says
    /// rather than discovering it on screen. `""` for a property string that sets
    /// none of them, and an unknown token sets nothing, so a typo reads as a
    /// short answer rather than as an error.
    fn highlight_properties(props: String) -> String {
        view::highlight_properties(&props)
    }

    /// The style tokens in `style` this crate does not understand, in the order
    /// they appear.
    ///
    /// The reader behind the `unknown style tokens:` banner. A program that
    /// builds a style string from data — from a theme, a config file, a computed
    /// size — can check it instead of seeing it drawn as an error card, and a
    /// test can assert that a spelling the documentation promises is really one
    /// the binding knows.
    fn unknown_style_tokens(style: String) -> Vec<String> {
        view::unknown_style_tokens(&style)
    }

    /// Every font family this machine offers, as the platform names them.
    ///
    /// The list to choose a `font=<family>` from. Sorted as the platform gives
    /// it; a family in it can be selected, and one that is not falls back to the
    /// default without complaint — so a program that cares asks this first.
    fn font_families() -> Vec<String> {
        view::font_families()
    }

    /// The window's line height, in pixels, as of the start of this frame.
    ///
    /// The distance from one line's baseline to the next, which is what a
    /// multi-line layout advances by — not the font size. `lh=` scales it per
    /// element; this reads what the window's inherited text style resolves to,
    /// and it grows when [`gpui::set_rem_size`] grows.
    fn line_height() -> f64 {
        view::line_height()
    }

    /// Set the window's rem size, in pixels: what every `rem` length and every
    /// ramp size (`font=2xl`) is measured against.
    ///
    /// Applied after the frame that asks for it, so the frame after next reads
    /// the new size. gpui's default is 16.
    fn set_rem_size(px: f64) {
        store::request(store::Request::SetRemSize { px })
    }

    /// The height one line occupies: `lh=` or `lhpx=` when the style names one,
    /// and the font's own ascent plus descent when it does not.
    ///
    /// A relative `lh=` is resolved against the size the style ends up asking
    /// for, so `fs=40 lh=2` is 80 pixels.
    fn text_line_height(style: String) -> f64 {
        view::text_line_height(&style)
    }

    // -----------------------------------------------------------------
    // scrolling
    // -----------------------------------------------------------------

    /// How far the scrolled container `id` has scrolled, in pixels.
    ///
    /// `axis` is `"y"` (the default) or `"x"`. The sign is gpui's own: **zero at
    /// the top, negative as the content scrolls down**. Put a scrollbar thumb at
    /// `-gpui::scroll_offset(id, "y") / gpui::scroll_max_offset(id, "y")`.
    ///
    /// `id` is a container that scrolls — `gpui::scroll("id=panel overflow=y")`,
    /// or any element whose style carries `overflow=x|y|scroll` — named either by
    /// an `id=` token in that style or by the argument of the leaf that takes one.
    /// An id nothing has drawn a scrolled container under answers `0.0`; so does a
    /// container still at the top, which is why [`gpui::scroll_max_offset`] is how
    /// to tell the two apart.
    fn scroll_offset(id: String, axis: String) -> f64 {
        view::scroll_offset(&id, &axis)
    }

    /// How much content the scrolled container `id` has to scroll past: the
    /// content size minus the container's, in pixels, never negative.
    ///
    /// The positive counterpart of [`gpui::scroll_offset`]. Zero means either that
    /// the content fits — there is nothing to scroll — or that no scrolled
    /// container has been drawn under that id.
    fn scroll_max_offset(id: String, axis: String) -> f64 {
        view::scroll_max_offset(&id, &axis)
    }

    /// The scrolled element `id`'s viewport, in window pixels. `field` is `x`,
    /// `y`, `w` or `h`.
    ///
    /// The container's own box, not its content's: `h` is how much of the
    /// scrolled thing is on screen, which is what a scrollbar sizes its thumb
    /// from. [`gpui::scroll_max_offset`] is the other half — content past the
    /// container.
    ///
    /// All three of [`gpui::scroll_offset`], [`gpui::scroll_max_offset`] and
    /// [`gpui::scroll_bounds`] answer for a virtual list as well as for a
    /// container: a `gpui::list` scrolls through a `ScrollHandle` of its own,
    /// and a program names the thing that scrolls rather than the kind of thing
    /// it is.
    fn scroll_bounds(id: String, field: String) -> f64 {
        view::scroll_bounds(&id, &field)
    }

    /// Scroll the container `id` to `(x, y)`, in gpui's sign: negative moves the
    /// content up and to the left.
    ///
    /// Applied when the next frame lays out, and clamped to the scrolled range
    /// there, so asking for further than the end parks at the end rather than
    /// leaving a gap. Answers `false` for an id nothing has drawn a scrolled
    /// container under — a misspelling is reported rather than silently ignored.
    fn scroll_to(id: String, x: f64, y: f64) -> bool {
        view::scroll_to(&id, x, y)
    }

    /// Name `element` as the window's root for this frame.
    ///
    /// The frame's last act; `|> gpui::root` reads well at the end of a chain.
    fn root(element: i64) {
        view::set_root(element)
    }

    /// Hang `node` under `parent`, and answer `parent` so a chain can carry
    /// on from it.
    fn child(parent: i64, node: i64) -> i64 {
        view::child(parent, node)
    }

    /// Hang a whole run of elements under `parent`, in order.
    fn children(parent: i64, nodes: Vec<i64>) -> i64 {
        view::children(parent, nodes)
    }

    // -----------------------------------------------------------------
    // leaves
    // -----------------------------------------------------------------

    /// A run of text.
    fn text(content: String, style: String) -> i64 {
        view::push(Kind::Text(content), style, String::new())
    }

    /// A clickable button. Clicking raises the event `id`.
    fn button(id: String, label: String, style: String) -> i64 {
        view::push(Kind::Button(label), style, id)
    }

    /// A checkbox that toggles the flag stored under `id` and raises `id`.
    ///
    /// The tick's colour comes from the string stored under `"<id>#tick"`,
    /// defaulting to `#11111b`: `gpui::set_text("agree#tick", "#ffffff")` puts a
    /// white tick in the "agree" box. It is read every frame, so it can change
    /// with the rest of the state.
    fn checkbox(id: String, label: String, checked: bool, style: String) -> i64 {
        view::push(Kind::Checkbox { label, checked }, style, id)
    }

    /// A text editor backed by the string stored under `key`.
    ///
    /// The editor itself lives in the binding: its `EntityInputHandler` is a
    /// trait the platform calls after this binding function has returned, so
    /// it cannot be a Gossamer closure. Raises `<key>|change` as it is edited.
    fn editor(key: String, placeholder: String, style: String) -> i64 {
        view::push(Kind::Editor { key, placeholder }, style, String::new())
    }

    /// A single-line text field backed by the string stored under `key`.
    ///
    /// Raises `<key>|submit` on Return and `<key>|cancel` on Escape.
    fn input(key: String, placeholder: String, style: String) -> i64 {
        view::push(Kind::Input { placeholder }, style, key)
    }

    /// A draggable slider between `min` and `max`, backed by the float stored
    /// under `key`. Raises `<key>|<value>` while it moves.
    fn slider(key: String, min: f64, max: f64, style: String) -> i64 {
        view::push(Kind::Slider { min, max }, style, key)
    }

    /// A progress bar showing `value` in `0.0 ..= 1.0`.
    fn progress(value: f64, style: String) -> i64 {
        view::push(Kind::Progress(value), style, String::new())
    }

    /// A raster image loaded from a file path.
    fn image(path: String, style: String) -> i64 {
        view::push(Kind::Image(path), style, String::new())
    }

    /// An SVG, tinted by the style's `color=`.
    fn icon(path: String, style: String) -> i64 {
        view::push(Kind::Icon(path), style, String::new())
    }

    /// An empty styled box — a swatch, a bar, a coloured block.
    fn rect(style: String) -> i64 {
        view::push(Kind::Plain, style, String::new())
    }

    /// A one-pixel horizontal rule.
    fn divider(style: String) -> i64 {
        view::push(Kind::Divider, style, String::new())
    }

    /// Vertical space.
    fn spacer(size: f64) -> i64 {
        view::push(Kind::Spacer(size), String::new(), String::new())
    }

    /// Text with per-byte-range highlights. Each entry is
    /// `"start-end prop,prop"`, and the properties are a vocabulary of their own
    /// — `gpui::HighlightStyle`, all seven of its properties reachable:
    ///
    /// | property | tokens |
    /// |---|---|
    /// | colour | `color=#rrggbb` |
    /// | background | `bg=#rrggbb` |
    /// | weight | `bold`, `semibold`, `weight=800` |
    /// | style | `italic` |
    /// | underline | `underline`, and its detail `undercolor=`, `underthick=`, `underwavy` |
    /// | strikethrough | `strike`, and its detail `strikecolor=`, `strikethick=` |
    /// | fade | `fade=0.4` |
    ///
    /// A detail token implies the decoration it belongs to, so `underthick=3` is
    /// an underline. A token that is none of these is **reported** — printed once
    /// and shown as a card — rather than silently dropped, because a highlight
    /// that quietly ignores `colur=#ff0000` is a syntax highlighter that quietly
    /// stops colouring one kind of token. See [`gpui::highlight_properties`] to
    /// check a spec without drawing it.
    fn text_styled(content: String, highlights: Vec<String>, style: String) -> i64 {
        view::push(Kind::StyledText { content, highlights }, style, String::new())
    }

    /// One of the application's default colours, as `#rrggbbaa`.
    ///
    /// `gpui::colour_names()` is the list, and it is the whole of what the
    /// platform offers: `gpui::colors::Colors` has exactly eight fields, so no
    /// colour is missing and none is invented. The set installed is chosen from
    /// the window's appearance, so `background` is `#ffffffff` on a light desktop
    /// and `#222222ff` on a dark one — which is what [`gpui::appearance`] reports
    /// and what [`gpui::default_color_for`] answers for the other.
    fn default_color(name: String) -> String {
        view::default_color(&name)
    }

    /// The system clipboard's text, as of the start of this frame, or `""` when
    /// it holds none.
    ///
    /// Read at the start of the frame, not when this is called, so a program that
    /// writes the clipboard does not see its own write until the next frame —
    /// which is also when every other program does.
    fn clipboard_text() -> String {
        view::clipboard_text()
    }

    /// What the system clipboard holds: `"text"`, `"image"` or `"empty"`.
    ///
    /// "image" means the item is there and none of its entries is text. The binding
    /// does not read or write image *content*: writing one needs gpui's own `Image`
    /// type, which nothing in this crate constructs, and that is the asset pipeline
    /// the register carries as `DW7`.
    fn clipboard_kind() -> String {
        view::clipboard_kind()
    }

    /// Put `text` on the system clipboard.
    ///
    /// Applied after this frame, so [`gpui::clipboard_text`] answers it from the
    /// frame after next.
    fn set_clipboard_text(text: String) {
        store::request(store::Request::SetClipboard { text })
    }

    /// The window's appearance: `"light"` or `"dark"`.
    ///
    /// The live OS appearance, asked of the window rather than assumed. gpui's two
    /// "vibrant" appearances are reported as the plain ones they resemble, because
    /// no colour in [`gpui::colors::Colors`] distinguishes them.
    fn appearance() -> String {
        view::appearance()
    }

    /// The eight names [`gpui::default_color`] answers to, sorted.
    fn colour_names() -> Vec<String> {
        view::colour_names()
    }

    /// What a default colour would be in `appearance`, which is `"light"` or
    /// `"dark"`.
    ///
    /// For a program that wants a colour the window is not using — a border that
    /// reads against the other theme, an overlay that has to stay legible whichever
    /// way the desktop is set.
    fn default_color_for(appearance: String, name: String) -> String {
        view::default_color_for(&appearance, &name)
    }

    /// A directly painted surface. `painter` is one of `"bars"`, `"wave"`,
    /// `"stripes"` or `"prism"`; `phase` drives its animation.
    ///
    /// `prism` reproduces `gradient.rs`'s trapezoid — a filled path with a
    /// linear gradient through it — so the set is four, not the three this doc
    /// and the README used to name.
    fn draw(painter: String, phase: f64, style: String) -> i64 {
        view::push(Kind::Canvas { painter, phase }, style, String::new())
    }

    // -----------------------------------------------------------------
    // paths
    // -----------------------------------------------------------------

    /// A new, empty path to be filled.
    fn path_fill() -> i64 {
        path::fill()
    }

    /// A new, empty path to be stroked at `width`.
    fn path_stroke(width: f64) -> i64 {
        path::stroke(width)
    }

    /// Start a contour at `x, y`.
    fn path_move_to(handle: i64, x: f64, y: f64) {
        path::move_to(handle, x, y)
    }

    /// Add a line to `x, y`.
    fn path_line_to(handle: i64, x: f64, y: f64) {
        path::line_to(handle, x, y)
    }

    /// Add a quadratic curve to `x, y` with control point `ctrl_x, ctrl_y`.
    fn path_curve_to(handle: i64, x: f64, y: f64, ctrl_x: f64, ctrl_y: f64) {
        path::curve_to(handle, x, y, ctrl_x, ctrl_y)
    }

    /// Add an elliptical arc.
    fn path_arc_to(handle: i64, rx: f64, ry: f64, x_rotation: f64, large_arc: bool, sweep: bool, x: f64, y: f64) {
        path::arc_to(handle, rx, ry, x_rotation, large_arc, sweep, x, y)
    }

    /// Add a polygon. `points` is a flat `x, y, x, y …` run.
    fn path_polygon(handle: i64, points: Vec<f64>, closed: bool) {
        path::polygon(handle, points, closed)
    }

    /// Close the current contour.
    fn path_close(handle: i64) {
        path::close(handle)
    }

    /// Set the dash pattern of a stroked path.
    fn path_dash(handle: i64, dashes: Vec<f64>) {
        path::dash(handle, dashes)
    }

    /// Translate and scale everything added so far.
    fn path_transform(handle: i64, translate_x: f64, translate_y: f64, scale: f64) {
        path::transform(handle, translate_x, translate_y, scale)
    }

    /// Finish a path. The handle keeps working and now names the built path.
    fn path_build(handle: i64) -> Result<i64, String> {
        path::build(handle)
    }

    /// Empty the path pool. Every handle issued before this stops working.
    fn path_reset() {
        path::reset()
    }

    /// Paint built paths, in order. `backgrounds` lines up with `paths` and
    /// each entry is a colour (`#rrggbbaa`, a name) or a `grad=…` token.
    ///
    /// Coordinates are window coordinates: a path is painted where it was
    /// built, exactly as `Window::paint_path` does.
    fn paint_paths(paths: Vec<i64>, backgrounds: Vec<String>, style: String) -> i64 {
        view::push(Kind::Paths { paths, backgrounds }, style, String::new())
    }

    // -----------------------------------------------------------------
    // pointer
    // -----------------------------------------------------------------

    /// Report pointer events for `element`. `kinds` is any of
    /// `down`, `up`, `move`, `click`, `right`, and each one raises
    /// `"<id>|<kind>"` and writes `<id>.x` / `<id>.y` into the state store,
    /// relative to the element's own top-left corner.
    fn on_mouse(element: i64, kinds: String, id: String) -> i64 {
        view::push(
            Kind::Mouse { child: element.max(0) as usize, id, kinds },
            String::new(),
            String::new(),
        )
    }

    /// Raise keystrokes as `"<id>|<unparsed>"`. Installed once.
    ///
    /// Not quite upstream's `cx.observe_keystrokes`: gpui stops the observer at
    /// the first handler that stops propagation, and the text editor does, so
    /// the keys it consumes — backspace, the arrows, and the rest — do not
    /// reach this. Keys nothing handled do.
    fn observe_keystrokes(id: String) {
        store::request(store::Request::ObserveKeys { id })
    }

    /// The name of the current keyboard layout, e.g. `"U.S."`.
    fn keyboard_layout() -> String {
        view::keyboard_layout()
    }

    /// The pointer's position in the window.
    fn mouse_position() -> (f64, f64) {
        view::mouse_position()
    }

    /// Move focus to the next focusable element.
    fn focus_next() {
        store::request(store::Request::FocusNext)
    }

    /// Move focus to the previous focusable element.
    fn focus_prev() {
        store::request(store::Request::FocusPrev)
    }

    /// Whether the element with this id currently holds focus.
    ///
    /// Only the elements this binding builds a handle for are ever reported: an
    /// `input`, an `editor`, and the `~root` handle `gpui::on_keys` installs.
    /// An element made focusable with the `focusable` token is **not** visible
    /// here — GPUI builds that handle internally and never hands it over — so
    /// this answers `false` for it even while it is focused.
    fn is_focused(id: String) -> bool {
        view::is_focused(&id)
    }

    // -----------------------------------------------------------------
    // list-valued floats
    // -----------------------------------------------------------------

    /// Append a float to the list stored under `key`.
    fn list_push_float(key: String, value: f64) {
        store::push_float(&key, value)
    }

    /// The whole float list, as a copy.
    fn list_all_float(key: String) -> Vec<f64> {
        store::get_floats(&key)
    }

    /// A virtualised list of `count` rows: `build` runs only for the rows on
    /// screen, is called with the row's index, and answers that row's element.
    cb_fn list(_d, id: String, count: i64, row_height: f64, style: String, build: PersistentCallback) -> i64 {
        view::push(
            // Retained: rows are built while the list lays out, after this
            // call has returned.
            Kind::Virtual { count: count.max(0) as usize, build: build.retain(), row_height },
            style,
            id,
        )
    }

    // -----------------------------------------------------------------
    // virtual lists
    // -----------------------------------------------------------------

    /// The rows the virtual list `id` has on screen, as `(first, last)` —
    /// inclusive, and `(-1, -1)` for a list nothing has drawn.
    ///
    /// `id` is the name given to [`gpui::list`]. The range is what gpui handed
    /// the list when it last laid out, recorded as the rows were built, so it is
    /// the rows that are *actually* visible rather than an estimate from the
    /// scroll offset. A list whose rows all fit reports its whole range; one that
    /// has scrolled reports a window into it.
    ///
    /// A program uses this to know what a user can see — to drive a "loaded
    /// 10–20 of 200" readout, or to decide what to fetch next.
    fn list_range(id: String) -> (i64, i64) {
        match view::list_range(&id) {
            Some((first, last)) => (first as i64, last as i64),
            None => (-1, -1),
        }
    }

    /// Scroll the virtual list `id` so that row `index` is placed by `strategy`.
    ///
    /// `strategy` is `"top"`, `"center"` or `"bottom"`: where in the viewport the
    /// row is put, as far as the rows above it allow. Applied when the list next
    /// lays out, so the new range is readable through [`gpui::list_range`] on the
    /// frame after this one.
    ///
    /// Answers `false` for a list nothing has drawn, and for a `strategy` that is
    /// none of the three — a misspelling is reported rather than quietly treated
    /// as `"top"`.
    fn list_scroll_to(id: String, index: i64, strategy: String) -> bool {
        view::list_scroll_to(&id, index, &strategy)
    }

    // -----------------------------------------------------------------
    // windows
    // -----------------------------------------------------------------

    /// Open another window. Its `view` becomes a permanent callback, valid for
    /// the life of the program.
    ///
    /// The window is opened once the frame that asked for it has finished —
    /// opening one draws it immediately, and a nested draw would disturb the
    /// frame being built.
    cb_fn open_window(d, title: String, x: f64, y: f64, w: f64, h: f64, options: String, view: PersistentCallback) -> () {
        let _ = d;
        // Retained: the window opens after the frame that asked for it, and
        // its view is invoked from then on.
        app::queue_window(app::PendingOpen { title, x, y, w, h, options, view: view.retain() })
    }

    /// Ask to be told before this window closes, and be able to refuse.
    ///
    /// `decide` answers `true` to let the window close and `false` to keep it
    /// open — upstream's `Window::on_window_should_close`, whose docstring is
    /// "if the callback returns false, the window won't be closed". The obvious
    /// use is an unsaved-changes guard.
    ///
    /// It is consulted for the platform's own close request — the traffic lights,
    /// the window's close item — **and for [`gpui::close_window`] and
    /// [`gpui::close_window_in`]**, so a program cannot bypass its own guard by
    /// closing a window in code. That is a deliberate departure: upstream's
    /// `remove_window` does not go through the handler.
    ///
    /// Installed once; a second call replaces the first. A handler that fails to
    /// run is treated as answering `true`, as gpui treats its own — a broken guard
    /// must not trap a window forever.
    cb_fn on_window_should_close(d, decide: PersistentCallback) -> () {
        let _ = d;
        // Retained: the handler is invoked when the platform asks, long after
        // this call has returned.
        view::set_should_close(decide.retain())
    }

    /// Keep the application alive after its last window closes.
    ///
    /// Off by default: a program that opens one window and closes it expects the
    /// process to end, which is what every example here does. Turned on, closing
    /// the last window leaves the app running with none — the state a dock menu, a
    /// notification, or a [`gpui::on_window_closed`] handler that opens another
    /// window needs.
    ///
    /// With no window there is no frame and no view callback, so the only program
    /// code that runs is [`gpui::on_window_closed`] and whatever a `gpui::tick` or
    /// `gpui::task` closure raises. State written then belongs to
    /// [`gpui::window_id`] `0`.
    fn keep_alive(on: bool) {
        app::set_keep_alive(on)
    }

    /// Be told when the last window has closed.
    ///
    /// `handler` runs *after* the last window is gone, in window 0's namespace. It
    /// is where a program re-opens a window — the app is still running at that
    /// moment, whether or not [`gpui::keep_alive`] is on, and a window it asks for
    /// is opened even though no frame is in flight to ask it.
    ///
    /// Installed once; a second call replaces the first.
    cb_fn on_window_closed(d, handler: PersistentCallback) -> () {
        let _ = d;
        // Retained: it runs when the last window closes, long after this returns.
        view::set_window_closed(handler.retain())
    }

    /// Close the window this frame belongs to.
    fn close_window() {
        store::request(store::Request::CloseWindow)
    }

    /// Close another window, named by the id [`gpui::window_id`] answers in it.
    ///
    /// `close_window()` addresses the window the call is in; this addresses one
    /// by name, whatever it is doing. A program learns another window's id from
    /// `gpui::window_id()` inside that window, or from the `_in` accessors.
    fn close_window_in(win: i64) {
        store::request(store::Request::CloseWindowAt { target: win })
    }

    /// Resize the window this frame belongs to.
    fn resize_window(w: f64, h: f64) {
        store::request(store::Request::ResizeWindow { w, h })
    }

    /// Install an application menu.
    ///
    /// Each entry is one of `"separator"`, `"quits:Quit"`, or
    /// `"events:<label>"`; an `events` entry raises `"menu|<position>"` when
    /// it is chosen. Menu items carry a boxed action, which no ABI can pass,
    /// so the binding owns eight action slots and maps entries onto them by
    /// position — a menu with more than eight `events` entries reuses slot 0.
    fn set_menus(name: String, items: Vec<String>) {
        store::request(store::Request::SetMenus { name, items })
    }

    /// The window's x, y, width or height, as of the start of this frame.
    /// `field` is `x`, `y`, `w` or `h`.
    fn window_field(field: String) -> f64 {
        let (x, y, w, h) = view::window_rect();
        match field.as_str() {
            "x" => x,
            "y" => y,
            "w" => w,
            "h" => h,
            _ => 0.0,
        }
    }

    /// How many displays are connected.
    fn display_count() -> i64 {
        view::displays().len() as i64
    }

    /// One field of one display. `field` is `id`, `x`, `y`, `w` or `h`;
    /// anything else, or an index out of range, answers `0.0`.
    fn display_field(index: i64, field: String) -> f64 {
        let Some(display) = view::displays().get(usize::try_from(index).unwrap_or(usize::MAX)).cloned()
        else {
            return 0.0;
        };
        match field.as_str() {
            "id" => display.id as f64,
            "x" => display.x,
            "y" => display.y,
            "w" => display.w,
            "h" => display.h,
            _ => 0.0,
        }
    }

    // -----------------------------------------------------------------
    // window
    // -----------------------------------------------------------------

    /// Route every keystroke to the program as a `"key|<name>"` event.
    ///
    /// Off by default. Clicking into a text field moves focus, and the field
    /// takes over until focus comes back to the window.
    fn on_keys(enabled: bool) {
        store::set_flag("~keys", enabled)
    }

    /// Record a theme name; programs read it back with [`gpui::theme`].
    ///
    /// Nothing consults it. It has no effect on any colour — `gpui::default_color`
    /// answers the platform's own colours, chosen from the window's appearance —
    /// and it exists only so a program can keep the name of the theme it thinks
    /// it is drawing with. Do not read it as a light/dark switch.
    fn set_theme(name: String) {
        store::set_text("~theme", &name)
    }

    /// The current theme name, `"dark"` by default.
    fn theme() -> String {
        store::get_text("~theme", "dark")
    }
);
