#!/usr/bin/env bash
#
# pair.sh -- run one ported example and its upstream Rust twin, photograph both,
# and write the comparison into tests/evidence/pairs/.
#
#   ./scripts/pair.sh 22_text          # both sides, both captures, the diff, the record
#   ./scripts/pair.sh 22_text --commands
#   ./scripts/pair.sh --list
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
# them, and it puts the pictures in the tree. It refuses to run without
# `DSH_ALLOW_WINDOWS=1`, through `scripts/window-guard.sh`, so nothing calls it
# by accident.
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
capture () {  # capture <pgrep-pattern> <out.png> <label>
  local pattern="$1" out="$2" label="$3"
  local pid="" num="" W="" H="" X="" Y="" i

  for i in $(seq 1 "$((timeout_s * 2))"); do
    pid=$(pgrep -f "$pattern" 2>/dev/null | head -1)
    [ -n "$pid" ] && break
    sleep 0.5
  done
  if [ -z "$pid" ]; then
    echo "  $label: no process matched '$pattern' within ${timeout_s}s"
    return 1
  fi

  for i in $(seq 1 "$((timeout_s * 2))"); do
    line=$( "$here/pointer.sh" windows 2>/dev/null | awk -v p="$pid" '$1==p { print; exit }' )
    [ -n "$line" ] && break
    sleep 0.5
  done
  if [ -z "$line" ]; then
    echo "  $label: pid $pid opened no window within ${timeout_s}s"
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

  "$here/window-guard.sh" --why >&2 || {
    echo >&2
    echo "pair.sh: two windows on this machine are a gesture, not a test result." >&2
    echo "pair.sh: the owner of the machine has to ask for it:" >&2
    echo "pair.sh:     DSH_ALLOW_WINDOWS=1 ./scripts/pair.sh $name" >&2
    return 3
  }

  echo "building the upstream side (no window opens for a build)"
  "$here/../tools/upstream-examples.sh" build "$upstream_name" >/tmp/pair-upstream-build.log 2>&1 \
    || { echo "  upstream build FAILED -- see /tmp/pair-upstream-build.log"; tail -5 /tmp/pair-upstream-build.log | sed 's/^/    /'; return 1; }
  echo "  built"
  echo "building the port (no window opens for a build)"
  "$here/unlock-stale-build.sh" 2>/dev/null || true
  gos build "$port_file" >/tmp/pair-port-build.log 2>&1 \
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
  ( cd "$PWD" && timeout -k 5 "$timeout_s" gos run --main-thread "$port_file" ) \
    >/tmp/pair-port-run.log 2>&1 &
  local port_job=$!
  capture "gos-runner" "$ev.port.png" "port"
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

  local ae
  ae=$(compare -metric AE "$ev.upstream.png" "$ev.port.png" "$ev.diff.png" 2>&1)
  {
    echo "compare -metric AE  $ae"
    echo
    if [ "$ae" = "0" ]; then
      echo "The two windows are pixel-identical."
    else
      echo "$ae pixels differ. tests/evidence/pairs/$base.diff.png shows where."
    fi
  } | tee -a "$ev.capture.txt"
}

status=0
for n in "${names[@]}"; do
  run_pair "$n" || status=$?
  echo
done

pkill -x gos-runner 2>/dev/null
exit "$status"
