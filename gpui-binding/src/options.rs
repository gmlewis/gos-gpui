//! The window-option vocabulary: one parse, one application, one reader.
//!
//! `run`, `run_with` and `open_window` all take the same `"key=value …"` string,
//! and each used to read its own handful of keys — so `resizable=false` meant one
//! thing for the first window and another for every window opened after it,
//! `place=center` was computed and then ignored (both arms of the `if` were the
//! same expression), and a token neither recognised was dropped in silence. This
//! module owns the vocabulary, so the three entry points cannot disagree, and a
//! token it does not know is reported rather than ignored.
//!
//! Two of the things a program asks for are not things a binding can simply
//! write down. `start=maximized` and `start=fullscreen` are *platform* calls
//! (`zoom`, `toggle_fullscreen`) that a window can only make once it is on
//! screen, so [`take_start`] hands the state to the window's own second frame
//! instead of into `WindowOptions`, where gpui would use the bounds and nothing
//! else. And the answers only the platform has — the scale factor, whether the
//! window is zoomed, which display it landed on — are read once a frame in
//! [`observe`], because a binding function has no `Window` to ask.
//!
//! [`window_option`] is the reader over both halves: what the window was asked
//! for, and what it reports about itself.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gpui::{point, px, size, App, Bounds, WindowBounds, WindowKind, WindowOptions};

/// Where a window opens: at the coordinates it was given, centred on its display,
/// or — when the spec says nothing — whatever the entry point's own default is.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum Place {
    #[default]
    Default,
    At,
    Center,
}

/// What is behind the window: nothing, the desktop, or the desktop blurred.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum Background {
    #[default]
    Opaque,
    Transparent,
    Blurred,
}

/// The state a window opens in. Only `Windowed` can be written into
/// `WindowOptions`; the other two are platform calls made after the window is on
/// screen.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub enum Start {
    #[default]
    Windowed,
    Maximized,
    Fullscreen,
}

/// The window options a spec asks for.
#[derive(Clone)]
pub struct Options {
    pub place: Place,
    /// Which display to open on, by index into `cx.displays()`.
    pub display: Option<usize>,
    pub kind: WindowKind,
    pub show: bool,
    pub movable: bool,
    pub minimizable: bool,
    pub focus: bool,
    pub resizable: bool,
    pub no_titlebar: bool,
    pub background: Background,
    pub start: Start,
    /// Minimum content size, in pixels.
    pub min: Option<(f64, f64)>,
    /// Where the window's top-left corner goes, in pixels.
    ///
    /// `gpui::run` takes a title and a size and no position, so without this the *first* window
    /// can only be centred or at 0,0 — and a remembered position could never be restored to it.
    /// An extra window already takes x and y in `gpui::open_window`. Asking for a position also
    /// sets `place=at`, so a later `place=center` in the same spec wins as it would for any other
    /// token.
    pub pos: Option<(f64, f64)>,
    /// Application id, which desktop environments on Linux group windows by.
    pub app_id: Option<String>,
    /// Native tab group name (macOS).
    pub tabbing: Option<String>,
    /// The tokens this vocabulary does not know, in the order they appeared.
    pub unknown: Vec<String>,
}

impl Options {
    fn new() -> Self {
        Self {
            place: Place::Default,
            display: None,
            kind: WindowKind::Normal,
            show: true,
            movable: true,
            minimizable: true,
            focus: true,
            resizable: true,
            no_titlebar: false,
            background: Background::Opaque,
            start: Start::Windowed,
            min: None,
            pos: None,
            app_id: None,
            tabbing: None,
            unknown: Vec::new(),
        }
    }
}

