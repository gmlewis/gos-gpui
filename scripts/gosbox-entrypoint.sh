#!/usr/bin/env bash
# Start the virtual display, then whatever the container was asked to run.
#
# Xvfb is the whole point of this box: a real X server with a real framebuffer
# that no one is sitting in front of.  Windows opened against it are laid out,
# drawn, hit-tested and photographed exactly as on a desk, and none of it
# reaches the machine hosting the container.
set -euo pipefail

: "${DISPLAY:=:99}"
export DISPLAY
mkdir -p /tmp/xdg
chmod 700 /tmp/xdg

if ! xdpyinfo -display "$DISPLAY" >/dev/null 2>&1; then
  Xvfb "$DISPLAY" -screen 0 1600x1200x24 -nolisten tcp -ac >/var/log/xvfb.log 2>&1 &
  for _ in $(seq 1 50); do
    xdpyinfo -display "$DISPLAY" >/dev/null 2>&1 && break
    sleep 0.1
  done
  xdpyinfo -display "$DISPLAY" >/dev/null 2>&1 || {
    echo "gosbox: Xvfb did not come up on $DISPLAY" >&2
    cat /var/log/xvfb.log >&2
    exit 1
  }
fi

# A window manager for the EWMH state the window checks read back.  It has no
# decorations worth speaking of and nobody will ever see it, which is fine:
# what matters is that maximize/minimize/placement have an answer.
if ! pgrep -x openbox >/dev/null 2>&1; then
  openbox --sm-disable >/var/log/openbox.log 2>&1 &
  sleep 0.3
fi

exec "$@"
