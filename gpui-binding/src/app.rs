//! The GPUI application: one window, one view callback, re-rendered whenever
//! state changes, a timer fires, or an animation asks for another frame.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    px, size, AnyElement, App, Application, Context, EntityId, ImgResourceLoader, PromptLevel,
    Render, Resource, Timer, Window,
};

use gossamer_binding::{NativeDispatch, PersistentCallback};

use crate::editor;
use crate::options::{self, Start};
use crate::prompts;
use crate::settings;
use crate::store::{self, Request};
use crate::view;
use crate::window;

/// One root view per window. Every window's root holds the *same* callback —
/// the one `gpui::run` was given — and rebuilds the element tree from scratch
/// on every frame it is asked for.
///
/// A window cannot have a view callback of its own, handed to `gpui::open_window`:
/// a callback there is released the moment `open_window` returns to its caller, so on
/// the compiled tiers a second window's view answers an error and draws an empty
/// frame. There is exactly one callback a binding may store — the one `run` was
/// given, which is never released because `run` does not return — so there is exactly
/// one view, and a program tells its windows apart with `gpui::window_id()`.
pub struct Root {
    view: PersistentCallback,
    /// The window this root is the view of. Everything the frame touches is
    /// scoped to it.
    window: i64,
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().entity_id();
        let win = self.window;

        // The whole frame — the view callback, its translation into elements,
        // and the requests it raised — runs as this window. Anything the
        // program asks for inside it lands in this window and no other.
        window::with(win, || {
            view::begin_frame(window, cx);
            view::reset();
            // A frame names its root with `gpui::root`, or simply answers the
            // element it wants; one that does neither reports itself.
            if let Err(e) = view::invoke(&self.view, vec![]) {
                view::note_error(format!("the view callback failed: {e}"));
            }
            let root: Option<i64> = view::named_root();
            let element: AnyElement = view::finish(root, window, cx, entity);

            if process_requests(window, cx, entity, win) {
                // The program asked to keep animating: line up the next frame.
                window.request_animation_frame();
            }
            // The prompt queue's turn: raise its head as a platform sheet, and collect the answer to
            // whichever sheet is up.
            pump_prompt(win, window, cx);

            element
        })
    }
}

// Windows a frame asked for. Kept here rather than in `store::Request`
// because a `static Mutex` would demand a `Send` callback.
thread_local! {
    /// Which windows have a prompt-task running. One at a time per window: gpui can only have one
    /// prompt up, and a second raised over a live one is a call it refuses in its own words
    /// (`unreachable!("Re-entrant window prompting is not supported by GPUI")`).
    static IN_FLIGHT: RefCell<HashSet<i64>> = RefCell::new(HashSet::new());
}

/// Turns `win`'s prompt queue into platform sheets, one at a time, collecting each answer.
///
/// This is where [`crate::prompts`]' queue becomes a *queue*: the head comes up and the next waits for
/// the answer to arrive. That is the whole difference between a queue of prompts and a list of
/// questions, and it is why gpui's one-prompt-at-a-time rule is not a program's problem.
///
/// **The answer is waited for on a timer, not on a frame.** macOS throttles the animation frames of a
/// window that is not frontmost — one frame every twenty seconds — so a
/// per-frame poll would leave an answered sheet sitting there, and the program's event would arrive
/// late. `try_recv` on a timer keeps running while the window is idle, and `cx.update` is how the task
/// gets the window back to raise the next sheet.
fn pump_prompt(win: i64, window: &mut Window, cx: &mut Context<Root>) {
    if IN_FLIGHT.with(|flight| flight.borrow().contains(&win)) {
        return;
    }
    if prompts::head_in(win).is_none() {
        return;
    }
    IN_FLIGHT.with(|flight| {
        flight.borrow_mut().insert(win);
    });
    let entity = cx.entity().entity_id();
    window
        .spawn(cx, async move |cx| {
            loop {
                // The head's turn. If the window has gone, there is nothing to raise.
                let raised = cx
                    .update(move |window, cx| {
                        let prompt = prompts::head_in(win)?;
                        let labels: Vec<&str> =
                            prompt.buttons.iter().map(|label| label.as_str()).collect();
                        let asked = window.prompt(
                            PromptLevel::Info,
                            &prompt.message,
                            Some(&prompt.title),
                            &labels,
                            cx,
                        );
                        Some((prompt.id, asked))
                    })
                    .ok()
                    .flatten();
                let Some((id, mut asked)) = raised else {
                    break;
                };
                // A program can hear that a question is on screen, which is the only way to tell a
                // sheet that never came up from one nobody answered.
                window::with(win, || {
                    store::push_event_with_in(win, "prompt.shown", format!("{id}"));
                });

                let choice = loop {
                    Timer::after(Duration::from_millis(100)).await;
                    if let Ok(Some(choice)) = asked.try_recv() {
                        break choice;
                    }
                };
                window::with(win, || {
                    prompts::answer_and_raise(win, id, choice as i64);
                });
                let _ = cx.update(|_window, cx| cx.notify(entity));
            }
            IN_FLIGHT.with(|flight| {
                flight.borrow_mut().remove(&win);
            });
        })
        .detach();
}

/// Whether the application survives its last window closing.
///
/// Off by default: a program that opens one window and closes it expects the
/// process to end, which is what every example here does.
static KEEP_ALIVE: AtomicBool = AtomicBool::new(false);

/// Sets whether the application survives its last window closing.
pub fn set_keep_alive(on: bool) {
    KEEP_ALIVE.store(on, Ordering::SeqCst);
}

/// The event a revived window finds waiting in its own queue when it opens.
///
/// It is the program's way of *learning* that it had closed its last window:
/// no frame runs while an app has no windows, so there is no moment at which a
/// program could be told. The window the binding opens in answer to
/// `gpui::revive_window` is that moment, and the event is what makes it
/// observable rather than merely true.
pub const WINDOW_CLOSED_EVENT: &str = "window.closed";

/// The last window has closed.
///
/// The program's standing instruction — `gpui::revive_window`, recorded during a
/// frame while it still had a window — is carried out here: one is opened and the
/// app carries on with it. Otherwise the app ends, unless the program asked to be
/// kept alive, which leaves it running and inert — nothing of the program's runs
/// without a window.
///
/// This does not run the program's `on_window_closed` *handler*, a stored Gossamer
/// callback: a compiled caller releases that callback when the binding fn that
/// received it returns, so on the compiled tiers the handler fails; and it is the
/// one capability whose moment — an app with no window — is defined by there being
/// no frame to run program code in.
fn last_window_closed(cx: &mut App) {
    let mut pending = PENDING_OPEN.with(|q| std::mem::take(&mut *q.borrow_mut()));
    // Taken, not read: a revival is one window for one closure. A program that
    // wants to be revived again asks again, which it can do in the revived
    // window's first frame — and a program that asked once is not trapped in a
    // window it cannot close.
    if let Some(mut revive) = REVIVE.with(|r| r.borrow_mut().take()) {
        revive.revival = true;
        pending.push(revive);
    }
    if !pending.is_empty() {
        // Deferred, not here: this runs while the last window is still being torn
        // down, and `open_window` draws the new one immediately.
        cx.defer(move |cx| open_pending(cx, pending));
        return;
    }
    if KEEP_ALIVE.load(Ordering::SeqCst) {
        return;
    }
    release_retained();
    cx.quit();
}

/// Opens the windows a frame asked for, once that frame is done.
fn open_pending(cx: &mut App, pending: Vec<PendingOpen>) {
    for request in pending {
        // Each window gets an id of its own before it is drawn, so the very first
        // frame of the new window already knows which one it is.
        let child = window::allocate();
        let options = options::build(
            cx,
            &request.title,
            request.x,
            request.y,
            request.w,
            request.h,
            &request.options,
            false,
            child,
        );
        let Some(view) = app_view() else {
            // No frame can have asked for a window, so there is no view to draw
            // it with; saying so beats a window that never paints.
            eprintln!("gpui: open_window was called with no run() view in place");
            continue;
        };
        if request.revival {
            // Waiting in its queue before its first frame runs, so the program
            // sees it the first time it drains events in the new window.
            store::push_event_in(child, WINDOW_CLOSED_EVENT);
        }
        let _ = cx.open_window(options, move |window, cx| {
            install_appearance_colors(window, cx);
            // Here, where the window is built, rather than on its first frame:
            // see `process_requests`. This is the last moment before the draw
            // that holds the platform's callback table.
            window.on_window_should_close(cx, move |_window, _cx| view::may_close(child));
            cx.new(|_| Root { view: view.clone(), window: child })
        });
    }
}

// Windows a frame asked to open, once the frame that asked for them is done.
// A plain comment: `rustdoc` does not document items a macro produces.
thread_local! {
    static PENDING_OPEN: RefCell<Vec<PendingOpen>> = const { RefCell::new(Vec::new()) };
    /// The view callback `gpui::run` was given, kept for the life of the app.
    ///
    /// Every window's root holds a clone of it, which is what makes a second
    /// window possible without a second callback. It is the one callback a
    /// binding may store: `run` does not return until the app exits, so
    /// the caller's release — the one a compiled caller emits as soon as a
    /// binding fn returns — is never reached.
    static APP_VIEW: RefCell<Option<PersistentCallback>> = const { RefCell::new(None) };
}

/// The view callback `gpui::run` was given.
fn app_view() -> Option<PersistentCallback> {
    APP_VIEW.with(|v| v.borrow().clone())
}

