//! The GPUI application: one window, one view callback, re-rendered whenever
//! state changes, a timer fires, or an animation asks for another frame.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    px, size, AnyElement, App, Application, Bounds, Context, EntityId, Render, Timer,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use gossamer_binding::{NativeDispatch, RetainedCallback};

use crate::store::{self, Request};
use crate::view;
use crate::window;

/// One root view per window. It owns that window's Gossamer view callback and
/// rebuilds the element tree from scratch on every frame it is asked for.
pub struct Root {
    view: RetainedCallback,
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

            element
        })
    }
}

// Windows a frame asked for. Kept here rather than in `store::Request`
// because a `static Mutex` would demand a `Send` callback.
/// Whether the application survives its last window closing.
///
/// Off by default: a program that opens one window and closes it expects the
/// process to end, which is what every example here does.
static KEEP_ALIVE: AtomicBool = AtomicBool::new(false);

/// Sets whether the application survives its last window closing.
pub fn set_keep_alive(on: bool) {
    KEEP_ALIVE.store(on, Ordering::SeqCst);
}

/// The last window has closed.
///
/// The program gets first say, through its `on_window_closed` handler; if that
/// asked for a window, one is opened and the app carries on with it. Otherwise the
/// app ends, unless the program asked to be kept alive — which is the state a dock
/// menu or an `activate` needs.
fn last_window_closed(cx: &mut App) {
    view::window_closed();
    let pending = PENDING_OPEN.with(|q| std::mem::take(&mut *q.borrow_mut()));
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
        let options = window_options(
            request.title,
            request.x,
            request.y,
            request.w,
            request.h,
            &request.options,
        );
        let view = request.view.clone();
        // Each window gets an id of its own before it is drawn, so the very first
        // frame of the new window already knows which one it is.
        let child = window::allocate();
        let _ = cx.open_window(options, move |window, cx| {
            install_appearance_colors(window, cx);
            cx.new(|_| Root { view: view.clone(), window: child })
        });
    }
}

// Windows a frame asked to open, once the frame that asked for them is done.
// A plain comment: `rustdoc` does not document items a macro produces.
thread_local! {
    static PENDING_OPEN: RefCell<Vec<PendingOpen>> = const { RefCell::new(Vec::new()) };
}

