#!/usr/bin/env bash
#
# Run one program that is waiting for the system appearance to change, provoke the
# change, and put the machine back exactly as it was found.
#
#   ./scripts/appearance-toggle.sh <command...>
#   ./scripts/appearance-toggle.sh gos run --main-thread tests/gui/th2_appearance_change.gos
#
# TH2's capability is "the window tells the program the appearance changed". Nothing
# inside a process can cause that: gpui has no public appearance setter, and
# registering an observer does not replay the current value
# (`subscription.rs:88` — `activate` is `active.set(true)` and nothing else). So the
# only way to check the row is to change the machine's appearance from outside while
# the program runs, and the only honest way to do that is to restore it afterwards
# whether the program passes, fails, or is killed.
#
# Three things make the restore exact rather than approximate:
#
#   * `AppleInterfaceStyle` is read before and written back after;
#   * `AppleInterfaceStyleSwitchesAutomatically` — the "Auto" switch in System
#     Settings, which is a *different* setting and survives a `dark mode` write — is
#     read and written back too, **including its absence**, which is the usual case;
#   * a `trap` on EXIT/INT/TERM runs the restore, so a timeout, a Ctrl-C or a probe
#     that dies still puts the machine back.
#
# The wrapper prints the before and after state and exits non-zero if they differ, so
# a failed restore is loud rather than silent.
#
# The flip happens `APPEARANCE_DELAY` seconds in (default 5), which is why the probe
# waits about ten.

set -uo pipefail

cd "$(dirname "$0")/.."

# This one both opens a window and changes the machine's appearance; it needs asking for twice.
$(dirname "$0")/window-guard.sh" --why >&2 || exit $?

if [ $# -lt 1 ]; then
  echo "usage: scripts/appearance-toggle.sh <command...>" >&2
  exit 2
fi

if ! command -v osascript >/dev/null 2>&1; then
  echo "appearance-toggle: no osascript; cannot change the system appearance" >&2
  exit 2
fi

delay="${APPEARANCE_DELAY:-5}"

# --- the state as found -----------------------------------------------------
style_before="$(defaults read -g AppleInterfaceStyle 2>/dev/null || true)"
auto_before="$(defaults read -g AppleInterfaceStyleSwitchesAutomatically 2>/dev/null || true)"
auto_present=1
[ -n "$auto_before" ] || auto_present=0

style_now() { defaults read -g AppleInterfaceStyle 2>/dev/null || true; }
auto_now() { defaults read -g AppleInterfaceStyleSwitchesAutomatically 2>/dev/null || true; }

set_dark() {
  # `$1` is `true` or `false`.
  osascript -e "tell application \"System Events\" to tell appearance preferences to set dark mode to $1" >/dev/null 2>&1
}

restore() {
  if [ "$style_before" = "Dark" ]; then set_dark true; else set_dark false; fi
  if [ "$auto_present" = 1 ]; then
    defaults write -g AppleInterfaceStyleSwitchesAutomatically -bool "$auto_before" 2>/dev/null || true
  else
    defaults delete -g AppleInterfaceStyleSwitchesAutomatically 2>/dev/null || true
  fi
}

restored=0
finish() {
  [ "$restored" = 1 ] && return
  restored=1
  restore
  local after_style after_auto
  after_style="$(style_now)"
  after_auto="$(auto_now)"
  echo
  echo "appearance: was style='${style_before:-<unset>}' autoswitch='${auto_before:-<unset>}'"
  echo "appearance: now style='${after_style:-<unset>}' autoswitch='${after_auto:-<unset>}'"
  if [ "$after_style" != "$style_before" ] || [ "$after_auto" != "$auto_before" ]; then
    echo "appearance: RESTORE FAILED — the machine is not as it was found" >&2
    exit 3
  fi
  echo "appearance: restored exactly"
}
trap finish EXIT INT TERM

# --- run the program, and flip the appearance part-way through --------------
log="$(mktemp -t gpui-appearance.XXXXXX)"
"$@" >"$log" 2>&1 &
prog=$!

sleep "$delay"

echo "appearance: flipping from '${style_before:-<unset>}' for the program to observe"
if [ "$style_before" = "Dark" ]; then set_dark false; else set_dark true; fi

wait "$prog"
status=$?

cat "$log"
rm -f "$log"
exit "$status"
