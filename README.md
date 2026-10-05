# gpui examples, written in Gossamer

Ten programs that open real [GPUI](https://gpui.rs) windows on macOS, written
in [Gossamer](https://gossamer-lang.org/) and ranging from a twenty-line hello
to a dashboard with a custom titlebar, a virtualised activity list and a live
timer. GPUI is Rust, so the two are joined by a `[rust-bindings]` crate that
publishes the framework behind a flat, Gossamer-shaped module called `gpui`.

```
examples/01_hello.gos        one window, one block of text
examples/02_styling.gos      fills, gradients, corners, borders, shadows, type
examples/03_layout.gos       flexbox: direction, gap, growth, alignment, stacking
examples/04_interaction.gos  events, state, buttons, a slider, the keyboard
examples/05_forms.gos        text fields, focus, submit, an editable list
examples/06_lists.gos        a virtualised list of 20,000 rows
examples/07_animation.gos    a frame loop, custom painting, progress
examples/08_async.gos        repeating timers and background work
examples/09_theme.gos        a light and a dark palette
examples/10_dashboard.gos    all of the above, as one application
```

## Requirements

- **macOS on Apple silicon or Intel.** GPUI renders through Metal; it also
  supports Linux, but these examples were built and run on an M2 Max.
- **Xcode command-line tools** (`xcode-select --install`).
- **Gosammer 0.66.0** — `gos --version`.
- **Rust 1.85+** — GPUI needs a recent stable `rustc` and a Cargo that
  understands edition 2024. `cargo --version`.

You do **not** need the full Metal Toolchain component. GPUI normally compiles
its shaders at build time with `xcrun metal`, which is what
`xcodebuild -downloadComponent MetalToolchain` installs; the binding crate
turns on GPUI's `runtime_shaders` feature instead, so the shader source is
handed to Metal when the window is created. See
[`gpui-binding/Cargo.toml`](gpui-binding/Cargo.toml).

## Run an example

```sh
./run.sh 04                       # or: ./run.sh examples/04_interaction.gos
```

or directly:

```sh
gos run --main-thread examples/04_interaction.gos
```

`--main-thread` is not optional. GPUI builds an AppKit application, and AppKit
insists on the process main thread; `gos run` otherwise executes the program on
its own `gos-vm` thread. Gosammer has the flag precisely for native libraries
that mandate the main thread. Without it GPUI stops with
`must construct App on main thread`.

**The first run is slow.** `gos run` against a project with `[rust-bindings]`
builds a per-project runner with Cargo, and that means compiling GPUI — a few
hundred crates, several minutes. The result is cached under
`$XDG_CACHE_HOME/gossamer/runners/`, so every run after the first starts
immediately. A debug-profile GPUI is unoptimised, which shows in the animation
examples; `./build.sh` produces something much smoother.

```sh
./run.sh                         # list the examples
```

## Build a native binary

```sh
./build.sh 07 --release
./target/release/learn-gpui
```

Gosammer's `gos build` links the binary itself, and its linker plan can only
emit `-L` and `-l` — there is no manifest key for `-framework`, and `-lAppKit`
does not resolve because AppKit is a framework rather than a `libAppKit.tbd`.
Left alone the link fails with a wall of undefined `_CFStringCreateWithBytes`,
`_CGBitmapContextCreate`, `_CGSSetWindowBackgroundBlurRadius` and friends.

`gos build` does honour `$CC`, so [`tools/cc-gpui.sh`](tools/cc-gpui.sh) is a
one-line `cc` wrapper that adds the frameworks GPUI names and then re-execs.
`build.sh` sets it for you. This is a workaround for a toolchain gap, not
something the binding crate can fix from inside a `[rust-bindings]` entry.

## How it fits together

```
project.toml            the Gossamer project; names the binding
gpui-binding/           the Rust crate that wraps the gpui crate
  src/lib.rs              the binding functions Gossamer calls
  src/style.rs            the "key=value" style language
  src/store.rs            app state, the event queue, pending requests
  src/view.rs             the described element tree, and its translation to GPUI
  src/app.rs              the Application, the root view, timers
examples/*.gos          the programs
run.sh, build.sh        wrappers that supply the flags Gosammer needs
tools/cc-gpui.sh        a cc that links the Apple frameworks
```

GPUI is *hybrid* immediate and retained: you implement `Render` for a type and
GPUI calls it once per frame to get the whole element tree. The binding turns
that into the shape a Gossamer callback can actually hold, because a Gossamer
closure is **call-scoped** — it is only valid until the binding function that
received it returns. `gpui::run` never returns until the program quits, so the
view callback stays valid for exactly as long as GPUI keeps asking for frames.
Every frame looks like this:

```
   Gossamer            binding                              GPUI
   ────────            ───────                              ────
   gpui::run(…, || {   stash the dispatch, open a window,
     …               →  then block in Application::run   →   asks Root to render
     take_events()   ←  drain the queue                     ←
     set_int(…)          write the store
     column("…", || {    push a build scope
       text(…)           push a described node
     })                  pop the scope
     …                ←  turn the described tree into elements
   })                  →  hand them back                  →   lays out and paints
```

`gpui::run` returns when the window is closed, when the program calls
`gpui::quit()`, or on ⌘Q. It answers `Result<(), String>`, which is `Err` if
something stopped the app before it started.

## The `gpui` module

### Running

| | |
|---|---|
| `gpui::run(title, width, height, view)` | Open a window and run until it closes. `view` is `\|\| { … }`. |
| `gpui::run_with(title, width, height, options, view)` | `options` is `"resizable=false titlebar=none float=true background=blur"`. |
| `gpui::quit()` | Close the window and end the program. |
| `gpui::frame()` | Frames painted so far. |
| `gpui::now()` | Seconds since the program started, as a float. |

### Making things happen

| | |
|---|---|
| `gpui::animate()` | Ask for another frame as soon as this one is on screen. Call it while you want a frame loop; stop calling it to park. |
| `gpui::tick(id, ms)` | Raise the event `id` every `ms` milliseconds. Installed once, then left running. |
| `gpui::task(id, ms)` | Do slow work off the UI thread for `ms`, then raise `id` and leave a description under `"<id>.result"`. |
| `gpui::on_keys(true)` | Route keystrokes to the program as `"key\|<name>"` events. |

### Events

```gossamer
for event in gpui::take_events() {
    let id, data = gpui::event_parts(event)
    if id == "inc" { … }
}
```

`take_events` drains everything raised since the previous frame. An entry is
`"id"` or `"id|payload"`; `event_parts` splits it, and `event_id` takes just the
first half (it consumes the event, so `event_parts` is the one to reach for when
you want both).

Built-in events:

| source | event | payload |
|---|---|---|
| `gpui::button(id, …)` | `id` | — |
| `gpui::checkbox(id, …)` | `id` | `"on"` / `"off"` — the flag under `id` is already toggled |
| `gpui::input(key, …)` | `key` | `"submit"`, `"cancel"`, `"focus"` |
| `gpui::slider(key, …)` | `key` | the new value, as text like `"0.420"` |
| `gpui::tick(id, …)` | `id` | — |
| `gpui::task(id, …)` | `id` | — |
| `gpui::on_keys(true)` | `"key"` | `"up"`, `"space"`, `"a"`, … |

### State

State lives in the binding, keyed by string, and is shared by the whole
program. That sidesteps the one capture rule that would otherwise bite here: a
Gossamer closure captures a scalar by value, so `count += 1` inside a view
callback would not persist — but writing to a store shared through a handle
does.

| | |
|---|---|
| `gpui::state_int(key, default)` / `set_int(key, value)` | |
| `gpui::state_float(key, default)` / `set_float(key, value)` | |
| `gpui::state_bool(key, default)` / `set_bool(key, value)` | |
| `gpui::state_text(key, default)` / `set_text(key, value)` | |
| `gpui::forget(key)` | drop one key, whatever type it holds |
| `gpui::list_push(key, value)` | append |
| `gpui::list_len(key)` | |
| `gpui::list_get(key, index)` | `""` when out of range |
| `gpui::list_set(key, index, value)` | |
| `gpui::list_insert(key, index, value)` | |
| `gpui::list_remove(key, index)` | |
| `gpui::list_all(key)` / `list_replace(key, values)` | |
| `gpui::list_clear(key)` | |

A widget that changes state calls back into GPUI and GPUI schedules the next
frame. Calling `set_int` from inside the view needs no notification at all: the
tree you are about to build already reads the new value.

### Layout

Each of these takes a style string and a `|| { … }` that describes its
children.

| | |
|---|---|
| `gpui::column(style, build)` | a flex column |
| `gpui::row(style, build)` | a flex row |
| `gpui::panel(style, build)` | a box with no direction of its own |
| `gpui::stack(style, build)` | children stack; place them with `absolute top=… left=…` |
| `gpui::scroll(style, build)` | a vertically scrolling box |
| `gpui::list(id, count, row_height, style, build)` | a virtualised list; `build` is called with the row index, for visible rows only |

### Leaves

| | |
|---|---|
| `gpui::text(content, style)` | |
| `gpui::button(id, label, style)` | |
| `gpui::checkbox(id, label, checked, style)` | |
| `gpui::input(key, placeholder, style)` | single line; the value lives in state under `key` |
| `gpui::slider(key, min, max, style)` | the value lives in state under `key`, as a float |
| `gpui::progress(value, style)` | `0.0 ..= 1.0` |
| `gpui::image(path, style)` / `gpui::icon(path, style)` | a raster image / an SVG tinted by `color=` |
| `gpui::rect(style)` | an empty styled box |
| `gpui::divider(style)` / `gpui::spacer(size)` | |
| `gpui::draw(painter, phase, style)` | a directly painted surface: `"bars"`, `"wave"` or `"stripes"` |

### Theme

`gpui::set_theme(name)` and `gpui::theme()` just record and read a name — the
palette itself is ordinary data the program computes per frame, which is what
`examples/09_theme.gos` does.

## The style language

A style is a whitespace-separated list of `key=value` tokens, each mapping
straight onto one of GPUI's Tailwind-shaped `Styled` helpers.

```gossamer
gpui::row("flex gap=10 p=16 bg=#181825 rounded=14 border=1 bordercolor=#313244", || { … })
```

A token the binding does not recognise is reported on stderr and drawn as a
banner in the window, once, rather than being ignored.

**Display and direction** — `flex`, `col`, `row`, `wrap`, `nowrap`, `grid`,
`block`, `hidden`, `visible`, `absolute`, `relative`, `flex=1|auto|initial|none`,
`grow`, `shrink`.

**Spacing** — `gap=`, `gapx=`, `gapy=`, `p=`, `px=`, `py=`, `pt=`, `pb=`, `pl=`,
`pr=`, `m=`, `mx=`, `my=`, `mt=`, `mb=`, `ml=`, `mr=`, `auto` (horizontal auto
margins). All take pixels.

**Size** — `w=`, `h=`, `size=` take a number (pixels), `full`, `auto`, or a
percentage like `50%`. Also `minw=`, `maxw=`, `minh=`, `maxh=`, `basis=`.

**Position** — `top=`, `right=`, `bottom=`, `left=`, `inset=` take pixels or a
percentage.

**Alignment** — `justify=start|center|end|between|around` along the main axis;
`items=start|center|end|baseline` across it; `content=` for a wrapped
container; `align=left|center|right` for the text inside.

**Paint** — `bg=` and `color=` take `#rgb`, `#rrggbb`, `#rrggbbaa`, or one of
`white black red green blue yellow orange cyan pink purple transparent`.
`grad=[angle,]from,to` is a linear gradient. `opacity=0..1`.

**Border and decoration** — `border=`, `bordercolor=`, `dashed`,
`rounded=N|sm|md|lg|xl|full|none`, `shadow=xs|sm|md|lg|xl|2xl|none`.

**Type** — `font=xs|sm|base|lg|xl|2xl|3xl`, `fs=` in pixels,
`weight=100..900|thin|light|medium|semibold|bold|black`, `bold`, `semibold`,
`italic`, `underline`, `strike`, `lh=` (line height in rem), `truncate`.

**Behaviour** — `cursor=pointer|text|move|grab|cross|default`,
`overflow=hidden|x|y|scroll`, `debug`.

**Interaction** — `hover=` and `active=` add a refinement applied while the
pointer is over the element or the button is down. A refinement is itself a
style, with `_` standing in for the spaces:

```gossamer
gpui::button("inc", "+1", "bg=#89b4fa color=#11111b hover=bg:#a6e3fa active=bg:#74c7ec_opacity:0.9")
```

## The examples

**01 — hello.** The smallest complete program: one window, one callback, one
column. Shows the shape every other example keeps.

**02 — styling.** A swatch book: the fourteen Catppuccin hues as gradient-ready
boxes, four gradient and shadow treatments, eight corners and borders, and the
whole type scale from `xs` to `3xl` with weights, italics and decorations.

**03 — layout.** A header/body/footer column with `flex=1` in the middle; every
`justify` and `items` value side by side; `gap=4` against `gap=12`; nested
padding; three flex weights sharing a row; a wrapped row of chips;
absolutely-positioned children in a `stack`.

**04 — interaction.** Buttons that raise events, `hover=` and `active=`
refinements, a slider writing straight into state, a checkbox showing and
hiding a panel, and `gpui::on_keys` wiring ↑ ↓ r space to the same actions.
Ends with a live "last event" readout.

**05 — forms.** Two text fields with focus and a caret, Return to submit,
Escape to cancel, live-derived text from what you have typed, and an editable
list you add to and delete from.

**06 — lists.** A 20,000-row `uniform_list`. Only visible rows are built, and
they are reused as you scroll; clicking one selects it. Compare the frame
counter in the header with how little work each frame does.

**07 — animation.** `gpui::animate()` drives a real frame loop. Two
custom-painted canvases (bars via `paint_quad`, a curve via `paint_path`), a
scrolling stripe pattern, boxes moved by absolute positioning every frame, and
two progress bars on the clock — with a checkbox that parks the loop.

**08 — async.** `gpui::tick` runs a clock and feeds an event log;
`gpui::task` runs 1.8 seconds of work on GPUI's background executor while the
window keeps painting, then drops its result into state. Includes a spinner
that only animates while the job is in flight.

**09 — theme.** One view, two palettes. Cards, a gradient hero panel, four
shadow depths, a settings list and a slider, all interpolated from a `Palette`
value chosen per frame.

**10 — dashboard.** `run_with(…, "titlebar=none")`, a sidebar that switches
panes, a live clock and metrics on a timer, a virtualised activity list, custom
charts, animated progress, a filter field, a status bar, and keyboard shortcuts
for the panes, the theme and the filter.

## Notes and limits

- **One window, one callback.** `gpui::run` may be called once per process, and
  the app ends when its window closes.
- **The row builder runs during layout.** A `gpui::list` callback is invoked
  while GPUI is laying out, not while your view is building, so keep it to
  reads and element descriptions.
- **Fonts are whatever the machine has.** GPUI picks its own UI font; the
  examples set sizes and weights but not families.
- **A panic in a binding is caught at the boundary.** It prints to stderr and
  the frame loses that element rather than taking the program down, so watch
  the terminal while an example runs.
- **Colours are managed by the display.** A screenshot of `#181825` reads back
  as `#171721` on a P3 panel; nothing in the binding or in GPUI is off.