/// Parses `"key=value …"` into the window options it names.
///
/// Every token either maps onto a field or lands in [`Options::unknown`]: a
/// misspelt key is not silently the same as not asking for anything. A key with
/// the right name and a value this vocabulary has no meaning for —
/// `kind=dialog`, `start=minimised` — is unknown too, for the same reason.
pub fn parse(spec: &str) -> Options {
    let mut options = Options::new();
    for (key, value) in crate::style::tokens(spec) {
        let v = value.as_deref().unwrap_or("");
        let known = match (key.as_str(), v) {
            ("place", "center") => {
                options.place = Place::Center;
                true
            }
            ("place", "at") => {
                options.place = Place::At;
                true
            }
            // An index that is not a number, or names no display, is still the
            // key being used correctly; the fallback is the primary display.
            ("display", n) => {
                if let Ok(index) = n.parse::<usize>() {
                    options.display = Some(index);
                    true
                } else {
                    false
                }
            }
            ("kind", "normal") => {
                options.kind = WindowKind::Normal;
                true
            }
            ("kind", "popup") => {
                options.kind = WindowKind::PopUp;
                true
            }
            ("kind", "floating") | ("float", "true") => {
                options.kind = WindowKind::Floating;
                true
            }
            ("show", "true") => {
                options.show = true;
                true
            }
            ("show", "false") => {
                options.show = false;
                true
            }
            ("movable", "true") => {
                options.movable = true;
                true
            }
            ("movable", "false") => {
                options.movable = false;
                true
            }
            ("minimizable", "true") => {
                options.minimizable = true;
                true
            }
            ("minimizable", "false") => {
                options.minimizable = false;
                true
            }
            ("focus", "true") => {
                options.focus = true;
                true
            }
            ("focus", "false") => {
                options.focus = false;
                true
            }
            ("resizable", "true") => {
                options.resizable = true;
                true
            }
            ("resizable", "false") => {
                options.resizable = false;
                true
            }
            ("titlebar", "none") => {
                options.no_titlebar = true;
                true
            }
            ("titlebar", "system") => {
                options.no_titlebar = false;
                true
            }
            ("background", "opaque") => {
                options.background = Background::Opaque;
                true
            }
            ("background", "transparent") => {
                options.background = Background::Transparent;
                true
            }
            ("background", "blur") | ("background", "blurred") => {
                options.background = Background::Blurred;
                true
            }
            ("start", "windowed") => {
                options.start = Start::Windowed;
                true
            }
            ("start", "maximized") => {
                options.start = Start::Maximized;
                true
            }
            ("start", "fullscreen") => {
                options.start = Start::Fullscreen;
                true
            }
            // The spelled-out forms, which read better in a one-key spec.
            ("maximized", "true") => {
                options.start = Start::Maximized;
                true
            }
            ("fullscreen", "true") => {
                options.start = Start::Fullscreen;
                true
            }
            // `minsize=420,240` — the comma is the same convention `font=` and
            // `shadow=` already use for a tuple.
            ("minsize", pair) => match pair.split_once(',') {
                Some((w, h)) => match (w.trim().parse::<f64>(), h.trim().parse::<f64>()) {
                    (Ok(w), Ok(h)) => {
                        options.min = Some((w, h));
                        true
                    }
                    _ => false,
                },
                None => false,
            },
            ("pos", pair) => match pair.split_once(',') {
                Some((x, y)) => match (x.trim().parse::<f64>(), y.trim().parse::<f64>()) {
                    (Ok(x), Ok(y)) => {
                        options.pos = Some((x, y));
                        options.place = Place::At;
                        true
                    }
                    _ => false,
                },
                None => false,
            },
            ("appid", id) => {
                options.app_id = Some(id.to_string());
                true
            }
            ("tabbing", name) => {
                options.tabbing = Some(name.to_string());
                true
            }
            _ => false,
        };
        if !known {
            options.unknown.push(if v.is_empty() { key } else { format!("{key}={v}") });
        }
    }
    options
}

