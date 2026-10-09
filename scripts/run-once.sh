#!/usr/bin/env bash
#
# Clear a stale runner build lock, then run one program under a bound.
#
#   ./scripts/run-once.sh tests/gui/w3_events.gos          # 40 seconds
#   ./scripts/run-once.sh tests/gui/w3_events.gos 90
#
# This exists because a bare `gos run` is a trap: if a previous `gos` was killed
# while it was rebuilding the runner, the next one blocks on
# `~/.cache/gossamer/runners/<hash>/.gos-build.lock` for five minutes and then
# errors. Under a timeout that is indistinguishable from a program that hangs —
# `exit 124`, nothing on stdout — and it produced several false readings in this
# project's ledger before it was understood, which is why
# `scripts/unlock-stale-build.sh` exists.
#
# Always prefer this, or `scripts/tiers.sh` / `scripts/probes.sh`, over typing
# `gos run` directly.

set -uo pipefail

cd "$(dirname "$0")/.."

# Every window this project opens goes through this, and this script only ever runs a program.
"$(dirname "$0")/window-guard.sh" --why >&2 || exit $?

export GOSSAMER_ROOT="${GOSSAMER_ROOT:-/Users/glenn/src/github.com/danpozmanter/gossamer}"

if [ $# -lt 1 ]; then
  echo "usage: scripts/run-once.sh <program.gos> [seconds]" >&2
  exit 2
fi

prog="$1"
secs="${2:-40}"

[ -f "$prog" ] || { echo "run-once: no such program: $prog" >&2; exit 2; }

"$(dirname "$0")/unlock-stale-build.sh" 2>/dev/null || true

timeout -k 5 "$secs" gos run --main-thread "$prog"
status=$?

pkill -x gos-runner 2>/dev/null
exit "$status"
