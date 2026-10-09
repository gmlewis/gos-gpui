#!/usr/bin/env bash
#
# Run one program on every tier, and print a transcript.
#
#   ./scripts/tiers.sh tests/gui/w1_window_id.gos
#
# The tiers are the ones a capability has to pass to be `tested`:
#
#   check            the type checker, and — because it builds the binding the
#                    way the toolchain does — the binding's own compile test
#   run              the interpreter
#   run --no-jit     the interpreter with the JIT off
#   build            a native debug binary, run
#   build --release  a native release binary, run
#
# A GUI program is given `$TIER_TIMEOUT` seconds and then killed; the runner it
# spawned survives that kill and is reaped before the next tier, because one
# left alive holds the window and poisons the next run.
#
# `CC` points at `tools/cc-gpui.sh` for the two builds: `gos build` links the
# binary itself and has no way to write `-framework AppKit` into that link line.

set -uo pipefail

cd "$(dirname "$0")/.."

# A stale build lock makes every run below look like a hang; clear it first.
"$(dirname "$0")/unlock-stale-build.sh" 2>/dev/null || true

# Whether anything may open a window at all. `check` never does; the other four all do.
windows=0
"$(dirname "$0")/window-guard.sh" && windows=1

# A locked screen paints nothing: refuse rather than report every run as a hang.
if [ "$windows" = 1 ]; then
  "$(dirname "$0")/screen-awake.sh" || exit $?
fi

export GOSSAMER_ROOT="${GOSSAMER_ROOT:-/Users/glenn/src/github.com/danpozmanter/gossamer}"

# Put the binding's notes on stderr as well as in the window's banner (`gpui: noted: …`),
# so this transcript carries them and the run below can fail on them. A note is what a
# person would see; a transcript that omits it is not a transcript of the run.
export GOSSAMER_NOTED_ERRORS=1

