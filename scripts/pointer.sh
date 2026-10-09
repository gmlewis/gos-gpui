#!/usr/bin/env bash
#
# Post a synthetic pointer event, so a window can be driven from outside its own process.
#
#   ./scripts/pointer.sh pos
#   ./scripts/pointer.sh move <x> <y>
#   ./scripts/pointer.sh click <x> <y> [count]
#   ./scripts/pointer.sh drag <x1> <y1> <x2> <y2> [steps]
#   ./scripts/pointer.sh scroll <x> <y> <dy> [shift]
#   ./scripts/pointer.sh hscroll <x> <y> <dx>
#   ./scripts/pointer.sh windows           # pid number layer x y w h, front to back
#   ./scripts/pointer.sh frame <pid>
#   ./scripts/pointer.sh owner <x> <y>
#
# Coordinates are **screen** coordinates in the space `CGWindowListCopyWindowInfo` reports window
# frames in, which is also the space `gpui::window_field("x")` and its neighbours answer in: origin
# at the top left of the main display, y increasing downwards. A program aiming at its own widget
# adds `gpui::editor_bounds` to the window's own rectangle — and which rectangle that is matters: on
# a window with a titlebar the *content* starts `frame height − asked height` below the frame, and
# `scripts/pointer.sh frame <pid>` answers the frame, so a caller measures the difference rather than
# assuming it.
#
# # Why this exists
#
# `DW4`'s row is what a click does — a caret, a double-click's word, a triple-click's line, a drag's
# range — and none of it can be produced from inside the process. gpui's `Window::dispatch_event`
# takes a `PlatformInput` but answers a `DispatchEventResult`, which is `pub(crate)`
# (`gpui-0.2.2/src/window.rs:1273`), so a binding crate cannot call it: a `pub` function whose
# return type is private is not callable from another crate, however it is wrapped. There is no
# public injection point on `Window` at all. The operating system's event tap is the way in, and
# this is the smallest thing that posts through it.
#
# # Why it is not in any suite
#
# It moves the **real pointer**, and the click it posts lands in whatever window is under that point.
# So it is not run by `scripts/probes.sh` or by `scripts/tiers.sh`: it needs the
# Accessibility permission granted to the process that runs it, and it takes the pointer away from
# whoever is using the machine. A probe that uses it says so in its header (`// needs: scripts/pointer.sh`)
# and is skipped by name. `scripts/pointer.sh owner <x> <y>` is what a caller uses to check that its own
# window is the frontmost one under the point before posting anything at it.
#
# The helper is compiled once into `${XDG_CACHE_HOME:-$HOME/.cache}/learn-gpui/pointer/` — never into
# this tree — and rebuilt when this script's own source changes.

set -uo pipefail

cd "$(dirname "$0")/.."

# Posting input is a gesture on the machine's screen, the same class as opening a window on it, and
# the same flag that allows one allows the other. Without it this refuses rather than hijacking a
# pointer that belongs to someone who is working.
if [ "${DSH_ALLOW_WINDOWS:-}" != "1" ]; then
  echo "pointer: refusing to move the pointer — set DSH_ALLOW_WINDOWS=1 on one command to override, deliberately" >&2
  exit 3
fi

