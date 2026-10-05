# gpui examples, written in Gossamer

[GPUI](https://gpui.rs)'s own examples, ported to
[Gossamer](https://gossamer-lang.org/) so each one can be run side by side with
its original. GPUI is Rust, so the two are joined by a `[rust-bindings]` crate
that publishes the framework behind a flat, Gossamer-shaped module called
`gpui` — and a small Gossamer module, `ui`, that puts a chainable `El` type on
top of it.

Ten of the twenty-seven are ported and runnable; the other seventeen open a
window naming exactly which GPUI API each is waiting on. See
[Porting status](#porting-status).

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
./run.sh 04                       # or: ./run.sh examples/04_interaction.gos
```

or directly:

```sh
gos run --main-thread examples/04_interaction.gos
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
./tools/upstream-examples.sh run hello_world      # ~ our 01_hello
./tools/upstream-examples.sh run uniform_list     # ~ our 06_lists
./tools/upstream-examples.sh run input            # ~ our 05_forms
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
./build.sh 07 --release        # links, but the binary aborts at startup
```

**Use `run.sh` for now.** `gos build` produces a binary that aborts before the
window appears, with a misaligned-pointer panic inside the runtime
(`c_abi/bufio.rs`). It does so for every shape tried, including a program that
uses no part of the `ui` module — see [Notes and limits](#notes-and-limits).
Nothing about it is specific to these examples; `gos run` is the working tier.

The linker half of the problem is real and separate, and is worth keeping for
when the codegen half is fixed. `gos build` links the binary itself, and its
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
| `ui::image(path, style)` / `ui::icon(path, style)` | a raster image / an SVG tinted by `color=` |
| `ui::rect(style)` | an empty styled box |
| `ui::divider(style)` / `ui::spacer(size)` | |
| `ui::draw(painter, phase, style)` | a directly painted surface: `"bars"`, `"wave"` or `"stripes"` |
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
| `gpui::animate()` | ask for another frame as soon as this one is on screen. Call it while you want a frame loop; stop calling it to park. |
| `gpui::tick(id, ms)` | raise the event `id` every `ms` milliseconds. Installed once, then left running. |
| `gpui::task(id, ms)` | do slow work off the UI thread for `ms`, then raise `id` and leave a description under `"<id>.result"`. |
| `gpui::on_keys(true)` | route keystrokes to the program as `"key\|<name>"` events. |

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

Calling `set_int` from inside the view needs no notification at all: the tree
you are about to build already reads the new value.

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
.button("inc", "+1", "bg=#89b4fa color=#11111b hover=bg:#a6e3fa active=bg:#74c7ec_opacity:0.9")
```

## Porting status

One file per upstream example, numbered to match. Run the pair:

```sh
./run.sh 07                                  # the Gossamer port
./tools/upstream-examples.sh run hello_world # the original
```

**Ported** — a faithful port, runnable today:

| | upstream | notes |
|---|---|---|
| `01_animation` | `animation.rs` | `bounce(ease_in_out)` reproduced exactly; frames driven by `gpui::animate()` rather than GPUI's animation driver |
| `05_gradient` | `gradient.rs` | including the sRGB/Oklab toggle; the canvas trapezoid is the `prism` painter |
| `06_grid_layout` | `grid_layout.rs` | the "holy grail" 5×5 grid |
| `07_hello_world` | `hello_world.rs` | |
| `10_on_window_close_quit` | `on_window_close_quit.rs` | single window — see deviations |
| `11_opacity` | `opacity.rs` | custom box shadow approximated by `shadow=lg` |
| `15_pattern` | `pattern.rs` | `pattern_slash` as the `slash=` token |
| `16_scrollable` | `scrollable.rs` | both axes; `overflow=scroll` is the two-way case |
| `23_tree` | `tree.rs` | reads `GPUI_TREE_DEPTH`, like upstream |
| `24_uniform_list` | `uniform_list.rs` | row height is explicit here, derived upstream |

**Not ported yet** — each file opens a window naming the GPUI APIs its upstream
example uses and the binding does not expose. That list *is* the work queue:

| | upstream | what it waits on |
|---|---|---|
| `02_data_table` | `data_table.rs` | `cx.processor`, `window.on_mouse_event` |
| `03_drag_drop` | `drag_drop.rs` | `on_drag` / `on_drop` / drag previews |
| `04_gif_viewer` | `gif_viewer.rs` | `with_assets` + an `AssetSource` |
| `08_image_loading` | `image_loading.rs` | background image decoding, an image cache |
| `09_input` | `input.rs` | `EntityInputHandler`, clipboard, IME, selection |
| `12_ownership_post` | `ownership_post.rs` | `cx.emit` / `cx.subscribe` — the entity graph |
| `13_painting` | `painting.rs` | arbitrary `PathBuilder` paths from Gossamer |
| `14_paths_bench` | `paths_bench.rs` | a custom `Element` impl |
| `17_set_menus` | `set_menus.rs` | `cx.set_menus` |
| `18_shadow` | `shadow.rs` | a custom `BoxShadow` (offset, blur, spread) |
| `19_tab_stop` | `tab_stop.rs` | `tab_stop` / `window.focus_next` / `focus_prev` |
| `20_text_layout` | `text_layout.rs` | `window.text_system()`, `ShapedLine`, `paint_text` |
| `21_text_wrapper` | `text_wrapper.rs` | `TextLayout` wrapping |
| `22_text` | `text.rs` | `cx.text_context`, `rem_size`, text runs |
| `25_window_positioning` | `window_positioning.rs` | `cx.displays()`, live window bounds |
| `26_window_shadow` | `window_shadow.rs` | hitboxes, mouse position, client inset |
| `27_window` | `window.rs` | multi-window, popups, prompts, menus |

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

  *On the native tier* (`gos build`), a binary built from a program that only
  creates elements through the binding and hands the frame its root aborts the
  same way, before any window appears. It reproduces on a twenty-line program
  with no `ui` module and no element chaining, and survives `gos clean` and a
  full rebuild, so it is not a stale artefact. This is why `run.sh` is the
  supported path and `build.sh` is not.
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