/// Forgets everything the app was holding, on the way out.
///
/// It is housekeeping, but it stays on every quit path: a callback whose `Drop`
/// calls back into the runtime to release a registration is dropped at *thread
/// exit*, after the runtime's own thread-locals are gone, and the runtime then
/// panics at its FFI entry:
///
/// ```text
/// thread '<unnamed>' panicked: cannot access a Thread Local Storage value
/// during or after destruction: AccessError
///   ... _gos_rt_binding_callback_release
/// ```
///
/// The one callback this binding stores is a `PersistentCallback`, which frees
/// nothing when dropped, so the panic cannot happen — but the guard stays, because
/// the next thing to be stored there might not be so lucky.
fn release_retained() {
    view::release_retained();
    PENDING_OPEN.with(|q| q.borrow_mut().clear());
    PENDING_CLOSE.with(|q| q.borrow_mut().clear());
    REVIVE.with(|r| *r.borrow_mut() = None);
    APP_VIEW.with(|v| *v.borrow_mut() = None);
}

// Windows a frame asked to close *by name*, closed once that frame is done.
//
// A `thread_local`, for the same reason as `PENDING_OPEN`: a `static Mutex` would
// have to hold whatever a `cx.defer` closure captures. A plain comment, not a doc
// comment, because `rustdoc` does not document items a macro produces.
thread_local! {
    static PENDING_CLOSE: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
}

/// One window waiting to be opened, once the frame that asked for it is done.
///
/// It carries a *description*, not a view: every window is drawn by the one
/// view `gpui::run` was given (see [`Root`]).
pub struct PendingOpen {
    pub title: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub options: String,
    /// Whether this is the window a program asked for with
    /// `gpui::revive_window`, opened because it had none. The revived window
    /// finds `window.closed` waiting in its queue, which is how a program learns
    /// what happened.
    pub revival: bool,
}

thread_local! {
    /// The window a program asked to have opened when it has none, if it asked.
    static REVIVE: RefCell<Option<PendingOpen>> = const { RefCell::new(None) };
}

/// Records the window to open when this app's last window closes.
///
/// It is a description rather than a handler, and it has to be: at that moment
/// there is no window and so no frame, and no program code can run at all. What
/// the program *can* do is say beforehand what it wants, which is exactly
/// `gpui::revive_window`.
pub fn set_revive(pending: PendingOpen) {
    REVIVE.with(|r| *r.borrow_mut() = Some(pending));
}

/// Queues a window. It is opened after the current frame, never during it:
/// `App::open_window` always draws the new window once, and a nested draw
/// would re-enter `Root::render` and reset the element arena the frame is
/// still using.
pub fn queue_window(pending: PendingOpen) {
    PENDING_OPEN.with(|q| q.borrow_mut().push(pending));
}

/// Removes another window, named by the id its view was given.
///
/// Gossamer holds no window handle, and `Window::remove_window` needs one, so
/// this walks the open windows and asks each window's root view which id it is.
/// Every root in this binding is a [`Root`], which is what makes the walk
/// possible at all. It runs from a deferred callback, never from inside a
/// frame: GPUI panics on `attempted to read a window that is already on the
/// stack`, and the window that asked for the close is the one being rendered.
fn close_named_window(cx: &mut App, target: i64) {
    for handle in cx.windows() {
        let id = handle
            .read::<Root, _, _>(cx, |root, app| root.read(app).window)
            .ok();
        if id == Some(target) {
            let _ = handle.update(cx, |_view, window, _app| window.remove_window());
            return;
        }
    }
}

/// Records which window the platform calls active, in the window that asked.
///
/// The answer is a handle, and turning it into a name this crate understands means reading each open window's
/// root view — which GPUI refuses while a frame is on the stack, since the window that asked is the one being
/// rendered. So this runs from a deferred callback, exactly as [`close_named_window`] does. A platform that
/// names no window, or names one this crate does not track, leaves `0`.
fn find_active_window(cx: &mut App, asker: i64) {
    let active = cx.active_window().map(|handle| handle.window_id());
    let mut found = 0;
    for handle in cx.windows() {
        if let Ok(id) = handle.read::<Root, _, _>(cx, |root, app| root.read(app).window)
            && Some(handle.window_id()) == active
        {
            found = id;
            break;
        }
    }
    store::set_int_in(asker, "~active.window", found);
}

/// Installs the default colours for the window's actual appearance.
///
/// `App::init_colors` installs `Colors::default()`, which is `Colors::light()`
/// whatever the system is set to — so `gpui::default_color("background")`
/// answers `#ffffff` on a dark-mode desktop unless this runs. `Colors` also
/// carries no appearance of its own, so the only way to know is to ask a
/// window.
fn install_appearance_colors(window: &mut Window, cx: &mut App) {
    let colors = gpui::colors::Colors::for_appearance(window);
    cx.set_global(gpui::colors::GlobalColors(std::sync::Arc::new(colors)));
}

/// Installed repeating timers, keyed by the window and the event id they emit.
/// One window's `tick("clock", 1000)` must not silence another's.
///
/// The value is the timer's **generation** and its period. A generation is what makes a
/// reschedule or a stop take effect on a loop that is already sleeping: the loop wakes,
/// finds that the entry belongs to a generation other than its own (or is gone), and
/// ends without raising anything. A period alone would not do — rescheduling to the same
/// period would leave the old loop alive alongside the new one, and two timers would
/// raise every event twice.
static TICKS: LazyLock<Mutex<HashMap<(i64, String), (u64, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Numbers every installed timer and delay, so a later call for the same id can retire
/// the task the earlier one started — a repeating timer's loop and a one-shot delay's
/// wait both read it back before they raise anything.
static TIMER_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Pending one-shot delays, keyed by the window and the event id they will raise.
///
/// The value is the delay's generation, for the same reason `TICKS` has one: asking for
/// the same id again restarts the wait, and the task the earlier call started has to find
/// itself retired rather than raise on the old schedule. A delay has no period to read
/// back — it fires once — so presence here is what `gpui::delay_pending` answers.
static DELAYS: LazyLock<Mutex<HashMap<(i64, String), u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether a one-shot delay is still waiting to raise `id` in `win`.
pub fn delay_pending(win: i64, id: &str) -> bool {
    DELAYS.lock().unwrap().contains_key(&(win, id.to_string()))
}

/// A named animation: how long it should take, its curve, and how far along it is.
///
/// The clock is the binding's rather than the program's. `elapsed_ms` is the time it has
/// already run for and `running_since` is when the current run began, so a **pause** is
/// "add the run to the total and stop counting" and a **resume** is "start counting again"
/// — neither of which has to be recomputed from a start time, and neither of which the
/// program has to keep in its own state.
struct Animation {
    duration_ms: f64,
    elapsed_ms: f64,
    running_since: Option<Instant>,
    easing: String,
}

impl Animation {
    /// How long it has been running, in milliseconds, as of now.
    fn elapsed(&self) -> f64 {
        self.elapsed_ms
            + self
                .running_since
                .map(|since| since.elapsed().as_secs_f64() * 1000.0)
                .unwrap_or(0.0)
    }

    /// The raw fraction, 0 to 1: where the animation is, before its curve is applied.
    fn fraction(&self) -> f64 {
        if self.duration_ms <= 0.0 {
            1.0
        } else {
            (self.elapsed() / self.duration_ms).clamp(0.0, 1.0)
        }
    }

    fn finished(&self) -> bool {
        self.elapsed() >= self.duration_ms
    }

    /// Counting *and* unfinished. A paused animation wants no frames, and neither does a
    /// finished one.
    fn wants_frames(&self) -> bool {
        self.running_since.is_some() && !self.finished()
    }
}

/// Background work that is in flight, keyed by the window and the event id it will raise.
///
/// The value is the task's **generation**, for the same reason `TICKS` and `DELAYS` have one:
/// a cancel has to reach a job that is already running on GPUI's background executor, and the
/// only moment anything can act on it is when the work comes back. The task then finds its
/// entry gone (or replaced by a later `task`) and returns without leaving a result or raising
/// anything.
static TASKS: LazyLock<Mutex<HashMap<(i64, String), u64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether background work for `id` is still in flight in `win`.
pub fn task_running(win: i64, id: &str) -> bool {
    TASKS.lock().unwrap().contains_key(&(win, id.to_string()))
}

/// How much of the work for `id` is behind it, `0.0` to `1.0`, and `1.0` once it is done.
pub fn task_progress(win: i64, id: &str) -> f64 {
    store::get_float_in(win, &format!("{id}.progress"), 0.0)
}

/// How often running work reports its progress.
///
/// Fifty milliseconds is a compromise a program cannot make for itself: often enough that a
/// bar moves smoothly at any frame rate a person can see, rare enough that a long job is not
/// asking for twenty repaints a frame. It is also the resolution of the reader, so a job
/// shorter than this reports nothing at all and its progress goes straight from 0 to 1.
const PROGRESS_STEP: Duration = Duration::from_millis(50);

/// The animations a program has started, one set per window.
static ANIMATIONS: LazyLock<Mutex<HashMap<(i64, String), Animation>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The easing curves a program may name, in the order they are documented.
pub fn easing_names() -> Vec<String> {
    ["linear", "ease-in", "ease-out", "ease-in-out"]
        .iter()
        .map(|name| name.to_string())
        .collect()
}

/// Where a curve puts an animation that is `t` of the way along.
///
/// `t` is clamped to 0..1, every curve is anchored at both ends, and a name the binding
/// does not have answers `linear` — the curve is the least important thing about a request
/// to move something, so an unknown one is not an error.
pub fn easing_at(easing: &str, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        "ease-in" => t * t,
        "ease-out" => 1.0 - (1.0 - t) * (1.0 - t),
        "ease-in-out" => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
            }
        }
        _ => t,
    }
}

