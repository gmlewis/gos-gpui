# gpui examples, written in Gossamer

[GPUI](https://gpui.rs)'s own examples, ported to
[Gossamer](https://gossamer-lang.org/) so each one can be run side by side with
its original. GPUI is Rust, so the two are joined by a `[rust-bindings]` crate
that publishes the framework behind a flat, Gossamer-shaped module called
`gpui` — and a small Gossamer module, `ui`, that puts a chainable `El` type on
top of it.

All twenty-seven are ported and runnable. See [Porting status](#porting-status) for the per-example detail,
[`PORTING.md`](PORTING.md) for the ABI decisions and the bugs the port itself
surfaced, and [`API-COVERAGE.md`](API-COVERAGE.md) for how much of gpui is
reachable from Gossamer and what the rest would take.

```
examples/01_animation.gos    …    examples/27_window.gos
```

`examples/` holds **one numbered port per upstream example**, in the same order,
so any port can be run against its original. `examples/status.gos` and
`examples/ui/` are the shared scaffolding those programs import — they are not
examples and `run.sh` will not accept them.

## Requirements

- **macOS on Apple silicon or Intel, or Linux on X11.** GPUI renders through
  Metal on macOS and through Vulkan or GLES on Linux. These examples were
  written and verified on an M2 Max; the Linux configuration is described below
  and has not been run end to end.
- **macOS:** Xcode command-line tools (`xcode-select --install`).
- **Linux:** the build and runtime packages listed under
  [On Linux](#on-linux).
- **Gosammer 0.66.0** — `gos --version`.
- **A Rust toolchain.** GPUI needs a recent stable `rustc` and a Cargo that
  understands edition 2024, so 1.85 is the floor for the binding crate.
  Building `gos` itself from source wants 1.95 — see its
  [install page](https://gossamer-lang.org/docs/install/).

On macOS you do **not** need the full Metal Toolchain component. GPUI normally
compiles its shaders at build time with `xcrun metal`, which is what
`xcodebuild -downloadComponent MetalToolchain` installs; the binding crate
turns on GPUI's `runtime_shaders` feature instead, so the shader source is
handed to Metal when the window is created. See
[`gpui-binding/Cargo.toml`](gpui-binding/Cargo.toml).

**The first run is slow.** `gos run` against a project with `[rust-bindings]`
builds a per-project runner with Cargo, and that means compiling GPUI — a few
hundred crates, several minutes. The result is cached per machine under
`$XDG_CACHE_HOME/gossamer/runners/`, so every run after the first starts
immediately. A debug-profile GPUI is unoptimised, which shows in the animation
examples.

## Run an example

```sh
./run.sh 04                       # or: ./run.sh examples/04_gif_viewer.gos
```

or directly:

```sh
gos run --main-thread examples/04_gif_viewer.gos
```

`--main-thread` is not optional, on either platform. GPUI drives a native event
loop — AppKit on macOS, X11 on Linux — and both insist on the process main
thread; `gos run` otherwise executes the program on its own `gos-vm` thread.
Gosammer has the flag precisely for native libraries that mandate the main
thread. Without it GPUI stops with `must construct App on main thread`.

The argument can be a number, a name, or a path; `--print` resolves it without
launching anything:

```sh
./run.sh                         # list the examples
./run.sh 24                      # examples/24_uniform_list.gos
./run.sh 24_uniform_list        # the same
./run.sh --print 24              # just print the path it would run
```

## Comparing against upstream GPUI

The crate ships its own Rust examples, and they are the control group: same
widgets, same GPUI, no binding and no Gossamer in the picture. When something
misbehaves here, running the upstream example for it separates three cases:

| upstream | here | so the fault is |
|---|---|---|
| works | misbehaves | the binding, or the Gosammer toolchain |
| misbehaves | misbehaves | gpui itself |
| works | works | your own change |

```sh
./tools/upstream-examples.sh list
./tools/upstream-examples.sh run hello_world      # ~ our 07_hello_world
./tools/upstream-examples.sh run uniform_list     # ~ our 24_uniform_list
./tools/upstream-examples.sh run input            # ~ our 09_input
./tools/upstream-examples.sh build painting       # compile without running
```

It generates `tools/upstream/` (gitignored), pinning the **same gpui version the
binding pins** and copying the examples unmodified out of the cargo registry.
The crate publishes no dev-dependencies, so the generated manifest supplies the
handful its examples reach for — `anyhow`, `env_logger`, `log`, `lyon`,
`rand`, `unicode-segmentation` — and sets `edition = "2024"`, which `input.rs`
needs for let-chains. `image_gallery` is skipped: it imports `reqwest_client`,
a zed-workspace crate that is not published.

**This will not reproduce the codegen bugs below.** Those abort inside
`gossamer-runtime` from Gossamer-generated code; the Rust examples never touch
that path. What upstream comparison is *for* is everything that renders wrong
or behaves oddly without crashing — a widget missing, a layout off, a list not
scrolling, a key not arriving.

## On Linux

The Gossamer side is platform-neutral — every `.gos` file, including
`examples/ui/mod.gos`, runs unchanged. What differs is all in the binding crate
and the two scripts, and it is already wired up:

- [`gpui-binding/Cargo.toml`](gpui-binding/Cargo.toml) takes gpui's **X11
  backend alone** on Linux (`default-features = false, features = ["x11"]`).
  gpui gates its Linux windowing backends independently, and its default
  features would otherwise pull the whole Wayland stack, which a Cinnamon
  desktop does not use. The `runtime_shaders` feature is macOS-only and is
  gated to match.
- The binding binds **Ctrl+Q** off macOS and ⌘Q on it.
- `titlebar=none` (example 10) becomes `window_decorations: Client` on Linux.
  The X11 backend reads only `titlebar.title`, so without that the window would
  get server-side decorations *and* example 10's own drawn titlebar.
- [`tools/cc-gpui.sh`](tools/cc-gpui.sh) branches on `uname`, adding the Apple
  frameworks on macOS and nothing on Linux — where most of GPUI's dependencies
  resolve through `dlopen` and no `-dev` packages are needed to link.

### Packages

`gos run` needs no LLVM: the [install page](https://gossamer-lang.org/docs/install/)
is explicit that only `gos build` wants LLVM 18–22. So the minimum is `gos`
0.66.0, a Rust toolchain, and GPUI's Linux build dependencies.

Of GPUI's dependency tree, only `x11`, `freetype-sys` and
`yeslogic-fontconfig-sys` shell out to `pkg-config`, and the last of those is
dlopen'd through `zed-font-kit`'s `source-fontconfig-dlopen` feature — as are
`xkbcommon`, `libxcb` and the Vulkan loader. That leaves a short list:

```sh
sudo apt-get install -y build-essential pkg-config     libx11-dev libxcb1-dev libxext-dev libxrender-dev     libxcursor-dev libxinput-dev libfreetype-dev
```

plus, at runtime, a Vulkan driver (`mesa-vulkan-drivers`, or your GPU vendor's)
and `libxkbcommon-x11-0`. Add `libfontconfig1-dev` if a build actually asks for
it. Mint ships fonts; the examples set sizes and weights but never families.

### What is not verified

The Linux path has not been run end to end — it was written from the source and
the dependency graph, on a Mac. Open questions, in rough order of risk:

1. Whether GPUI 0.2.2's Linux backend builds and runs at all. Zed develops on
   Linux, so it should; 0.2.2 is a snapshot, so it is not certain.
2. Vulkan versus GLES. `blade-graphics` ships `vulkan/`, `gles/` and `metal/`
   backends. A machine with working Mesa/Vulkan is the happy path.
3. The exact package list above, which is derived from reading build scripts
   rather than from a build. Whatever the compiler asks for wins.
4. Whether the two codegen bugs below reproduce on x86_64.
5. Your session type. Mint 22.3 Cinnamon is X11 by default, which is what this
   configuration targets; a Wayland session would need the `wayland` feature
   back.

**Do not cross-compile.** `gos build --target x86_64-unknown-linux-gnu` from
macOS needs an external sysroot and sits outside Gosammer's support contract.
Build natively on each machine from the same repo; the runner cache is
per-machine and regenerates cleanly.

## Building a native binary — currently blocked

```sh
./build.sh 07                  # a native binary, same pixels as ./run.sh 07
./build.sh 07 --release        # and optimised
```

**Both tiers work now.** `gos build` used to produce a binary that aborted
before the window appeared, with a misaligned-pointer panic inside the runtime
(`c_abi/bufio.rs`); as of `gos` 0.66.0 as rebuilt on this machine it links and
renders, and `07_hello_world` draws its swatches at the same panel geometry and
the same colours as `gos run` and as upstream — see
[Notes and limits](#notes-and-limits).

One half of the old problem was real and is still there to be solved. `gos build`
links the binary itself, and its
linker plan can only emit `-L` and `-l` — there is no manifest key for
`-framework`, and `-lAppKit` does not resolve because AppKit is a framework
rather than a `libAppKit.tbd`. Left alone the link fails with a wall of
undefined `_CFStringCreateWithBytes`, `_CGBitmapContextCreate`,
`_CGSSetWindowBackgroundBlurRadius` and friends. `gos build` does honour `$CC`,
so [`tools/cc-gpui.sh`](tools/cc-gpui.sh) is a one-line `cc` wrapper that adds
the frameworks GPUI names and then re-execs; `build.sh` sets it for you.

## Describing a window

A frame is a chain. The view callback describes the window it wants right now
and hands the finished tree over:

```gossamer
use gpui
use ui

fn main() {
    gpui::run("Counter", 420.0, 260.0, || {
        gpui::take_events() |> handle

        let count = gpui::state_int("count", 0)

        ui::column("gap=10 p=20 bg=#181825 size=full")
            .text(f"count = {count}", "font=2xl bold color=#cdd6f4")
            .child(ui::row("gap=8")
                .button("inc", "+1", "px=14 py=8 rounded=8 bg=#89b4fa hover=bg:#a6e3fa")
                .button("reset", "reset", "px=14 py=8 rounded=8 bg=#313244"))
            |> ui::root
    })
}
```

Three Gosammer features do the work here, and none of them needed anything from
the Rust side:

- **`.` chaining.** `ui::El` is a `newtype` over the binding's `i64` handle, and
  `impl El` gives it one method per leaf. `.child(..)`, `.text(..)`,
  `.button(..)` each answer the receiver, so a chain never needs a temporary —
  exactly the shape GPUI's own Rust builder has.
- **`|>` piping.** `ui::root` is a plain function, and the frame's last act is
  feeding the tree to it. `gpui::take_events() |> handle` is the same move for
  the frame's first act, and `data |> on_key` for a keystroke.
- **`map` and `children`.** A run of siblings is one step:
  `ui::row("gap=8").children(hues.map(swatch))`.

```
gpui::take_events() |> handle          the frame's events
ui::column(..).text(..).button(..)     the frame's window
    |> ui::root
```

There is nothing to redraw by hand: a widget that changes state calls back into
GPUI, and GPUI schedules the next frame.

## The `ui` module

`examples/ui/mod.gos` is the ergonomic layer. It is small and worth reading —
it is where the chaining lives.

### Containers

| | |
|---|---|
| `ui::column(style)` | a flex column |
| `ui::row(style)` | a flex row |
| `ui::panel(style)` | a box with no direction of its own |
| `ui::stack(style)` | children stack; place them with `absolute top=… left=…` |
| `ui::scroll(style)` | a vertically scrolling box |

### Leaves

| | |
|---|---|
| `ui::text(content, style)` | |
| `ui::button(id, label, style)` | |
| `ui::checkbox(id, label, checked, style)` | |
| `ui::input(key, placeholder, style)` | single line; the value lives in state under `key` |
| `ui::slider(key, min, max, style)` | the value lives in state under `key`, as a float |
| `ui::progress(value, style)` | `0.0 ..= 1.0` |
| `gpui::scrollbar(id, axis, style)` | a track with a thumb on it, for the scrolled element `id`: `"y"` or `"x"`, the track from the style's `bg=` and `w=`/`h=`, the thumb from its `thumb=` token. Pressing or dragging moves the thumb to the pointer and the offset with it; a track with nothing to scroll draws no thumb. `gpui::scrollbar_thumb(id, axis, track, min)` reads where the thumb is — as `"<start>,<length>"` — and `gpui::thumb_geometry(offset, max, track, min)` is the same arithmetic from four numbers, for a program drawing its own. |
| `ui::image(path, style)` / `ui::icon(path, style)` | a raster image / an SVG tinted by `color=`. A path is resolved against the root `gpui::asset_root(dir)` declared first — which matters because a program started from a desktop icon has a working directory its assets are not in — and falls back on the working directory when the root does not have it. `gpui::asset_path(name)` is the same resolution, for reading a file yourself. |
| `ui::rect(style)` | an empty styled box |
| `ui::divider(style)` / `ui::spacer(size)` | |
| `ui::draw(painter, phase, style)` | a directly painted surface: `"bars"`, `"wave"`, `"stripes"` or `"prism"` |
| `ui::list(id, count, row_height, style, build)` | a virtualised list; `build` takes the row index and answers that row's element |

### On any element

| | |
|---|---|
| `.child(el)` | hang one element under this one |
| `.children(els)` | hang a `Vec<El>` under this one, in order |
| `.text(..)`, `.button(..)`, `.input(..)`, `.slider(..)`, `.checkbox(..)`, `.progress(..)`, `.icon(..)`, `.image(..)`, `.rect(..)`, `.divider(..)`, `.spacer(..)`, `.draw(..)`, `.list(..)` | shorthand for `.child(ui::…)` — this is what takes the `ui::` prefix off every leaf inside a chain |

### Finishing

`ui::root(el)` names `el` as the window's root for this frame. Written as the
last link of the chain, `… |> ui::root`.

## The `gpui` module

`gpui` is the binding underneath. Everything it answers is an `i64` handle into
a per-frame arena, which is why `ui` exists; a program can also use it directly.

### Running

| | |
|---|---|
| `gpui::run(title, width, height, view)` | open a window and run until it closes; `view` is `\|\| { … }` |
| `gpui::run_with(title, width, height, options, view)` | `options` is `"resizable=false titlebar=none float=true background=blur"` |
| `gpui::root(element)` | name the frame's root |
| `gpui::quit()` | close the window and end the program |
| `gpui::frame()` / `gpui::now()` | frames painted / seconds since start |

### Making things happen

| | |
|---|---|
| `gpui::animate()` | ask for another frame as soon as this one is on screen. Call it while you want a frame loop; stop calling it to move. |
| `gpui::animation(id, ms, easing)` | start a clock that runs from 0 to 1 over `ms`; `gpui::animation_phase(id)` is where it is, which is what a painter takes — `gpui::draw("bars", gpui::animation_phase("grow"), …)`. While one runs the frames come by themselves. `gpui::animation_pause` / `_resume` / `_stop` hold it, carry it on and take it away; `gpui::animation_state(id)` is `"none"`/`"running"`/`"paused"`/`"done"`, `gpui::animation_easings()` the curves, and `gpui::easing_at(name, t)` one of them applied. |
| `gpui::tick(id, ms)` | raise the event `id` every `ms` milliseconds. Calling it again for a running id **reschedules** it; `gpui::tick_stop(id)` ends it. `gpui::tick_period(id)` answers the period, or `0`. |
| `gpui::delay(id, ms)` | raise the event `id` **once**, `ms` milliseconds from now. Ask again for an id already waiting to restart the wait; `gpui::delay_cancel(id)` takes it back, and `gpui::delay_pending(id)` says whether one is waiting. |
| `gpui::task(id, ms)` | do slow work off the UI thread for `ms`, then raise `id` and leave a summary under `"<id>.result"`. The window keeps painting while it runs. `gpui::task_result(id)` reads the summary and is `""` until the work is done; `gpui::task_running(id)` says whether it is still in flight; `gpui::task_cancel(id)` stops it — no event, no result, and only that one id. While the work runs it raises `"<id>.progress"` and `gpui::task_progress(id)` is the number those events are about, `0.0` to `1.0`, so a progress bar is `gpui::draw("bars", gpui::task_progress("id"), …)`. **What the work *is*** is a Rust-side job: no callback a program can write is ever run off the UI thread (see the ledger's `## Unreachable`). |
| `gpui::set_menus(name, items)` | install an application menu. An item is `"separator"`, `"quits:Quit"`, `"events:<label>[:<stroke>]"`, `"submenu:<label>"`, `"services:Services"` or `"os:<label>:<selector>"` — the last wired to a system selector (`cut`, `copy`, `paste`, `selectAll`, `undo`, `redo`) that the OS acts on itself. An entry indented with one `>` per level goes inside the submenu opened above it. An action raises `"menu\|<slot>"`; `gpui::menu_tree(items)` reads the nesting back, `gpui::installed_menu()` what the platform was actually given, and `gpui::installed_menu_os_actions()` the selectors among its items. |
| `gpui::set_dock_menu(items)` | the menu the dock icon offers on a right-click, in the same entry grammar. gpui has no getter for it, so it can be installed and not inspected. |
| `gpui::on_keys(true)` | route keystrokes to the program as `"key\|<stroke>"` events, and releases as `"keyup\|<stroke>"`. |
| `gpui::emit(id, payload)` | raise one event for an id you name — upstream's `cx.emit`. |
| `gpui::subscribe(topic, id)` / `unsubscribe(topic, id)` | ask the event id `id` to hear `topic`. A subscription is an *event id*, not a callback: the program handles its events once a frame, and a topic arrives in the same place as everything else. Subscribing twice is subscribing once, and a subscription belongs to a window. `gpui::subscribers(topic)` and `topics()` read the roster back. |
| `gpui::publish(topic, payload)` | raise `"<id>\|<payload>"` for every subscriber of `topic`, and answer how many heard it. A topic nobody is on raises nothing — a signal nobody is listening for is not an error. |
| `gpui::bind_keys(spec)` | bind keystrokes to the program's own event names: `"cmd-s=save; cmd-shift-z=redo"`. Each bound stroke raises `"<name>"`, in the window that asked. |
| `gpui::keymap()` | what is bound, as `"<stroke>:<name>"`, in the order it was bound. |
| `gpui::keymap_plan(spec)` | what a spec reads as, without binding it. The same parse `bind_keys` uses. |

### Events

```gossamer
fn handle(events: Vec<String>) {
    for event in events {
        let id, data = gpui::event_parts(event)
        if id == "inc" { … }
    }
}

gpui::take_events() |> handle
```

`take_events` drains everything raised since the previous frame. An entry is
`"id"` or `"id|payload"`; `event_parts` splits it, and `event_id` takes just the
first half (it consumes the event, so `event_parts` is the one to reach for when
you want both).

| source | event | payload |
|---|---|---|
| `ui::button(id, …)` | `id` | — |
| `ui::checkbox(id, …)` | `id` | `"on"` / `"off"` — the flag under `id` is already toggled |
| `ui::input(key, …)` | `key` | `"submit"`, `"cancel"`, `"focus"` |
| `ui::slider(key, …)` | `key` | the new value, as text like `"0.420"` |
| `gpui::tick(id, …)` | `id` | — |
| `gpui::delay(id, …)` | `id` | — (once, `ms` after it is asked for) |
| `gpui::task(id, …)` | `id` | — |
| `gpui::on_keys(true)` | `"key"` / `"keyup"` | the whole stroke: `"up"`, `"space"`, `"a"`, `"shift-tab"`, `"cmd-c"`. The modifiers of the last one are also under `~key.mods` and `~key.shift` / `.control` / `.alt` / `.platform` / `.function`. |
| `gpui::publish(topic, payload)` | each subscriber of `topic` | the payload |
| `gpui::bind_keys(spec)` | the name in the spec | — |

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

Calling `set_int` from inside the view needs no notification at all: the tree
you are about to build already reads the new value.

### Lines, carets and words

A program keeps its text in the store, so the arithmetic of that text is the program's to do — and
the binding can do the parts that need no window:

```gossamer
let text = gpui::state_text("doc", "")
let line = gpui::text_line(text, gpui::state_int("caret", 0))
let bound = gpui::text_line_bounds(text, gpui::state_int("caret", 0))
println("line {} of {}, spanning {}", line, gpui::text_lines(text), bound)
```

| | |
|---|---|
| `gpui::text_lines(text)` | how many lines; a text with no break is one, and so is an empty one |
| `gpui::text_line(text, offset)` | which line an offset is on, counting from zero |
| `gpui::text_line_bounds(text, offset)` | that line's byte range, as `"start,end"`, **excluding** its break |
| `gpui::text_line_move(text, offset, lines, column)` | where the caret lands `lines` lines away, aiming `column` bytes in — negative is up, so a page move is one call |
| `gpui::text_word(text, offset)` | the word at an offset, as `"start,end"` |

**Offsets are UTF-8 byte offsets**, the same as `gpui::styled_text`'s highlight ranges, and every
answer is snapped down to a character boundary and clamped into the text: an offset inside a
multi-byte character gets a whole one, and one past the end belongs at the end. A word is a run of
letters, digits and underscores — **anything else is not a word**, so a double-click on a comma takes
the comma, not the run of punctuation and spaces around it. And the **column is yours to keep**:
`text_line_move` remembers nothing, so a caret that should return to the column it left passes the
same number again.

### What the machine is

`gpui::gpu_specs()` is public in gpui, but only a window can answer it, so the binding samples it
**once**, on that window's first frame, and keeps it:

```gossamer
for name in gpui::gpu_fields() {
    println("{}: {}", name, gpui::gpu_field(name))
}
println("software rendering: {}", gpui::gpu_is_software())
```

| | |
|---|---|
| `gpui::gpu_field(name)` | `"device"`, `"driver"` or `"info"` — the names are exact, and anything else is empty |
| `gpui::gpu_fields()` | those three names, in that order |
| `gpui::gpu_is_software()` | whether the GPU is really a fake running on the CPU, `llvmpipe` and its like |

**Nothing is reported until a frame has asked.** Outside a frame — a `gos test`, or before the first
window — these are empty and `false` rather than a placeholder: a diagnostics panel prints nothing
sooner than a device name nobody looked up.

### Answers to questions

A prompt is a question with buttons, and **prompts queue**: only one is up at a time, and a second
waits its turn however many a program asks for in one frame. gpui's own dialog can only be up once
(`Window::prompt`), so the queue is what keeps that from being a program's problem.

```gossamer
gpui::prompt("Unsaved", "Save before closing?", "Save;Discard;Cancel")

// Once a frame: draw whichever prompt is up, with the ordinary element tree.
let head = gpui::prompt_current()
if head != "" {
    let buttons = gpui::prompt_buttons()
    // …ui::rect / ui::button, one per label…
    // and when one is clicked:
    //   gpui::prompt_answer(id, index)
}
```

| | |
|---|---|
| `gpui::prompt(title, message, buttons)` | queue a question; `buttons` is `";"`-separated, and answers its id, or `0` when it was refused |
| `gpui::prompt_current()` | the one whose turn it is, as `"<id>\|<title>\|<message>"`, or `""` |
| `gpui::prompt_buttons()` | its labels, in order — the index of one is what `prompt_answer` takes |
| `gpui::prompt_answer(id, button)` | answer it, and answer whether that was the one whose turn it was |
| `gpui::prompt_cancel(id)` / `prompts_clear()` | drop one, or all — cancelling is not answering |
| `gpui::prompts()` | everything waiting, in order, as `"<id>\|<title>"` |

An answer raises `"prompt\|<id>\|<button>"` in the window that asked, so a program hears it in the
same place as every other event. A prompt with no buttons is refused rather than queued: a dialog
with nothing to click is a dead end.

### Undo and redo

**The store is the document.** `ui::editor` publishes its content under its key and takes on
whatever is written there, so a history of the *store's* text is a history of the document — and
it works the same for a program with no editor, since `ui::input` keeps its value under `key` too.

```gossamer
gpui::set_text("notes", "")
gpui::history_watch("notes")     // from here, steps are recorded

// …the program or the editor writes "notes"…

if gpui::history_can_undo("notes") {
    gpui::history_undo("notes")  // writes the older text back
}
```

| | |
|---|---|
| `gpui::history_watch(key)` | start recording `key`; the text it holds now is the baseline |
| `gpui::history_unwatch(key)` | stop, dropping the history |
| `gpui::history_undo(key)` / `history_redo(key)` | step back or forward, and answer whether there was a step |
| `gpui::history_can_undo(key)` / `history_can_redo(key)` | |
| `gpui::history_depth(key)` | how many steps back it is possible to go |

A **run of typing is one step**, not one per keystroke: insertions in a row coalesce, deletions in
a row coalesce, and a change of kind — or a replacement, or a paste over a selection — starts a
new one. The rule is arithmetic on two strings with no timers in it, so it behaves the same in a
test as at a keyboard. A new edit after an undo throws the redo away.

`ui::editor` handles ⌘Z and ⇧⌘Z itself, through this same history — one writer, one stack.

### Settings that survive a restart

State lives for one run. A settings file lives for the next one, and it is
`key=value` a line at a time so you can read it, edit it and delete keys by hand:

```gossamer
let path = "/tmp/myapp/settings.txt"

// On close — or every frame; it is one small file.
gpui::remember_window_state(path, "main",
    gpui::window_field("x"), gpui::window_field("y"),
    gpui::window_field("w"), gpui::window_field("h"))

// Next run, open where it was. `gpui::run` takes no position of its own, so the
// saved one goes in the option string as `pos=x,y`.
let at = gpui::saved_bounds(path, "main")
gpui::run("My App", 900.0, 600.0, f"pos={at}", || { … })
```

| | |
|---|---|
| `gpui::settings_set(path, key, value)` / `settings_get(path, key)` | one string; writing one key leaves the others |
| `gpui::settings_forget(path, key)` | drop one key |
| `gpui::remember_window_state(path, key, x, y, w, h)` | the four numbers a window's bounds are |
| `gpui::saved_bounds(path, key)` | `"x,y,w,h"`, or `""` when nothing whole is stored |
| `gpui::saved_field(path, key, field)` | one of the four, for `gpui::open_window` |
| `gpui::save_window_state(path, key)` | the same, read from the live window instead |

A **half-written** window state is nothing: if any of the four is missing or is
not a number, `saved_bounds` is `""` and `saved_field` is `0.0`, so a window opens
where it was told to rather than at 0,0. `pos=x,y` places a window itself;
`place=center` after it wins, and `gpui::open_window` already takes x and y.

## The style language

A style is a whitespace-separated list of `key=value` tokens, each mapping
straight onto one of GPUI's Tailwind-shaped `Styled` helpers.

```gossamer
ui::row("flex gap=10 p=16 bg=#181825 rounded=14 border=1 bordercolor=#313244")
    .text("hi", "font=sm color=#cdd6f4")
```

A token the binding does not recognise is reported on stderr and drawn as a
banner in the window, once, rather than being ignored.

**Display and direction** — `flex`, `col`, `row`, `wrap`, `nowrap`, `grid`,
`block`, `hidden`, `visible`, `absolute`, `relative`,
`flex=1|auto|initial|none`, `grow`, `shrink`.

**Spacing** — `gap=`, `gapx=`, `gapy=`, `p=`, `px=`, `py=`, `pt=`, `pb=`, `pl=`,
`pr=`, `m=`, `mx=`, `my=`, `mt=`, `mb=`, `ml=`, `mr=`, `auto` (horizontal auto
margins). All take pixels.

**Size** — `w=`, `h=`, `size=` take a number (pixels), `full`, `auto`, or a
percentage like `50%`. Also `minw=`, `maxw=`, `minh=`, `maxh=`, `basis=`.

**Position** — `top=`, `right=`, `bottom=`, `left=`, `inset=` take pixels or a
percentage.

**Alignment** — `justify=start|center|end|between|around` along the main axis;
`items=start|center|end|baseline` across it; `content=` for a wrapped container;
`align=left|center|right` for the text inside.

**Paint** — `bg=` and `color=` take `#rgb`, `#rrggbb`, `#rrggbbaa`, or one of
`white black red green blue yellow orange cyan pink purple transparent`.
`grad=[angle,]from,to` is a linear gradient. `opacity=0..1`.

Anywhere a colour is taken, `theme:<name>` names the platform's colour table instead:
`bg=theme:background`, `color=theme:text`, `bordercolor=theme:border`. The eight names
are `gpui::colour_names()` — `text`, `selected_text`, `background`, `disabled`,
`selected`, `border`, `separator`, `container` — and the value follows the scheme, so the
same style string draws light on a light desktop and dark on a dark one, and follows
`gpui::set_theme` when a program pins the palette. A name the table does not have is
reported as an unknown token rather than drawn as black. `gpui::resolve_colour(spec)`
answers what a value resolves to right now; `gpui::default_color(name)` answers the
window's; `theme.<name>=` overrides one.

**Border and decoration** — `border=`, `bordercolor=`, `dashed`,
`rounded=N|sm|md|lg|xl|full|none`, `shadow=xs|sm|md|lg|xl|2xl|none`.

**Type** — `font=xs|sm|base|lg|xl|2xl|3xl`, `fs=` in pixels,
`weight=100..900|thin|light|medium|semibold|bold|black`, `bold`, `semibold`,
`italic`, `underline`, `strike`, `lh=` (line height in rem), `truncate`.

**Behaviour** — `cursor=pointer|text|move|grab|cross|default`,
`overflow=hidden|x|y|scroll`, `debug`.

**Interaction** — `hover=`, `active=` and `focus=` add a refinement applied while
the pointer is over the element, the button is down, or the element is focused. A
refinement is itself a style, with `_` standing in for the spaces:

```gossamer
.button("inc", "+1", "bg=#89b4fa color=#11111b hover=bg:#a6e3fa active=bg:#74c7ec_opacity:0.9")
```

`ctx=<predicate>` names a **key context** for the element: a keymap binding may
require one (`gpui::bind_keys("cmd-s=save@Editor")`), and gpui evaluates it against
the context stack of the focused element. Like `hover=`, a `ctx=` is what makes a
plain box part of the dispatch tree. See `gpui::bind_keys`.

## Porting status

One file per upstream example, numbered to match. Run the pair:

```sh
./run.sh 07                                  # the Gossamer port
./tools/upstream-examples.sh run hello_world # the original
```

**All 27 ported.** A faithful port unless its row says otherwise:

| | upstream | notes |
|---|---|---|
| `01_animation` | `animation.rs` | `bounce(ease_in_out)` reproduced exactly |
| `02_data_table` | `data_table.rs` | 10,000 rows; the scrollbar thumb is inert — no `ScrollHandle` |
| `03_drag_drop` | `drag_drop.rs` | **the drag protocol is emulated** — `on_drag`/`on_drop` take a `'static` payload the ABI cannot carry |
| `04_gif_viewer` | `gif_viewer.rs` | animated GIF |
| `05_gradient` | `gradient.rs` | including the sRGB/Oklab toggle |
| `06_grid_layout` | `grid_layout.rs` | the "holy grail" 5×5 grid |
| `07_hello_world` | `hello_world.rs` | |
| `08_image_loading` | `image_loading.rs` | staged arrivals via `gpui::task`, not GPUI's asset pipeline |
| `09_input` | `input.rs` | **the editor is a binding widget, not Gossamer code** — see below |
| `10_on_window_close_quit` | `on_window_close_quit.rs` | single window |
| `12_ownership_post` | `ownership_post.rs` | **approximation** — upstream's entity half (`emit`/`subscribe`) is absent, shown through the event queue instead |
| `11_opacity` | `opacity.rs` | custom box shadow field-for-field |
| `13_painting` | `painting.rs` | drag-to-draw; the Rust logo and shift-snap are omitted |
| `14_paths_bench` | `paths_bench.rs` | one path painted 2000× |
| `15_pattern` | `pattern.rs` | `pattern_slash` |
| `16_scrollable` | `scrollable.rs` | both axes |
| `17_set_menus` | `set_menus.rs` | including the *Services* system menu; the quit item is the binding's |
| `18_shadow` | `shadow.rs` | every shadow band |
| `19_tab_stop` | `tab_stop.rs` | focus order is real; **the blue focus ring is absent** — `is_focused` cannot observe a `focusable` element |
| `20_text_layout` | `text_layout.rs` | including per-range highlights |
| `21_text_wrapper` | `text_wrapper.rs` | truncation, ellipsis and clamping |
| `22_text` | `text.rs` | `default_color`; no menu bar |
| `23_tree` | `tree.rs` | renders to depth 40; upstream's 50 trips `GX0008` |
| `24_uniform_list` | `uniform_list.rs` | explicit row height |
| `25_window_positioning` | `window_positioning.rs` | nine windows per display, on the primary display |
| `26_window_shadow` | `window_shadow.rs` | upstream's client-decoration arm is **dead on macOS**; the port renders the Server arm and labels its own pointer additions |
| `27_window` | `window.rs` | multi-window; `Prompt` emulated as an in-window dialog, `Hide Application` marked unavailable |

### The one port that had to be a widget

`09_input` is the exception, and it is worth understanding why. Its editor is
implemented in Rust, in `gpui-binding/src/editor.rs`, not in Gossamer.
`EntityInputHandler`, `InputHandler` and `window.handle_input` are trait
callbacks the platform invokes *after* the binding call has returned, so they
cannot cross a call-scoped callback — a Gossamer program physically cannot
implement them. The bodies are upstream's `TextInput` reproduced (its
`EntityInputHandler` impl, the `ElementInputHandler` wiring in the canvas paint,
the shaped-line prepaint with its selection and cursor quads, the mouse
selection, the UTF-8/UTF-16 arithmetic), and Gossamer reaches them as
`ui::editor(key, placeholder, style)` and reads the text back with
`gpui::state_text(key, "")`.

Its header documents the deviations: the application keymap is the binding's, so
the editor matches keystrokes itself rather than through thirteen bound actions,
and the widget paints no chrome of its own.

### Keeping up with gpui

`gpui` publishes to crates.io as pre-1.0 versions — three of them in its first
thirteen days — so a patch bump can break a signature. `scripts/update-api`
answers whether the binding is still in step, and what a bump would cost:

```sh
./scripts/update-api            # is gpui-binding current? changes nothing
./scripts/update-api --apply    # bump the pin, keeping a backup
./scripts/update-api --check    # compile and triage the fallout
./scripts/update-api --revert   # put the backup back
```

`--check` compiles the binding and sorts the errors into what a script could fix
(a rename, an unresolved import) and what always needs a person (a changed
signature, a hand-written trait method, a struct literal with no `..Default`).
It flags the files where the second kind always lands.

The binding currently calls 338 distinct gpui items across 620 call sites, and
two real releases would have cost two version literals — the nine public changes
between 0.2.0 and 0.2.2 do not intersect anything used here. That is luck rather
than contract, which is the reason to keep the check cheap.

It deliberately does **not** generate API. A signature-driven generator reaches
about 16% of the remaining work and would be wrong about the style surface: see
[`API-COVERAGE.md`](API-COVERAGE.md).

Two things worth knowing about it, both learned the hard way while writing it.
The check must run with `GOSSAMER_IN_RUNNER` **removed** rather than emptied — the
toolchain tests it with `var_os(..).is_some()` and skips the whole binding
dispatch when it is set, so a check started from inside a running program cannot
resolve the binding's own modules. And it must be a **separate invocation** from
`--apply`, which is why those are two commands rather than one.

### Deviations that apply to every port

Knowing these keeps a difference from being mistaken for a bug:

- **The tree is rebuilt every frame.** Upstream holds an `Entity<View>` with
  retained state and re-renders it; a Gossamer frame describes the window
  again from scratch. Layout comes out the same, but anything GPUI keys by
  element id — hover, active, focus, `uniform_list` row recycling — depends on
  ids being stable frame to frame. They are deterministic here.
- **Assets come from `examples/assets/`.** Upstream loads through
  `AssetSource` relative to the crate's own `examples/`; the binding takes a
  filesystem path, so the four images the examples use are copied in.
- **One window.** `gpui::run` opens exactly one.
- **The binding owns the key bindings.** It binds the desktop quit shortcut
  (⌘Q / Ctrl+Q); an example cannot add its own.
- **`ui::list` takes an explicit row height**; upstream's `uniform_list`
  derives it from the first row laid out.
- **`gpui::run` is called with `--main-thread`**, which upstream has no
  equivalent of — it is a Gosammer requirement, not a GPUI one.

## Notes and limits

- **One window, one callback.** `gpui::run` may be called once per process, and
  the app ends when its window closes.
- **A binding method cannot take a closure.** That is why `ui::list` is a method
  taking an `Fn(i64) -> El`: it lives in Gossamer, where closures are ordinary
  parameters.
- **`.into()` needs a target.** An `El` and an `i64` are different types to the
  checker, and the target of `.into()` comes from the use site, so the
  conversions inside `ui` are written as `let handle: i64 = el.into()`.
- **A view callback that names no root shows an error card** rather than a blank
  window.
- **The row builder runs during layout.** A `list` callback is invoked while
  GPUI is laying out, not while your view is building, so keep it to reads and
  element descriptions.
- **Two codegen bugs, found while writing these.** Both are Gosammer's, and
  both show up as a call that lands on the wrong runtime symbol.

  *On the JIT tier* (`gos run`), a `for` loop that carries an element handle and
  calls a helper per iteration mis-compiles. The program aborts with
  `misaligned pointer dereference` in `c_abi/bufio.rs`. `gos run --no-jit` runs
  the same program correctly, which is how the tier was pinned down. The
  examples avoid the shape by collecting a run of siblings and hanging them off
  a container at once — `ui::row(..).children((0..n).map(build).collect())` —
  which is tidier anyway.

  *On the native tier* (`gos build`), the same shape used to abort before any
  window appeared, reproducing on a twenty-line program with no `ui` module and
  surviving `gos clean`. **That is fixed** as of `gos` 0.66.0 rebuilt on this
  machine: `./build.sh 07` links and renders, and the swatches land where the
  interpreter puts them. So `build.sh` is now a supported path alongside
  `run.sh` — with the caveat that its linker plan still needs `$CC` pointed at
  `tools/cc-gpui.sh`, which `build.sh` does.
- **Every frame is rebuilt from scratch.** GPUI is hybrid immediate/retained,
  and this binding takes the immediate half: a custom `Render` impl re-invokes
  the Gossamer callback and describes the whole window again, rather than
  holding an `Entity` with retained state. Most things are unaffected, but
  anything GPUI keys by element id — hover, active, focus, and
  `uniform_list`'s row recycling — depends on those ids being stable from
  frame to frame. Ids here are deterministic (`gpui-auto-N` in build order, or
  the id you passed), so they are stable as long as the tree shape is. An id
  reused for two different elements is the first thing to suspect if a widget
  behaves oddly.
- **Fonts are whatever the machine has.** GPUI picks its own UI font; the
  examples set sizes and weights but not families.
- **A panic in a binding is caught at the boundary.** It prints to stderr and
  the frame loses that element rather than taking the program down, so watch the
  terminal while an example runs.
- **Colours are managed by the display.** A screenshot of `#181825` reads back
  as `#171721` on a P3 panel; nothing in the binding or in GPUI is off.
