#!/usr/bin/env bash
#
# Build one of the examples into a standalone native binary.
#
#   ./build.sh 04            # -> target/debug/learn-gpui
#   ./build.sh 04 --release
#
# NOTE: `gos build` currently miscompiles this shape — the binary links but
# aborts at startup with a misaligned-pointer panic in the runtime. Use
# `./run.sh` instead. This script is kept because the *linker* half of the
# problem is real and will need solving once the codegen half is fixed.
#
# `gos build` links the binary itself and has no way to write `-framework …`
# into that link line, so a GPUI binary comes out with CoreFoundation,
# CoreGraphics, AppKit and Metal symbols undefined. It does read $CC, so
# tools/cc-gpui.sh supplies the frameworks on the way past.

set -euo pipefail

cd "$(dirname "$0")"

if [ $# -eq 0 ]; then
  echo "examples:"
  for file in examples/*.gos; do
    printf '  %s\n' "$(basename "$file" .gos)"
  done
  echo
  echo "usage: ./build.sh <number|path> [--release]"
  exit 0
fi

case "$1" in
  */*|*.gos) file="$1" ;;
  *) file="examples/$1"*.gos ;;
esac
shift || true

if [ ! -f "$file" ]; then
  echo "build.sh: no example matches that" >&2
  exit 1
fi

CC="$PWD/tools/cc-gpui.sh" exec gos build "$file" "$@"