/// Where the animation `id` is, 0 to 1, with its curve applied. `0.0` when there is none.
pub fn animation_phase(win: i64, id: &str) -> f64 {
    ANIMATIONS
        .lock()
        .unwrap()
        .get(&(win, id.to_string()))
        .map(|animation| easing_at(&animation.easing, animation.fraction()))
        .unwrap_or(0.0)
}

/// What the animation `id` is doing: `"none"`, `"running"`, `"paused"` or `"done"`.
pub fn animation_state(win: i64, id: &str) -> String {
    match ANIMATIONS.lock().unwrap().get(&(win, id.to_string())) {
        None => "none".into(),
        Some(animation) if animation.finished() => "done".into(),
        Some(animation) if animation.running_since.is_none() => "paused".into(),
        Some(_) => "running".into(),
    }
}

/// Whether any of `win`'s animations still wants a frame.
pub fn animations_running(win: i64) -> bool {
    ANIMATIONS
        .lock()
        .unwrap()
        .iter()
        .any(|((window, _), animation)| *window == win && animation.wants_frames())
}

/// The period a running timer repeats at, or `0` when none is installed.
pub fn tick_period(win: i64, id: &str) -> i64 {
    TICKS
        .lock()
        .unwrap()
        .get(&(win, id.to_string()))
        .map(|(_, ms)| *ms as i64)
        .unwrap_or(0)
}

/// Which window's menu is currently installed.
///
/// `cx.set_menus` is application-wide, so the last window to ask for a menu
/// owns the whole bar; the action handler routes the click back to that window
/// rather than to whichever window happened to be opened first.
static MENU_OWNER: LazyLock<Mutex<(i64, Option<EntityId>)>> =
    LazyLock::new(|| Mutex::new((0, None)));


/// One entry of a menu, as the binding will build it.
pub struct MenuEntry {
    /// `"separator"`, `"quits"` or `"events"`.
    pub kind: &'static str,
    pub label: String,
    /// The action slot this entry answers with, counting **`events` entries only**.
    pub slot: usize,
    /// How deep in the nesting this entry sits: `0` is an item of the menu itself.
    pub depth: usize,
    /// The system selector this entry names, for an `"os:"` entry.
    ///
    /// The six [`gpui::OsAction`]s are the system's own: the OS performs `Copy` itself, and an
    /// item wired to one is a different thing from an item wired to an action of the program's.
    pub selector: Option<String>,
    /// The keystroke this entry declares as its accelerator, if it declares one.
    ///
    /// An accelerator is a key *binding*, not a field on the item: gpui hands the
    /// keymap to the platform when it installs a menu, and the macOS menu looks each
    /// item's own action up in it and sets the `NSMenuItem`'s key equivalent and
    /// modifier mask from the result (`platform/mac/platform.rs:355-392`).
    /// `MenuItem` has no accelerator field of its own. So an entry that names a
    /// stroke is bound to its own slot action, and both halves follow: the keymap
    /// shows the shortcut beside the label and fires the item when it is pressed.
    pub stroke: Option<String>,
}

/// Reads a menu's entry list into the items it describes.
///
/// The slot is what `set_menus`'s entries mean by "the menu's slot": the index of the
/// entry among the *events* entries, and not among the entries, so a `"separator"`
/// added in front of an action does not renumber the actions after it — a program
/// that keys its own handlers off `"menu|<n>"` need not renumber them when the menu's
/// shape changes. A separator is not an event and takes no slot.
///
/// An `events` entry may name a third part, its accelerator:
/// `"events:Save:cmd-s"`. A `quits` entry takes none (⌘Q is already bound to
/// quitting).
///
/// One function, two callers: `process_requests` builds the menu from this, and
/// `gpui::menu_slots` answers the same numbering to a program, so the reader and the
/// wiring cannot drift apart.
pub fn menu_plan(items: &[String]) -> Vec<MenuEntry> {
    menu_parse(items).0
}

/// The six system selectors a menu item can be wired to, by the name the grammar uses.
///
/// `gpui::OsAction` is a closed set — the OS performs these six itself — so a name that is not
/// one of them is an entry with nowhere to go rather than a ninth selector.
fn os_action_of(name: &str) -> Option<gpui::OsAction> {
    match name {
        "cut" => Some(gpui::OsAction::Cut),
        "copy" => Some(gpui::OsAction::Copy),
        "paste" => Some(gpui::OsAction::Paste),
        "selectAll" => Some(gpui::OsAction::SelectAll),
        "undo" => Some(gpui::OsAction::Undo),
        "redo" => Some(gpui::OsAction::Redo),
        _ => None,
    }
}

/// The name [`os_action_of`] knows a selector by.
fn os_action_name(action: &gpui::OsAction) -> &'static str {
    match action {
        gpui::OsAction::Cut => "cut",
        gpui::OsAction::Copy => "copy",
        gpui::OsAction::Paste => "paste",
        gpui::OsAction::SelectAll => "selectAll",
        gpui::OsAction::Undo => "undo",
        gpui::OsAction::Redo => "redo",
    }
}

/// Splits an entry's leading `>` markers from the entry itself: one per level of nesting.
fn split_depth(entry: &str) -> ( usize, &str) {
    let depth = entry.chars().take_while(|c| *c == '>').count();
    (depth, &entry[depth..])
}

/// Reads a menu's entry list into the items it describes, **and** the entries it could not place.
///
/// The grammar is one entry per string — `"separator"`, `"quits:Quit"`,
/// `"events:<label>[:<stroke>]"` or `"submenu:<label>"` — optionally indented with `>`, one per
/// level, to put it inside the submenu most recently opened one level above it. **Depth 0 is the
/// menu itself**; an entry at depth `d > 0` belongs to a submenu opened at `d - 1`, and a submenu
/// is closed by the next entry that is not deeper than it.
///
/// The slot is unchanged from MN3: the index of the entry among the **`events` entries**, counted
/// in document order wherever they sit in the nesting, because the eight action slots belong to
/// the menu bar and not to a level of it. Nesting is not an entry, and neither is a submenu.
///
/// One parse, several views: [`menu_plan`], [`menu_slots`], [`menu_accelerators`],
/// [`menu_tree`] and [`menu_bad`] all read this, and so does the request that installs the menu,
/// so the reader and the wiring cannot drift apart.
pub fn menu_parse(items: &[String]) -> (Vec<MenuEntry>, Vec<String>) {
    let mut slot = 0usize;
    let mut plan = Vec::new();
    let mut dropped = Vec::new();
    // The depths of the submenus currently open, innermost last.
    let mut open: Vec<usize> = Vec::new();
    for raw in items {
        let (depth, entry) = split_depth(raw);
        // Anything at this depth or deeper closes: this entry is a sibling of what opened it.
        while open.last().is_some_and(|open_depth| *open_depth >= depth) {
            open.pop();
        }
        if depth > 0 && open.last() != Some(&(depth - 1)) {
            // Nothing is open at the level above it, so there is nowhere to put it.
            dropped.push(raw.clone());
            continue;
        }
        let mut parts = entry.splitn(3, ':');
        let kind = parts.next().unwrap_or("");
        let label = parts.next().unwrap_or("");
        let third = parts.next().unwrap_or("").trim();
        let stroke = if third.is_empty() { None } else { Some(third.to_string()) };
        match kind {
            // An item the OS acts on itself. It is an action of the menu's, so it takes a slot.
            "os" => {
                // A label and a selector, both required: the label is what the person reads and
                // the selector is what the system does. An empty selector is not one of the six.
                if label.is_empty() || os_action_of(third).is_none() {
                    dropped.push(raw.clone());
                    continue;
                }
                plan.push(MenuEntry {
                    kind: "os",
                    label: label.to_string(),
                    slot,
                    depth,
                    stroke: None,
                    selector: Some(third.to_string()),
                });
                slot += 1;
            }
            // The Services menu, which the system fills in. A menu, so no slot.
            "services" => plan.push(MenuEntry {
                kind: "services",
                label: if label.is_empty() { "Services".to_string() } else { label.to_string() },
                slot: 0,
                depth,
                stroke: None,
                selector: None,
            }),
            "separator" => plan.push(MenuEntry {
                kind: "separator",
                label: String::new(),
                slot: 0,
                depth,
                stroke: None,
                selector: None,
            }),
            "quits" => plan.push(MenuEntry {
                kind: "quits",
                label: if label.is_empty() { "Quit".to_string() } else { label.to_string() },
                slot: 0,
                depth,
                stroke: None,
                selector: None,
            }),
            "events" => {
                plan.push(MenuEntry {
                    kind: "events",
                    label: label.to_string(),
                    slot,
                    depth,
                    stroke,
                    selector: None,
                });
                slot += 1;
            }
            "submenu" => {
                plan.push(MenuEntry {
                    kind: "submenu",
                    label: label.to_string(),
                    slot: 0,
                    depth,
                    stroke: None,
                    selector: None,
                });
                open.push(depth);
            }
            _ => dropped.push(raw.clone()),
        }
    }
    (plan, dropped)
}

/// The slot and label of every entry that is an **action**, as `"<slot>:<label>"`.
///
/// That is `"events:"` entries and `"os:"` ones alike: an item the system acts on is still an item
/// the program is told about when it is chosen, so it takes a slot like any other action. A
/// separator, a submenu and the Services menu take none.
pub fn menu_slots(items: &[String]) -> Vec<String> {
    menu_parse(items)
        .0
        .into_iter()
        .filter(|entry| entry.kind == "events" || entry.kind == "os")
        .map(|entry| format!("{}:{}", entry.slot, entry.label))
        .collect()
}

