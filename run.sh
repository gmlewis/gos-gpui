#!/usr/bin/env bash
#
# Run one of the examples.
#
#   ./run.sh                # lists the examples
#   ./run.sh 04             # runs examples/04_gif_viewer.gos
#   ./run.sh 04_gif_viewer  # the same, by name
#   ./run.sh examples/06_grid_layout.gos
#
# `--main-thread` is not optional: GPUI drives a native event loop — AppKit on
# macOS, X11 on Linux — and both insist on the process main thread. `gos run`
# otherwise executes the program on its own `gos-vm` thread with a larger
# stack, which is the right default for ordinary programs and the wrong one
# for a windowing library.

set -euo pipefail

cd "$(dirname "$0")"

if [ $# -eq 0 ]; then
  echo "examples:"
  # Only the numbered ports are examples; examples/status.gos is scaffolding
  # they import, and `ui` is the package at src/
  for file in examples/[0-9][0-9]_*.gos; do
    printf '  %s\n' "$(basename "$file" .gos)"
  done
  echo
  echo "usage: ./run.sh <number|name|path>"
  exit 0
fi

# `--print` resolves and prints, without launching anything.
print_only=0
if [ "$1" = "--print" ] || [ "$1" = "-n" ]; then
  print_only=1
  shift
fi

# Resolves an argument to a path under examples/.
#
# The glob has to be expanded by a `for` loop's word list: bash performs no
# pathname expansion on the right-hand side of a plain assignment, so
# `file="examples/$1"*.gos` would leave the asterisk in the string.
# Only the numbered ports are examples; examples/status.gos is scaffolding the
# examples import, and `ui` is the chaining layer (src/lib.gos).
is_example() {
  case "$(basename "$1")" in
    [0-9][0-9]_*.gos) return 0 ;;
    *) return 1 ;;
  esac
}

resolve() {
  local want="$1" found=()
  if [ -f "$want" ]; then is_example "$want" && { printf '%s' "$want"; return 0; }; fi
  if [ -f "examples/$want" ]; then
    is_example "examples/$want" && { printf '%s' "examples/$want"; return 0; }
  fi
  if [ -f "examples/$want.gos" ]; then
    is_example "examples/$want.gos" && { printf '%s' "examples/$want.gos"; return 0; }
  fi
  for candidate in examples/"$want"*.gos; do
    [ -f "$candidate" ] || continue
    is_example "$candidate" && found+=("$candidate")
  done
  case "${#found[@]}" in
    1) printf '%s' "${found[0]}" ;;
    0) return 1 ;;
    *) echo "run.sh: '$want' matches more than one example:" >&2
       printf '  %s\n' "${found[@]}" >&2
       return 1 ;;
  esac
}

if ! file=$(resolve "$1"); then
  echo "run.sh: no example matches '$1'" >&2
  exit 1
fi

if [ "$print_only" = 1 ]; then
  printf '%s\n' "$file"
  exit 0
fi

# The first run against this project builds a runner with Cargo, which
# compiles GPUI and takes a while. Later runs reuse it.
exec gos run --main-thread "$file"
