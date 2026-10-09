# The headless GUI container `scripts/gosbox.sh` runs programs in.
#
#   docker build -t gosbox:2 -f scripts/gosbox.Dockerfile scripts/
#
# **The tag is 2: `gosbox:1`'s recipe is recorded nowhere but this machine's Docker
# daemon**, recovered only by `docker history --no-trunc`. This file reproduces it, so
# the image is not a thing only one machine can rebuild. The two additions over `:1`
# are the LLVM layer at the bottom and the `tools/` staging that `gosbox.sh` does —
# the container can run a window but cannot link a native binary without them, and
# `gos build` needs both.
#
# What each layer is for:
#
#   * openbox  — EWMH state: maximize/minimize/placement have to have an answer for
#                the window rows to read back.
#   * Xvfb     — the actual service.  A real X server, a real framebuffer, nobody
#                sitting in front of it (see gosbox-entrypoint.sh).
#   * x11vnc / xdotool / imagemagick — a way to look at and poke the display while
#                developing the container itself; not used by a probe.
#   * libxkbcommon-dev, libxcb*-dev, libfontconfig1-dev, libfreetype6-dev,
#     libvulkan1 + mesa-vulkan-drivers — GPUI's X11/blade backend and lavapipe,
#     a software Vulkan device, so rendering needs no GPU.
#   * llvm-22 + clang-22 — the native backend compiles through `opt` and `llc`;
#                without them `gos build` stops at
#                `error: build: native backend failed: opt (LLVM toolchain) not found`.
#                Debian 12.15 packages LLVM 22 in main, as `1:22.1.8-1~deb12u1`,
#                which is the version this toolchain is built against.  The two
#                symlinks are needed because the backend looks for the bare names.

FROM rust:1.99.0-bookworm

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
      openbox procps iproute2 x11vnc \
    && rm -rf /var/lib/apt/lists/*

RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config cmake clang lld \
      libssl-dev zlib1g-dev \
      libx11-dev libxcb1-dev libxcb-render0-dev libxcb-shape0-dev \
      libxcb-xfixes0-dev libxcb-xkb-dev libxcb-randr0-dev \
      libxkbcommon-dev libxkbcommon-x11-dev libxrandr-dev libxi-dev \
      libxcursor-dev libxdamage-dev libxcomposite-dev \
      libfontconfig1-dev libfreetype6-dev \
      xvfb xauth libvulkan1 mesa-vulkan-drivers vulkan-tools \
      x11-utils xdotool imagemagick \
      fonts-dejavu-core fonts-liberation fonts-noto-core \
      git curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# The native tier: `opt`/`llc` for the LLVM backend, named as it looks for them.
RUN apt-get update && apt-get install -y --no-install-recommends llvm-22 clang-22 \
    && ln -sf /usr/lib/llvm-22/bin/opt /usr/local/bin/opt \
    && ln -sf /usr/lib/llvm-22/bin/llc /usr/local/bin/llc \
    && rm -rf /var/lib/apt/lists/*

COPY gosbox-entrypoint.sh /usr/local/bin/gosbox-entrypoint
RUN chmod +x /usr/local/bin/gosbox-entrypoint

ENV XDG_RUNTIME_DIR=/tmp/xdg
ENV DISPLAY=:99
ENV GOSBOX=1

ENTRYPOINT ["/usr/local/bin/gosbox-entrypoint"]
CMD ["sleep", "infinity"]