/// The structure a menu's entries describe, as `"<depth>:<kind>:<label>"`, in order.
///
/// What the program declared, read back — the nesting included, so a program can see the menu it
/// is about to install rather than trusting its own bookkeeping.
pub fn menu_tree(items: &[String]) -> Vec<String> {
    menu_parse(items)
        .0
        .into_iter()
        .map(|entry| format!("{}:{}:{}", entry.depth, entry.kind, entry.label))
        .collect()
}

/// The entries a menu could not place, in order: an unknown kind, or one indented deeper than any
/// submenu above it.
pub fn menu_bad(items: &[String]) -> Vec<String> {
    menu_parse(items).1
}

/// One item of a menu bar, as the platform holds it.
struct MenuLine {
    depth: usize,
    kind: &'static str,
    label: String,
    /// The action the item was given, named by gpui.
    action: Option<String>,
    /// The system selector the item was given, if it has one.
    selector: Option<String>,
}

impl MenuLine {
    /// `"<depth>:<kind>:<label>:<action>"` — the shape `gpui::installed_menu` answers with.
    fn describe(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.depth,
            self.kind,
            self.label,
            self.action.clone().unwrap_or_default(),
        )
    }

    /// `"<depth>:<label>:<selector>"` when the item has a selector, and nothing otherwise.
    fn selector_line(&self) -> Option<String> {
        self.selector
            .as_ref()
            .map(|selector| format!("{}:{}:{}", self.depth, self.label, selector))
    }
}

/// Walks the menu bar the platform was given into the lines describing it.
///
/// The application menu is depth 0, its items depth 1, a submenu's items one deeper again, so
/// this is [`menu_tree`]'s shape read back from the *built* menu rather than from the
/// declaration — which is what tells a program that its nesting survived the build.
///
/// The action and the selector are the parts nothing else can see: a menu click cannot be
/// synthesized, so without them a menu whose items all carried the same action, or none of whose
/// items carried a system selector, would look exactly like one whose items did.
fn walk_menu_bar(menus: &[gpui::OwnedMenu], out: &mut Vec<MenuLine>) {
    for menu in menus {
        out.push(MenuLine {
            depth: 0,
            kind: "menu",
            label: menu.name.to_string(),
            action: None,
            selector: None,
        });
        walk_menu_items(&menu.items, 1, out);
    }
}

fn walk_menu_items(items: &[gpui::OwnedMenuItem], depth: usize, out: &mut Vec<MenuLine>) {
    for item in items {
        match item {
            gpui::OwnedMenuItem::Separator => out.push(MenuLine {
                depth,
                kind: "separator",
                label: String::new(),
                action: None,
                selector: None,
            }),
            gpui::OwnedMenuItem::Submenu(menu) => {
                out.push(MenuLine {
                    depth,
                    kind: "submenu",
                    label: menu.name.to_string(),
                    action: None,
                    selector: None,
                });
                walk_menu_items(&menu.items, depth + 1, out);
            }
            gpui::OwnedMenuItem::SystemMenu(menu) => out.push(MenuLine {
                depth,
                kind: "system",
                label: menu.name.to_string(),
                action: None,
                selector: None,
            }),
            gpui::OwnedMenuItem::Action { name, action, os_action } => out.push(MenuLine {
                depth,
                kind: "events",
                label: name.clone(),
                action: Some(action.name().to_string()),
                selector: os_action.as_ref().map(|selector| os_action_name(selector).to_string()),
            }),
        }
    }
}

/// The menu bar as the platform has it, `"<depth>:<kind>:<label>:<action>"` per item.
pub fn installed_menu() -> Vec<String> {
    lines_of("~menu.installed")
}

/// The system selectors among the menu bar's items, as `"<depth>:<label>:<selector>"`.
pub fn installed_menu_os_actions() -> Vec<String> {
    lines_of("~menu.os-actions")
}

