//! `gpui` — Zed's GPU-accelerated UI framework, exposed to Gossamer.
//!
//! A Gossamer program describes its window once per frame, in the middle of
//! painting it. The shape is:
//!
//! ```gossamer
//! use gpui
//!
//! fn main() {
//!     gpui::run("Counter", 420.0, 260.0, || {
//!         for event in gpui::take_events() {
//!             if gpui::event_id(event) == "inc" {
//!                 gpui::set_int("count", gpui::state_int("count", 0) + 1)
//!             }
//!         }
//!         gpui::column("flex gap=10 p=20 bg=#181825 w=full h=full", || {
//!             gpui::text(f"count = {gpui::state_int(\"count\", 0)}", "font=2xl color=#cdd6f4")
//!             gpui::button("inc", "+1", "px=14 py=8 rounded=8 bg=#89b4fa color=#11111b hover=bg:#a6e3fa")
//!         })
//!     })
//! }
//! ```
//!
//! `run` blocks for the life of the program: GPUI calls the view callback
//! once per frame, and the callback stays valid throughout, which is exactly
//! the lifetime a Gossamer callback has.

// Imports must live outside the `register_module!` block: the macro lifts the
// binding bodies into a generated module that reaches the surrounding scope
// with `use super::*;`.
use gossamer_binding::PersistentCallback;

mod app;
mod store;
mod style;
mod view;

use view::Kind;

gossamer_binding::register_module!(
    name: gpui,
    doc: "Zed's gpui UI framework: windows, flexbox layout, styling, interaction, animation and custom painting.",

    // -----------------------------------------------------------------
    // running
    // -----------------------------------------------------------------

    /// Open a window and run the event loop until the program quits.
    ///
    /// `view` is called once per frame and must describe the whole window.
    cb_fn run(d, title: String, width: f64, height: f64, view: PersistentCallback) -> Result<(), String> {
        app::run_app(d, title, width, height, "", view)
    }

    /// `run`, with window options: `"resizable=false titlebar=none float=true background=blur"`.
    cb_fn run_with(d, title: String, width: f64, height: f64, options: String, view: PersistentCallback) -> Result<(), String> {
        app::run_app(d, title, width, height, &options, view)
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

    /// The number of frames painted so far.
    fn frame() -> i64 {
        view::frame_number() as i64
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
    /// An entry is `"id"` or `"id|payload"`; use [`gpui::event_id`] and
    /// [`gpui::event_data`] to split one.
    fn take_events() -> Vec<String> {
        store::take_events()
    }

    /// The event's id: everything before the first `|`.
    fn event_id(event: String) -> String {
        match event.split_once('|') {
            Some((id, _)) => id.to_string(),
            None => event,
        }
    }

    /// Split an event into `(id, payload)` in one step.
    ///
    /// `event_id` consumes its argument, so reaching for both halves of the
    /// same event means taking a copy; this takes it once.
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
    // layout
    // -----------------------------------------------------------------

    /// A column container. `style` is a GPUI style string; see the README.
    cb_fn column(d, style: String, build: PersistentCallback) -> () {
        let style = format!("flex col {}", style);
        view::container(d, Kind::Plain, style, String::new(), &build)
    }

    /// A row container.
    cb_fn row(d, style: String, build: PersistentCallback) -> () {
        let style = format!("flex row {}", style);
        view::container(d, Kind::Plain, style, String::new(), &build)
    }

    /// A container with no flex direction of its own.
    cb_fn panel(d, style: String, build: PersistentCallback) -> () {
        view::container(d, Kind::Plain, style, String::new(), &build)
    }

    /// A container whose children stack on top of one another; position them
    /// with `absolute top=… left=…`.
    cb_fn stack(d, style: String, build: PersistentCallback) -> () {
        let style = format!("relative {}", style);
        view::container(d, Kind::Plain, style, String::new(), &build)
    }

    /// A vertically scrolling area.
    cb_fn scroll(d, style: String, build: PersistentCallback) -> () {
        let style = format!("flex col overflow=y {}", style);
        view::container(d, Kind::Plain, style, String::new(), &build)
    }

    /// A virtualised list of `count` rows: `build` runs only for the rows on
    /// screen, and is called with the row's index.
    cb_fn list(_d, id: String, count: i64, row_height: f64, style: String, build: PersistentCallback) -> () {
        view::leaf(
            Kind::Virtual { count: count.max(0) as usize, build, row_height },
            style,
            id,
        )
    }

    // -----------------------------------------------------------------
    // leaves
    // -----------------------------------------------------------------

    /// A run of text.
    fn text(content: String, style: String) {
        view::leaf(Kind::Text(content), style, String::new())
    }

    /// A clickable button. Clicking raises the event `id`.
    fn button(id: String, label: String, style: String) {
        view::leaf(Kind::Button(label), style, id)
    }

    /// A checkbox that toggles the flag stored under `id` and raises `id`.
    fn checkbox(id: String, label: String, checked: bool, style: String) {
        view::leaf(Kind::Checkbox { label, checked }, style, id)
    }

    /// A single-line text field backed by the string stored under `key`.
    ///
    /// Raises `<key>|submit` on Return and `<key>|cancel` on Escape.
    fn input(key: String, placeholder: String, style: String) {
        view::leaf(Kind::Input { placeholder }, style, key)
    }

    /// A draggable slider between `min` and `max`, backed by the float stored
    /// under `key`. Raises `<key>|<value>` while it moves.
    fn slider(key: String, min: f64, max: f64, style: String) {
        view::leaf(Kind::Slider { min, max }, style, key)
    }

    /// A progress bar showing `value` in `0.0 ..= 1.0`.
    fn progress(value: f64, style: String) {
        view::leaf(Kind::Progress(value), style, String::new())
    }

    /// A raster image loaded from a file path.
    fn image(path: String, style: String) {
        view::leaf(Kind::Image(path), style, String::new())
    }

    /// An SVG, tinted by the style's `color=`.
    fn icon(path: String, style: String) {
        view::leaf(Kind::Icon(path), style, String::new())
    }

    /// An empty styled box — a swatch, a bar, a coloured block.
    fn rect(style: String) {
        view::leaf(Kind::Plain, style, String::new())
    }

    /// A one-pixel horizontal rule.
    fn divider(style: String) {
        view::leaf(Kind::Divider, style, String::new())
    }

    /// Vertical space.
    fn spacer(size: f64) {
        view::leaf(Kind::Spacer(size), String::new(), String::new())
    }

    /// A directly painted surface. `painter` is one of `"bars"`, `"wave"` or
    /// `"stripes"`; `phase` drives its animation.
    fn draw(painter: String, phase: f64, style: String) {
        view::leaf(Kind::Canvas { painter, phase }, style, String::new())
    }

    // -----------------------------------------------------------------
    // theme
    // -----------------------------------------------------------------

    /// Route every keystroke to the program as a `"key|<name>"` event.
    ///
    /// Off by default. Clicking into a text field moves focus, and the field
    /// takes over until focus comes back to the window.
    fn on_keys(enabled: bool) {
        store::set_flag("~keys", enabled)
    }

    /// Record a theme name; programs read it back with [`gpui::theme`].
    fn set_theme(name: String) {
        store::set_text("~theme", &name)
    }

    /// The current theme name, `"dark"` by default.
    fn theme() -> String {
        store::get_text("~theme", "dark")
    }
);
