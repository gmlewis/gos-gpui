#!/usr/bin/env bash
#
# The window guard: the one place that decides whether this project is allowed to put a window
# on the screen.
#
#   ./scripts/window-guard.sh          # exit 0 if windows are allowed, 3 if they are not
#   ./scripts/window-guard.sh --why    # ... and one line saying so either way
#
# The owner of this machine has asked that nothing pop up on their screen: GUI probes opening
# real windows on the desktop make the laptop unusable while they work. That is not a preference
# to be weighed against evidence: it is the person who owns the machine, and it outranks every
# tier in this register.
#
# So **headless is the default**, and opening a window is the exception that has to be asked for:
#
#   DSH_ALLOW_WINDOWS=1 ./scripts/tiers.sh tests/gui/foo.gos      # the whole five tiers
#   DSH_ALLOW_WINDOWS=1 ./scripts/probes.sh                       # every GUI probe
#
# Nothing else may open a window. The scripts that do — `tiers`, `probes`, `regress`, `run-once`,
# `appearance-toggle` — all call this first and either skip the window-opening half or refuse
# outright, so nothing can quietly reintroduce the nuisance.
#
# What headless mode still allows, and it is most of the work: `gos check` (which compiles the
# binding exactly the way the toolchain does), `gos test tests` (the `#[test]` cases, which need no
# window), `gos build` (a compile and a link; running the binary is what opens a window),
# `scripts/docs.sh`, and `scripts/pair-census.sh`.

set -uo pipefail

reason="headless mode: the owner of this machine asked that nothing open a window"

# Bulk runs are a second decision, not the same one. `scripts/probes.sh` opens a window per
# probe, and `DSH_ALLOW_WINDOWS=1` is consent to one window rather than to a suite of them. So
# a suite that opens a window per program needs a flag of its own:
#
#   DSH_ALLOW_WINDOWS=1 ./scripts/tiers.sh tests/gui/foo.gos          # one program, a window or two
#   DSH_ALLOW_WINDOWS=1 DSH_ALLOW_WINDOWS_BULK=1 ./scripts/probes.sh  # one window per probe, asked for
#
# `--bulk <what>` is how a suite says which it is.

if [ "${1:-}" = "--bulk" ]; then
  what="${2:-a suite of windows}"
  if [ "${DSH_ALLOW_WINDOWS_BULK:-}" = "1" ] && [ "${DSH_ALLOW_WINDOWS:-}" = "1" ]; then
    echo "window-guard: windows ALLOWED, bulk ($what) — asked for by name"
    exit 0
  fi
  if [ "${DSH_ALLOW_WINDOWS:-}" = "1" ]; then
    echo "window-guard: REFUSED in bulk ($what) — a single window is allowed, a suite is not" >&2
    echo "window-guard: add DSH_ALLOW_WINDOWS_BULK=1 as well if the owner has asked for ($what)" >&2
    exit 3
  fi
fi

if [ "${DSH_ALLOW_WINDOWS:-}" = "1" ]; then
  [ "${1:-}" = "--why" ] && echo "window-guard: windows ALLOWED (DSH_ALLOW_WINDOWS=1)"
  exit 0
fi

if [ "${1:-}" = "--why" ]; then
  echo "window-guard: windows refused — $reason"
  echo "window-guard: set DSH_ALLOW_WINDOWS=1 on one command to override, deliberately"
fi
exit 3
