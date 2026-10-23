#!/bin/sh
# A `cc` that adds the native libraries GPUI needs and `gos build` cannot spell.
#
#   CC="$PWD/tools/cc-gpui.sh" gos build examples/01_hello.gos
#
# `gos build` links the binary itself. Its linker plan emits only `-L` and `-l`,
# and the manifest has no key for linker arguments, so a GPUI binary would
# otherwise come out with the windowing and rendering symbols undefined. It
# does read `$CC`, so this wrapper supplies them on the way past.

set -e

case "$(uname -s)" in
Darwin)
  # Named by GPUI or its dependencies: AppKit for the window,
  # CoreText/CoreGraphics for glyph rasterization, Metal for the renderer.
  exec cc \
    -framework AppKit \
    -framework Foundation \
    -framework CoreFoundation \
    -framework CoreGraphics \
    -framework CoreText \
    -framework CoreVideo \
    -framework CoreMedia \
    -framework CoreAudio \
    -framework CoreImage \
    -framework CoreServices \
    -framework QuartzCore \
    -framework Metal \
    -framework MetalKit \
    -framework IOKit \
    -framework Security \
    -framework UniformTypeIdentifiers \
    -framework ApplicationServices \
    -framework Carbon \
    -framework AVFoundation \
    -framework UserNotifications \
    "$@"
  ;;
Linux)
  # Most of GPUI's Linux dependencies resolve through dlopen — fontconfig
  # (`source-fontconfig-dlopen`), xkbcommon, libxcb and the Vulkan loader among
  # them — so the default link needs nothing extra and those `-dev` packages are
  # not required to build. If the linker does report undefined symbols, name the
  # libraries here rather than editing this file:
  #
  #   GOS_LINUX_LIBS="-lX11 -lxkbcommon" ./build.sh 04
  #
  # shellcheck disable=SC2086
  exec cc ${GOS_LINUX_LIBS:-} "$@"
  ;;
*)
  exec cc "$@"
  ;;
esac