thread_local! {
    /// Reports each distinct bad spec once, so a program that opens ten windows
    /// with the same typo is told once rather than ten times.
    static REPORTED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Says on stderr which tokens a spec asked for that nothing here understands.
///
/// Off the frame path deliberately: the style language reports its unknown
/// tokens on screen, where a program can see them, but a window option is read
/// before there is a window to draw in, so this is the stderr the toolchain
/// already surfaces.
fn report_unknown(spec: &str, unknown: &[String]) {
    if unknown.is_empty() {
        return;
    }
    let fresh = REPORTED.with(|r| r.borrow_mut().insert(spec.to_string()));
    if fresh {
        eprintln!("gpui: unknown window options: {}", unknown.join(" "));
    }
}

/// Builds the `WindowOptions` a window is opened with, and remembers it.
///
/// `center_by_default` is the entry point's own habit — `run` has always centred
/// its window, `open_window` has always taken the coordinates it was given — and
/// an explicit `place=` overrides it either way.
///
/// `win` is remembered **from the struct this returns**, not from the spec: the
/// reader is a mirror of what gpui was handed, so a field dropped between the
/// parse and the `WindowOptions` literal is a change the checks can see.
pub fn build(
    cx: &App,
    title: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    spec: &str,
    center_by_default: bool,
    win: i64,
) -> WindowOptions {
    let options = parse(spec);
    report_unknown(spec, &options.unknown);
    // A remembered position, if the spec asked for one; otherwise where the caller said.
    let (x, y) = options.pos.unwrap_or((x, y));

    // A display index that names nothing falls back on the primary display,
    // which is what `cx.displays()` resolving to `None` means here.
    let display = options
        .display
        .and_then(|index| cx.displays().get(index).map(|display| display.id()));
    let extent = size(px(w as f32), px(h as f32));
    let centered = match options.place {
        Place::Center => true,
        Place::At => false,
        Place::Default => center_by_default,
    };
    let bounds = if centered {
        Bounds::centered(display, extent, cx)
    } else {
        Bounds::new(point(px(x as f32), px(y as f32)), extent)
    };

    let built = WindowOptions {
        // Only `Windowed` is a size. `Maximized` and `Fullscreen` on this
        // platform are calls the window makes once it exists — `take_start`
        // raises them from its own first frame.
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some(title.to_owned().into()),
            appears_transparent: options.no_titlebar,
            traffic_light_position: traffic_lights(options.no_titlebar),
        }),
        window_decorations: client_decorations(options.no_titlebar),
        focus: options.focus,
        show: options.show,
        kind: options.kind,
        is_movable: options.movable,
        is_resizable: options.resizable,
        is_minimizable: options.minimizable,
        display_id: display,
        window_background: match options.background {
            Background::Opaque => gpui::WindowBackgroundAppearance::Opaque,
            Background::Transparent => gpui::WindowBackgroundAppearance::Transparent,
            Background::Blurred => gpui::WindowBackgroundAppearance::Blurred,
        },
        window_min_size: options
            .min
            .map(|(w, h)| size(px(w as f32), px(h as f32))),
        app_id: options.app_id.clone(),
        tabbing_identifier: options.tabbing.clone(),
        ..Default::default()
    };
    remember(win, &built, options.start, options.place);
    built
}

