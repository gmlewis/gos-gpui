#!/bin/sh
# A `cc` that adds the Apple frameworks GPUI needs.
#
# `gos build` links a native binary itself and has no way to spell
# `-framework ...` (the manifest has no linker-argument key, and the linker
# plan only emits `-L` / `-l`). It does read $CC, so this wrapper supplies the
# frameworks on the way through:
#
#   CC="$PWD/tools/cc-gpui.sh" gos build examples/01_hello.gos
#
# Every framework below is named by GPUI or its dependencies (AppKit for the
# window, CoreText/CoreGraphics for glyph rasterisation, Metal for the
# renderer, and so on).
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
