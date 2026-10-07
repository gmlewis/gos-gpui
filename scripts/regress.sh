#!/usr/bin/env bash
#
# The regression suite: every ported example, on every tier.
#
#   ./scripts/regress.sh              # check + run, the fast pass
#   ./scripts/regress.sh --build      # ... and a native debug build of each
#   ./scripts/regress.sh 07 23        # only these examples, by number
#
# The rule is strict: **any non-empty output is a failure**, not just a panic.
# Filtering for the failure you expect once hid an unknown-token banner and a
# stack-overflow error across the whole example set for hours. A healthy example
# opens its window, prints nothing at all, and is killed by the timeout; a noisy
# one is reported with the first line of what it said.
#
# The binding's own notes — what it draws as the banner inside the window — are
# part of that rule: `GOSSAMER_NOTED_ERRORS=1` puts them on stderr and a note is
# reported as `NOTED ERROR`, ahead of whatever else the example said. Without it a
# window can be empty, silent and wrong and still pass every check here.
#
# Every GUI run is followed by `pkill -x gos-runner`: `gos` spawns the generated
# runner as a child, and killing the parent does not reap it, so the next
# example inherits a window, a GPU and a cache it should not.
#
# `GOSSAMER_ROOT` is taken from the environment, falling back to the checkout
# this project was bootstrapped against — a non-interactive shell does not read
# `~/.zshrc`.

set -uo pipefail

cd "$(dirname "$0")/.."

# A stale build lock makes every run below look like a hang; clear it first.
$(dirname "$0")/unlock-stale-build.sh" 2>/dev/null || true

# Whether anything may open a window at all. The `check` tier never does; every run does.
windows=0
# Twenty-seven runs, twenty-seven windows: the bulk key, not the single one. See scripts/window-guard.
$(dirname "$0")/window-guard.sh" --bulk "27 example windows" && windows=1

# A locked screen paints nothing: refuse rather than report every run as a hang.
if [ "$windows" = 1 ]; then
  $(dirname "$0")/screen-awake.sh" || exit $?
fi

export GOSSAMER_ROOT="${GOSSAMER_ROOT:-/Users/glenn/src/github.com/danpozmanter/gossamer}"

if [ ! -d "$GOSSAMER_ROOT/crates/gossamer-binding" ]; then
  echo "regress: no Gossamer source checkout at $GOSSAMER_ROOT" >&2
  exit 2
fi

# The binding's notes are the banner a person would read inside the window. This puts
# them on stderr too (`gpui: noted: …`), so the silence rule below sees them. It is the
# one channel that reports a problem nothing else does — an empty window is silent.
export GOSSAMER_NOTED_ERRORS=1

do_build=0
wanted=()
for arg in "$@"; do
  case "$arg" in
    --build) do_build=1 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) wanted+=("$arg") ;;
  esac
done

files=()
for f in examples/[0-9][0-9]_*.gos; do
  [ -f "$f" ] || continue
  if [ "${#wanted[@]}" -gt 0 ]; then
    keep=0
    for w in "${wanted[@]}"; do
      case "$(basename "$f")" in
        "$w"|"$w"_*|"$w".gos) keep=1 ;;
      esac
    done
    [ "$keep" = 1 ] || continue
  fi
  files+=("$f")
done

if [ "${#files[@]}" -eq 0 ]; then
  echo "regress: no examples matched" >&2
  exit 2
fi

log="$(mktemp -t gpui-regress.XXXXXX)"
failures=0
checked=0
suppressed=0

# `$do_build` is the string "0" or "1", so `${do_build:+…}` is always non-empty: the
# banner claimed a build tier that `./scripts/regress.sh` (with no `--build`) never ran.
if [ "$windows" = 1 ]; then
  [ "$do_build" = 1 ] && tiers_run="check, build, run" || tiers_run="check, run"
  echo "regress: ${#files[@]} example(s), tiers $tiers_run"
else
  echo "regress: ${#files[@]} example(s), tier check ONLY — every run opens a window and is suppressed"
fi

for f in "${files[@]}"; do
  name="$(basename "$f" .gos)"

  # ---- tier: the type checker --------------------------------------------
  # `gos check` prints `check: ok (N items typed)` on success; anything else,
  # on either stream, is a failure.
  if ! out=$(env -u GOSSAMER_IN_RUNNER gos check "$f" 2>&1); then
    echo "CHECK FAILED  $name"
    printf '%s\n' "$out" | head -5 | sed 's/^/    /'
    failures=$((failures + 1))
    continue
  fi
  checked=$((checked + 1))

  # ---- tier: the interpreter, driving a window ---------------------------
  # Suppressed in headless mode: this is the half that opens 27 windows on the desktop.
  if [ "$windows" = 0 ]; then
    suppressed=$((suppressed + 1))
    continue
  fi
  : > "$log"
  timeout 8 gos run --main-thread "$f" > "$log" 2>&1
  # `timeout` kills `gos`; the runner it spawned survives it and holds the
  # window. Reap it before the next example starts.
  pkill -x gos-runner 2>/dev/null
  # A note first and on its own line: it is the binding telling the person that
  # something is wrong, and it must not read as one more noisy line of output.
  if grep -q '^gpui: noted:' "$log"; then
    echo "NOTED ERROR   $name: $(grep -m1 '^gpui: noted:' "$log")"
    failures=$((failures + 1))
  elif [ -s "$log" ]; then
    echo "NOISY         $name: $(head -1 "$log")"
    failures=$((failures + 1))
  fi
  sleep 0.3

  # ---- tier: a native build ----------------------------------------------
  if [ "$do_build" = 1 ]; then
    : > "$log"
    if ! CC="$PWD/tools/cc-gpui.sh" gos build "$f" > "$log" 2>&1; then
      echo "BUILD FAILED  $name: $(head -1 "$log")"
      failures=$((failures + 1))
      continue
    fi
    : > "$log"
    timeout 8 ./target/debug/learn-gpui > "$log" 2>&1
    pkill -x learn-gpui 2>/dev/null
    if grep -q '^gpui: noted:' "$log"; then
      echo "NOTED (build) $name: $(grep -m1 '^gpui: noted:' "$log")"
      failures=$((failures + 1))
    elif [ -s "$log" ]; then
      echo "NOISY (build) $name: $(head -1 "$log")"
      failures=$((failures + 1))
    fi
    sleep 0.3
  fi
done

pkill -x gos-runner 2>/dev/null
pkill -x learn-gpui 2>/dev/null
rm -f "$log"

if [ "$failures" -gt 0 ]; then
  echo "regress: $failures failure(s) across ${#files[@]} example(s)"
  exit 1
fi

if [ "$suppressed" -gt 0 ]; then
  # Said loudly, because the silent-run pass is this suite's strongest evidence and this
  # transcript is not it: what ran is every example's type checker and binding compile.
  echo "regress: checked $checked, NO RUNS — $suppressed windowed run(s) suppressed (headless mode)"
  echo "regress: this is a compile pass, not the silent-run pass"
  exit 0
fi

echo "regress: silent — $checked checked, no output from any run"
