#!/usr/bin/env bash
#
# pair.sh -- run one ported example and its upstream Rust twin, photograph both,
# and write the comparison into tests/evidence/pairs/.
#
#   ./scripts/pair.sh 22_text          # both sides, both captures, the diff, the record
#   ./scripts/pair.sh 22_text --commands
#   ./scripts/pair.sh --list
#
# Two deadlines, because the two sides do not start at the same speed. `--timeout`
# (default 45s, `PAIR_TIMEOUT`) bounds the search for each side's *process* and
# the window wait for the upstream side. `--port-timeout` (default 900s,
# `PAIR_PORT_TIMEOUT`) is what the port gets, because the port is compiled by the
# VM it runs on every time it is run and one example's compile has been measured
# at 401s before its first frame (13_painting). A port that is merely slow and a
# port that crashed are told apart by `capture`, which reports which happened.
#
# # Why this is the owner's command and not mine
#
# Goal B asks that every `examples/NN_*.gos` be visually indistinguishable from
# its twin in `tools/upstream/examples/`. The port's own window cannot be
# photographed where the port is built: the container that runs the five tiers
# has no compositor that will give up a GPUI window's pixels. What is recorded
# under `tests/evidence/` from here is *behaviour* -- window title, geometry,
# widget inventory, colours read back from the binding -- plus every deviation
# classified. The pixel half needs a machine with a real window server, and the
# only one available is this one, which the owner has asked to be left alone.
#
# So: this script is one command, it opens exactly two windows, it photographs
# them, and it puts the pictures in the tree.
#
# # What it leaves behind
#
#   tests/evidence/pairs/<name>.upstream.png    the upstream window, as captured
#   tests/evidence/pairs/<name>.port.png        the port's window, as captured
#   tests/evidence/pairs/<name>.diff.png        `compare` output, when sizes agree
#   tests/evidence/pairs/<name>.capture.txt     geometry, sizes, the AE metric, the two commands
#
# # What it does NOT do
#
# It does not resize either window to force a match. A size difference is one of
# the deviations under test; papering over it here would be the exact thing the
# rules for this project forbid. When the sizes differ the diff is skipped and
# the report says so, in those words.

set -uo pipefail

cd "$(dirname "$0")/.." || exit 2

here="$(cd "$(dirname "$0")" && pwd)"

usage () {
  sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
}

# The guard is not consulted for `--list` or `--commands`: neither opens a
# window and both are useful when the machine is meant to be left alone. It is
# consulted inside `run_pair`, on the path that actually photographs.

names=()
commands_only=0
sizes=0
timeout_s="${PAIR_TIMEOUT:-45}"
# The port's own deadline, which is deliberately not `timeout_s`. The two sides
# are not equally quick to start: the upstream example is a linked binary, and
# the port is compiled by the VM it runs on each time it runs, so its first frame
# can be minutes away (measured: 401s, 13_painting). `timeout_s` bounds the
# searches for a *process*; `port_timeout` bounds the port's compile. See the
# note in `capture`.
port_timeout="${PAIR_PORT_TIMEOUT:-900}"
settle="${PAIR_SETTLE:-3}"

while [ $# -gt 0 ]; do
  case "$1" in
    --list)
      for f in examples/[0-9][0-9]_*.gos; do
        b=$(basename "$f")
        u=$(grep -m1 -o 'tools/upstream/examples/[A-Za-z0-9_]*\.rs' "$f" | xargs -r basename | sed 's/\.rs$//')
        printf '  %-34s upstream: %s\n' "${b%.gos}" "${u:-?}"
      done
      exit 0 ;;
    --sizes) sizes=1 ;;
    --commands) commands_only=1 ;;
    --timeout) timeout_s="${2:-45}"; shift ;;
    --port-timeout) port_timeout="${2:-900}"; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "pair.sh: unknown option $1" >&2; exit 2 ;;
    *) names+=("$1") ;;
  esac
  shift
done