if [ $# -lt 1 ]; then
  echo "usage: scripts/pointer.sh <pos|move|click|drag|scroll|hscroll|windows|owner> …" >&2
  exit 2
fi

cache="${XDG_CACHE_HOME:-$HOME/.cache}/learn-gpui/pointer"
src="$cache/post.swift"
bin="$cache/post"

if ! command -v swiftc >/dev/null 2>&1; then
  echo "pointer: no swiftc; cannot build the event poster" >&2
  exit 2
fi

mkdir -p "$cache" || exit 3

# Recompile only when the source below has changed. The comparison is on content, not on mtime, so
# a checkout that rewrites the file with the same bytes does not rebuild.
tmp="$(mktemp "$cache/src.XXXXXX")"
cat > "$tmp" <<'SWIFT'
// Posts pointer events through the operating system's event tap.
//
// Every call is a real event at a real screen position: it moves the pointer, and a click lands in
// whatever window is under it. `scripts/pointer.sh` is the only caller, and it gates on
// `DSH_ALLOW_WINDOWS` before this runs.
import ApplicationServices
import CoreGraphics
import Foundation

func fail(_ message: String, _ code: Int32) -> Never {
    FileHandle.standardError.write("pointer: \(message)\n".data(using: .utf8)!)
    exit(code)
}

let argv = CommandLine.arguments
guard argv.count >= 2 else { fail("no subcommand", 2) }

// The one permission this needs. Asked first and reported plainly, because the failure mode without
// it is silent: `CGEvent.post` succeeds and nothing arrives.
if !AXIsProcessTrusted() {
    fail("this process does not have the Accessibility permission, so posted events go nowhere — grant it to the terminal in System Settings › Privacy & Security › Accessibility", 4)
}

func num(_ index: Int) -> Double {
    guard argv.count > index, let value = Double(argv[index]) else {
        fail("expected a number for argument \(index)", 2)
    }
    return value
}

func post(_ type: CGEventType, _ point: CGPoint, clicks: Int64 = 1) {
    guard let event = CGEvent(
        mouseEventSource: nil,
        mouseType: type,
        mouseCursorPosition: point,
        mouseButton: .left
    ) else {
        fail("could not create a \(type) event", 2)
    }
    // The running click count is what makes a second and third press a double and a triple click:
    // it is the platform's own counter, and the widget reads it back as `click_count`.
    event.setIntegerValueField(.mouseEventClickState, value: clicks)
    event.post(tap: .cghidEventTap)
}

func pause(_ microseconds: UInt32) {
    usleep(microseconds)
}

/// Every on-screen window, front to back, as `pid number layer x y w h`.
///
/// The number is the window server's own id, which is what `screencapture -l` takes — the way to
/// photograph one window without whatever is in front of it.
func windowRows() -> [(pid: Int, number: Int, layer: Int, x: Double, y: Double, w: Double, h: Double)] {
    guard let raw = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID)
        as? [[String: Any]] else {
        return []
    }
    return raw.compactMap { info in
        guard let pid = info[kCGWindowOwnerPID as String] as? Int,
              let number = info[kCGWindowNumber as String] as? Int,
              let layer = info[kCGWindowLayer as String] as? Int,
              let bounds = info[kCGWindowBounds as String] as? [String: Any],
              let x = bounds["X"] as? Double,
              let y = bounds["Y"] as? Double,
              let w = bounds["Width"] as? Double,
              let h = bounds["Height"] as? Double
        else { return nil }
        return (pid, number, layer, x, y, w, h)
    }
}