fn lines_of(key: &str) -> Vec<String> {
    store::get_text(key, "")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

/// Records both descriptions of a menu bar, from one walk.
fn record_menu_bar(win: i64, menus: &[gpui::OwnedMenu]) {
    let mut lines = Vec::new();
    walk_menu_bar(menus, &mut lines);
    let described = lines.iter().map(MenuLine::describe).collect::<Vec<_>>().join("\n");
    let selectors = lines
        .iter()
        .filter_map(MenuLine::selector_line)
        .collect::<Vec<_>>()
        .join("\n");
    window::with(win, || {
        store::set_text("~menu.installed", &described);
        store::set_text("~menu.os-actions", &selectors);
    });
}

/// Turns a plan into GPUI's items, nesting the entries that are deeper.
///
/// `index` walks the plan once; an entry at `depth` is an item of the menu being built, and a
/// `submenu` entry recurses for everything indented under it.
fn build_items(entries: &[MenuEntry], index: &mut usize, depth: usize) -> Vec<gpui::MenuItem> {
    let mut items = Vec::new();
    while *index < entries.len() {
        let entry = &entries[*index];
        if entry.depth < depth {
            // Shallower than this submenu: it belongs to an outer one.
            break;
        }
        *index += 1;
        match entry.kind {
            "separator" => items.push(gpui::MenuItem::separator()),
            "quits" => items.push(gpui::MenuItem::action(entry.label.clone(), Quit)),
            "events" => push_menu_action(&mut items, entry.label.clone(), entry.slot),
            "os" => {
                if let Some(selector) = entry.selector.as_deref().and_then(os_action_of) {
                    push_os_action(&mut items, entry.label.clone(), selector, entry.slot);
                }
            }
            "services" => {
                items.push(gpui::MenuItem::os_submenu(
                    entry.label.clone(),
                    gpui::SystemMenuType::Services,
                ));
            }
            "submenu" => {
                let children = build_items(entries, index, depth + 1);
                items.push(gpui::MenuItem::submenu(gpui::Menu {
                    name: entry.label.clone().into(),
                    items: children,
                }));
            }
            _ => {}
        }
    }
    items
}

/// The accelerator of every `"events:"` entry that declares one, as
/// `"<slot>:<stroke>"`, in slot order.
///
/// The same numbering [`menu_slots`] answers, for the part of an entry that is a
/// keystroke rather than a label. A reader rather than a second parse: the request
/// that binds an accelerator and this both call [`menu_plan`].
pub fn menu_accelerators(items: &[String]) -> Vec<String> {
    menu_plan(items)
        .into_iter()
        .filter(|entry| entry.kind == "events")
        .filter_map(|entry| entry.stroke.map(|stroke| format!("{}:{}", entry.slot, stroke)))
        .collect()
}

/// The eight edges a window can be resized by, by the name a program writes.
///
/// The names are the variants of `gpui::ResizeEdge` spelled out — `top-left` and
/// so on — because the ABI carries no enum: a call is a name and a list of scalars.
/// Anything else answers `None`, and the caller says so rather than picking one.
pub fn resize_edge(name: &str) -> Option<gpui::ResizeEdge> {
    match name {
        "top-left" => Some(gpui::ResizeEdge::TopLeft),
        "top" => Some(gpui::ResizeEdge::Top),
        "top-right" => Some(gpui::ResizeEdge::TopRight),
        "right" => Some(gpui::ResizeEdge::Right),
        "bottom-right" => Some(gpui::ResizeEdge::BottomRight),
        "bottom" => Some(gpui::ResizeEdge::Bottom),
        "bottom-left" => Some(gpui::ResizeEdge::BottomLeft),
        "left" => Some(gpui::ResizeEdge::Left),
        _ => None,
    }
}


/// Runs every request a frame asked for. Answers `true` when the program wants
/// another frame immediately.
///
/// The requests are the ones `win`'s frame raised: a `close_window()` in window
/// A cannot close window B, and `resize_window` resizes the window that asked.
fn process_requests(
    window: &mut Window,
    cx: &mut Context<Root>,
    entity: EntityId,
    win: i64,
) -> bool {
    // Every window is given the platform's should-close closure when it is *built*,
    // guarded or not, because a program may arm its guard at any later frame. The
    // closure is a native one — it reads state — rather than the program's own
    // callback, and that is what makes the guard work on the compiled tiers: a
    // Gossamer callback stored by a binding fn that returns is released, and the
    // guard silently answered "yes, close" there.
    //
    // It is not installed here, in the middle of a frame, and that is not a
    // style choice: gpui's X11 backend holds its callback table borrowed while it
    // draws, so registering one from inside `render` panics there
    // (`RefCell already borrowed`, `x11/window.rs:1459`) — every window after the
    // first dies on its first frame this way. See `Root`'s two
    // construction sites.

    // The state a window asked to open in — `start=maximized` or
    // `start=fullscreen` — is a platform call, not a size: gpui reads the bounds
    // out of `WindowBounds::Maximized` and ignores the state. Only a window that
    // is already on screen can zoom or go fullscreen, so its own first frame
    // raises the request and the drain below carries it out.
    if view::frame_number() == 1 {
        match options::take_start(win) {
            Some(Start::Maximized) => store::request(Request::ZoomWindow),
            Some(Start::Fullscreen) => store::request(Request::ToggleFullscreen),
            _ => {}
        }
    }

    let mut animate = false;
    for request in store::take_requests() {
        match request {
            Request::Animate => animate = true,

            Request::Quit => {
                release_retained();
                cx.quit()
            }

            Request::FocusNext => window.focus_next(),
            Request::FocusPrev => window.focus_prev(),
            Request::ToggleFullscreen => window.toggle_fullscreen(),
            Request::ZoomWindow => window.zoom_window(),
            // The platform's close request and the program's own go through the
            // same handler, so a guard cannot be bypassed in code.
            Request::CloseWindow => {
                if view::may_close(win) {
                    window.remove_window();
                }
            }

            Request::CloseWindowAt { target } => {
                if target == win {
                    if view::may_close(win) {
                        window.remove_window();
                    }
                } else if !view::may_close(target) {
                    // The target refuses; nothing to defer.
                } else {
                    // Not here: finding the handle means reading every open
                    // window's root view, and GPUI refuses a nested read of a
                    // window that is already on the stack — which this one is,
                    // because the frame is being rendered right now.
                    // `close_named_window` runs from the defer below.
                    PENDING_CLOSE.with(|q| q.borrow_mut().push(target));
                }
            }

            Request::ObserveKeys { id } => {
                // Not `cx.observe_keystrokes`. gpui skips that observer the moment
                // an element stops propagation (`window.rs:3861-3869`) and the text
                // editor does exactly that for every key it handles, so the keys a
                // program most wants to watch would be the ones it never sees. The
                // program's listeners are recorded per window instead and run from
                // the root element's **capture** listener, which is dispatched before
                // any element's handler can stop anything. See `view::add_key_observer`.
                view::add_key_observer(win, id);
            }

            Request::SendKey { stroke } => match gpui::Keystroke::parse(&stroke) {
                Ok(keystroke) => {
                    // Deferred, not dispatched here: this runs inside the frame that
                    // is being built, and dispatching a key event walks the dispatch
                    // tree of the frame already on screen.
                    window.defer(cx, move |window, cx| {
                        window.dispatch_keystroke(keystroke, cx);
                    });
                }
                Err(_) => {
                    window::with(win, || store::push_event_with("~key.bad", &stroke));
                }
            },

            Request::Compose { key, text } => {
                // Not deferred, unlike `SendKey` beside it: a composition is a write to the field,
                // not an event that has to walk a dispatch tree, so it lands now and the frame being
                // built already draws it. That is also what a real input method gets — the platform
                // calls the handler the moment the composition changes, not a frame later.
                editor::compose(win, &key, &text, window, cx);
            }

            Request::ObserveBounds { id } => {
                // Upstream's `cx.observe_window_bounds`: the callback runs when the
                // platform reports the window moved or resized, which is a place a
                // binding function has no `Window` to stand in. Installed once per
                // (window, id), like the keystroke observer above.
                static OBSERVED_BOUNDS: LazyLock<Mutex<HashSet<(i64, String)>>> =
                    LazyLock::new(|| Mutex::new(HashSet::new()));
                if !OBSERVED_BOUNDS.lock().unwrap().insert((win, id.clone())) {
                    continue;
                }
                let key = id.clone();
                cx.observe_window_bounds(window, move |_this, window, cx| {
                    let bounds = window.bounds();
                    window::with(win, || {
                        store::push_event_with(
                            &key,
                            format!(
                                "{},{},{},{}",
                                f32::from(bounds.origin.x),
                                f32::from(bounds.origin.y),
                                f32::from(bounds.size.width),
                                f32::from(bounds.size.height),
                            ),
                        );
                    });
                    cx.notify();
                })
                .detach();
            }

            Request::ObserveAppearance { id } => {
                // Upstream's `cx.observe_window_appearance`, and the same shape as the
                // bounds observer just above: the platform reports the change on a
                // thread a binding function has no `Window` to stand in, and the
                // callback is delivered from GPUI's own loop. Installed once per
                // (window, id).
                //
                // Nothing can *provoke* this from inside the process — gpui has no
                // public appearance setter, and registering the observer does not
                // replay the current value — so `tests/gui/th2_appearance_change.gos`
                // is run under `scripts/appearance-toggle.sh`, which changes the system
                // appearance and puts it back.
                static OBSERVED_APPEARANCE: LazyLock<Mutex<HashSet<(i64, String)>>> =
                    LazyLock::new(|| Mutex::new(HashSet::new()));
                if !OBSERVED_APPEARANCE.lock().unwrap().insert((win, id.clone())) {
                    continue;
                }
                let key = id.clone();
                cx.observe_window_appearance(window, move |_this, window, cx| {
                    let appearance = view::appearance_of(window.appearance());
                    window::with(win, || store::push_event_with(&key, &appearance));
                    cx.notify();
                })
                .detach();
            }

            Request::SetTheme { name } => {
                // The theme is read at the *start* of a frame, before the view callback
                // that could set it, so it is recorded here and a frame is asked for:
                // the next frame's colour table answers from the new palette.
                match name.as_str() {
                    "system" | "light" | "dark" => {
                        window::with(win, || store::set_text("~theme", &name));
                        cx.notify();
                    }
                    // A name the binding does not know is refused rather than taken:
                    // the choice stands, and the program is told.
                    _ => window::with(win, || store::push_event_with("~theme.bad", &name)),
                }
            }

            Request::SetMenus { name, items } => {
                // Menu items carry a `Box<dyn Action>`, which no ABI can pass,
                // so the binding owns a fixed set of action slots and maps a
                // menu entry onto one by position.
                static MENU_WIRED: AtomicBool = AtomicBool::new(false);
                if !MENU_WIRED.swap(true, Ordering::SeqCst) {
                    install_menu_actions(cx);
                }
                // The window that installs a menu is the one its items speak
                // for, until another window installs one.
                *MENU_OWNER.lock().unwrap() = (win, Some(entity));
                let (entries, bad) = menu_parse(&items);
                // An entry with nowhere to go is reported rather than dropped in silence: a
                // menu that is one item short is worse than one that says so.
                for entry in bad {
                    window::with(win, || store::push_event_with("~menu.bad", &entry));
                }
                // Every accelerator is bound *before* the menu is installed: the platform is
                // handed the keymap when it is, and that is where a menu item's key equivalent
                // comes from.
                for entry in &entries {
                    if let Some(stroke) = entry.stroke.clone() {
                        bind_menu_stroke(cx, &stroke, entry.slot);
                    }
                }
                let mut index = 0;
                let menu_items = build_items(&entries, &mut index, 0);
                cx.set_menus(vec![gpui::Menu {
                    name: name.into(),
                    items: menu_items,
                }]);
                // Read back what the platform was given, so a program can tell a menu that was
                // built the way it asked from one that was not.
                record_menu_bar(win, &cx.get_menus().unwrap_or_default());
            }
            Request::BindKeys { spec } => {
                // The action type is the binding's and carries the program's own
                // event name; see `ProgramKey`. Wired once, then every spec adds
                // to the one application keymap.
                bind_keys_spec(cx, win, entity, &spec);
            }

            Request::ResizeWindow { w, h } => window.resize(size(px(w as f32), px(h as f32))),

            Request::RequestDecorations { client } => {
                // The window manager is asked who draws the window's decoration. This is a
                // *request*: a Linux window manager with no compositor refuses it and keeps
                // drawing its own titlebar, which is why the answer to a request is read back
                // with `gpui::window_decorations` rather than assumed.
                window.request_decorations(if client {
                    gpui::WindowDecorations::Client
                } else {
                    gpui::WindowDecorations::Server
                });
            }

            Request::SetClientInset { px: inset } => {
                // The width of the strip a program draws its own titlebar and resize band in.
                // A no-op on a platform that draws the window's decoration itself.
                window.set_client_inset(gpui::px(inset as f32));
            }

            Request::StartWindowMove => window.start_window_move(),

            Request::StartWindowResize { edge } => match resize_edge(&edge) {
                Some(edge) => window.start_window_resize(edge),
                // A program that names an edge this binding does not know is told, rather than
                // handed a resize it did not ask for: the eight names are `gpui::ResizeEdge`'s
                // own, and a ninth would be a typo.
                None => view::note_error(format!(
                    "start_window_resize: {edge:?} is not one of top-left, top, top-right, right, \
                     bottom-right, bottom, bottom-left, left"
                )),
            },

            Request::SetRemSize { px: rem } => window.set_rem_size(gpui::px(rem as f32)),

            Request::SetClipboard { text } => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text))
            }

            Request::ForgetAsset { path } => {
                // Eviction is one map lookup in `loading_assets` (`app.rs:1998`), keyed by the
                // asset type and a hash of the source — and it is the *only* cache there is:
                // the binding never pushes an image cache of its own, so `Img::paint`'s
                // `window.use_asset::<ImgResourceLoader>` and this crate's `render_image` are
                // looking at the same entry. Forgetting it makes the next frame's `use_asset`
                // start a load from scratch, which is what upstream's cell clicks do.
                let resource: Resource = std::path::PathBuf::from(view::asset_path(&path)).into();
                cx.remove_asset::<ImgResourceLoader>(&resource);
            }

            Request::Tick { id, ms } => {
                // A second `tick` for a running id **reschedules** it: a fresh generation,
                // so the loop the earlier call started finds itself retired the next time
                // it wakes and ends without raising anything, and a new loop with the new
                // period takes over. Leaving an id that is already there alone — a
                // set — would let a program start a clock and never change it.
                let generation = TIMER_GENERATION.fetch_add(1, Ordering::SeqCst);
                TICKS
                    .lock()
                    .unwrap()
                    .insert((win, id.clone()), (generation, ms));
                let tick_id = id.clone();
                cx.spawn(async move |_this, cx| {
                    loop {
                        Timer::after(Duration::from_millis(ms)).await;
                        // Retired by a later `tick` or by `tick_stop`: end without raising.
                        let current = TICKS
                            .lock()
                            .unwrap()
                            .get(&(win, tick_id.clone()))
                            .map(|(generation, _)| *generation);
                        if current != Some(generation) {
                            break;
                        }
                        let alive = cx.update(|cx| {
                            window::with(win, || store::push_event(tick_id.clone()));
                            cx.notify(entity);
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                })
                .detach();
            }

            Request::TickStop { id } => {
                // Dropping the entry retires the loop the next time it wakes, so the id
                // may be started again — with any period — the moment this returns.
                TICKS.lock().unwrap().remove(&(win, id));
            }

            Request::Delay { id, ms } => {
                // One wait, one event. A fresh generation, so asking again for the same id
                // restarts the wait and the earlier task finds itself retired.
                let generation = TIMER_GENERATION.fetch_add(1, Ordering::SeqCst);
                DELAYS
                    .lock()
                    .unwrap()
                    .insert((win, id.clone()), generation);
                let delay_id = id.clone();
                cx.spawn(async move |_this, cx| {
                    Timer::after(Duration::from_millis(ms)).await;
                    // Take the entry only if it is still ours. A cancel removed it, or a
                    // later `delay` replaced it, and either way this task must not raise.
                    let ours = {
                        let mut delays = DELAYS.lock().unwrap();
                        if delays.get(&(win, delay_id.clone())).copied() == Some(generation) {
                            delays.remove(&(win, delay_id.clone()));
                            true
                        } else {
                            false
                        }
                    };
                    if !ours {
                        return;
                    }
                    let _ = cx.update(|cx| {
                        window::with(win, || store::push_event(delay_id.clone()));
                        cx.notify(entity);
                    });
                })
                .detach();
            }

            Request::AskActiveWindow => {
                let asker = win;
                cx.defer(move |cx| find_active_window(cx, asker));
            }

            Request::HideApplication => {
                // `App::hide` takes `&self` and does the whole job: on macOS it puts the application away, every
                // window of it. Nothing comes back, so a program learns it happened the way a person does.
                cx.hide();
            }

            Request::RefreshWindows => {
                cx.refresh_windows();
            }

            Request::Reveal { path } => {
                // Three calls with no answer: the platform does the thing, and the binding has nothing to
                // read back. `scripts/docs.sh` and the register both say so rather than implying otherwise.
                cx.reveal_path(std::path::Path::new(&path));
            }

            Request::OpenWithSystem { path } => {
                cx.open_with_system(std::path::Path::new(&path));
            }

            Request::OpenUrl { url } => {
                cx.open_url(&url);
            }

            Request::OpenPicker { id, what, prompt } => {
                // `App::prompt_for_paths` takes `&self` — no window — and answers on a oneshot, so the
                // wait belongs in a task. `Vec<PathBuf>` cannot cross this ABI, so what comes back is a
                // list of strings under `"<id>.paths"` and an event carrying how many.
                let folders = what == "folders";
                let options = gpui::PathPromptOptions {
                    files: !folders,
                    directories: folders,
                    multiple: true,
                    prompt: if prompt.is_empty() {
                        None
                    } else {
                        Some(prompt.into())
                    },
                };
                let mut asked = cx.prompt_for_paths(options);
                let entity = cx.entity().entity_id();
                cx.spawn(async move |_this, cx| {
                    let outcome = loop {
                        Timer::after(Duration::from_millis(100)).await;
                        match asked.try_recv() {
                            Ok(Some(outcome)) => break Some(outcome),
                            Ok(None) => continue,
                            // The channel went away: the panel was dismissed in a way that reports nothing.
                            Err(_) => break None,
                        }
                    };
                    window::with(win, || match outcome {
                        Some(Ok(Some(paths))) => {
                            let names: Vec<String> = paths
                                .iter()
                                .map(|path| path.to_string_lossy().into_owned())
                                .collect();
                            store::set_texts(&format!("{id}.paths"), names.clone());
                            store::push_event_with(&id, format!("{}", names.len()));
                        }
                        Some(Ok(None)) => store::push_event_with(&id, "cancelled"),
                        _ => store::push_event_with(&id, "failed"),
                    });
                    let _ = cx.update(|cx| cx.notify(entity));
                })
                .detach();
            }

            Request::SavePicker {
                id,
                directory,
                suggested,
            } => {
                let where_to = if directory.is_empty() {
                    std::env::current_dir().unwrap_or_default()
                } else {
                    std::path::PathBuf::from(&directory)
                };
                let name = if suggested.is_empty() {
                    None
                } else {
                    Some(suggested.as_str())
                };
                let mut asked = cx.prompt_for_new_path(&where_to, name);
                let entity = cx.entity().entity_id();
                cx.spawn(async move |_this, cx| {
                    let outcome = loop {
                        Timer::after(Duration::from_millis(100)).await;
                        match asked.try_recv() {
                            Ok(Some(outcome)) => break Some(outcome),
                            Ok(None) => continue,
                            Err(_) => break None,
                        }
                    };
                    window::with(win, || match outcome {
                        Some(Ok(Some(path))) => {
                            store::set_text(&format!("{id}.path"), &path.to_string_lossy());
                            store::push_event_with(&id, "saved");
                        }
                        Some(Ok(None)) => store::push_event_with(&id, "cancelled"),
                        _ => store::push_event_with(&id, "failed"),
                    });
                    let _ = cx.update(|cx| cx.notify(entity));
                })
                .detach();
            }

            Request::Focus { id } => {
                // Only an element that already has a handle can be focused; a request for one that
                // does not is silently nothing, which is what the reader will report.
                if let Some(handle) = view::handle_for(win, &id) {
                    window.focus(&handle);
                }
            }

            Request::SetTitle { title } => {
                window.set_window_title(&title);
                // The read-back happens on the **next frame**, not here: setting a title is synchronous
                // on macOS and a request to the compositor on Wayland, and a binding that reads it back
                // immediately would be answering "did the platform apply it yet", which is a different
                // question on every platform. `begin_frame` samples it because this marks it stale.
                options::mark_title_stale(win);
            }

            Request::SaveWindowState { path, key } => {
                // The live window's own bounds — what a person would call "where the window is"
                // — recorded through the same function a program uses when it reads them itself.
                let bounds = window.bounds();
                let outcome = settings::remember_bounds(
                    &path,
                    &key,
                    f32::from(bounds.origin.x) as f64,
                    f32::from(bounds.origin.y) as f64,
                    f32::from(bounds.size.width) as f64,
                    f32::from(bounds.size.height) as f64,
                );
                if let Err(problem) = outcome {
                    window::with(win, || store::push_event_with("~settings.bad", &problem));
                }
            }

            Request::SetDockMenu { items } => {
                // The menu the dock icon offers on a right-click. It is the application's rather
                // than a window's, and gpui has no getter for it, so there is nothing to read
                // back — see the row's residual doubt.
                let (entries, bad) = menu_parse(&items);
                for entry in bad {
                    window::with(win, || store::push_event_with("~menu.bad", &entry));
                }
                for entry in &entries {
                    if let Some(stroke) = entry.stroke.clone() {
                        bind_menu_stroke(cx, &stroke, entry.slot);
                    }
                }
                let mut index = 0;
                cx.set_dock_menu(build_items(&entries, &mut index, 0));
            }

            Request::Animation { id, ms, easing } => {
                // Starting an id that is already animating restarts it, the way `delay`
                // restarts a wait: one clock per id, and the last one asked for wins.
                ANIMATIONS.lock().unwrap().insert(
                    (win, id),
                    Animation {
                        duration_ms: ms as f64,
                        elapsed_ms: 0.0,
                        running_since: Some(Instant::now()),
                        easing,
                    },
                );
                animate = true;
            }

            Request::AnimationPause { id } => {
                if let Some(animation) = ANIMATIONS.lock().unwrap().get_mut(&(win, id)) {
                    // Add the run that just ended to the total, and stop counting.
                    if let Some(since) = animation.running_since.take() {
                        animation.elapsed_ms += since.elapsed().as_secs_f64() * 1000.0;
                    }
                }
            }

            Request::AnimationResume { id } => {
                if let Some(animation) = ANIMATIONS.lock().unwrap().get_mut(&(win, id)) {
                    // A resume on something already running, or already over, is not a
                    // restart: it is nothing.
                    if animation.running_since.is_none() && !animation.finished() {
                        animation.running_since = Some(Instant::now());
                    }
                }
            }

            Request::TaskCancel { id } => {
                // Dropping the entry is what takes the cancel. The work itself runs to its
                // end — a thread that is already asleep cannot be interrupted — but when it
                // comes back it finds itself retired and reports nothing.
                TASKS.lock().unwrap().remove(&(win, id));
            }

            Request::AnimationStop { id } => {
                ANIMATIONS.lock().unwrap().remove(&(win, id));
            }

            Request::DelayCancel { id } => {
                // Dropping the entry is what takes the delay back: the waiting task wakes,
                // finds the entry gone (or replaced by a later `delay`), and raises nothing.
                DELAYS.lock().unwrap().remove(&(win, id));
            }

            Request::Task { id, ms } => {
                let generation = TIMER_GENERATION.fetch_add(1, Ordering::SeqCst);
                TASKS.lock().unwrap().insert((win, id.clone()), generation);
                // Starting an id again restarts it, so its progress starts again too.
                window::with(win, || store::set_float(&format!("{id}.progress"), 0.0));

                // The progress reporter: a clock beside the work, because the work is a thread
                // that cannot be asked how far along it is. It reports until the deadline and
                // stops the moment the task is retired — a cancelled job that went on reporting
                // would be drawing a bar for something that is never going to happen.
                let progress_id = id.clone();
                cx.spawn(async move |_this, cx| {
                    let started = Instant::now();
                    let total = Duration::from_millis(ms).as_secs_f64();
                    loop {
                        Timer::after(PROGRESS_STEP).await;
                        let retired = TASKS
                            .lock()
                            .unwrap()
                            .get(&(win, progress_id.clone()))
                            .copied()
                            != Some(generation);
                        if retired {
                            return;
                        }
                        let elapsed = started.elapsed().as_secs_f64();
                        if elapsed >= total {
                            // The completion path sets the last one, to exactly 1.
                            return;
                        }
                        let fraction = elapsed / total;
                        let _ = cx.update(|cx| {
                            window::with(win, || {
                                store::set_float(&format!("{progress_id}.progress"), fraction);
                                store::push_event_with(
                                    &format!("{progress_id}.progress"),
                                    format!("{fraction}"),
                                );
                            });
                            cx.notify(entity);
                        });
                    }
                })
                .detach();

                let task_id = id.clone();
                cx.spawn(async move |_this, cx| {
                    // Slow work on GPUI's background executor — the UI thread
                    // is free to keep painting while this runs.
                    let worker_id = task_id.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            std::thread::sleep(Duration::from_millis(ms));
                            format!("{worker_id} finished after {ms} ms")
                        })
                        .await;
                    // Take the entry only if it is still this task's. A cancel removed it, or
                    // a later `task` replaced it, and either way this work must not report:
                    // nothing can interrupt a thread that is already asleep, so the only
                    // moment a cancel can land is here, when the work comes back.
                    let ours = {
                        let mut tasks = TASKS.lock().unwrap();
                        if tasks.get(&(win, task_id.clone())).copied() == Some(generation) {
                            tasks.remove(&(win, task_id.clone()));
                            true
                        } else {
                            false
                        }
                    };
                    if !ours {
                        return;
                    }
                    let _ = cx.update(|cx| {
                        window::with(win, || {
                            // The end of the progress, so that "it is 1" and "there is a result"
                            // are the same moment and a program never has to reconcile them.
                            store::set_float(&format!("{task_id}.progress"), 1.0);
                            store::set_text(&format!("{task_id}.result"), &result);
                            store::push_event(task_id.clone());
                        });
                        cx.notify(entity);
                    });
                })
                .detach();
            }
        }
    }
    // An animation that is still running wants the next frame as well, whether or not the
    // program asked for one: it is the binding's clock, so it is the binding's job to keep
    // the frames coming until the animation is over.
    let animate = animate || animations_running(win);

    // Windows another window asked to close, closed once this frame is done.
    let closing = PENDING_CLOSE.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if !closing.is_empty() {
        cx.defer(move |cx| {
            for target in closing {
                close_named_window(cx, target);
            }
        });
    }

    // Windows the frame asked for, opened once it is finished.
    let pending = PENDING_OPEN.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if !pending.is_empty() {
        cx.defer(move |cx| open_pending(cx, pending));
    }

    animate
}