/// The traffic-light position is a macOS-only window option.
#[cfg(target_os = "macos")]
fn traffic_lights(no_titlebar: bool) -> Option<gpui::Point<gpui::Pixels>> {
    if no_titlebar {
        Some(point(px(12.0), px(12.0)))
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

/// What a window was asked for, and what it has reported about itself since.
#[derive(Clone, Default)]
struct Record {
    min: Option<(f64, f64)>,
    app_id: Option<String>,
    tabbing: Option<String>,
    kind: &'static str,
    place: &'static str,
    start: Start,
    background: &'static str,
    titlebar: &'static str,
    show: bool,
    movable: bool,
    minimizable: bool,
    resizable: bool,
    focus: bool,
    /// Whether the state the window asked to open in has been raised yet.
    started: bool,
    // ---- observed, once a frame ----
    live: bool,
    scale: f64,
    maximized: bool,
    fullscreen: bool,
    display: i64,
}

thread_local! {
    /// One record per window, written when it is opened and updated once a frame.
    static RECORDS: RefCell<HashMap<i64, Record>> = RefCell::new(HashMap::new());
}

/// Remembers what window `win` was opened with, read back off the options gpui
/// was handed. `start` and `place` come from the parse: they are the two facts
/// that do not survive into `WindowOptions` (a start state is a call, and a
/// centred window is just where its bounds landed).
fn remember(win: i64, built: &WindowOptions, start: Start, place: Place) {
    let record = Record {
        min: built
            .window_min_size
            .map(|size| (f32::from(size.width) as f64, f32::from(size.height) as f64)),
        app_id: built.app_id.clone(),
        tabbing: built.tabbing_identifier.clone(),
        kind: match built.kind {
            WindowKind::Normal => "normal",
            WindowKind::PopUp => "popup",
            WindowKind::Floating => "floating",
        },
        place: match place {
            Place::Center => "center",
            Place::At => "at",
            Place::Default => "",
        },
        start,
        background: match built.window_background {
            gpui::WindowBackgroundAppearance::Opaque => "opaque",
            gpui::WindowBackgroundAppearance::Transparent => "transparent",
            gpui::WindowBackgroundAppearance::Blurred => "blur",
        },
        titlebar: if built
            .titlebar
            .as_ref()
            .map(|titlebar| titlebar.appears_transparent)
            .unwrap_or(false)
        {
            "none"
        } else {
            "system"
        },
        show: built.show,
        movable: built.is_movable,
        minimizable: built.is_minimizable,
        resizable: built.is_resizable,
        focus: built.focus,
        ..Default::default()
    };
    RECORDS.with(|records| {
        records.borrow_mut().insert(win, record);
    });
}

/// Records what only the platform can say about `win`, read once a frame.
pub fn observe(win: i64, scale: f64, maximized: bool, fullscreen: bool, display: i64) {
    RECORDS.with(|r| {
        if let Some(record) = r.borrow_mut().get_mut(&win) {
            record.live = true;
            record.scale = scale;
            record.maximized = maximized;
            record.fullscreen = fullscreen;
            record.display = display;
        }
    });
}

/// The state `win` asked to open in, once, for the frame to raise.
pub fn take_start(win: i64) -> Option<Start> {
    RECORDS.with(|r| {
        let mut records = r.borrow_mut();
        let record = records.get_mut(&win)?;
        if record.started || record.start == Start::Windowed {
            return None;
        }
        record.started = true;
        Some(record.start)
    })
}

/// Formats a number the way a program would read it back: `420`, not `420.0`.
fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn flag(on: bool) -> String {
    if on { "1".to_string() } else { "0".to_string() }
}

/// What the current window was asked for, or reports about itself.
///
/// The asked-for half is fixed at open: `min`, `min_w`, `min_h`, `app_id`,
/// `tabbing`, `kind`, `place`, `start`, `background`, `titlebar`, `show`,
/// `movable`, `minimizable`, `focus`, `resizable`. The observed half is read once
/// a frame and is empty until the window has painted: `scale`, `maximized`,
/// `fullscreen`, `display`.
///
/// Anything else — including a name from the observed half in a program running
/// with no window at all — answers `""`.
pub fn window_option(name: &str) -> String {
    let win = crate::window::current();
    let record = RECORDS.with(|r| r.borrow().get(&win).cloned());
    let Some(record) = record else {
        return String::new();
    };
    match name {
        "min" => record
            .min
            .map(|(w, h)| format!("{},{}", number(w), number(h)))
            .unwrap_or_default(),
        "min_w" => record.min.map(|(w, _)| number(w)).unwrap_or_default(),
        "min_h" => record.min.map(|(_, h)| number(h)).unwrap_or_default(),
        "app_id" => record.app_id.clone().unwrap_or_default(),
        "tabbing" => record.tabbing.clone().unwrap_or_default(),
        "kind" => record.kind.to_string(),
        "place" => record.place.to_string(),
        "start" => match record.start {
            Start::Windowed => "windowed".to_string(),
            Start::Maximized => "maximized".to_string(),
            Start::Fullscreen => "fullscreen".to_string(),
        },
        "background" => record.background.to_string(),
        "titlebar" => record.titlebar.to_string(),
        "show" => flag(record.show),
        "movable" => flag(record.movable),
        "minimizable" => flag(record.minimizable),
        "focus" => flag(record.focus),
        "resizable" => flag(record.resizable),
        // Observed. `""` rather than a wrong answer before the first paint.
        "scale" if record.live => number(record.scale as f64),
        "maximized" if record.live => flag(record.maximized),
        "fullscreen" if record.live => flag(record.fullscreen),
        "display" if record.live => format!("{}", record.display),
        _ => String::new(),
    }
}

/// The window options in `spec` this crate does not understand.
///
/// The reader behind the `unknown window options:` line, for a program that
/// builds an option string from data and would rather check it than see a
/// banner.
pub fn unknown_window_tokens(spec: &str) -> Vec<String> {
    parse(spec).unknown
}