switch argv[1] {
case "pos":
    let here = CGEvent(source: nil)?.location ?? .zero
    print("\(here.x),\(here.y)")

case "move":
    let point = CGPoint(x: num(2), y: num(3))
    post(.mouseMoved, point)
    pause(60_000)

case "click":
    let point = CGPoint(x: num(2), y: num(3))
    let clicks = argv.count > 4 ? (Int64(argv[4]) ?? 1) : 1
    post(.mouseMoved, point)
    pause(60_000)
    // A double click is two presses, not one press carrying a count: the platform's counter is the
    // press sequence itself, and this is what a hand does.
    for n in 1...max(clicks, 1) {
        post(.leftMouseDown, point, clicks: n)
        pause(40_000)
        post(.leftMouseUp, point, clicks: n)
        pause(40_000)
    }

case "drag":
    let from = CGPoint(x: num(2), y: num(3))
    let to = CGPoint(x: num(4), y: num(5))
    let steps = argv.count > 6 ? max(Int(argv[6]) ?? 8, 1) : 8
    post(.mouseMoved, from)
    pause(60_000)
    post(.leftMouseDown, from)
    pause(60_000)
    for step in 1...steps {
        let t = Double(step) / Double(steps)
        let point = CGPoint(x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t)
        post(.leftMouseDragged, point, clicks: 1)
        pause(40_000)
    }
    post(.leftMouseUp, to)
    pause(60_000)

case "scroll":
    let point = CGPoint(x: num(2), y: num(3))
    let dy = Int32(num(4))
    post(.mouseMoved, point)
    pause(60_000)
    guard let event = CGEvent(
        scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: dy, wheel2: 0, wheel3: 0
    ) else { fail("could not create a scroll event", 2) }
    event.location = point
    // `shift` is the horizontal-scroll convention gpui documents — "Holding `Shift` and using the mouse
    // wheel will scroll the X axis" — but that sentence belongs to `restrict_scroll_to_axis`, which
    // defaults to false, and the swap itself is done by two backends and no more: the X11 one
    // (`platform/linux/x11/client.rs:2208`) and the Windows one (`platform/windows/events.rs:532`).
    // The macOS backend hands `scrollingDeltaX`/`scrollingDeltaY` on as they come
    // (`platform/mac/events.rs:226`) and gpui's core never reads the modifier for a scroll, so on this
    // platform the flag is a measured no-op — the gesture with it and without it are the same picture
    // to the pixel (`tests/evidence/scroll-axis.txt`). The flag stays because a caller driving a
    // container that scrolls x-only does not need it either way, and because deleting it would leave
    // the next reader to re-derive what this comment now says.
    if argv.count > 5 && argv[5] == "shift" {
        event.flags = .maskShift
    }
    event.post(tap: .cghidEventTap)
    pause(60_000)

case "hscroll":
    // The wheel's *second* axis, posted on its own. An ordinary mouse wheel does not have one and
    // neither does `scroll` above; a trackpad's sideways swipe does, and it is the axis a container
    // with `overflow=x` scrolls on. `tests/evidence/scroll-axis.txt` measured that: 1600px of this
    // drove `examples/16_scrollable.gos`'s 2000px box to the end of its clip, while the same quantity
    // of `scroll` over the same point moved it sideways too — through gpui's core, which spends a
    // vertical-only delta on an x-only container (`elements/div.rs:2427`) — and carried Shift nowhere.
    // There is no `shift` word here because on macOS there is nothing for it to do.
    let point = CGPoint(x: num(2), y: num(3))
    let dx = Int32(num(4))
    post(.mouseMoved, point)
    pause(60_000)
    guard let event = CGEvent(
        scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 2, wheel1: 0, wheel2: dx, wheel3: 0
    ) else { fail("could not create a scroll event", 2) }
    event.location = point
    event.post(tap: .cghidEventTap)
    pause(60_000)

case "windows":
    for row in windowRows() {
        print("\(row.pid) \(row.number) \(row.layer) \(Int(row.x)) \(Int(row.y)) \(Int(row.w)) \(Int(row.h))")
    }

case "frame":
    // The window server's own rectangle for a process's frontmost on-screen window, as
    // `x y w h`. This is the number a caller needs to turn a point inside its own content
    // into a screen position: `gpui::window_field` answers the same rectangle for `x`,
    // `y`, `w` and `h`, but the content rectangle `inner_*` reports can be the frame
    // instead, and the height a window asked for is what tells the two apart.
    guard argv.count > 2, let want = Int(argv[2]) else {
        fail("frame needs a pid", 2)
    }
    guard let row = windowRows().first(where: { $0.pid == want && $0.layer == 0 }) else {
        fail("no on-screen window for process \(want)", 5)
    }
    print("\(Int(row.x)) \(Int(row.y)) \(Int(row.w)) \(Int(row.h))")

case "owner":
    let x = num(2), y = num(3)
    // Front to back, so the first match is the topmost window under the point. Layer 0 is the
    // ordinary window layer; anything above it (a menu, a tooltip, a screen saver) is not ours and
    // a caller that clicked through it would be clicking at something else.
    let under = windowRows().first { row in
        row.layer == 0 && x >= row.x && x < row.x + row.w && y >= row.y && y < row.y + row.h
    }
    print(under.map { String($0.pid) } ?? "0")

default:
    fail("unknown subcommand \(argv[1])", 2)
}
SWIFT

if [ ! -f "$bin" ] || ! cmp -s "$tmp" "$src"; then
  mv "$tmp" "$src"
  if ! swiftc -O -o "$bin.new" "$src" 2>"$cache/build.log"; then
    echo "pointer: could not build the event poster:" >&2
    cat "$cache/build.log" >&2
    rm -f "$bin.new"
    exit 3
  fi
  mv "$bin.new" "$bin"
else
  rm -f "$tmp"
fi

exec "$bin" "$@"
