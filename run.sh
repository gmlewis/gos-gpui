#!/usr/bin/env bash
#
# Run one of the examples.
#
#   ./run.sh                # lists the examples
#   ./run.sh 04             # runs examples/04_interaction.gos
#   ./run.sh examples/06_lists.gos
#
# `--main-thread` is not optional: GPUI builds an AppKit application, and
# AppKit requires the process main thread. `gos run` otherwise executes the
# program on its own `gos-vm` thread with a larger stack, which is the right
# default for ordinary programs and the wrong one for a windowing library.

set -euo pipefail

cd "$(dirname "$0")"

if [ $# -eq 0 ]; then
  echo "examples:"
  for file in examples/*.gos; do
    printf '  %s\n' "$(basename "$file" .gos)"
  done
  echo
  echo "usage: ./run.sh <number|path>"
  exit 0
fi

case "$1" in
  */*|*.gos) file="$1" ;;
  *) file="examples/$1"*.gos ;;
esac

if [ ! -f "$file" ]; then
  echo "run.sh: no example matches '$1'" >&2
  exit 1
fi

# The first run against this project builds a runner with Cargo, which
# compiles GPUI and takes a while. Later runs reuse it.
exec gos run --main-thread "$file"
