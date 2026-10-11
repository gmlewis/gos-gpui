#!/usr/bin/env bash
#
# The window guard: permits windows on the desktop.
#
#   ./scripts/window-guard.sh          # exit 0
#   ./scripts/window-guard.sh --why    # ... and prints a confirmation line

set -uo pipefail

if [ "${1:-}" = "--why" ]; then
  echo "window-guard: windows ALLOWED"
fi

exit 0
