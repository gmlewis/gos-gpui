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
# Called by `scripts/probes.sh` and `scripts/tiers.sh` before their
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
  # "is this pid alive" is not the question — "is this pid a `gos`" is. A recorded pid that the
  # system has since handed to something else answers `ps -p` happily, and the lock survives to
  # cost the next run five minutes. Round 96 found the container-side twin of this: see
  # `reap_locks` in `scripts/gosbox.sh`.
  if ! ps -p "$pid" -o command= 2>/dev/null | grep -q 'gos'; then
    echo "unlock: reaping stale build lock $lock (pid $pid is not a running gos)" >&2
    rm -f "$lock"
  fi
done < <(find "$cache" -name '.gos-build.lock' 2>/dev/null)

exit 0
