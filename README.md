# gos-gpui

GPU-accelerated desktop GUI applications in [Gossamer](https://github.com/gossamer-lang/gossamer), powered by [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui).

`gos-gpui` provides high-level declarative UI components and native bindings for GPUI, giving Gossamer programs fast GPU rendering (Metal on macOS, Vulkan/X11 on Linux) with a clean, fluent styling API.

---

## Features

- **GPU Acceleration**: Smooth 60+ FPS rendering and animations backed by GPUI.
- **Fluent UI API**: Ergonomic tree-building DSL (`ui::column`, `ui::row`, `ui::button`, `ui::input`, `ui::slider`, `ui::canvas`, etc.).
- **Tailwind-inspired Styling**: Concise utility strings for sizing, layout, padding, colors, borders, shadows, and interaction states (`hover=`, `active=`).
- **Reactive State**: Window-scoped reactive state helpers (`state_int`, `set_int`, `state_text`, `set_bool`, `ui::Store`, `ui::Model`).
- **Virtual Lists**: Smoothly handles 50,000+ rows with constant memory using `ui::list` and `gpui::run_with_rows`.
- **Keyboard & Pointer Navigation**: Native shortcuts with `gpui::bind_keys`, focus management, and pointer tracking.
- **Native OS Dialogs**: Native modal alerts with `gpui::prompt` and file/folder pickers.
- **Vector Canvas**: Hardware-accelerated 2D vector painting with Bézier paths, arcs, and gradients.
- **37 Interactive Examples**: Ready-to-run examples demonstrating layouts, animations, forms, canvas drawing, menus, and themes.

---

## Prerequisites

- **Gossamer**: The [`gos`](https://github.com/gossamer-lang/gossamer) compiler toolchain.
- **Rust**: Rust stable with Cargo (`rustup update stable`).
- **macOS**: Works out of the box (uses macOS Metal and AppKit frameworks).
- **Linux**: Build dependencies for GPUI:
  ```bash
  sudo apt-get install build-essential pkg-config \
    libx11-dev libxcb1-dev libxext-dev libxrender-dev \
    libxcursor-dev libxinput-dev libfreetype-dev
  ```

---

## Installation

Add `gos-gpui` to your `project.toml`:

```toml
[dependencies]
gos-gpui = { git = "https://github.com/gmlewis/gos-gpui" }
```

Or reference a local checkout during development:

```toml
[dependencies]
gos-gpui = { path = "../gos-gpui" }
```

---

## Desktop Application Architecture

Gossamer + GPUI applications follow an immediate-mode declarative frame callback architecture:

```
┌────────────────────────────────────────────────────────┐
│                   GPUI Event Loop                      │
│      (Drives native AppKit / X11 window events)        │
└──────────────────────────┬─────────────────────────────┘
                           │ triggers redraw (60-120 FPS)
                           ▼
┌────────────────────────────────────────────────────────┐
│               Frame Callback (|| { ... })              │
│                                                        │
│  1. Drain events:      gpui::take_events() |> handle   │
│  2. Read state:        gpui::state_int("count", 0)     │
│  3. Construct scene:   ui::column(...)                 │
│                          .button("inc", "+", ...)      │
│  4. Return root:       |> ui::root                     │
└──────────────────────────┬─────────────────────────────┘
                           │ layout & render passes
                           ▼
┌────────────────────────────────────────────────────────┐
│            GPU Rendering (Metal / Vulkan)              │
└────────────────────────────────────────────────────────┘
```

### The Mental Model

1. **The Native Event Loop**: GPUI owns the application runloop and handles window creation, display scaling, font rasterization, and GPU command submission.
2. **The Frame Callback**: Your application provides a view closure `|| { ... }`. GPUI invokes this closure on demand—when input occurs, timers fire, animations tick, or state updates.
3. **Event Draining**: Each frame begins by draining pending events with `gpui::take_events()` (or `ui::poll_and_dispatch()`). Controls emit identifiers (e.g. `"inc"`, `"submit"`), and keys emit bound action names.
4. **State Persistence**: Because the view closure runs repeatedly, state that must outlive a single frame is stored in GPUI's per-window state store (`gpui::state_*` / `gpui::set_*`) or a reactive `ui::Store<T>`.
5. **Declarative Scene Graph**: You assemble UI elements fluently, terminating with `|> ui::root`. GPUI calculates flexbox/grid layout via Taffy and submits drawing commands to the GPU.

---

## Quick Start

A complete interactive counter application:

```gos
use gpui
use ui

fn handle(events: Vec<String>) {
    for event in events {
        let id, _ = gpui::event_parts(event)
        let count = gpui::state_int("count", 0)
        if id == "inc" {
            gpui::set_int("count", count + 1)
        } else if id == "dec" {
            gpui::set_int("count", count - 1)
        }
    }
}

fn main() {
    let outcome = gpui::run("Counter", 360.0, 240.0, || {
        gpui::take_events() |> handle
        let count = gpui::state_int("count", 0)

        ui::column("size=full items=center justify=center bg=#1e1e2e gap=16")
            .text("Counter", "font=2xl bold color=#cdd6f4")
            .text(f"{count}", "font=4xl bold color=#89b4fa")
            .child(ui::row("gap=12")
                .button("dec", "−", "w=48 h=40 rounded=8 bg=#313244 hover=bg:#45475a color=#cdd6f4 font=xl")
                .button("inc", "+", "w=48 h=40 rounded=8 bg=#89b4fa hover=bg:#b4befe color=#11111b font=xl"))
            |> ui::root
    })

    if let Err(err) = outcome {
        eprintln(f"Error: {err}")
    }
}
```

Run with:
```bash
gos run --main-thread main.gos
```

> [!NOTE]
> Always pass `--main-thread` when running GPUI applications with `gos run`, as desktop window servers (especially macOS AppKit) require GUI operations to execute on the platform main thread.

---

## Running the Examples

The repository includes 37 standalone interactive examples under `examples/`.

### Using `./run.sh`

```bash
./run.sh 01               # Run 01_animation.gos
./run.sh 07               # Run 07_hello_world.gos
./run.sh 27               # Run 27_window.gos (native modals & window options)
./run.sh 33               # Run 33_lists.gos (default 20,000 rows)
./run.sh 33 50000         # Pass arguments (e.g. 50,000 rows)
./run.sh 37               # Run 37_dashboard.gos (full desktop app showcase)
./run.sh dashboard        # Match example by name
```

### Building Standalone Native Binaries

Compile any example into an optimized standalone binary:

```bash
./build.sh 37             # Debug executable in target/debug/
./build.sh 37 --release   # Optimized executable in target/release/
```

---

## Examples Directory Catalog

The 37 examples showcase different features and architecture patterns:

| Category | Examples | Features Demonstrated |
|---|---|---|
| **Basics & Layout** | [`07_hello_world`](examples/07_hello_world.gos)<br>[`28_hello`](examples/28_hello.gos)<br>[`29_styling`](examples/29_styling.gos)<br>[`30_layout`](examples/30_layout.gos) | Minimal window, flex layout, spacing, alignment, nested containers. |
| **Controls & Forms** | [`09_input`](examples/09_input.gos)<br>[`19_tab_stop`](examples/19_tab_stop.gos)<br>[`31_interaction`](examples/31_interaction.gos)<br>[`32_forms`](examples/32_forms.gos) | Text inputs, checkboxes, sliders, keyboard tab navigation, focus rings. |
| **Lists & Tables** | [`02_data_table`](examples/02_data_table.gos)<br>[`23_tree`](examples/23_tree.gos)<br>[`24_uniform_list`](examples/24_uniform_list.gos)<br>[`33_lists`](examples/33_lists.gos) | Virtualized lists (50,000+ rows), column headers, Home/End/PgUp/PgDn keyboard scrolling, tree views. |
| **Windows & Menus** | [`10_on_window_close_quit`](examples/10_on_window_close_quit.gos)<br>[`17_set_menus`](examples/17_set_menus.gos)<br>[`25_window_positioning`](examples/25_window_positioning.gos)<br>[`26_window_shadow`](examples/26_window_shadow.gos)<br>[`27_window`](examples/27_window.gos) | Native application menus, keyboard accelerators, modal alerts (`gpui::prompt`), popup windows, frameless windows (`titlebar=none`). |
| **Painting & Canvas** | [`05_gradient`](examples/05_gradient.gos)<br>[`13_painting`](examples/13_painting.gos)<br>[`14_paths_bench`](examples/14_paths_bench.gos)<br>[`15_pattern`](examples/15_pattern.gos) | 2D vector drawing (`ui::canvas`), Bézier curves, path strokes and fills, linear/radial gradients. |
| **Graphics & Images** | [`03_drag_drop`](examples/03_drag_drop.gos)<br>[`04_gif_viewer`](examples/04_gif_viewer.gos)<br>[`08_image_loading`](examples/08_image_loading.gos)<br>[`11_opacity`](examples/11_opacity.gos)<br>[`18_shadow`](examples/18_shadow.gos) | Drag-and-drop gestures, animated GIF rendering, asynchronous image loading with placeholder fallbacks, drop shadows. |
| **Animation & Async** | [`01_animation`](examples/01_animation.gos)<br>[`34_animation`](examples/34_animation.gos)<br>[`35_async`](examples/35_async.gos) | Physics springs, easing curves (`gpui::animate`, `gpui::tick`), non-blocking background tasks (`gpui::task`). |
| **Themes & Apps** | [`36_theme`](examples/36_theme.gos)<br>[`37_dashboard`](examples/37_dashboard.gos) | Dark/Light mode theme switching, full dashboard app with sidebar, stat cards, data grids, and charts. |

---

## Desktop Development Recipes

### 1. Native Modal Prompts

Show native OS message boxes and confirmation dialogs:

```gos
// Queue a prompt with button choices:
let id = gpui::prompt("Unsaved Changes", "Do you want to save before quitting?", #["Save", "Don't Save", "Cancel"])

// Read prompt response events:
for event in gpui::take_events() {
    let id, action = gpui::event_parts(event)
    if id == "prompt" {
        if action == "Save" {
            save_document()
        } else if action == "Don't Save" {
            gpui::quit()
        }
    }
}
```

### 2. Native File & Folder Dialogs

Prompt users to open or save files:

```gos
// Open File Dialog:
let opts = ui::PathPromptOptions {
    files: true,
    directories: false,
    multiple: false,
    prompt: "Choose File",
}
let dialog_id = ui::prompt_for_paths(opts)

// Save File Dialog:
let save_opts = ui::NewPathPromptOptions {
    directory: "/tmp",
    suggested_name: "document.txt",
}
let save_id = ui::prompt_for_new_path(save_opts)
```

### 3. Application Menus & Global Shortcuts

Install native system menus with keyboard accelerators:

```gos
gpui::set_menus("MyApp", #[
    "submenu:File",
    ">events:New:cmd-n",
    ">events:Open...:cmd-o",
    ">events:Save:cmd-s",
    ">separator",
    "submenu:Edit",
    ">events:Undo:cmd-z",
    ">events:Redo:cmd-shift-z",
    ">separator",
    ">events:Cut:cmd-x",
    ">events:Copy:cmd-c",
    ">events:Paste:cmd-v",
    ">separator",
    "events:Quit:cmd-q",
])

// Bind global key shortcuts:
gpui::bind_keys("cmd-s=save; cmd-o=open; cmd-q=quit; home=home; end=end; pageup=pageup; pagedown=pagedown")
```

### 4. Continuous Dragging & Slider Focus

When building draggable controls (like sliders or splitters), register window-level mouse listeners (`on_mouse_window`) so dragging continues uninterrupted even if the cursor moves outside the control's bounding box:

```gos
// Window-level mouse tracking keeps drag alive outside element bounds:
let slider = ui::slider("volume", 0.0, 100.0, "w=240 h=24")
    .on_mouse_window("move up", "volume_drag")
```

### 5. High-Performance Virtual Lists

Render datasets with 100,000+ items smoothly at 60 FPS:

```gos
fn render_row(list_id: String, index: i64) -> ui::El {
    ui::row("w=full h=32 px=12 items=center bg=#181825 borderb=1 bordercolor=#313244")
        .text(f"Record #{index}", "color=#cdd6f4 font=sm")
}

fn main() {
    gpui::run_with_rows("Data Viewer", 600.0, 500.0, "", render_row, || {
        ui::column("size=full p=16 bg=#11111b")
            .list("records_list", 100_000, 32.0, "size=full")
            |> ui::root
    })
}
```

### 6. Hardware-Accelerated Vector Canvas

Draw custom charts, diagrams, and shapes with Bézier curves:

```gos
let my_canvas = ui::canvas(|cx| {
    // Stroked Bézier curve:
    let _ = cx.begin_stroke(3.0, "#89b4fa")
        .move_to(20.0, 20.0)
        .line_to(80.0, 60.0)
        .cubic_to(180.0, 120.0, 120.0, 40.0, 160.0, 90.0)
        .close()

    // Filled polygon:
    let _ = cx.begin_fill("#a6e3a1")
        .move_to(40.0, 40.0)
        .line_to(80.0, 40.0)
        .line_to(60.0, 10.0)
        .close()
})
```

### 7. Background Tasks & Non-Blocking Async Work

Keep the UI responsive during computation or I/O:

```gos
// Start background task:
gpui::task("fetch_data", || {
    // Perform heavy computation or I/O
    gpui::set_text("data.result", "Fetched 1,000 items")
})

// In the frame callback, check results:
let result = gpui::task_result("fetch_data")
if result != "" {
    // Update UI with results
}
```

---

## Core API Reference

### Layout Elements (`ui`)

- `ui::column(style)`: Vertical flex container.
- `ui::row(style)`: Horizontal flex container.
- `ui::panel(style)`: Generic container element (`ui::div()`).
- `ui::text(content, style)`: Text label.
- `ui::button(id, label, style)`: Clickable button.
- `ui::input(key, placeholder, style)`: Text input field.
- `ui::checkbox(id, label, checked, style)`: Checkbox toggle.
- `ui::slider(key, min, max, style)`: Numeric slider control.
- `ui::progress(value, style)`: Progress bar (0.0 to 1.0).
- `ui::canvas(callback)`: 2D vector painting surface.
- `ui::list(id, count, row_height, style)`: Virtualized list container.
- `ui::modal(content)`: High-priority modal dialog overlay.
- `ui::anchored(corner, x, y, content)`: Positioned overlay (tooltips, dropdowns).
- `ui::root(element)`: Terminates frame chain and returns root to GPUI.

### Styling DSL

Styles are space-separated key-value tokens:

| Token | Description | Examples |
|---|---|---|
| **Sizing** | Width and height | `w=full`, `w=320`, `h=48`, `size=full`, `minw=200`, `maxw=800` |
| **Spacing** | Padding and margins | `p=16`, `px=12`, `py=8`, `m=4`, `gap=10`, `gapx=8`, `gapy=12` |
| **Flex** | Alignment & distribution | `items=center`, `justify=between`, `justify=center`, `grow`, `shrink` |
| **Colors** | Background and foreground | `bg=#1e1e2e`, `color=#cdd6f4`, `bordercolor=#313244` |
| **Typography** | Font size & weight | `font=xs`, `font=sm`, `font=base`, `font=xl`, `font=2xl`, `bold`, `italic` |
| **Borders** | Border width & radius | `border=1`, `rounded=8`, `rounded=full`, `rounded=none` |
| **Shadows** | Drop shadow elevation | `shadow=xs`, `shadow=sm`, `shadow=md`, `shadow=lg`, `shadow=xl` |
| **States** | Interactive pseudoclasses | `hover=bg:#313244`, `active=bg:#45475a`, `focus=bg:#45475a` |

You can also use typed chaining methods directly on any `El`:
```gos
ui::div()
    .flex_col()
    .items_center()
    .justify_between()
    .w(400.0)
    .h(300.0)
    .p(24.0)
    .bg("#1e1e2e")
    .border(1.0, "#313244")
    .rounded(12.0)
    .shadow_lg()
```

---

## Best Practices for Gossamer Desktop Apps

1. **Always Use `--main-thread`**: Desktop GUI frameworks (macOS Cocoa/AppKit in particular) require UI events and window management on thread 0.
2. **Keep Frame Callbacks Pure & Fast**: Avoid expensive synchronous calculations inside `|| { ... }`. Delegate heavy work to `gpui::task` and store the result in the state store.
3. **Use Window-Level Drag Listeners for Custom Controls**: For sliders, scrubbers, or splitters, attach `on_mouse_window("move up", id)` so the interaction continues smoothly even when the pointer briefly leaves the control bounds.
4. **Contrast Safety in Themes**: When building themes with light/dark toggles (see [`36_theme.gos`](examples/36_theme.gos)), ensure interactive hover and active styles adapt along with the base colors so text remains readable across all states.
5. **Virtualize Large Collections**: Never render thousands of elements directly into a column. Use `ui::list` and `gpui::run_with_rows` to maintain 60 FPS performance and flat memory usage.

---

## Project Structure

```
gos-gpui/
├── src/                # Gossamer package source (fluent ui layer)
│   └── lib.gos         # Element builders, chaining methods, style DSL
├── gpui-binding/       # Native Rust crate binding GPUI to Gossamer
│   ├── Cargo.toml
│   └── src/            # Platform integration, view tree, and event queues
├── examples/           # 37 numbered example programs (01_.. to 37_)
├── tests/              # Consolidated headless test suite
├── run.sh              # CLI runner for examples (./run.sh <num>)
├── build.sh            # Standalone native binary builder (./build.sh <num>)
└── scripts/
    └── check-all.sh    # Verifies all 37 examples pass `gos check`
```

---

## Verification & Testing

Run the test suite and typechecks:

```bash
# Run headless unit tests (39 tests, 183 assertions in ~3s):
gos test tests

# Typecheck all 37 examples:
./scripts/check-all.sh
```

---

## License

Apache License 2.0. Copyright 2026 Glenn Lewis. See [LICENSE](LICENSE) for details.