# --- --sizes: the second feasibility question, for all 27 --------------------
#
# "Can its window size be set to the same value the port uses?" The port always
# names its size, because `gpui::run` takes one. Upstream sometimes names one
# and sometimes leaves `window_bounds` unset, in which case GPUI hands it its
# own default and the size is not the example's to set. Both are read out of the
# sources, so this is a fact about the files and not a claim about a run.
if [ "$sizes" = 1 ]; then
  printf '%-30s %-14s %-14s %s\n' pair port upstream upstream-places-it
  printf '%-30s %-14s %-14s %s\n' ------------------------------ -------------- -------------- ------------------
  for f in examples/[0-9][0-9]_*.gos; do
    b=$(basename "$f" .gos)
    u=$(grep -m1 -o 'tools/upstream/examples/[A-Za-z0-9_]*\.rs' "$f" | xargs -r basename | sed 's/\.rs$//')
    ps=$(grep -m1 -o 'gpui::run[a-z_]*("[^"]*", *[0-9.]*, *[0-9.]*' "$f" \
         | sed 's/.*",[[:space:]]*//' | awk -F', *' '{printf "%sx%s", $1+0, $2+0}')
    if [ -z "$ps" ]; then
      # A port may name its size with a constant -- 26_window_shadow uses WINDOW.
      cw=$(grep -m1 -o 'gpui::run[a-z_]*("[^"]*", *[A-Za-z_][A-Za-z0-9_]*, *[A-Za-z_][A-Za-z0-9_]*' "$f" \
           | sed 's/.*",[[:space:]]*//' | awk -F', *' '{print $1" "$2}')
      wp=${cw%% *}; hp=${cw##* }
      if [ -n "$wp" ] && [ -n "$hp" ]; then
        vw=$(sed -n "s/^const $wp: *[A-Za-z0-9_:<>]* *= *\([0-9.]*\).*/\1/p" "$f" | head -1 | sed 's/\.$//; s/\.0$//')
        vh=$(sed -n "s/^const $hp: *[A-Za-z0-9_:<>]* *= *\([0-9.]*\).*/\1/p" "$f" | head -1 | sed 's/\.$//; s/\.0$//')
        [ -n "$vw" ] && [ -n "$vh" ] && ps="${vw}x${vh}"
      fi
    fi
    us="?"
    placed="no (GPUI's default)"
    src="tools/upstream/examples/$u.rs"
    if [ -n "$u" ] && [ -f "$src" ] && grep -q 'window_bounds' "$src"; then
      # `window_bounds` is the line that is about the window; its size is
      # sometimes inline, sometimes a `bounds` bound up to twenty lines above,
      # and sometimes named constants. Take the last `size(..)` in that
      # neighbourhood, then resolve identifiers through the file's own `const`s.
      placed="explicitly"
      grep -q 'Bounds::centered' "$src" && placed="centred"
      # `size(a, b)` is either two `px(..)` literals or two constants; both come
      # out of the same reduction. Identifiers are resolved through the file's
      # own `const NAME: Pixels = px(N);` lines.
      inner=$(grep -B22 -A4 'window_bounds' "$src" | grep -o 'size(.*' | tail -1 \
              | tr -cd '0-9A-Za-z_.,')
      inner=${inner#size}
      if [ -n "$inner" ]; then
        left=${inner%%,*}
        rest=${inner#*,}
        right=${rest%%,*}
        for side in left right; do
          v=$(eval "printf %s \"\$$side\"")
          v=${v#px}
          case "$v" in
            [0-9]*)
              # `500.0` -> `500`, `300.` -> `300`, and anything trailing is not
              # part of the number.
              v=$(printf '%s' "$v" | sed 's/[^0-9.].*$//; s/\.$//; s/\.0$//') ;;
            *)
              c=$(sed -n "s/^const $v: *[A-Za-z0-9_:<>]* *= *[a-z]*(\([0-9.]*\)).*/\1/p" "$src" | head -1)
              [ -n "$c" ] && v=$(printf '%s' "$c" | sed 's/\.$//; s/\.0$//') ;;
          esac
          eval "$side=\$v"
        done
        case "$left$right" in
          *[!0-9.]*) us="?" ;;
          *) [ -n "$left" ] && [ -n "$right" ] && us="${left}x${right}" ;;
        esac
      fi
    fi
    printf '%-30s %-14s %-14s %s\n' "$b" "${ps:-?}" "$us" "$placed"
  done
  exit 0