/// Releases everything still holding a retained callback, while the runtime is
/// standing.
///
/// A `uniform_list`'s row builder sits in the arena, and a window waiting to
/// open holds a whole view callback. Both are `RetainedCallback`s whose `Drop`
/// calls back into the runtime to release the registration — and a thread-local
/// dropped at *thread exit* runs that after the runtime's own thread-locals are
/// gone, so the runtime panics at its FFI entry:
///
/// ```text
/// thread '<unnamed>' panicked: cannot access a Thread Local Storage value
/// during or after destruction: AccessError
///   ... _gos_rt_binding_callback_release
/// ```
///
/// It has to happen here rather than after the event loop returns, because
/// `Application::run` does not return: it ends the process. Every path that
/// quits therefore calls this *before* `cx.quit()`.
fn release_retained() {
    view::release_retained();
    PENDING_OPEN.with(|q| q.borrow_mut().clear());
    PENDING_CLOSE.with(|q| q.borrow_mut().clear());
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
pub struct PendingOpen {
    pub title: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub options: String,
    pub view: RetainedCallback,
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
static TICKS: LazyLock<Mutex<HashSet<(i64, String)>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Which window's menu is currently installed.
///
/// `cx.set_menus` is application-wide, so the last window to ask for a menu
/// owns the whole bar; the action handler routes the click back to that window
/// rather than to whichever window happened to be opened first.
static MENU_OWNER: LazyLock<Mutex<(i64, Option<EntityId>)>> =
    LazyLock::new(|| Mutex::new((0, None)));

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
    // A handler registered this frame goes to the platform here, where there is
    // a `Window` to give it to.
    if let Some(decide) = view::take_should_close_for_platform(win) {
        window.on_window_should_close(cx, move |_window, _cx| {
            view::ask_should_close(win, &decide)
        });
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
                // Upstream's `cx.observe_keystrokes`: every keystroke the
                // window sees, whether or not a focused element handled it.
                static OBSERVED: LazyLock<Mutex<HashSet<(i64, String)>>> =
                    LazyLock::new(|| Mutex::new(HashSet::new()));
                if !OBSERVED.lock().unwrap().insert((win, id.clone())) {
                    continue;
                }
                let key = id.clone();
                // `Context::observe_keystrokes`, so the closure takes the
                // entity as well as the event.
                cx.observe_keystrokes(move |_this, event, _window, cx| {
                    window::with(win, || {
                        store::push_event_with(&key, event.keystroke.unparse());
                        store::set_text(
                            "~key.char",
                            event.keystroke.key_char.as_deref().unwrap_or(""),
                        );
                    });
                    cx.notify();
                })
                .detach();
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
                let mut menu_items = Vec::new();
                for (index, entry) in items.iter().enumerate() {
                    let mut parts = entry.splitn(3, ':');
                    match parts.next().unwrap_or("") {
                        "separator" => menu_items.push(gpui::MenuItem::separator()),
                        "quits" => menu_items.push(gpui::MenuItem::action(
                            parts.next().unwrap_or("Quit").to_string(),
                            Quit,
                        )),
                        "events" => {
                            let label = parts.next().unwrap_or("").to_string();
                            push_menu_action(&mut menu_items, label, index);
                        }
                        _ => {}
                    }
                }
                cx.set_menus(vec![gpui::Menu {
                    name: name.into(),
                    items: menu_items,
                }]);
            }
            Request::ResizeWindow { w, h } => window.resize(size(px(w as f32), px(h as f32))),

            Request::SetRemSize { px: rem } => window.set_rem_size(gpui::px(rem as f32)),

            Request::SetClipboard { text } => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text))
            }

            Request::Tick { id, ms } => {
                let fresh = TICKS.lock().unwrap().insert((win, id.clone()));
                if !fresh {
                    continue;
                }
                let tick_id = id.clone();
                cx.spawn(async move |_this, cx| {
                    loop {
                        Timer::after(Duration::from_millis(ms)).await;
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

            Request::Task { id, ms } => {
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
                    let _ = cx.update(|cx| {
                        window::with(win, || {
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

/// Builds the window options a program asked for, from the
/// `"kind=popup show=false place=center …"` vocabulary.
fn window_options(
    title: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    spec: &str,
) -> WindowOptions {
    let mut centered = false;
    let mut kind = gpui::WindowKind::Normal;
    let mut show = true;
    let mut movable = true;
    let mut minimizable = true;
    let mut focus = true;
    let mut resizable = true;
    let mut transparent = false;
    let mut no_titlebar = false;
    let mut background = false;
    let mut float = false;
    for (key, value) in crate::style::tokens(spec) {
        let v = value.as_deref().unwrap_or("");
        match (key.as_str(), v) {
            ("place", "center") => centered = true,
            ("kind", "popup") => kind = gpui::WindowKind::PopUp,
            ("kind", "floating") => kind = gpui::WindowKind::Floating,
            ("show", "false") => show = false,
            ("movable", "false") => movable = false,
            ("minimizable", "false") => minimizable = false,
            ("focus", "false") => focus = false,
            ("resizable", "false") => resizable = false,
            ("titlebar", "none") => no_titlebar = true,
            ("background", "transparent") => transparent = true,
            ("background", "blur") => background = true,
            ("float", "true") => float = true,
            _ => {}
        }
    }
    if float {
        kind = gpui::WindowKind::Floating;
    }
    WindowOptions {
        window_bounds: Some(if centered {
            WindowBounds::Windowed(Bounds::new(
                gpui::point(px(x as f32), px(y as f32)),
                size(px(w as f32), px(h as f32)),
            ))
        } else {
            WindowBounds::Windowed(Bounds::new(
                gpui::point(px(x as f32), px(y as f32)),
                size(px(w as f32), px(h as f32)),
            ))
        }),
        titlebar: Some(TitlebarOptions {
            title: Some(title.clone().into()),
            appears_transparent: no_titlebar,
            traffic_light_position: traffic_lights(no_titlebar),
        }),
        window_decorations: client_decorations(no_titlebar),
        kind,
        show,
        is_movable: movable,
        is_minimizable: minimizable,
        focus,
        is_resizable: resizable,
        window_background: if background {
            gpui::WindowBackgroundAppearance::Blurred
        } else if transparent {
            gpui::WindowBackgroundAppearance::Transparent
        } else {
            gpui::WindowBackgroundAppearance::Opaque
        },
        ..Default::default()
    }
}

/// Opens the window and runs GPUI's event loop until the program quits.
///
/// This call does not return until the app exits: the Gossamer view callback
/// stays valid (and is re-invoked once per frame) for its whole duration.
pub fn run_app(
    dispatch: &mut dyn NativeDispatch,
    title: String,
    width: f64,
    height: f64,
    options: &str,
    view: RetainedCallback,
) -> Result<(), String> {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return Err("gpui::run has already been called in this process".into());
    }

    let mut resizable = true;
    let mut no_titlebar = false;
    let mut always_on_top = false;
    let mut background = false;
    for (key, value) in crate::style::tokens(options) {
        match (key.as_str(), value.as_deref()) {
            ("resizable", Some("false")) => resizable = false,
            ("titlebar", Some("none")) => no_titlebar = true,
            ("float", Some("true")) => always_on_top = true,
            ("background", Some("blur")) => background = true,
            _ => {}
        }
    }

    view::stash_dispatch(dispatch);

    // The first window and every other: a window's id is allocated before it
    // is drawn, so its very first frame already knows which window it is.
    let main_window = window::allocate();

    let root_view = view.clone();
    Application::new().run(move |cx: &mut App| {
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
        let bounds = Bounds::centered(None, size(px(width as f32), px(height as f32)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.clone().into()),
                appears_transparent: no_titlebar,
                traffic_light_position: traffic_lights(no_titlebar),
            }),
            // macOS and Windows hide the system titlebar with
            // `appears_transparent` and let the program draw its own. Linux
            // spells that `window_decorations` — the X11 backend reads only
            // `titlebar.title`, so a program asking for `titlebar=none` there
            // would otherwise get a decorated window *and* its own drawn bar.
            window_decorations: client_decorations(no_titlebar),
            kind: if always_on_top {
                gpui::WindowKind::Floating
            } else {
                gpui::WindowKind::Normal
            },
            is_resizable: resizable,
            window_background: if background {
                gpui::WindowBackgroundAppearance::Blurred
            } else {
                gpui::WindowBackgroundAppearance::Opaque
            },
            ..Default::default()
        };

        let opened = cx.open_window(options, move |window, cx| {
            install_appearance_colors(window, cx);
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

        cx.activate(true);

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

/// The traffic-light position is a macOS-only window option.
#[cfg(target_os = "macos")]
fn traffic_lights(no_titlebar: bool) -> Option<gpui::Point<gpui::Pixels>> {
    if no_titlebar {
        Some(gpui::point(px(12.0), px(12.0)))
    } else {
        None
    }
}

#[cfg(not(target_os = "macos"))]
fn traffic_lights(_no_titlebar: bool) -> Option<gpui::Point<gpui::Pixels>> {
    None
}

/// Hiding the system titlebar is `window_decorations` on Linux, and nothing at
/// all off it — macOS and Windows use `appears_transparent` for the same thing.
#[cfg(target_os = "linux")]
fn client_decorations(no_titlebar: bool) -> Option<gpui::WindowDecorations> {
    if no_titlebar {
        Some(gpui::WindowDecorations::Client)
    } else {
        None
    }
}

#[cfg(not(target_os = "linux"))]
fn client_decorations(_no_titlebar: bool) -> Option<gpui::WindowDecorations> {
    None
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
