#!/usr/bin/env bash
#
# Run every GUI probe under tests/gui/, on the interpreter tier.
#
#   ./scripts/probes.sh              # every probe
#   ./scripts/probes.sh w3 w7        # only those whose name matches
#
# A probe is a self-terminating program: it drives one capability, asserts
# in-program, prints `OK <id> …` and exits 0 through `gpui::quit()`. A probe that
# leaves a window open is not a check, so every run is bounded and the runner it
# spawned is reaped afterwards — killing `gos` does not reap it, and a survivor
# holds the window and poisons the next probe.
#
# This is the fast pass. For the evidence a ledger row needs — the interpreter,
# the interpreter without the JIT, and both LLVM builds — use
# `scripts/tiers.sh <probe>`.

set -uo pipefail

cd "$(dirname "$0")/.."

# A probe opens a real window on someone's desktop, so this refuses unless asked for by name.
# The count is read from the directory rather than written down, so it cannot go stale.
probe_count=$(ls tests/gui/*.gos 2>/dev/null | wc -l | tr -d ' ')
"$(dirname "$0")/window-guard.sh" --bulk "$probe_count probes, each its own window" >&2 || {
  echo "probes: every probe opens a window, so none were run" >&2
  exit 3
}

# A stale build lock makes every run below look like a hang; clear it first.
"$(dirname "$0")/unlock-stale-build.sh" 2>/dev/null || true

# A locked screen paints nothing: refuse rather than report every run as a hang.
"$(dirname "$0")/screen-awake.sh" || exit $?

export GOSSAMER_ROOT="${GOSSAMER_ROOT:-/Users/glenn/src/github.com/danpozmanter/gossamer}"

# The binding's notes are what a person would see as the banner inside the window.
# Turning them on puts them on stderr too (`gpui: noted: …`), so this suite fails on
# them exactly as it fails on any other output. Without this an example can draw an
# empty, silent, wrong window and pass — a data table missing its rows, a scrollbar
# that never appears: the banner is the only thing that says so.
export GOSSAMER_NOTED_ERRORS=1

timeout_s="${PROBE_TIMEOUT:-30}"
log="$(mktemp -t gpui-probe.XXXXXX)"

want=("$@")
probes=()
for p in tests/gui/*.gos; do
  [ -f "$p" ] || continue
  if [ "${#want[@]}" -gt 0 ]; then
    keep=0
    for w in "${want[@]}"; do
      case "$(basename "$p")" in *"$w"*) keep=1 ;; esac
    done
    [ "$keep" = 1 ] || continue
  fi
  probes+=("$p")
done

if [ "${#probes[@]}" -eq 0 ]; then
  echo "probes: nothing matched" >&2
  exit 2
fi

fails=0
flaky=0
skipped=0

# Runs one probe, answering 0 when its transcript is a pass.
#
# `expects_noted` is 1 for a probe whose subject *is* the noted-error channel
# (`noted_errors.gos`); it declares that in its header with `// noted: expected`, and
# is the only probe allowed to be noisy in this way. Its transcript still has to be
# read by a person — which is what `scripts/tiers.sh` prints.
run_probe() {
  local p="$1" expects_noted="$2"
  : > "$log"
  timeout -k 5 "$timeout_s" gos run --main-thread "$p" > "$log" 2>&1
  local status=$?
  pkill -x gos-runner 2>/dev/null
  # Give the window and its GPU resources a moment to go before the next one.
  sleep 1

  [ "$status" -eq 0 ] || return 1
  grep -q "^OK " "$log" || return 1
  grep -qi "^FAIL \|panicked\|error\[" "$log" && return 1
  # A noted error is a failure: it is the banner, and without the notes on stderr
  # the banner is invisible to every check here.
  if [ "$expects_noted" != 1 ] && grep -q "^gpui: noted:" "$log"; then
    return 1
  fi
  return 0
}

for p in "${probes[@]}"; do
  name="$(basename "$p" .gos)"

  # A probe whose acceptance needs something this suite cannot do says so in its
  # header, with a `// needs: <what it needs>` line, and is skipped rather than counted
  # as a failure. The value names its need in a person's words — a command (`TH2` needs
  # `scripts/appearance-toggle.sh`, `DW4` needs `scripts/pointer.sh`), or a person (`CL2` needs
  # someone to click Discard on its sheet). TH2 is the case this exists for: its trigger
  # is an operating-system appearance change, which nothing in-process can cause, so a
  # plain run *must* fail (`tests/evidence/TH2.before.txt`). Reporting that as a failure
  # would be this suite crying wolf, which is worse than not running it — the same lesson
  # as the lock and the locked screen, from a third direction. A skipped probe is still a
  # check: it is run by hand, and the row it belongs to carries that transcript.
  needs="$(sed -n '1,20p' "$p" | sed -n 's|^// needs: *||p' | head -1)"
  if [ -n "$needs" ]; then
    echo "skip   $name: needs $needs — not runnable by this suite"
    skipped=$((skipped + 1))
    continue
  fi

  # The mirror of `// needs:`, for a probe this machine cannot run but the container can. It says
  # so with `// container: <why>` and is skipped here for the same reason a `// linux:` probe is
  # skipped *there* — the environment the claim was written for is not this one. `mp1` is the
  # case: it measures a pointer event against a pointer driven by `xdotool` through XTEST on an X
  # server, and this machine has neither the server, the tool, nor leave to open a window for it.
  # `scripts/gosbox.sh probes` does not read this line, so the probe still runs where it counts.
  container="$(sed -n '1,20p' "$p" | sed -n 's|^// container: *||p' | head -1)"
  if [ -n "$container" ]; then
    echo "skip   $name: container-only — $container"
    skipped=$((skipped + 1))
    continue
  fi

  # The one probe allowed to note an error: `noted_errors.gos` exists to prove the
  # channel, so a note from it is the point rather than a finding.
  noted="$(sed -n '1,20p' "$p" | sed -n 's|^// noted: *expected.*|1|p' | head -1)"
  noted="${noted:-0}"

  # Once per probe, not once per run: a probe that is killed while `gos` is
  # rebuilding the runner leaves a build lock behind, and the probe after it then
  # blocks on it for five minutes and reads as a hang. A probe blocked by a lock
  # passes the moment it is run alone.
  "$(dirname "$0")/unlock-stale-build.sh" 2>/dev/null || true

  if run_probe "$p" "$noted"; then
    echo "ok     $name: $(grep -m1 '^OK ' "$log")"
    continue
  fi

  # A failure is re-run once before it is believed, and **a pass on the retry is
  # reported as flaky rather than as success**. A long run of GUI probes can come
  # back mostly-failed and then pass unchanged, so a single timeout is not evidence
  # of a regression — but silently retrying until green would hide the one thing
  # worth knowing about it, which is that the probe is unreliable. Flaky counts as a
  # pass and is named in the summary and on its own line.
  first="$(sed -n '1p' "$log")"
  "$(dirname "$0")/unlock-stale-build.sh" 2>/dev/null || true
  if run_probe "$p" "$noted"; then
    echo "FLAKY  $name: passed on retry; the first attempt said: ${first:-no output}"
    flaky=$((flaky + 1))
  elif grep -q "^gpui: noted:" "$log"; then
    # Said on its own line: a noted error is the banner, and naming it as "failed
    # twice" would bury the one line that says what is actually wrong.
    echo "NOTED  $name: the binding noted $(grep -c '^gpui: noted:' "$log") message(s)"
    grep -m2 '^gpui: noted:' "$log" | sed 's/^/       /'
    fails=$((fails + 1))
  else
    echo "FAIL   $name: failed twice"
    sed 's/^/       /' "$log" | head -4
    fails=$((fails + 1))
  fi
done

pkill -x gos-runner 2>/dev/null
rm -f "$log"

if [ "$flaky" -gt 0 ]; then
  echo "probes: $(( ${#probes[@]} - fails - skipped ))/$(( ${#probes[@]} - skipped )) passed, $flaky flaky (passed only on a retry)"
else
  echo "probes: $(( ${#probes[@]} - fails - skipped ))/$(( ${#probes[@]} - skipped )) passed"
fi
# A skipped probe is named, not counted. Counting it as a pass would be the mirror of
# counting it as a failure: the summary has to say what the suite actually ran.
if [ "$skipped" -gt 0 ]; then
  echo "probes: $skipped skipped (see the 'skip' lines above for what each one needs)"
fi
[ "$fails" -eq 0 ] || exit 1
