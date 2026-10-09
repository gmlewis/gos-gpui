#!/usr/bin/env bash
#
# Refuse to run a GUI check while the screen is locked.
#
# A locked session paints nothing. A window still gets its *first* draw — the one
# that happens as it opens — and then no more frames at all, so `gpui::animate()`
# never delivers a second one and every probe that needs to reach a later frame
# reads as `exit 124` and silence. That is indistinguishable from the bug the probe
# is looking for: a locked screen through a run makes the whole tier run and probe
# suite look like a catastrophic regression when nothing is wrong at all.
#
#   ./scripts/screen-awake.sh      # exit 0 when the screen is awake, 3 when locked
#
# Callers: `scripts/tiers.sh` and `scripts/probes.sh`.
set -uo pipefail

# `lsappinfo front` names the frontmost process; while the session is locked that is
# the login window. The name is not available any other way that does not need
# Screen Recording permission, which this machine does not have.
front="$(lsappinfo info -only name "$(lsappinfo front 2>/dev/null)" 2>/dev/null || true)"
case "$front" in
  *loginwindow*)
    echo "screen-awake: the screen is locked (the front process is the login window)." >&2
    echo "screen-awake: every window gets one draw and then no frames, so every GUI" >&2
    echo "screen-awake: check would read as a hang. Unlock the screen and run again." >&2
    exit 3
    ;;
esac
exit 0