fi

[ ${#names[@]} -gt 0 ] || { usage >&2; exit 2; }

resolve () {  # resolve <name> -> sets port_file, upstream_name, port_title
  local want="$1"
  port_file=""
  for f in examples/*.gos; do
    b=$(basename "$f" .gos)
    case "$b" in
      "$want"|*_"$want"|"${want%.gos}"|"${want%.gos}"_*) port_file="$f"; break ;;
    esac
  done
  [ -n "$port_file" ] || { echo "pair.sh: no example matching '$want' (try --list)" >&2; return 1; }
  upstream_name=$(grep -m1 -o 'tools/upstream/examples/[A-Za-z0-9_]*\.rs' "$port_file" | xargs -r basename | sed 's/\.rs$//')
  [ -n "$upstream_name" ] || { echo "pair.sh: $port_file names no upstream file" >&2; return 1; }
  port_title=$(grep -m1 -o 'gpui::run[a-z_]*("[^"]*"' "$port_file" | sed 's/.*("//; s/"$//')
}

# --- capture one window, given a process-name pattern and an output path -----
#
# `scripts/pointer.sh windows` lists "pid number layer x y w h" front to back.
# The window we want is the one owned by the process we launched.
#
# `pgrep -f` matches a process's whole argument vector, and that is not the same
# question as *which executable this is*. Cargo links an example with
# `clang … -o …/target/debug/examples/<name>-<hash> …`, so the path of the
# example that is being *built* is an argument of the compiler — and a pair that
# latches onto the compiler then waits the whole timeout for a window from a
# process that exited when the link finished. It reports "opened no window"
# about a window that is about to appear, and that reads as a fact about the
# port's rendering rather than about the search. The same is true of the
# runner's own build, whose link line carries `-o …/target/debug/gos-runner`.
#
# So the candidates are filtered by the executable `ps` reports for them, which
# is the path of the program itself and never its arguments: the compiler's
# `comm` is `…/usr/bin/clang`, the example's is its own path. Without the
# filter the upstream side of a pair fails whenever cargo decides to relink —
# which, because the build step just touched it, is most of the time.
#
# The port needs a second distinction, and it is the one that made this function
# hang for the whole timeout. `gos`'s runner is ONE executable that both builds
# and runs:
#
#     …/runner/target/debug/gos-runner build examples/13_painting.gos
#     …/runner/target/debug/gos-runner run --main-thread examples/13_painting.gos
#
# Both match `pgrep -f gos-runner`, and both pass the `comm` filter — the `comm`
# of the build is the runner's own path, exactly as the run's is. `gos run`
# builds before it runs, so the build half is very often the *older* of the two
# pids, which is what `head -1` picks; the build ends without ever opening a
# window, and the capture then waits `${timeout_s}s` for a window from a process
# that has already exited. That reads as "the port opened no window", which is a
# statement about the port's rendering rather than about the search. So the
# pattern for a port names the runner's **subcommand** as well, and the filter
# checks the executable against the pattern's first word and the whole argument
# vector against the whole pattern — the executable test that keeps the compiler
# out, and the argv test that keeps the build half out.
capture () {  # capture <pgrep-pattern> <out.png> <label> [window-timeout-s]
  local pattern="$1" out="$2" label="$3" window_timeout="${4:-$timeout_s}"
  local pid="" num="" W="" H="" X="" Y="" i p exe elapsed

  exe="${pattern%% *}"
  for i in $(seq 1 "$((timeout_s * 2))"); do
    # (`grep -qF` rather than a `case` here: bash 3.2, which is what macOS
    # ships, refuses to parse a `case` inside a command substitution that is
    # piped into something else. Same test, one bash older.)
    pid=$(pgrep -f "$pattern" 2>/dev/null | while IFS= read -r p; do
            ps -p "$p" -o comm= 2>/dev/null | grep -F -- "$exe" >/dev/null || continue
            ps -p "$p" -o args= 2>/dev/null | grep -F -- "$pattern" >/dev/null && printf '%s\n' "$p"
          done | head -1)
    [ -n "$pid" ] && break
    sleep 0.5
  done
  if [ -z "$pid" ]; then
    echo "  $label: no process whose executable matches '$pattern' within ${timeout_s}s"
    return 1
  fi

  # Wait for the window — but bounded by the caller's own generosity rather than
  # by the 45-second default, because the two sides are not equally quick to
  # start. The port is *compiled by the VM it runs on* every time it is run
  # (`gossamer_cli::cmd::run::run_on_vm` -> `Vm::load` -> the MIR passes), and on
  # this machine one example's compile has been measured at 401s before its first
  # frame — 13_painting, whose `move_unique_clones` pass dominates. A fixed 45s
  # wait does not report that the port is slow; it reports that the port opened no
  # window, which is a claim about the port's rendering and is false. So the
  # deadline is a parameter and the port is given `port_timeout`.
  #
  # What the wait must still not do is sit out the whole deadline for a process
  # that has already *exited* — a crashed program and a slow one are different
  # facts and the two messages say which happened.
  elapsed=0
  for i in $(seq 1 "$((window_timeout * 2))"); do
    line=$( "$here/pointer.sh" windows 2>/dev/null | awk -v p="$pid" '$1==p { print; exit }' )
    [ -n "$line" ] && break
    kill -0 "$pid" 2>/dev/null || {
      echo "  $label: pid $pid exited after ${elapsed}s with no window"
      return 1
    }
    sleep 0.5
    elapsed=$(( elapsed + 1 ))
    if [ $(( i % 60 )) -eq 0 ]; then
      echo "  $label: pid $pid is still starting up — no window after $(( i / 2 ))s"
    fi
  done
  if [ -z "$line" ]; then
    echo "  $label: pid $pid opened no window within ${window_timeout}s"
    return 1
  fi

  num=$(echo "$line" | awk '{print $2}')
  X=$(echo "$line" | awk '{print $4}')
  Y=$(echo "$line" | awk '{print $5}')
  W=$(echo "$line" | awk '{print $6}')
  H=$(echo "$line" | awk '{print $7}')
  echo "  $label: pid $pid  window $num  frame ${W}x${H}+${X}+${Y}"

  sleep "$settle"
  screencapture -x -o -l"$num" "$out" 2>/dev/null || {
    echo "  $label: screencapture failed"
    return 1
  }
  [ -s "$out" ] || { echo "  $label: capture is empty"; return 1; }
  echo "  $label: wrote $out"
  # shellcheck disable=SC2034
  CAP_W="$W"; CAP_H="$H"; CAP_X="$X"; CAP_Y="$Y"
}

run_pair () {
  local name="$1"
  resolve "$name" || return 1
  # resolve sets port_file, upstream_name and port_title in this scope; do not
  # declare them local here or it would blank them.

  local base
  base=$(basename "$port_file" .gos)
  local ev="tests/evidence/pairs/$base"
  mkdir -p tests/evidence/pairs

  local up_cmd="cargo run --manifest-path tools/upstream/Cargo.toml --example $upstream_name"
  local port_cmd="gos run --main-thread $port_file"

  echo "pair: $base"
  echo "  upstream file   tools/upstream/examples/$upstream_name.rs"
  echo "  port file       $port_file"
  echo "  upstream title  (GPUI's own -- whatever $upstream_name.rs sets, often none)"
  echo "  port title      $port_title"
  echo
  echo "  THE TWO COMMANDS, exactly as they are run here:"
  echo "      ( cd \"$PWD\" && $up_cmd )"
  echo "      ( cd \"$PWD\" && $port_cmd )"
  echo

  if [ "$commands_only" = 1 ]; then
    return 0
  fi

  "$here/window-guard.sh" --why >&2 || return $?

  echo "building the upstream side (no window opens for a build)"
  "$here/../tools/upstream-examples.sh" build "$upstream_name" >/tmp/pair-upstream-build.log 2>&1 \
    || { echo "  upstream build FAILED -- see /tmp/pair-upstream-build.log"; tail -5 /tmp/pair-upstream-build.log | sed 's/^/    /'; return 1; }
  echo "  built"
  echo "building the port (no window opens for a build)"
  "$here/unlock-stale-build.sh" 2>/dev/null || true
  # `gos build` links the binary itself and cannot write `-framework …` into
  # that link line, so a GPUI binary comes out with the whole of CoreFoundation
  # undefined. It does read `$CC`, and `tools/cc-gpui.sh` supplies them on the
  # way past -- which is what `build.sh` does too. Without this the port never
  # links and the pair never gets as far as a window.
  CC="$PWD/tools/cc-gpui.sh" gos build "$port_file" >/tmp/pair-port-build.log 2>&1 \
    || { echo "  port build FAILED -- see /tmp/pair-port-build.log"; tail -5 /tmp/pair-port-build.log | sed 's/^/    /'; return 1; }
  echo "  built"
  echo

  echo "running the upstream side"
  ( cd "$PWD" && cargo run --manifest-path tools/upstream/Cargo.toml --example "$upstream_name" ) \
    >/tmp/pair-upstream-run.log 2>&1 &
  local up_job=$!
  capture "target/debug/examples/$upstream_name" "$ev.upstream.png" "upstream"
  local up_status=$?
  kill "$up_job" 2>/dev/null
  pkill -f "target/debug/examples/$upstream_name" 2>/dev/null
  sleep 1

  echo "running the port"
  # `port_timeout` and not `timeout_s`: this `timeout` is what *kills* the port,
  # and the port spends its first seconds-to-minutes compiling itself on the VM
  # before it can draw anything (see `capture`). Killing it at 45s kills a port
  # that was working. `kill`/`pkill` below are what stop it once the capture is
  # done, so a generous deadline here costs nothing on a pair that starts quickly.
  ( cd "$PWD" && timeout -k 5 "$port_timeout" gos run --main-thread "$port_file" ) \
    >/tmp/pair-port-run.log 2>&1 &
  local port_job=$!
  capture "gos-runner run" "$ev.port.png" "port" "$port_timeout"
  local port_status=$?
  kill "$port_job" 2>/dev/null
  pkill -x gos-runner 2>/dev/null
  sleep 1

  {
    echo "pair $base"
    echo "upstream  tools/upstream/examples/$upstream_name.rs"
    echo "port      $port_file"
    echo "captured  $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo
    echo "the two commands:"
    echo "    $up_cmd"
    echo "    $port_cmd"
    echo
  } > "$ev.capture.txt"

  if [ "$up_status" != 0 ] || [ "$port_status" != 0 ]; then
    echo "  a capture failed; nothing to compare" | tee -a "$ev.capture.txt"
    return 1
  fi

  local uw uh pw ph
  uw=$(identify -format '%w' "$ev.upstream.png" 2>/dev/null)
  uh=$(identify -format '%h' "$ev.upstream.png" 2>/dev/null)
  pw=$(identify -format '%w' "$ev.port.png" 2>/dev/null)
  ph=$(identify -format '%h' "$ev.port.png" 2>/dev/null)
  {
    echo "upstream capture  ${uw}x${uh}"
    echo "port capture      ${pw}x${ph}"
    echo
  } >> "$ev.capture.txt"

  if [ "$uw" != "$pw" ] || [ "$uh" != "$ph" ]; then
    {
      echo "the two captures are different sizes (${uw}x${uh} vs ${pw}x${ph})."
      echo "That is itself one of the deviations under test, and it is NOT corrected for here:"
      echo "scaling one to the other would hide the very difference this pair exists to measure."
      echo "Fix the size, then run this again, and the pixel diff will be produced."
    } | tee -a "$ev.capture.txt"
    return 0
  fi

  # Two numbers, and neither of them is a pixel count.
  #
  # `compare -metric AE` prints "A (N)". N is the normalised absolute error: the
  # total difference over the whole image as a fraction of what an image that
  # was wrong in every pixel would score. A is the same figure scaled back up by
  # the pixel count, so `0 (0)` means identical and `1.29 (1.3e-05)` means a
  # total difference equal to about one and a third wholly-wrong pixels. A
  # hundred pixels each one grey level out of 255 off score far *better* than
  # one pixel that is black where the other is white, which is the point of the
  # metric and also why reading A as "A pixels differ" is wrong twice over: it
  # flatters a near-miss and it libels a pair whose windows merely rounded
  # differently. This pair was reported as "1 pixels differ" while 409 pixels
  # were each one grey level apart.
  #
  # So the count is taken separately, and exactly: the difference thresholded at
  # zero counts every pixel that is not bit-for-bit equal, however small the
  # difference. Both go into the record, each named for what it is.
  local ae ae_norm differ mask chrome content
  ae=$(compare -metric AE "$ev.upstream.png" "$ev.port.png" "$ev.diff.png" 2>&1)
  ae_norm=$(printf '%s' "$ae" | sed 's/^[^(]*(//; s/).*$//')
  differ=$(convert "$ev.upstream.png" "$ev.port.png" -compose difference -composite \
             -channel RGB -separate -evaluate-sequence max -threshold 0 \
             -format '%[fx:int(mean*w*h+0.5)]' info: 2>/dev/null)

  # ... and the same count for the client area on its own.
  #
  # The top 32 rows of these captures are the window's title bar, which macOS
  # draws and the example does not. An unfocused window draws its traffic lights
  # and its bar tint differently from a focused one, and only one window can be
  # key: a pair photographed with the two sides in different focus states carries
  # a difference that is photography and not rendering, and it is not small --
  # 10_on_window_close_quit and 26_window_shadow measured 15,032 and 18,478
  # differing pixels, all of them in that band, with client areas that are equal
  # to the pixel. The figure is added, not substituted: the whole-window count
  # above still stands and still fails, and this says which part of it is the
  # platform's chrome. A reader who wants to know whether the example rendered
  # differently wants this number; a reader who wants to know whether two
  # photographs are equal wants the one above.
  mask=$(mktemp -t pair-mask) || mask=""
  if [ -n "$mask" ]; then
    convert "$ev.upstream.png" "$ev.port.png" -compose difference -composite \
      -colorspace gray -threshold 0 "$mask" 2>/dev/null || mask=""
  fi
  if [ -n "$mask" ] && [ -s "$mask" ]; then
    chrome=$(convert "$mask" -crop "${uw}x32+0+0" +repage \
               -format '%[fx:int(mean*w*h+0.5)]' info: 2>/dev/null)
    rm -f "$mask"
  fi
  {
    echo "compare -metric AE  $ae"
    echo "    the normalised figure:      ${ae_norm:-?}   (0 identical, 1 wrong in every pixel)"
    echo "    pixels not bit-for-bit equal: ${differ:-?}"
    if [ -n "${chrome:-}" ]; then
      content=$(awk -v t="$differ" -v c="$chrome" 'BEGIN{ printf "%d", t - c }')
      echo "        of which, in the 32px title bar macOS draws: $chrome"
      echo "        of which, in the client area the example draws: $content"
    fi
    echo
    case "${differ:-}" in
      "")
        echo "the difference could not be thresholded, so there is no count of differing"
        echo "pixels here; the metric above is all that came back from ImageMagick." ;;
      0)
        echo "The two windows are pixel-identical." ;;
      *)
        if [ -n "${chrome:-}" ] && [ "$chrome" = "$differ" ]; then
          echo "$differ pixels are not bit-for-bit equal, and every one of them is in the"
          echo "window's title bar -- which is macOS's to draw, and which is drawn for a"
          echo "focused window differently from an unfocused one. The client areas agree:"
          echo "the example rendered identically on both sides."
        else
          echo "$differ pixels are not bit-for-bit equal. tests/evidence/pairs/$base.diff.png"
          echo "shows where. The metric above says how far apart they are in total: compare it"
          echo "against the count before reading a small number as a good one -- one pixel"
          echo "black-on-white outscores a hundred pixels a grey level apart."
        fi ;;
    esac
  } | tee -a "$ev.capture.txt"
}

status=0
for n in "${names[@]}"; do
  run_pair "$n" || status=$?
  echo
done

pkill -x gos-runner 2>/dev/null
exit "$status"
