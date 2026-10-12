# gos-gpui

GPU-accelerated GUI applications in [Gossamer](https://github.com/gossamer-lang/gossamer), powered by [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui).

`gos-gpui` provides high-level declarative UI components and native bindings for GPUI, giving Gossamer programs fast GPU rendering (Metal on macOS, Vulkan/X11 on Linux) with a clean, fluent styling API.

---

## Features

- **GPU Acceleration**: Smooth 60+ FPS rendering and animations backed by GPUI.
- **Fluent UI API**: Ergonomic tree-building DSL (`ui::column`, `ui::row`, `ui::button`, `ui::input`, etc.).
- **Tailwind-inspired Styling**: Concise utility strings for sizing, layout, padding, colors, borders, and interaction states (`hover=`, `active=`).
- **Reactive State**: Window-scoped reactive state helpers (`state_int`, `set_int`, `state_text`, `set_bool`, etc.).
- **Virtual Lists**: Smoothly handles 50,000+ rows with constant memory using `ui::list` and `gpui::run_with_rows`.
- **Keyboard & Pointer Navigation**: Native shortcuts with `gpui::bind_keys`, focus management, and pointer interaction.
- **37 Interactive Examples**: Ready-to-run examples demonstrating layouts, animations, forms, canvas drawing, and theming.

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

## Quick Start

A minimal counter application:

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

---

## Running the Examples

The repository includes 37 examples numbered `01_` through `37_` under `examples/`.

### Using `./run.sh`

```bash
./run.sh 01               # Run 01_animation.gos
./run.sh 07               # Run 07_hello_world.gos
./run.sh 33               # Run 33_lists.gos (default 20,000 rows)
./run.sh 33 50000         # Pass arguments (e.g. 50,000 rows)
./run.sh 23 8             # Tree view with depth 8
./run.sh dashboard        # Match example by name (37_dashboard.gos)
```

### Using `gos` Directly

```bash
gos run --main-thread examples/01_animation.gos
```

### Building Standalone Native Binaries

To compile an example into a native standalone executable:

```bash
./build.sh 01             # Output in target/debug/
./build.sh 01 --release   # Output in target/release/
```

---

## Core API Reference

### Layout Elements (`ui`)

- `ui::column(style)`: Vertical flex layout container.
- `ui::row(style)`: Horizontal flex layout container.
- `ui::panel(style)`: General container div.
- `ui::root(element)`: Marks the root container to return to GPUI.
- `.child(element)`: Append a child element.
- `.text(string, style)`: Add a text element.
- `.button(id, label, style)`: Add a clickable button.
- `.input(key, placeholder, style)`: Add a text input field.
- `.checkbox(id, label, checked, style)`: Add a checkbox toggle.
- `.list(id, count, row_height, style)`: Add a virtualized list.

### Styling DSL

Styles are expressed as space-separated key-value tokens:

| Token | Description | Examples |
|---|---|---|
| **Sizing** | Width and height | `w=full`, `w=320`, `h=48`, `size=full` |
| **Spacing** | Padding and margins | `p=16`, `px=12`, `py=8`, `m=4`, `gap=10` |
| **Flex** | Alignment & justification | `items=center`, `justify=between`, `justify=center` |
| **Colors** | Background and text color | `bg=#1e1e2e`, `color=#cdd6f4`, `bordercolor=#313244` |
| **Typography** | Font size & weight | `font=xs`, `font=sm`, `font=base`, `font=xl`, `bold` |
| **Borders** | Border width & radius | `border=1`, `rounded=8`, `rounded=full` |
| **States** | Interactive pseudoclasses | `hover=bg:#313244`, `active=bg:#45475a` |

### State Management (`gpui`)

State is preserved across redraw frames:

```gos
// Read state with a fallback default:
let count = gpui::state_int("count", 0)
let query = gpui::state_text("query", "")
let dark  = gpui::state_bool("dark", true)

// Update state:
gpui::set_int("count", count + 1)
gpui::set_text("query", "search text")
gpui::set_bool("dark", false)
```

### Event Handling

```gos
for event in gpui::take_events() {
    let id, data = gpui::event_parts(event)
    // Buttons raise their id ("click_me")
    // Keybindings raise their bound action name
}
```

### Keybindings & Keyboard Navigation

Bind application shortcuts globally with `gpui::bind_keys`:

```gos
// Format: "<stroke>=<action>; ..."
gpui::bind_keys("home=home; end=end; pageup=pageup; pagedown=pagedown; up=up; down=down")
```

For raw keyboard capture on the window:
```gos
gpui::on_keys(true)
```

### Virtual Lists

Virtual lists render only the items visible on screen, scaling to hundreds of thousands of items:

```gos
fn my_row(list_id: String, index: i64) -> ui::El {
    ui::button(f"row-{index}", f"Row #{index}", "w=full h=full px=12 items=center bg=#181825")
}

fn main() {
    gpui::run_with_rows("Virtual List", 500.0, 600.0, "", my_row, || {
        ui::column("size=full p=16")
            .list("my_list", 100_000, 28.0, "w=full h=500")
            |> ui::root
    })
}
```

Navigate virtual lists programmatically:
```gos
gpui::list_scroll_to("my_list", 0, "top")           // Scroll to start
gpui::list_scroll_to("my_list", 99999, "bottom")    // Scroll to end
let first, last = gpui::list_range("my_list")       // Read visible index range
```

---

## Project Structure

```
gos-gpui/
├── src/                # Gossamer package source (ui chaining layer)
│   └── lib.gos         # Fluent ui element builders and style mappings
├── gpui-binding/       # Rust crate binding GPUI to Gossamer
│   ├── Cargo.toml
│   └── src/            # Platform integration, view tree, and event queues
├── examples/           # 37 numbered example programs (01_.. to 37_)
├── tests/              # Headless regression and unit tests
├── run.sh              # CLI runner for examples (./run.sh <num>)
├── build.sh            # Standalone native binary builder (./build.sh <num>)
└── scripts/
    └── check-all.sh    # Verifies all 37 examples pass `gos check`
```

---

## Verification & Testing

Run the test suite and typechecks:

```bash
# Run headless unit tests:
gos test tests

# Typecheck all 37 examples:
./scripts/check-all.sh
```

---

## License

Apache License 2.0. See [LICENSE](LICENSE) for details.
