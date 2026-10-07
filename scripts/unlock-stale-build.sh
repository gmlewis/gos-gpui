#!/usr/bin/env bash
#
# Reap a stale runner build lock, before it costs five silent minutes.
#
# `gos` guards each runner workdir with
# `~/.cache/gossamer/runners/<hash>/.gos-build.lock`, a file holding the PID of
# the `gos` that made it (`gossamer-driver/src/binding_runner.rs:1339`). A lock
# whose owner is gone is meant to be reclaimed, but when it is not, the next
# `gos` polls it for five minutes and then errors — which under a
# `timeout 30` is indistinguishable from a program that never quits.
#
# Called by `scripts/regress.sh`, `scripts/probes.sh` and `scripts/tiers.sh` before their
# first run. Prints what it removed, and nothing when there is nothing to do.

set -uo pipefail

cache="${GOSSAMER_CACHE:-$HOME/.cache/gossamer}"
[ -d "$cache" ] || exit 0

while IFS= read -r lock; do
  [ -f "$lock" ] || continue
  pid="$(tr -d '[:space:]' < "$lock" 2>/dev/null)"
  case "$pid" in
    ''|*[!0-9]*) continue ;;
  esac
  if ! ps -p "$pid" >/dev/null 2>&1; then
    echo "unlock: reaping stale build lock $lock (pid $pid is gone)" >&2
    rm -f "$lock"
  fi
done < <(find "$cache" -name '.gos-build.lock' 2>/dev/null)

exit 0