/// Where gpui's `svg()` reads the bytes it draws.
///
/// `Application::new()` hands gpui the *unit type* as its asset source, and
/// gpui's `impl AssetSource for ()` answers `Ok(None)` for every path. That is
/// not "not found yet" — `SvgRenderer::render` takes the `None`, returns without
/// rasterising, and the element paints nothing, for good. **So no `ui::icon` had
/// ever drawn**: not the arrow in `01_animation` nor the one in `11_opacity`.
///
/// Nothing before this could see it. `gos check` resolves names and not
/// `Kind::Icon`'s pixels, and the register said so in its own row for the icon
/// element — "the element arms are read, not measured ... seeing it needs a
/// window, which this machine's owner has forbidden". The window was allowed
/// once, `scripts/pair.sh` photographed its first pair, and the arrow that
/// upstream draws was not there.
///
/// Upstream installs a source of its own for exactly this reason — `Assets {}`
/// in `animation.rs`, fifteen lines that read the path off the filesystem — and
/// a Gosammer program has no way to hand `Application` one, so the binding does.
///
/// The path arriving here has already been through [`crate::view::asset_path`],
/// which is where a program's declared root is tried first. This is a reader,
/// then, not a second resolver: "which file does this icon name" is answered
/// once, in one place, and gpui is handed the answer.
struct FilesystemAssets;

