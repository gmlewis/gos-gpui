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
mod gpu;
mod history;
mod list;
mod options;
mod prompts;
mod path;
mod scroll;
mod settings;
mod text;
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
    ///
    /// A clock on the event queue: `gpui::tick("clock", 1000)` raises `"clock"` once a
    /// second, and a program handles it wherever it handles its other events. The id is
    /// what identifies the timer, so two windows can use the same one without silencing
    /// each other.
    ///
    /// Calling it again for an id that is already running **reschedules** that timer to
    /// the new period — `gpui::tick("clock", 100)` turns a slow clock into a fast one.
    /// It used to be ignored, so a program could start a clock and never change it; the
    /// period in force is readable with [`gpui::tick_period`].
    fn tick(id: String, ms: i64) {
        store::request(store::Request::Tick { id, ms: ms.max(1) as u64 })
    }

    /// Stop the repeating timer `id`.
    ///
    /// Nothing is raised afterwards, and the id may be started again — with any period —
    /// immediately. Stopping a timer that is not running does nothing.
    fn tick_stop(id: String) {
        store::request(store::Request::TickStop { id })
    }

    /// The period the timer `id` repeats at, or `0` when none is running.
    fn tick_period(id: String) -> i64 {
        app::tick_period(crate::window::current(), &id)
    }

    /// Raise the event `id` once, `ms` milliseconds from now.
    ///
    /// The one-shot counterpart of [`gpui::tick`]: `gpui::delay("save", 250)` raises
    /// `"save"` once and then it is over — no handler that has to ignore every second
    /// event, and nothing left running afterwards. Asking again for an id that is already
    /// waiting **restarts** the wait from now. [`gpui::delay_pending`] says whether one is
    /// waiting and [`gpui::delay_cancel`] takes it back before it fires.
    ///
    /// The id is what identifies the delay, so two windows can use the same one without
    /// cancelling each other.
    fn delay(id: String, ms: i64) {
        store::request(store::Request::Delay { id, ms: ms.max(1) as u64 })
    }

    /// Take back a one-shot delay that has not fired yet.
    ///
    /// Nothing is raised afterwards, and the id may be delayed again immediately.
    /// Cancelling a delay that is not waiting does nothing.
    fn delay_cancel(id: String) {
        store::request(store::Request::DelayCancel { id })
    }

    /// Whether [`gpui::delay`] is still waiting to raise `id`.
    fn delay_pending(id: String) -> bool {
        app::delay_pending(crate::window::current(), &id)
    }

    /// Start, or restart, a named animation that runs for `ms`.
    ///
    /// An animation here is a **clock, not a property**: it counts from 0 to 1 over `ms`
    /// and the program asks where it has got to with [`gpui::animation_phase`], which is
    /// what a painter takes — `gpui::draw("bars", gpui::animation_phase("grow"), …)`. So a
    /// program can move anything it can describe, not only what GPUI has an animation for.
    ///
    /// While one of its animations is running the binding keeps asking for frames, so a
    /// program does not have to call [`gpui::animate`] to keep its own animation moving.
    /// Starting an id that is already animating restarts it. `easing` is one of
    /// [`gpui::animation_easings`]; a name the binding does not have is treated as
    /// `"linear"` rather than refused, the curve being the least important part of a request
    /// to move something.
    fn animation(id: String, ms: i64, easing: String) {
        store::request(store::Request::Animation { id, ms: ms.max(1) as u64, easing })
    }

    /// Hold a running animation where it is.
    ///
    /// Its phase stops moving and it stops asking for frames, and it stays that way until
    /// [`gpui::animation_resume`]. Pausing one that is not running, or has already finished,
    /// does nothing.
    fn animation_pause(id: String) {
        store::request(store::Request::AnimationPause { id })
    }

    /// Carry on from where the animation was held.
    ///
    /// It does not restart: what it had already run for is counted. Resuming one that is
    /// running, or already over, does nothing.
    fn animation_resume(id: String) {
        store::request(store::Request::AnimationResume { id })
    }

    /// Take an animation away.
    ///
    /// Afterwards its state is `"none"` and its phase is `0.0`, and the id may be started
    /// again immediately. Stopping one that does not exist does nothing.
    fn animation_stop(id: String) {
        store::request(store::Request::AnimationStop { id })
    }

    /// Where the animation `id` has got to: `0.0` to `1.0`, with its curve applied, and
    /// `0.0` when there is no such animation.
    ///
    /// The same reading a painter takes, so a program that wants to move something GPUI does
    /// not paint — a position, a size, an opacity it recomputes itself — uses this.
    fn animation_phase(id: String) -> f64 {
        app::animation_phase(crate::window::current(), &id)
    }

    /// What the animation `id` is doing: `"none"`, `"running"`, `"paused"` or `"done"`.
    fn animation_state(id: String) -> String {
        app::animation_state(crate::window::current(), &id)
    }

    /// The easing curves [`gpui::animation`] accepts, in order.
    fn animation_easings() -> Vec<String> {
        app::easing_names()
    }

    /// Where `easing` puts an animation that is `t` of the way along.
    ///
    /// The curve itself, for a program applying the same shape to something of its own: it
    /// is anchored at both ends, `t` is clamped to `0..1`, and a name the binding does not
    /// have answers `linear`.
    fn easing_at(easing: String, t: f64) -> f64 {
        app::easing_at(&easing, t)
    }

    /// Do slow work off the UI thread, then raise the event `id` and store a
    /// human-readable summary under `<id>.result`.
    ///
    /// The window keeps painting while the work runs — the work is GPUI's background
    /// executor's, not the UI thread's — and the event arrives when it is done. Read the
    /// summary with [`gpui::task_result`].
    ///
    /// **What the work *is*** is a Rust-side job, not a program's own code: a thread that is
    /// not the one running the Gossamer VM cannot call back into it, so no callback a program
    /// can write is ever run here. That is `## Unreachable` in the ledger, with the compiler's
    /// own words.
    fn task(id: String, ms: i64) {
        store::request(store::Request::Task { id, ms: ms.max(1) as u64 })
    }

    /// Whether background work for `id` is still in flight.
    ///
    /// True from the moment [`gpui::task`] is asked for until its work is done or it is
    /// cancelled. It says nothing about whether the result has been read yet.
    fn task_running(id: String) -> bool {
        app::task_running(crate::window::current(), &id)
    }

    /// How far along background work for `id` is: `0.0` to `1.0`, and `1.0` once it is done.
    ///
    /// While the work runs, [`gpui::task`] raises `"<id>.progress"` as it goes, and this is the
    /// number those events are about — the event is the signal that there is something to draw,
    /// the reader is the value. A program that is already animating can poll this and ignore the
    /// events; one that is sitting still needs the event to know to repaint.
    ///
    /// Work is reported on every fiftieth of a second, so a job shorter than that goes straight
    /// from 0 to 1. Cancelled work stops reporting and stays where it reached; work that never
    /// ran reads `0.0`.
    fn task_progress(id: String) -> f64 {
        app::task_progress(crate::window::current(), &id)
    }

    /// Stop background work for `id` that is still in flight.
    ///
    /// The work raises no event and leaves no result. Cancelling work that has already
    /// finished, or was never started, does nothing, and the id may be used again
    /// immediately. Cancelling one id never touches another.
    fn task_cancel(id: String) {
        store::request(store::Request::TaskCancel { id })
    }

    /// The result of the last [`gpui::task`] with this id, or `""` if it has not finished.
    ///
    /// `gpui::task` leaves a summary under `"<id>.result"` when its work is done and then
    /// raises `id`; this is that convention with a name, so a program does not have to invent
    /// the key. Empty means the work has not finished — which is also what an id that was
    /// never used answers, so a program that starts work and reads it back immediately should
    /// treat `""` as "not yet".
    fn task_result(id: String) -> String {
        store::get_text(&format!("{id}.result"), "")
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

    // -----------------------------------------------------------------
    // lines, carets and words
    // -----------------------------------------------------------------
    //
    // The arithmetic three rows stand on — DW1's caret moves, DW4's double-click word and
    // triple-click line, and DW5's gutter. **Offsets are UTF-8 byte offsets**, the same as
    // `gpui::styled_text`'s highlight ranges, and every answer is snapped to a character boundary
    // and clamped into the text: an offset in the middle of a multi-byte character gets a whole
    // one, and one past the end belongs at the end.

    /// How many lines the text has. A text with no break is one line; so is an empty text.
    fn text_lines(text: String) -> i64 {
        text::lines(&text)
    }

    /// The line `offset` is on, counting from zero. The break belongs to the line it ends.
    fn text_line(text: String, offset: i64) -> i64 {
        text::line_of(&text, offset.max(0) as usize)
    }

    /// The byte range of the line `offset` is on, as `"<start>,<end>"`, excluding its break.
    fn text_line_bounds(text: String, offset: i64) -> String {
        text::line_bounds(&text, offset.max(0) as usize)
    }

    /// Where the caret lands `lines` lines from `offset`, aiming for `column` bytes into the target
    /// line. Negative is up, positive is down, so a page move is one call with a bigger number.
    ///
    /// The aim is clamped into the target line, so moving down onto a line too short for that column
    /// stops at its end; moving up from the first line or down from the last stays put. The
    /// **column is the caller's to keep** — this remembers nothing, so a program that wants the caret
    /// to come back to the column it left passes the same number again.
    fn text_line_move(text: String, offset: i64, lines: i64, column: i64) -> i64 {
        text::line_move(&text, offset.max(0) as usize, lines, column)
    }

    /// The word `offset` is on, as `"<start>,<end>"`.
    ///
    /// A word is a run of letters, digits and underscores; **anything else is not a word**, and the
    /// answer is the single character under the offset, so a double-click on a comma takes the comma
    /// rather than the run of punctuation and spaces around it.
    fn text_word(text: String, offset: i64) -> String {
        text::word_at(&text, offset.max(0) as usize)
    }

    // -----------------------------------------------------------------
    // the GPU behind this window
    // -----------------------------------------------------------------

    /// One field of this window's GPU, by name: `"device"`, `"driver"` or `"info"`.
    ///
    /// `Window::gpu_specs()` is public, but only a frame can ask it — it goes through the platform
    /// window — so the binding samples it once, on a window's first frame. **Until then this is
    /// empty**, and so is a name that is not one of the three: a diagnostics panel prints nothing
    /// rather than a device name nobody looked up. The names are exact, like
    /// [`gpui::window_field`]'s.
    fn gpu_field(name: String) -> String {
        gpu::field(crate::window::current(), &name)
    }

    /// [`gpui::gpu_field`] in a window named by hand, which is what `gos test` can do.
    fn gpu_field_in(win: i64, name: String) -> String {
        gpu::field(win, &name)
    }

    /// The field names [`gpui::gpu_field`] answers to: `"device"`, `"driver"`, `"info"`.
    ///
    /// The vocabulary, for a program that would rather iterate it than hard-code it.
    fn gpu_fields() -> Vec<String> {
        gpu::names()
    }

    /// Whether this window's GPU is really a fake, running on the CPU — `llvmpipe` and its like.
    ///
    /// `false` until a frame has asked, which is the same "nothing is reported rather than guessed"
    /// rule as [`gpui::gpu_field`].
    fn gpu_is_software() -> bool {
        gpu::is_software(crate::window::current())
    }

    // -----------------------------------------------------------------
    // prompts
    // -----------------------------------------------------------------

    /// Ask the person a question, as a prompt with buttons. Answers its id, or `0` when it was
    /// refused.
    ///
    /// `buttons` is a `;`-separated list of labels, trimmed, and there must be at least one: a dialog
    /// with nothing to click is a dead end and is not queued at all.
    ///
    /// Prompts **queue**. Only one is up at a time — the one [`gpui::prompt_current`] describes — and a
    /// second waits its turn no matter how many a program asks for in one frame. A program reads the
    /// head, draws it with the ordinary element tree, and answers it with [`gpui::prompt_answer`].
    fn prompt(title: String, message: String, buttons: String) -> i64 {
        prompts::ask_in(crate::window::current(), &title, &message, &buttons)
    }

    /// The prompt whose turn it is, as `"<id>|<title>|<message>"`, or `""` when nothing is waiting.
    ///
    /// Everything after the second `|` is the message, so a message may contain one.
    fn prompt_current() -> String {
        prompts::head_in(crate::window::current())
            .map(|prompt| prompts::describe(&prompt))
            .unwrap_or_default()
    }

    /// The head's button labels, in the order the prompt gave them, or nothing when none is waiting.
    ///
    /// The index of a label in this list is what [`gpui::prompt_answer`] takes.
    fn prompt_buttons() -> Vec<String> {
        prompts::head_in(crate::window::current())
            .map(|prompt| prompt.buttons)
            .unwrap_or_default()
    }

    /// Answer the prompt `id` with the button at `button`, and answer whether that was the one whose
    /// turn it was.
    ///
    /// Raises `"prompt\|<id>\|<button>"` from the answered prompt's window, so a program hears the
    /// answer in the same place as every other event. A queued prompt cannot be answered before it
    /// comes up.
    fn prompt_answer(id: i64, button: i64) -> bool {
        prompts::answer_and_raise(crate::window::current(), id, button)
    }

    /// Drop the prompt `id` wherever it is in this window's queue. Answers whether it was there.
    ///
    /// Cancelling is not answering: a dropped prompt never takes its turn and raises nothing.
    fn prompt_cancel(id: i64) -> bool {
        prompts::cancel_in(crate::window::current(), id)
    }

    /// Every prompt waiting in this window, in order, as `"<id>|<title>"`.
    fn prompts() -> Vec<String> {
        prompts::queue_in(crate::window::current())
            .iter()
            .map(prompts::listed)
            .collect()
    }

    /// Drop this window's whole queue, answering how many were dropped.
    fn prompts_clear() -> i64 {
        prompts::clear_in(crate::window::current())
    }

    /// [`gpui::prompt`] in a window named by hand, which is what `gos test` can do.
    fn prompt_in(win: i64, title: String, message: String, buttons: String) -> i64 {
        prompts::ask_in(win, &title, &message, &buttons)
    }

    /// [`gpui::prompt_current`] in a window named by hand.
    fn prompt_current_in(win: i64) -> String {
        prompts::head_in(win)
            .map(|prompt| prompts::describe(&prompt))
            .unwrap_or_default()
    }

    /// [`gpui::prompt_answer`] in a window named by hand.
    fn prompt_answer_in(win: i64, id: i64, button: i64) -> bool {
        prompts::answer_and_raise(win, id, button)
    }

    /// [`gpui::prompts_clear`] in a window named by hand.
    fn prompts_clear_in(win: i64) -> i64 {
        prompts::clear_in(win)
    }

    // -----------------------------------------------------------------
    // undo and redo
    // -----------------------------------------------------------------

    /// Start recording a history for the text stored under `key`.
    ///
    /// The text it holds now is the baseline — the state an undo all the way back would return to.
    /// Watching a key again starts over.
    ///
    /// This is undo for the editor widget, whose content lives under its key: `changed()` publishes
    /// there and `adopt_program_text()` adopts what is written there, so reverting the stored text
    /// reverts the editor. It is also undo for a program with no editor, since `ui::input` keeps its
    /// value under `key` too.
    ///
    /// A run of typing is **one** step, not one per keystroke: see `history.rs` for the rule. Undo
    /// and redo write the text back with [`gpui::state_text`]'s ordinary store, so a program should
    /// ask for a frame to see it — the editor does.
    fn history_watch(key: String) {
        let win = crate::window::current();
        let current = store::get_text_in(win, &key, "");
        history::watch_in(win, &key, &current);
    }

    /// Stop recording `key`, dropping its history. Answers whether anything was being recorded.
    fn history_unwatch(key: String) -> bool {
        history::unwatch_in(crate::window::current(), &key)
    }

    /// Step back one step, and answer whether there was one.
    ///
    /// The text is restored to what it was before that step, and the redo stack keeps what it
    /// stepped over so [`gpui::history_redo`] can go forward again.
    fn history_undo(key: String) -> bool {
        history::step(crate::window::current(), &key, false)
    }

    /// Step forward one step, and answer whether there was one.
    fn history_redo(key: String) -> bool {
        history::step(crate::window::current(), &key, true)
    }

    /// How many steps back it is possible to go — `0` when `key` is at its baseline, or when
    /// nobody is watching it.
    fn history_depth(key: String) -> i64 {
        history::depth_in(crate::window::current(), &key)
    }

    /// Whether [`gpui::history_undo`] would do anything.
    fn history_can_undo(key: String) -> bool {
        history::can_undo_in(crate::window::current(), &key)
    }

    /// Whether [`gpui::history_redo`] would do anything.
    fn history_can_redo(key: String) -> bool {
        history::can_redo_in(crate::window::current(), &key)
    }

    // -----------------------------------------------------------------
    // who hears about what
    // -----------------------------------------------------------------

    /// Ask to hear `topic`, as the event id `id`. Answers `false` if already subscribed.
    ///
    /// The `cx.subscribe` of a program with no entity graph: there is nothing to subscribe *to*,
    /// because the tree is rebuilt every frame, so a subscriber is an event id and a topic is a
    /// name. [`gpui::publish`] raises `"<id>|<payload>"` for the subscribers of a topic, and the
    /// subscriber reads it in the same place as every other event — as does [`gpui::emit`], which
    /// raises to an id the caller names itself.
    ///
    /// Calling it every frame is safe and normal: a second call for the same pair is ignored, so a
    /// program that rebuilds its tree every frame does not accumulate subscribers.
    fn subscribe(topic: String, id: String) -> bool {
        store::subscribe_in(crate::window::current(), &topic, &id)
    }

    /// Stop hearing `topic`. Answers `false` when there was nothing to remove.
    fn unsubscribe(topic: String, id: String) -> bool {
        store::unsubscribe_in(crate::window::current(), &topic, &id)
    }

    /// Raise `"<id>|<payload>"` for every subscriber of `topic`, and answer how many heard it.
    ///
    /// The fan-out half of [`gpui::subscribe`], and the reason a topic is worth having: `emit`
    /// raises one event for an id the caller names, and this raises it for whoever asked. A topic
    /// nobody is on raises nothing and answers `0` — a signal nobody is listening for is not an
    /// error. Subscriptions belong to a window, so one window's `publish` is not heard in another.
    fn publish(topic: String, payload: String) -> i64 {
        store::publish_in(crate::window::current(), &topic, &payload)
    }

    /// The ids subscribed to `topic` in this window, in the order they subscribed.
    fn subscribers(topic: String) -> Vec<String> {
        store::subscribers_in(crate::window::current(), &topic)
    }

    /// Every topic with a subscriber in this window, sorted.
    fn topics() -> Vec<String> {
        store::topics_in(crate::window::current())
    }

    /// [`gpui::subscribe`], for a window named by hand — which is what `gos test` can do, since it
    /// runs outside every frame and its current window is 0.
    fn subscribe_in(win: i64, topic: String, id: String) -> bool {
        store::subscribe_in(win, &topic, &id)
    }

    /// [`gpui::unsubscribe`], for a window named by hand.
    fn unsubscribe_in(win: i64, topic: String, id: String) -> bool {
        store::unsubscribe_in(win, &topic, &id)
    }

    /// [`gpui::publish`], for a window named by hand.
    fn publish_in(win: i64, topic: String, payload: String) -> i64 {
        store::publish_in(win, &topic, &payload)
    }

    /// [`gpui::subscribers`], for a window named by hand.
    fn subscribers_in(win: i64, topic: String) -> Vec<String> {
        store::subscribers_in(win, &topic)
    }

    /// [`gpui::topics`], for a window named by hand.
    fn topics_in(win: i64) -> Vec<String> {
        store::topics_in(win)
    }

    /// Every event raised for `win` since the previous frame, drained.
    ///
    /// The twin the state accessors have, and the one that makes the per-window question
    /// answerable where no frame is running.
    fn take_events_in(win: i64) -> Vec<String> {
        store::take_events_in(win)
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

    /// The interaction refinements a style string carries, as
    /// `"hover,active,focus"`.
    ///
    /// A refinement is a second style string applied only while the element is in that
    /// state: `hover="bg:#ff0000"`, `active=bg:#990000`, `focus="border=2
    /// bordercolor=#89b4fa"`. The value is a style language of its own, so anything the
    /// style language knows can appear inside one — and because a refinement's value
    /// usually contains a space, it is normally quoted.
    ///
    /// The answer is the three kinds an element has, in a fixed order, and `""` for a
    /// style with none. It is the reader a program needs when it assembles a style string
    /// from data and wants to know which states it has actually described — and the one a
    /// test can assert against, since what a refinement *looks* like is a pixel question.
    ///
    /// `focus=` is applied with gpui's `InteractiveElement::focus`, which takes effect
    /// only for an element something can focus: `tabindex=`, `tabstop`, `focusable`, or a
    /// `gpui::focus_next`/`focus_prev` order it takes part in.
    fn style_refinements(style: String) -> String {
        view::style_refinements(&style)
    }

    /// The platform colours a style overrides, as `"background,selected"`.
    ///
    /// The tokens are `theme.<field>=<colour>`, where the field is one of the eight
    /// `gpui::colors::Colors` has: `text`, `selected_text`, `background`, `disabled`,
    /// `selected`, `border`, `separator`, `container`. Put them on the window's **root**
    /// — they are the window's colours, not the element's, so unlike `bg=` they change
    /// what the platform answers for a name and what a framework default is chosen from:
    ///
    /// ```gossamer
    /// ui::column("p=8 size=full theme.background=#111111 theme.text=#eeeeee")
    /// ```
    ///
    /// They take effect on the frame **after** the one that carries them, because a frame
    /// reads the colours before it runs the callback that describes its root;
    /// [`gpui::default_color`] therefore reports the overridden value from the next frame
    /// on. Anything but the eight field names is reported as an unknown token.
    fn theme_override(style: String) -> String {
        view::theme_override(&style)
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

    /// Where a scrollbar's thumb goes and how long it is, as `"<start>,<length>"` in pixels, or
    /// `""` when there is nothing to scroll.
    ///
    /// Four numbers in and two out, with no window in it: `offset` is gpui's (zero at the top,
    /// negative going down), `max` is [`gpui::scroll_max_offset`]'s positive amount, `track` is the
    /// length of the scrollbar in the same axis, and `min` is the shortest thumb worth drawing.
    /// The start is measured over the thumb's *travel*, so its far end meets the track's end at the
    /// bottom rather than stopping short of it.
    ///
    /// This is the arithmetic the scrollbar element uses, for a program drawing its own scrollbar —
    /// which is what `02_data_table` does — or for one scrolling something the binding does not
    /// track.
    fn thumb_geometry(offset: f64, max: f64, track: f64, min: f64) -> String {
        scroll::thumb(offset, max, track, min)
    }

    /// The same, for a scrolled element the binding keeps an offset for, as `"<start>,<length>"`.
    ///
    /// Reads [`gpui::scroll_offset`] and [`gpui::scroll_max_offset`] for `id` on `axis` and answers
    /// where its thumb goes in a track of `track` pixels. `""` when no scrolled container has been
    /// drawn under `id`, or its content fits.
    fn scrollbar_thumb(id: String, axis: String, track: f64, min: f64) -> String {
        scroll::scrollbar_thumb(crate::window::current(), &id, &axis, track, min)
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

    /// Declare where this window's assets live.
    ///
    /// `gpui::image` and `gpui::icon` resolve the name they are given against this directory
    /// first, and fall back on the process's working directory when it is not there — which
    /// matters because a program started from a desktop icon has a working directory its assets
    /// are not in. `""` (the default) declares no root, so nothing changes for a program that
    /// does not want one. A name that is already an absolute path is never touched.
    ///
    /// Per window, like the rest of this window's state, and readable back through
    /// [`gpui::asset_path`].
    fn asset_root(path: String) {
        store::set_text("~asset.root", &path)
    }

    /// Where an asset name resolves to, given the root [`gpui::asset_root`] declared.
    ///
    /// The same resolution `gpui::image` and `gpui::icon` use, for a program that wants to read
    /// the file itself — a font, a stylesheet, a data file — or simply to check that its assets
    /// are where it thinks they are.
    fn asset_path(name: String) -> String {
        view::asset_path(&name)
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

    /// A scrollbar for a scrolled element, as a track with a thumb on it.
    ///
    /// `id` is the name the scrolled container was drawn under — the same one
    /// [`gpui::scroll_offset`] reads — `axis` is `"x"` or `"y"`, and the style is an ordinary element
    /// style: its `bg=` is the track, its `w=`/`h=` the bar's thickness, and a `thumb=` token the
    /// thumb's colour. Pressing or dragging on the track moves the thumb to the pointer and sets the
    /// offset with it, which is what makes this a scrollbar rather than a picture of one.
    ///
    /// A track with nothing to scroll draws no thumb at all. [`gpui::scrollbar_thumb`] is the same
    /// arithmetic as a reader, for a program drawing its own.
    fn scrollbar(id: String, axis: String, style: String) -> i64 {
        view::push(Kind::Scrollbar { id, axis }, style, String::new())
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

    /// Raise `id` with the new appearance whenever the system's changes.
    ///
    /// The payload is the new appearance, `"light"` or `"dark"` — the same two words
    /// [`gpui::appearance`] answers. Installed once; a second call with the same id
    /// is ignored.
    ///
    /// This is the running half of [`gpui::appearance`]: reading it tells a program
    /// what the window looks like when it rebuilds, and this tells it when that
    /// answer has changed, so a program that is sitting still can re-colour itself
    /// instead of waiting for the next rebuild to notice.
    ///
    /// Nothing inside a program can cause the change — gpui has no public appearance
    /// setter, and registering does not replay the current value — so this fires only
    /// when the operating system's appearance actually changes.
    fn observe_appearance(id: String) {
        store::request(store::Request::ObserveAppearance { id })
    }

    /// What a style value resolves to right now, as `"#rrggbbaa"`.
    ///
    /// Literals resolve to themselves — `"#ff0000"` answers `"#ff0000ff"`, `"white"` and
    /// `"grad=…"`'s colours likewise — and a `theme:<name>` value resolves against this
    /// window's colour table: the platform's palette for its appearance, the theme
    /// [`gpui::set_theme`] chose, and any `theme.<field>=` overrides, in that order.
    ///
    /// The same call the element path makes, for a program that wants to see what it is
    /// about to draw with — a colour to branch on, a contrast to compute, or simply to
    /// check a token before committing it. `name` is one of [`gpui::colour_names`];
    /// anything else is reported by [`gpui::unknown_style_tokens`] and draws black.
    fn resolve_colour(spec: String) -> String {
        view::resolve_colour(&spec)
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
    /// **Every** keystroke the window receives, including the ones an element
    /// handled: backspace and the arrows while a text editor is focused, a click's
    /// key while a field has it. Upstream's `cx.observe_keystrokes` cannot promise
    /// this — gpui stops its observers at the first handler that stops propagation
    /// (`gpui-0.2.2/src/window.rs:3861-3869`) and the editor does that for every key
    /// it consumes — so this is reported from a listener in the **capture** phase
    /// instead, which is dispatched from the window's root down to the focused
    /// element and before any element can stop anything.
    ///
    /// The payload is gpui's unparsed keystroke, e.g. `"shift-tab"` or `"a"`, and it
    /// is raised for keys nothing handled too. Contrast [`gpui::on_keys`], which
    /// takes focus for the window and reports only the key's name.
    fn observe_keystrokes(id: String) {
        store::request(store::Request::ObserveKeys { id })
    }

    /// Deliver a keystroke to this window as if it had been typed.
    ///
    /// `stroke` is gpui's keystroke grammar: `"a"`, `"shift-tab"`, `"cmd-c"`,
    /// `"up"`, `"escape"`. It goes through the window's ordinary dispatch, so the
    /// focused element handles it exactly as it would a real key — an editor
    /// inserts the character, `cmd-z` reaches whatever is bound to it — and every
    /// observer of [`gpui::observe_keystrokes`] sees it.
    ///
    /// The delivery happens **after the frame that asked for it**, because
    /// dispatching a key walks the dispatch tree of the frame already on screen;
    /// read the effect on the next frame. A string this binding cannot parse raises
    /// `"~key.bad|<stroke>"` rather than failing silently.
    fn send_key(stroke: String) {
        store::request(store::Request::SendKey { stroke })
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
    /// An element made focusable with the `focusable` token *used* to be invisible
    /// here — GPUI builds that handle internally and never hands it over — so this
    /// answered `false` for it even while it was focused. `MR5`'s change gives such an
    /// element a handle of this crate's own as well, so it *should* be visible now;
    /// **that is unverified**, because the probe that would confirm it found no focus
    /// moving at all (`tests/evidence/MR5.probe.txt`), and [`gpui::focused`] is the
    /// instrument that will say which of the two is true.
    fn is_focused(id: String) -> bool {
        view::is_focused(&id)
    }

    /// The id of the element that currently holds focus, or `""` when none does.
    ///
    /// [`gpui::is_focused`] answers about one id a program already knows; this answers which one, which
    /// is what a program drawing its own focus ring needs, and what tells a diagnostic "nothing has
    /// focus" apart from "something has focus that you did not ask about".
    ///
    /// When more than one handle reports focus at once — a focused element inside a focused container —
    /// this answers the lexicographically first, because a set has no order.
    fn focused() -> String {
        view::focused()
    }

    /// Ask the platform which of this program's windows is active.
    ///
    /// The answer arrives on the **next frame**, because finding it means reading every open window and GPUI
    /// refuses that from inside a frame. Read it with [`gpui::active_window`].
    fn ask_active_window() {
        store::request(store::Request::AskActiveWindow)
    }

    /// The window the platform last named as active, or `0` when it named none this binding tracks.
    ///
    /// `0` is also what this answers before [`gpui::ask_active_window`] has been called, and what it answers if
    /// the platform names no active window at all — which happens when this application is not frontmost. The
    /// two are not distinguished, on purpose: both mean "no window of ours is the active one".
    fn active_window() -> i64 {
        store::get_int("~active.window", 0)
    }

    /// Hide the application, every window of it — the platform's own Hide.
    ///
    /// **Nothing comes back**, and the program does not stop: its windows are put away, and a person brings them
    /// back from the dock. This is the Hide that MN2's row found no way to express in the application menu —
    /// gpui 0.2.2 offers no `os_action` for it — so a program that wants one calls this from a menu item of its
    /// own.
    fn hide() {
        store::request(store::Request::HideApplication)
    }

    /// Ask the platform to re-read its windows.
    ///
    /// For a program that has seen a display change or a window it did not open; gpui keeps its own list, and
    /// this is the call that refreshes it. Nothing comes back.
    fn refresh() {
        store::request(store::Request::RefreshWindows)
    }

    /// Show `path` in the platform's file manager — Finder on this machine.
    ///
    /// **Nothing observable comes back.** The platform opens a window of its own, the binding has nothing to
    /// read, and no event is raised; a program hears about it only from the person watching. That is the whole
    /// of the capability, and the row says so rather than pretending a check exists.
    fn reveal(path: String) {
        store::request(store::Request::Reveal { path })
    }

    /// Hand `path` to whatever the platform uses to open a file of its kind.
    ///
    /// As unobservable as [`gpui::reveal`], and for the same reason. On macOS this opens the file in its
    /// default application, which is why it is worth having — and why it must be asked for rather than
    /// assumed.
    fn open_with_system(path: String) {
        store::request(store::Request::OpenWithSystem { path })
    }

    /// Open `url` in whatever the platform uses for URLs — the default browser, on this machine.
    ///
    /// As unobservable as [`gpui::reveal`] and [`gpui::open_with_system`]: the browser is another program's
    /// window, and this binding neither waits for it nor hears from it. The URL is handed over unvalidated —
    /// the platform decides what it will accept.
    fn open_url(url: String) {
        store::request(store::Request::OpenUrl { url })
    }

    /// Ask the person for files or folders, with `prompt_for_paths` behind it.
    ///
    /// `id` names the answer. `what` is `"files"` or `"folders"` — `"folders"` asks for a directory instead.
    /// `prompt` is the sentence the panel shows; empty means the platform's own.
    ///
    /// The answer arrives as the event `id`, whose payload is the **number of paths chosen**, or
    /// `"cancelled"`, or `"failed"`. The paths themselves are left in the store's list under `"<id>.paths"`:
    ///
    /// ```gossamer
    /// gpui::open_files("open", "files", "Choose a document")
    /// // …and later, in a frame…
    /// for event in gpui::take_events() {
    ///     let name, count = gpui::event_parts(event)
    ///     if name == "open" { let paths = gpui::list_all("open.paths") … }
    /// }
    /// ```
    ///
    /// A `Vec<PathBuf>` cannot cross this ABI, so the shim is **strings** — one path per entry, absolute,
    /// exactly as the platform reported them.
    fn open_files(id: String, what: String, prompt: String) {
        store::request(store::Request::OpenPicker { id, what, prompt })
    }

    /// Ask the person where to save a file, with `prompt_for_new_path` behind it.
    ///
    /// `directory` is where the panel opens — empty means the program's working directory. `suggested` is a
    /// name to start from; empty means none.
    ///
    /// The chosen path is left as text under `"<id>.path"`, and the event `id` carries `"saved"`,
    /// `"cancelled"` or `"failed"`.
    fn save_file(id: String, directory: String, suggested: String) {
        store::request(store::Request::SavePicker {
            id,
            directory,
            suggested,
        })
    }

    /// Give focus to the element with this id.
    ///
    /// The element must be one this binding has a handle for — an `input`, an `editor`, a `button`, or any
    /// element whose style asks to be `focusable`. An id with no handle does nothing, and the reader says
    /// so: ask [`gpui::focused`] on the next frame to see whether it took.
    ///
    /// This is how a program opens a dialog with its first field ready, rather than making a person click.
    fn focus(id: String) {
        store::request(store::Request::Focus { id })
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

    /// Set this window's title, at any time.
    ///
    /// The title is a program's to change as its state changes — an editor's dirty marker is the usual
    /// reason, and it is the program's to compose: `gpui::set_window_title(f"{name} — edited")`.
    ///
    /// Read back with [`gpui::window_option`]`("title")`, which answers what the **platform** says the
    /// title is, not what this call asked for. That is `""` until the window has painted once.
    fn set_window_title(title: String) {
        store::request(store::Request::SetTitle { title })
    }

    /// Resize the window this frame belongs to.
    fn resize_window(w: f64, h: f64) {
        store::request(store::Request::ResizeWindow { w, h })
    }

    /// Toggle the window this frame belongs to in and out of fullscreen.
    fn toggle_fullscreen() {
        store::request(store::Request::ToggleFullscreen)
    }

    /// Zoom the window this frame belongs to, the way a titlebar's zoom button
    /// does: fill the display's work area, or go back to the size it had.
    fn zoom_window() {
        store::request(store::Request::ZoomWindow)
    }

    /// One window option, as the window itself answers it.
    ///
    /// The asked-for names are fixed when the window opens — `min`, `min_w`,
    /// `min_h` (`"420,240"` and its two halves), `app_id`, `tabbing`, `kind`
    /// (`normal`/`popup`/`floating`), `place` (`center`/`at`/`""`), `start`
    /// (`windowed`/`maximized`/`fullscreen`), `background`, `titlebar`, and the
    /// five flags `show`, `movable`, `minimizable`, `focus`, `resizable` as
    /// `"1"`/`"0"`.
    ///
    /// The observed names are read from the window once a frame and answer `""`
    /// until it has painted: `scale` (the scale factor), `maximized`,
    /// `fullscreen`, and `display` (the id `gpui::display_field` reports).
    ///
    /// Anything else answers `""`, and so does every name in a program with no
    /// window — which is what `gos test` runs in.
    fn window_option(name: String) -> String {
        options::window_option(&name)
    }

    /// The window options in `spec` this binding does not understand.
    ///
    /// The same list that is printed as `gpui: unknown window options: …` when a
    /// window is opened with one, for a program that builds an option string from
    /// data and would rather check it than read stderr.
    fn unknown_window_tokens(spec: String) -> Vec<String> {
        options::unknown_window_tokens(&spec)
    }

    /// Install an application menu.
    ///
    /// Each entry is one of `"separator"`, `"quits:Quit"`, or `"events:<label>"` —
    /// and an `events` entry may name a third part, the **accelerator** it declares:
    /// `"events:Save:cmd-s"`. An `events` entry raises `"menu|<slot>"` when it is
    /// chosen. Menu items carry a boxed action, which no ABI can pass, so the binding
    /// owns eight action slots and maps entries onto them by counting **`events`
    /// entries** — a `"separator"` between two actions does not renumber the second
    /// one, and a menu with more than eight `events` entries reuses slot 0.
    /// [`gpui::menu_slots`] answers the same numbering.
    ///
    /// An accelerator is not a field on the item — `MenuItem` has none. It is a key
    /// *binding*: the binding binds the stroke to the entry's own slot action before
    /// it installs the menu, so the shortcut both fires the item (the same
    /// `"menu|<slot>"` a click raises) and is displayed beside its label, which is
    /// where the platform gets it from — the macOS menu looks each item's action up
    /// in the keymap and sets its key equivalent from the result. A stroke gpui
    /// cannot parse raises `"~keybind.bad|<stroke>"` rather than being bound or
    /// panicking. [`gpui::menu_accelerators`] reads the declarations back.
    fn set_menus(name: String, items: Vec<String>) {
        store::request(store::Request::SetMenus { name, items })
    }

    /// The action slot each `"events:"` entry of a menu answers with, as
    /// `"<slot>:<label>"`, in order.
    ///
    /// The slot is the entry's index among the menu's **`events` entries**, not among its
    /// entries: a separator between two actions does not renumber the second one. That is
    /// what [`gpui::set_menus`] means by `"menu|<slot>"`, and it is read from the same
    /// function the menu is built with, so a program can compute its handler table instead
    /// of guessing at it.
    fn menu_slots(items: Vec<String>) -> Vec<String> {
        app::menu_slots(&items)
    }

    /// The structure a menu's entries describe, as `"<depth>:<kind>:<label>"`, in order.
    ///
    /// What the program declared, read back — the nesting included. Depth 0 is an item of the
    /// menu itself; depth `d` belongs to the submenu opened at `d - 1`, which is written as `>`
    /// markers in front of the entry, one per level.
    fn menu_tree(items: Vec<String>) -> Vec<String> {
        app::menu_tree(&items)
    }

    /// Install the menu the dock icon offers on a right-click.
    ///
    /// The same entry grammar as [`gpui::set_menus`], including `submenu:` and the `>` nesting, but
    /// for the application rather than for a window — one dock menu, not one per window. An entry
    /// that declares an accelerator gets it, exactly as a menu-bar entry does.
    ///
    /// gpui offers no way to read this menu back (`App::set_dock_menu` has no getter), so a program
    /// can install it and not inspect it.
    fn set_dock_menu(items: Vec<String>) {
        store::request(store::Request::SetDockMenu { items })
    }

    /// The system selectors among the installed menu bar's items, as
    /// `"<depth>:<label>:<selector>"`.
    ///
    /// What [`gpui::installed_menu`] cannot show: an `"os:"` entry is only doing its job if the
    /// **selector** reached the platform, and the OS is what acts on it. `selector` is one of
    /// `cut`, `copy`, `paste`, `selectAll`, `undo`, `redo`.
    fn installed_menu_os_actions() -> Vec<String> {
        app::installed_menu_os_actions()
    }

    /// The menu bar as the platform has it: `"<depth>:<kind>:<label>:<action>"` per item.
    ///
    /// [`gpui::menu_tree`] answers what a program *declared*; this answers what was **built** from
    /// it, read back from the platform after the menu was installed. Depth 0 is the application
    /// menu, depth 1 its items, depth 2 a submenu's items, and so on — so a program can check
    /// that its nesting survived the build rather than assuming it. The fourth field names the
    /// action each item was given, which no keystroke can stand in for.
    fn installed_menu() -> Vec<String> {
        app::installed_menu()
    }

    /// The entries of a menu that had nowhere to go, in order.
    ///
    /// An unknown kind, or one indented deeper than any submenu above it. Installing a menu
    /// raises `"~menu.bad|<entry>"` for each one as well, so a program that did not think to ask
    /// still hears about it.
    fn menu_bad(items: Vec<String>) -> Vec<String> {
        app::menu_bad(&items)
    }

    /// The accelerator each `"events:"` entry of a menu declares, as
    /// `"<slot>:<stroke>"`, in slot order.
    ///
    /// An entry names one with a third part: `"events:Save:cmd-s"`. Rides on
    /// [`gpui::menu_slots`]'s numbering — the third part of an entry does not change
    /// which slot it takes.
    fn menu_accelerators(items: Vec<String>) -> Vec<String> {
        app::menu_accelerators(&items)
    }

    // -----------------------------------------------------------------
    // settings that survive a restart
    // -----------------------------------------------------------------

    /// Write one setting to the file at `path`, creating the file and its directory.
    ///
    /// `true` when it was written. The format is one `key=value` per line, so the file is
    /// something a person can read, edit and delete keys from; writing one key leaves the others
    /// alone, and rewrites the file whole.
    fn settings_set(path: String, key: String, value: String) -> bool {
        settings::set(&path, &key, &value).is_ok()
    }

    /// One setting from the file at `path`, or `""` when it is not there.
    ///
    /// A file that does not exist is not an error: a program's first run has nothing to read.
    fn settings_get(path: String, key: String) -> String {
        settings::get(&path, &key)
    }

    /// Remove one setting, leaving the rest. `true` when the file was written.
    fn settings_forget(path: String, key: String) -> bool {
        settings::forget(&path, &key).is_ok()
    }

    /// Remember a window's bounds under `key`, for the next run to read back.
    ///
    /// Four numbers, because that is what a window's bounds are and what
    /// [`gpui::open_window`] takes. The program supplies them — read them with
    /// [`gpui::window_field`]`("x")` and its three neighbours — rather than the binding
    /// reaching for the window itself, which is what makes this half of the row checkable
    /// without one. `true` when the file was written.
    fn remember_window_state(
        path: String,
        key: String,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) -> bool {
        settings::remember_bounds(&path, &key, x, y, w, h).is_ok()
    }

    /// A remembered window's bounds as `"x,y,w,h"` — the shape
    /// [`gpui::observe_window_bounds`] raises — or `""` when nothing whole is stored.
    ///
    /// **A half-written state is nothing**: if any of the four numbers is missing or is not a
    /// number, this is empty rather than a window at 0,0.
    fn saved_bounds(path: String, key: String) -> String {
        settings::bounds(&path, &key)
    }

    /// One field of a remembered window — `"x"`, `"y"`, `"w"` or `"h"` — or `0.0` when nothing
    /// whole is stored, so a half-written state cannot put a window at 0,0 one number at a time.
    fn saved_field(path: String, key: String, field: String) -> f64 {
        settings::field(&path, &key, &field)
    }

    /// Remember the bounds of the window this frame belongs to, under `key`.
    ///
    /// The live half of [`gpui::remember_window_state`], for a program that would rather the
    /// binding read the window than read it itself with [`gpui::window_field`]. Applied during the
    /// frame that asked for it; a failure to write raises `"~settings.bad|<why>"`.
    fn save_window_state(path: String, key: String) {
        store::request(store::Request::SaveWindowState { path, key })
    }

    /// The window's x, y, width or height, as of the start of this frame.
    ///
    /// `field` is `x`, `y`, `w` or `h` for the window itself, `inner_x`,
    /// `inner_y`, `inner_w` or `inner_h` for its *content* rectangle — the same
    /// numbers where the platform draws no decoration of its own, which is this
    /// one — and `restore_w` or `restore_h` for the size a maximized or fullscreen
    /// window would go back to, which is zero while it is windowed.
    fn window_field(field: String) -> f64 {
        let (x, y, w, h) = view::window_rect();
        let (inner_x, inner_y, inner_w, inner_h) = view::inner_rect();
        let (_, restore_w, restore_h) = view::bounds_state();
        match field.as_str() {
            "x" => x,
            "y" => y,
            "w" => w,
            "h" => h,
            "inner_x" => inner_x,
            "inner_y" => inner_y,
            "inner_w" => inner_w,
            "inner_h" => inner_h,
            "restore_w" => restore_w,
            "restore_h" => restore_h,
            _ => 0.0,
        }
    }

    /// What the platform says the window's state is: `"windowed"`, `"maximized"`
    /// or `"fullscreen"`.
    ///
    /// Read from `Window::window_bounds`, so it is the platform's answer and not
    /// the binding's guess — which matters, because the two do not always agree:
    /// macOS reports a *zoomed* window as windowed, since it has no maximized state
    /// of its own to be in, and reports `"fullscreen"` only for a real fullscreen
    /// window. See [`gpui::window_field`]'s `restore_w` and `restore_h` for the size
    /// the window would go back to.
    fn window_bounds_state() -> String {
        view::bounds_state().0.to_string()
    }

    /// Raise `id` whenever the window is moved or resized.
    ///
    /// The event's payload is the new geometry, `"x,y,w,h"`, so a program can read
    /// it with [`gpui::event_parts`] without waiting for the next frame to ask
    /// [`gpui::window_field`]. Installed once; a second call with the same id is
    /// ignored.
    fn observe_window_bounds(id: String) {
        store::request(store::Request::ObserveBounds { id })
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

    /// Route every keystroke to the program as a `"key|<stroke>"` event, and every
    /// release as `"keyup|<stroke>"`.    ///
    /// `<stroke>` is gpui's unparsed keystroke — the whole thing, modifiers
    /// included: `"shift-tab"`, `"cmd-c"`, `"a"`, `"up"`. It used to be only the
    /// key's name, and gpui maps **both** Tab and Shift-Tab to the name `"tab"`,
    /// so a program had no way to tell the two apart; the modifiers under
    /// `~key.*` below say the same thing one bit at a time.
    ///
    /// The window itself takes focus to receive them, and it does so **on the frame
    /// you turn this on** — whatever frame that is. It used to take focus only while
    /// the window was in its first two frames, so a program that enabled the keyboard
    /// later got a listener and no focus, and a key event with no focus is delivered
    /// to nothing: its shortcuts were dead with no error to show for it.
    ///
    /// Once it has focus the window keeps it until you give it up — `on_keys(false)`
    /// and then `true` takes it again. Clicking into a text field moves focus there
    /// and leaves it there while the field is focused.
    ///
    /// The last press is also readable from the state store without draining the
    /// event queue: `~key.unparse` (the whole stroke), `~key.key` (the name on the
    /// key), `~key.char` (what would have been typed), `~key.mods` (`"shift"`,
    /// `"ctrl-alt"`, `""`), `~key.shift` / `.control` / `.alt` / `.platform` /
    /// `.function`, and `~key.held`. The release mirrors them under `~keyup.`.
    ///
    /// Off by default. [`gpui::observe_keystrokes`] reports keys without asking for
    /// focus at all, and unlike this it fires for keys an element handled.
    fn on_keys(enabled: bool) {
        if !enabled {
            // Forgetting that the window was armed is what lets it take focus
            // again the next time it is asked to.
            view::disarm_keys(crate::window::current());
        }
        store::set_flag("~keys", enabled)
    }

    /// Bind keystrokes to the program's own event names.
    ///
    /// `spec` is `"<stroke>=<name>"` pairs separated by `;`:
    /// `"cmd-s=save; cmd-shift-z=redo"`. Each stroke is gpui's keystroke grammar
    /// (`"cmd-s"`, `"ctrl-alt-p"`, `"f5"`), and each name is any event id the
    /// program likes. When a bound stroke happens, the binding raises `"<name>"` —
    /// a bare id, the way `ui::button` does — in the window that asked for it.
    ///
    /// A name may carry a **key context** after an `@`, and that is what makes a
    /// binding focus-scoped: `"cmd-s=save@Editor"` fires only while the focused
    /// element is inside something that names the `Editor` context — an element
    /// styled `ctx=Editor`. The context is gpui's own predicate grammar, so
    /// `"!Editor"`, `"A && B"` and `"A || B"` are all accepted; a binding with no
    /// context is enabled whenever the keymap is consulted, and a deeper context
    /// match wins over a shallower one. A predicate gpui cannot parse raises
    /// `"~keybind.bad|<context>"` rather than panicking.
    ///
    /// This is the shape an `Action` takes here. Upstream binds a keystroke to a
    /// *program-authored* action, and that cannot cross this ABI: a trait
    /// implementation is never a value (R3) and the traits cannot be named (R2).
    /// What a program needs is narrower — "when this stroke happens, tell me which
    /// one" — and that is exactly what the event carries, so no program-side action
    /// type is needed.
    ///
    /// Bindings are added, never replaced: calling this twice binds both sets, and
    /// a stroke bound twice runs both actions — unless their contexts differ enough
    /// for gpui to prefer one. A pair that is not a pair is dropped rather than
    /// bound to something; read what was bound back with [`gpui::keymap`], or parse
    /// a spec without binding it with [`gpui::keymap_plan`]. A stroke gpui cannot
    /// parse raises `"~keybind.bad|<stroke>"`. ⌘Q stays bound to quitting.
    fn bind_keys(spec: String) {
        store::request(store::Request::BindKeys { spec })
    }

    /// The keymap a program has asked for, as `"<stroke>:<name>"` — or
    /// `"<stroke>:<name>@<context>"` when the binding carries one — in the order it
    /// asked for it.
    fn keymap() -> Vec<String> {
        app::keymap_spec()
    }

    /// What a keymap spec reads as, without binding anything: `"<stroke>:<name>"`
    /// per well-formed pair, malformed pairs dropped.
    ///
    /// The same parse [`gpui::bind_keys`] uses, for a program that wants to check a
    /// spec — or hold one — before committing a keystroke to it.
    fn keymap_plan(spec: String) -> Vec<String> {
        app::keymap_plan(&spec)
    }

    /// Choose which of the platform's two palettes [`gpui::default_color`] answers from.
    ///
    /// `name` is `"system"`, `"light"` or `"dark"`. `"system"` is the default and follows
    /// the window's own appearance; the other two pin the named colour table to a palette
    /// whatever the desktop is set to, which is what a program offering
    /// "Appearance: System / Light / Dark" needs.
    ///
    /// It changes nothing else. [`gpui::appearance`] still reports what the operating
    /// system says, because choosing how to draw is not the same as lying about the
    /// desktop; and the platform's own window chrome is not repainted, because the
    /// palette is the one a *program* reads and draws with — put it in a style string.
    ///
    /// It used to record a name nothing consulted, so a program could name a theme and
    /// get the platform's colours anyway.
    ///
    /// A name the binding does not know is refused: the choice stands and the program
    /// hears `"~theme.bad|<name>"`. Read the current choice back with [`gpui::theme`];
    /// as with any other request, the next frame is the first that answers from it, so
    /// a program that is not animating should ask for one to see the change.
    fn set_theme(name: String) {
        store::request(store::Request::SetTheme { name })
    }

    /// The palette the program asked for, `"system"` by default.
    ///
    /// What [`gpui::set_theme`] chose, not what the desktop is: [`gpui::appearance`]
    /// answers that.
    fn theme() -> String {
        view::theme_choice(crate::window::current())
    }
);
