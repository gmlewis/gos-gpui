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

use gossamer_binding::{NativeDispatch, PersistentCallback};

use crate::store::{self, Request};
use crate::view;

/// The single root view. It owns the Gossamer view callback and rebuilds the
/// element tree from scratch on every frame.
pub struct Root {
    view: PersistentCallback,
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().entity_id();

        view::begin_frame(window, cx);
        view::reset();
        // A frame names its root with `gpui::root`, or simply answers the
        // element it wants; one that does neither reports itself.
        if let Err(e) = view::invoke(&self.view, vec![]) {
            view::note_error(format!("the view callback failed: {e}"));
        }
        let root: Option<i64> = view::named_root();
        let element: AnyElement = view::finish(root, window, cx, entity);

        if process_requests(window, cx, entity) {
            // The program asked to keep animating: line up the next frame.
            window.request_animation_frame();
        }

        element
    }
}

// Windows a frame asked for. Kept here rather than in `store::Request`
// because a `static Mutex` would demand `PersistentCallback: Send`.
thread_local! {
    static PENDING_OPEN: RefCell<Vec<PendingOpen>> = const { RefCell::new(Vec::new()) };
}

/// One window waiting to be opened, once the frame that asked for it is done.
pub struct PendingOpen {
    pub title: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub options: String,
    pub view: PersistentCallback,
}

/// Queues a window. It is opened after the current frame, never during it:
/// `App::open_window` always draws the new window once, and a nested draw
/// would re-enter `Root::render` and reset the element arena the frame is
/// still using.
pub fn queue_window(pending: PendingOpen) {
    PENDING_OPEN.with(|q| q.borrow_mut().push(pending));
}

/// Installed repeating timers, keyed by the event id they emit.
static TICKS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Runs every request a frame asked for. Answers `true` when the program wants
/// another frame immediately.
fn process_requests(window: &mut Window, cx: &mut Context<Root>, entity: EntityId) -> bool {
    let _ = window;
    let mut animate = false;
    for request in store::take_requests() {
        match request {
            Request::Animate => animate = true,

            Request::Quit => cx.quit(),

            Request::FocusNext => window.focus_next(),
            Request::FocusPrev => window.focus_prev(),
            Request::CloseWindow => window.remove_window(),

            Request::ObserveKeys { id } => {
                // Upstream's `cx.observe_keystrokes`: every keystroke the
                // window sees, whether or not a focused element handled it.
                static OBSERVED: LazyLock<Mutex<HashSet<String>>> =
                    LazyLock::new(|| Mutex::new(HashSet::new()));
                if !OBSERVED.lock().unwrap().insert(id.clone()) {
                    continue;
                }
                let key = id.clone();
                // `Context::observe_keystrokes`, so the closure takes the
                // entity as well as the event.
                cx.observe_keystrokes(move |_this, event, _window, cx| {
                    store::push_event_with(&key, event.keystroke.unparse());
                    store::set_text(
                        "~key.char",
                        event.keystroke.key_char.as_deref().unwrap_or(""),
                    );
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
                    install_menu_actions(cx, entity);
                }
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

            Request::Tick { id, ms } => {
                let fresh = TICKS.lock().unwrap().insert(id.clone());
                if !fresh {
                    continue;
                }
                let tick_id = id.clone();
                cx.spawn(async move |_this, cx| {
                    loop {
                        Timer::after(Duration::from_millis(ms)).await;
                        let alive = cx.update(|cx| {
                            store::push_event(tick_id.clone());
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
                        store::set_text(&format!("{task_id}.result"), &result);
                        store::push_event(task_id.clone());
                        cx.notify(entity);
                    });
                })
                .detach();
            }
        }
    }
    // Windows the frame asked for, opened once it is finished.
    let pending = PENDING_OPEN.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if !pending.is_empty() {
        cx.defer(move |cx| {
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
                let _ = cx.open_window(options, move |_window, cx| {
                    cx.new(|_| Root { view: view.clone() })
                });
            }
        });
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
    view: PersistentCallback,
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

    let root_view = view.clone();
    Application::new().run(move |cx: &mut App| {
        // Before any window exists: opening one draws it once, and that first
        // frame reads the default colours.
        cx.init_colors();
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

        let opened = cx.open_window(options, move |_window, cx| {
            cx.new(|_| Root { view: root_view.clone() })
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
                cx.quit();
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

/// Wires every action slot once. A menu entry raises `"menu|<index>"`.
fn install_menu_actions(cx: &mut App, entity: EntityId) {
    fn wire<A: gpui::Action>(cx: &mut App, entity: EntityId, index: i64) {
        cx.on_action(move |_: &A, cx| {
            store::push_event(format!("menu|{index}"));
            cx.notify(entity);
        });
    }
    wire::<Slot0>(cx, entity, 0);
    wire::<Slot1>(cx, entity, 1);
    wire::<Slot2>(cx, entity, 2);
    wire::<Slot3>(cx, entity, 3);
    wire::<Slot4>(cx, entity, 4);
    wire::<Slot5>(cx, entity, 5);
    wire::<Slot6>(cx, entity, 6);
    wire::<Slot7>(cx, entity, 7);
}
