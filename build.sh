#!/usr/bin/env bash
#
# Build one of the examples into a standalone native binary.
#
#   ./build.sh 04            # -> target/debug/learn-gpui
#   ./build.sh 04 --release
#   ./build.sh --print 04    # just resolve the argument
#
# NOTE: `gos build` used to miscompile this shape — the binary linked but
# aborted at startup with a misaligned-pointer panic in the runtime. That is
# fixed as of the released `gos` 0.68.0, so a native build renders
# the same pixels `./run.sh` does. The *linker* half of the problem is still
# real and is why the wrapper below is required.
#
# `gos build` links the binary itself and has no way to write `-framework …`
# (macOS) or `-l…` for the windowing libraries (Linux) into that link line, so
# a GPUI binary comes out with those symbols undefined. It does read $CC, so
# tools/cc-gpui.sh supplies them on the way past.

set -euo pipefail

cd "$(dirname "$0")"

if [ $# -eq 0 ]; then
  echo "examples:"
  # Only the numbered ports are examples; examples/status.gos and
  # examples/ui/ are the shared scaffolding they import.
  for file in examples/[0-9][0-9]_*.gos; do
    printf '  %s\n' "$(basename "$file" .gos)"
  done
  echo
  echo "usage: ./build.sh <number|name|path> [--release]"
  exit 0
fi

print_only=0
if [ "$1" = "--print" ] || [ "$1" = "-n" ]; then
  print_only=1
  shift
fi

if [ $# -eq 0 ]; then
  echo "build.sh: no example given" >&2
  exit 1
fi

# See run.sh for why the glob needs a `for` loop rather than an assignment.
# Only the numbered ports are examples; examples/status.gos is scaffolding the
# examples import, and examples/ui/ is the chaining layer.
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
    *) echo "build.sh: '$want' matches more than one example:" >&2
       printf '  %s\n' "${found[@]}" >&2
       return 1 ;;
  esac
}

if ! file=$(resolve "$1"); then
  echo "build.sh: no example matches '$1'" >&2
  exit 1
fi
shift || true

if [ "$print_only" = 1 ]; then
  printf '%s\n' "$file"
  exit 0
fi

CC="$PWD/tools/cc-gpui.sh" exec gos build "$file" "$@"