if [ $# -lt 1 ]; then
  echo "usage: scripts/tiers.sh <program.gos> [tier ...]" >&2
  echo "tiers: check run nojit build release" >&2
  exit 2
fi

prog="$1"; shift
[ -f "$prog" ] || { echo "tiers: no such program: $prog" >&2; exit 2; }

# A probe may declare itself `// container: <why>` — the environment it was written for is the
# container's, not this one, and no tier here would be a measurement of anything. `mp1` says so:
# it reads a pointer event against a pointer driven by `xdotool` through XTEST on an X server,
# and this machine has no server and no such tool. Refused rather than run, and refused rather
# than skipped quietly, because a transcript of five tiers that all failed for the want of an X
# server is exactly the sort of noise that trains a person to ignore the suite. The five tiers
# are the container's job for this one: `./scripts/gosbox.sh tiers <prog>`.
container_only="$(sed -n '1,20p' "$prog" | sed -n 's|^// container: *||p' | head -1)"
if [ -n "$container_only" ]; then
  echo "tiers: $prog is container-only — $container_only" >&2
  echo "tiers: run it where it can be measured: ./scripts/gosbox.sh tiers $prog" >&2
  exit 3
fi

# The one program whose subject *is* the noted-error channel (`tests/gui/noted_errors.gos`) says so in
# its header with `// noted: expected`, exactly as `scripts/probes.sh` reads it. Its whole point is to make
# the binding note something, so the note is the result rather than a finding — but only for a program
# that has asked for it by name, and the notes are still printed either way.
# Without this the probe is green on all five tiers and `tiers` still exits 1, which
# reads as "the channel is broken" when the channel is doing exactly what the probe asked.
expects_noted=0
[ -n "$(sed -n '1,20p' "$prog" | sed -n 's|^// noted: *expected.*|1|p' | head -1)" ] && expects_noted=1

tiers=("$@")
[ "${#tiers[@]}" -eq 0 ] && tiers=(check run nojit build release)

timeout_s="${TIER_TIMEOUT:-30}"
log="$(mktemp -t gpui-tier.XXXXXX)"
noted="$(mktemp -t gpui-tier-noted.XXXXXX)"
name="$(basename "$prog" .gos)"

# Runs a GUI program, prints its output, and reports how it ended.
gui_tier() {
  local label="$1"; shift
  : > "$log"
  timeout -k 5 "$timeout_s" "$@" > "$log" 2>&1
  local status=$?
  # `-x` matches the process *name*, not the whole command line. `-f` is wrong here: the pattern
  # "learn-gpui" appears in the command line of anything running from this directory — including the
  # shell that invoked this script — so a plain `pkill -f learn-gpui` kills its own caller mid-run,
  # exiting 144 (128 + SIGTERM's neighbour) with its transcript truncated part-way through a tier,
  # which reads like a probe that had hung.
  pkill -x gos-runner 2>/dev/null
  pkill -x gos-gpui 2>/dev/null
  echo "=== $label (exit $status) ==="
  cat "$log"
  echo
  # Kept aside: a transcript is read once, but this decides the exit status at the end.
  grep '^gpui: noted:' "$log" | sed "s|^|$label: |" >> "$noted"
}

refused=0

for tier in "${tiers[@]}"; do
  # The interpreter tiers *are* a window; nothing else in this script is. `check` compiles,
  # and the build tiers compile too — only running the binary opens anything, so in headless
  # mode the builds still happen and are reported as built-but-not-run.
  if [ "$windows" = 0 ] && { [ "$tier" = "run" ] || [ "$tier" = "nojit" ]; }; then
    echo "=== $tier: NOT RUN (headless mode — this tier runs the program, which opens a window) ==="
    echo
    refused=$((refused + 1))
    continue
  fi
  case "$tier" in
    check)
      echo "=== check ==="
      env -u GOSSAMER_IN_RUNNER gos check "$prog" 2>&1
      echo
      ;;

    run)
      gui_tier "run (interpreter)" gos run --main-thread "$prog"
      ;;

    nojit)
      gui_tier "run --no-jit" gos run --main-thread --no-jit "$prog"
      ;;

    build)
      echo "=== build (debug) ==="
      if CC="$PWD/tools/cc-gpui.sh" gos build "$prog" 2>&1; then
        if [ "$windows" = 1 ]; then
          gui_tier "build -> ./target/debug/gos-gpui" ./target/debug/gos-gpui
        else
          echo "=== built, NOT RUN (headless mode) ==="
          echo
          refused=$((refused + 1))
        fi
      else
        echo "build FAILED"
      fi
      ;;

    release)
      echo "=== build --release ==="
      if CC="$PWD/tools/cc-gpui.sh" gos build --release "$prog" 2>&1; then
        if [ "$windows" = 1 ]; then
          gui_tier "release -> ./target/release/gos-gpui" ./target/release/gos-gpui
        else
          echo "=== built, NOT RUN (headless mode) ==="
          echo
          refused=$((refused + 1))
        fi
      else
        echo "build --release FAILED"
      fi
      ;;

    *)
      echo "tiers: unknown tier '$tier'" >&2
      exit 2
      ;;
  esac
done

rm -f "$log"

if [ "$refused" -gt 0 ]; then
  echo "tiers: $refused tier(s) not run — nothing may open a window (see scripts/window-guard.sh)" >&2
  echo "tiers: the transcript above is a compile pass, not a run" >&2
  rm -f "$noted"
  exit 3
fi

# Last, because it is the thing a ledger row must not be written over: a tier that ran
# cleanly but noted an error has drawn a banner at the person and is not a pass.
if [ -s "$noted" ]; then
  count="$(wc -l < "$noted" | tr -d ' ')"
  if [ "$expects_noted" = 1 ]; then
    echo "tiers: the binding noted $count message(s), which is this program's result rather than a finding — it exists to prove the channel:" >&2
    sed 's/^/    /' "$noted" >&2
    rm -f "$noted"
    exit 0
  fi
  echo "tiers: the binding noted $count message(s) — not a clean run:" >&2
  sed 's/^/    /' "$noted" >&2
  rm -f "$noted"
  exit 1
fi

rm -f "$noted"