impl gpui::AssetSource for FilesystemAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        // A file that is not there is "nothing to draw", not a failure. `Svg`
        // paints inside `log_err`, so an `Err` here would be logged once per
        // frame for the life of the window, and every other asset this binding
        // reads treats a miss as an absence rather than an error.
        Ok(std::fs::read(path).ok().map(Into::into))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        Ok(std::fs::read_dir(path)?
            .filter_map(|entry| {
                Some(gpui::SharedString::from(
                    entry.ok()?.path().to_string_lossy().into_owned(),
                ))
            })
            .collect())
    }
}

/// Opens the window and runs GPUI's event loop until the program quits.
///
/// This call does not return until the app exits: the Gossamer view callback
/// stays valid (and is re-invoked once per frame) for its whole duration, and
/// every window this app ever opens is drawn by it.
pub fn run_app(
    dispatch: &mut dyn NativeDispatch,
    title: String,
    width: f64,
    height: f64,
    options: &str,
    view: PersistentCallback,
) -> Result<(), String> {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return Err("gpui::run has already been called in this process".into());
    }

    view::stash_dispatch(dispatch);
    // Kept for the life of the app: every window's root, now and later, clones
    // this one callback. See `APP_VIEW`.
    APP_VIEW.with(|v| *v.borrow_mut() = Some(view.clone()));

    // The first window and every other: a window's id is allocated before it
    // is drawn, so its very first frame already knows which window it is.
    let main_window = window::allocate();

    let root_view = view.clone();
    let options = options.to_string();
    // `with_assets` before `run`, because the asset source has to be in place
    // before the first frame draws: see `FilesystemAssets` for what gpui does
    // with the default one and why no icon ever came out of it.
    let app = Application::new().with_assets(FilesystemAssets);
    app.run(move |cx: &mut App| {
        // Before any window exists: opening one draws it once, and that first
        // frame reads the default colours.
        // A fallback for frames drawn before a window exists. `init_colors`
        // installs the *light* set unconditionally, so the real scheme is
        // installed per window below, where `appearance()` is knowable.
        cx.init_colors();
        // Kept for the whole run: "what fonts does this machine have" is not a
        // per-window question, and the answer has to be there between frames too.
        view::set_text_system(cx.text_system().clone());
        view::set_keyboard_layout(cx.keyboard_layout().name().to_string());

        let window_options =
            options::build(cx, &title, 0.0, 0.0, width, height, &options, true, main_window);

        let opened = cx.open_window(window_options, move |window, cx| {
            install_appearance_colors(window, cx);
            // Here, where the window is built, rather than on its first frame:
            // see `process_requests`.
            window.on_window_should_close(cx, move |_window, _cx| view::may_close(main_window));
            cx.new(|_| Root { view: root_view.clone(), window: main_window })
        });

        match opened {
            Ok(_) => {}
            Err(e) => {
                eprintln!("gpui: could not open a window: {e}");
                cx.quit();
                return;
            }
        }

        // Only when the spec asked for it. An activated app is the one in front
        // and its window is the key window, which on macOS is the difference
        // between three coloured traffic lights and three grey ones. Every port
        // that is a plain `gpui::run` wants that, so `run` still activates; the
        // ports of the upstream examples that do not activate ask for
        // `activate=false`, and then this is the one call that is not made. See
        // `options::activate`.
        if options::activate(main_window) {
            cx.activate(true);
        }

        // Closing the window (or the desktop's quit shortcut) ends the app.
        // `== 0`, not `<= 1`: with more than one window open, `<= 1` would
        // quit the moment any window closed.
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                last_window_closed(cx);
            }
        })
        .detach();
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([gpui::KeyBinding::new(quit_keystroke(), Quit, None)]);
    });

    view::clear_dispatch();
    Ok(())
}

/// ⌘Q on macOS, Ctrl+Q everywhere else.
#[cfg(target_os = "macos")]
fn quit_keystroke() -> &'static str {
    "cmd-q"
}

#[cfg(not(target_os = "macos"))]
fn quit_keystroke() -> &'static str {
    "ctrl-q"
}

gpui::actions!(
    gpui_binding,
    [Quit, Slot0, Slot1, Slot2, Slot3, Slot4, Slot5, Slot6, Slot7]
);

/// Adds an action item backed by the slot for `index`. `Slot0` absorbs
/// anything past the last slot, so an over-long menu fires the first slot
/// rather than panicking.
fn push_menu_action(items: &mut Vec<gpui::MenuItem>, label: String, index: usize) {
    match index {
        1 => items.push(gpui::MenuItem::action(label, Slot1)),
        2 => items.push(gpui::MenuItem::action(label, Slot2)),
        3 => items.push(gpui::MenuItem::action(label, Slot3)),
        4 => items.push(gpui::MenuItem::action(label, Slot4)),
        5 => items.push(gpui::MenuItem::action(label, Slot5)),
        6 => items.push(gpui::MenuItem::action(label, Slot6)),
        7 => items.push(gpui::MenuItem::action(label, Slot7)),
        _ => items.push(gpui::MenuItem::action(label, Slot0)),
    }
}

