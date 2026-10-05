//! The GPUI application: one window, one view callback, re-rendered whenever
//! state changes, a timer fires, or an animation asks for another frame.

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
    animate
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
        cx.on_window_closed(|cx| {
            if cx.windows().len() <= 1 {
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

gpui::actions!(gpui_binding, [Quit]);