/// Adds an item wired to a system selector, backed by the slot for `index`.
///
/// `Slot0` absorbs anything past the last slot, as [`push_menu_action`] does, so an over-long
/// menu fires the first slot rather than panicking. The selector is what the OS acts on; the
/// slot is what tells the program the item was chosen.
fn push_os_action(
    items: &mut Vec<gpui::MenuItem>,
    label: String,
    selector: gpui::OsAction,
    index: usize,
) {
    match index {
        1 => items.push(gpui::MenuItem::os_action(label, Slot1, selector)),
        2 => items.push(gpui::MenuItem::os_action(label, Slot2, selector)),
        3 => items.push(gpui::MenuItem::os_action(label, Slot3, selector)),
        4 => items.push(gpui::MenuItem::os_action(label, Slot4, selector)),
        5 => items.push(gpui::MenuItem::os_action(label, Slot5, selector)),
        6 => items.push(gpui::MenuItem::os_action(label, Slot6, selector)),
        7 => items.push(gpui::MenuItem::os_action(label, Slot7, selector)),
        _ => items.push(gpui::MenuItem::os_action(label, Slot0, selector)),
    }
}

/// Binds one menu entry's declared accelerator to its own slot action.
///
/// A `KeyBinding` holds a concrete action, so the slot has to be chosen at compile
/// time; the arms mirror [`push_menu_action`], `Slot0` absorbing anything past the
/// last slot, so an entry's accelerator always drives the same action its click
/// does.
///
/// The binding is also what the platform reads when it draws the menu — the macOS
/// menu looks the item's action up in the keymap and sets the `NSMenuItem`'s key
/// equivalent from it — so showing and firing the shortcut are one mechanism, not
/// two.
fn bind_menu_stroke(cx: &mut App, stroke: &str, index: usize) {
    // `KeyBinding::new` panics on a stroke it cannot parse, and a panic here would
    // take the window down. Reported instead, like an unparseable spec in
    // `bind_keys_spec`.
    if gpui::Keystroke::parse(stroke).is_err() {
        let stroke = stroke.to_string();
        let (win, _) = *MENU_OWNER.lock().unwrap();
        window::with(win, || store::push_event_with("~keybind.bad", &stroke));
        return;
    }
    match index {
        1 => cx.bind_keys([gpui::KeyBinding::new(stroke, Slot1, None)]),
        2 => cx.bind_keys([gpui::KeyBinding::new(stroke, Slot2, None)]),
        3 => cx.bind_keys([gpui::KeyBinding::new(stroke, Slot3, None)]),
        4 => cx.bind_keys([gpui::KeyBinding::new(stroke, Slot4, None)]),
        5 => cx.bind_keys([gpui::KeyBinding::new(stroke, Slot5, None)]),
        6 => cx.bind_keys([gpui::KeyBinding::new(stroke, Slot6, None)]),
        7 => cx.bind_keys([gpui::KeyBinding::new(stroke, Slot7, None)]),
        _ => cx.bind_keys([gpui::KeyBinding::new(stroke, Slot0, None)]),
    }
}

/// Wires every action slot once. A menu entry raises `"menu|<index>"` in the
/// window whose menu is installed.
fn install_menu_actions(cx: &mut App) {
    fn wire<A: gpui::Action>(cx: &mut App, index: i64) {
        cx.on_action(move |_: &A, cx| {
            let (win, entity) = *MENU_OWNER.lock().unwrap();
            window::with(win, || store::push_event(format!("menu|{index}")));
            if let Some(entity) = entity {
                cx.notify(entity);
            }
        });
    }
    wire::<Slot0>(cx, 0);
    wire::<Slot1>(cx, 1);
    wire::<Slot2>(cx, 2);
    wire::<Slot3>(cx, 3);
    wire::<Slot4>(cx, 4);
    wire::<Slot5>(cx, 5);
    wire::<Slot6>(cx, 6);
    wire::<Slot7>(cx, 7);
}

// ---------------------------------------------------------------------------
// the program's own keymap
// ---------------------------------------------------------------------------

/// The one action type this binding owns for a program's keymap.
///
/// Upstream binds a keystroke to a *program-authored* action — `Save`, `Redo`,
/// whatever the program declares — and that cannot cross this ABI: a trait
/// implementation is never a value (R3), and the traits a program would implement
/// cannot be named (R2). What a program needs is narrower than "my action runs":
/// it is "when this stroke happens, tell me which one fired". So the action is the
/// binding's, it carries the program's own event name, and the handler raises that
/// name. One type serves every binding, because gpui dispatches an action by its
/// `TypeId` and hands the handler the value.
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = gpui_binding, no_json, no_register)]
struct ProgramKey {
    id: String,
}

/// Which window, and which of its roots, a bound event name belongs to.
///
/// A stroke bound in window A must fire in window A, and not in whichever window
/// happened to render first — the same hazard [`MENU_OWNER`] exists for, answered
/// the same way. The root is kept so the window can be asked for another frame
/// when one of its own shortcuts fires.
static KEYMAP_OWNER: LazyLock<Mutex<HashMap<String, (i64, EntityId)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The bindings a program has asked for, as `"<stroke>:<name>"`, in the order it
/// asked for them. The request below writes this and `gpui::keymap` reads it, so
/// the reader cannot drift from what was actually bound.
static KEYMAP_SPEC: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Reads a keymap spec into `"<stroke>:<name>"` pairs, each optionally
/// `"<stroke>:<name>@<context>"`.
///
/// The grammar is `"<stroke>=<name>"`, pairs separated by `;`, and a name may carry
/// a **key context** after an `@` — `"cmd-s=save@Editor; cmd-j=close"`. The split is
/// at the **first** `=` and then the first `@`, so an event name may contain `=`; it
/// may not contain `:` or `@`, which separate the three parts.
///
/// The context is gpui's own context-predicate grammar and reaches
/// `KeyBinding::new` verbatim: a binding that has one is evaluated against the
/// context stack of the **focused** node, which is what `ctx=` on an element (see
/// [`crate::style::key_context`]) puts it on. A binding with none is enabled
/// whenever the keymap is consulted, which is upstream's own rule.
///
/// A pair with no `=`, an empty stroke or an empty name is **dropped** rather than
/// bound to something. That is deliberate: a stroke bound to the wrong event fires
/// at the wrong moment, and a program has no way to notice. One function, two
/// callers — the request that binds them and the reader `gpui::keymap_plan` — so
/// what a program reads back is what was parsed.
pub fn keymap_plan(spec: &str) -> Vec<String> {
    spec.split(';')
        .filter_map(|pair| {
            let (stroke, rest) = pair.split_once('=')?;
            let (stroke, rest) = (stroke.trim(), rest.trim());
            if stroke.is_empty() || rest.is_empty() {
                return None;
            }
            let (name, context) = match rest.split_once('@') {
                Some((name, context)) => (name.trim(), Some(context.trim())),
                None => (rest, None),
            };
            if name.is_empty() {
                return None;
            }
            Some(match context {
                Some(context) if !context.is_empty() => format!("{stroke}:{name}@{context}"),
                _ => format!("{stroke}:{name}"),
            })
        })
        .collect()
}

/// The bindings a program has asked for, in the order it asked for them.
pub fn keymap_spec() -> Vec<String> {
    KEYMAP_SPEC.lock().unwrap().clone()
}

/// Wires the program keymap's action once: a bound stroke raises its own name in
/// the window that bound it.
fn install_program_actions(cx: &mut App) {
    cx.on_action(move |action: &ProgramKey, cx| {
        let owner = KEYMAP_OWNER.lock().unwrap().get(&action.id).copied();
        if let Some((win, entity)) = owner {
            window::with(win, || store::push_event(&action.id));
            cx.notify(entity);
        }
    });
}

/// Binds one spec: every well-formed pair onto the keymap, and the window and root
/// that asked for it onto the routing table.
pub fn bind_keys_spec(cx: &mut App, win: i64, entity: EntityId, spec: &str) {
    static KEYMAP_WIRED: AtomicBool = AtomicBool::new(false);
    if !KEYMAP_WIRED.swap(true, Ordering::SeqCst) {
        install_program_actions(cx);
    }
    for pair in keymap_plan(spec) {
        // Split the canonical form back apart at the first `:`, then at the first
        // `@`; a stroke never contains either, and a name may not (see
        // `keymap_plan`).
        let (stroke, rest) = match pair.split_once(':') {
            Some(parts) => parts,
            None => continue,
        };
        let (name, context) = match rest.split_once('@') {
            Some((name, context)) => (name, Some(context)),
            None => (rest, None),
        };
        // A stroke gpui cannot parse is reported rather than bound to nothing:
        // `KeyBinding::new` panics on a parse error, and a panic here would take
        // the window with it.
        if gpui::Keystroke::parse(stroke).is_err() {
            window::with(win, || store::push_event_with("~keybind.bad", stroke));
            continue;
        }
        // And a context it cannot parse panics the same way — `KeyBinding::new`
        // unwraps the predicate — so it is checked here for the same reason.
        if let Some(context) = context {
            if gpui::KeyBindingContextPredicate::parse(context).is_err() {
                window::with(win, || store::push_event_with("~keybind.bad", context));
                continue;
            }
        }
        cx.bind_keys([gpui::KeyBinding::new(
            stroke,
            ProgramKey { id: name.to_string() },
            context,
        )]);
        KEYMAP_OWNER
            .lock()
            .unwrap()
            .insert(name.to_string(), (win, entity));
        let mut known = KEYMAP_SPEC.lock().unwrap();
        if !known.contains(&pair) {
            known.push(pair);
        }
    }
}
