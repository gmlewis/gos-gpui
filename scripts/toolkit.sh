#!/usr/bin/env bash
#
# The toolkit check: can a stranger build on this?
#
#   ./scripts/toolkit.sh              # everything this machine can run
#   ./scripts/toolkit.sh --no-run     # the headless half only (no window, no container)
#
# The toolkit check: can a stranger build on this? It stands up a
# Gossamer project **outside this repository**, in a scratch directory, whose
# only relationship to this one is a dependency declaration in its
# `project.toml`. That project opens a window, draws a label and a button,
# handles a click (via its own event path), and closes.
#
# The check passes only when that project builds and runs on every tier this
# machine can run. When it fails, it says **which of the seven things a toolkit
# owes a stranger is not yet true**:
#
#   1. a licence, stated
#   2. a dependency declaration, not a copy
#   3. a window on the first attempt
#   4. every callable function documented
#   5. what it cannot do, stated with reasons
#   6. a native binary that runs, not crashes
#   7. checks that run without being asked
#
# Nothing here opens a window on this Mac unless asked: `--no-run` skips
# runtime tiers and checks only the headless claims.
#
# Scratch lives in /tmp and never in the tree.

set -uo pipefail

cd "$(dirname "$0")/.."
root="$PWD"

scratch="${TOOLKIT_SCRATCH:-/tmp/zz_toolkit}"
cache="${GOSBOX_CACHE:-$HOME/.cache/learn-gpui}"
image="${GOSBOX_IMAGE:-gosbox:2}"
toolchain="${GOSBOX_TOOLCHAIN:-linux070}"
gossamer="${GOSBOX_GOSSAMER:-gossamer-070}"
timeout_s="${TOOLKIT_TIMEOUT:-60}"

run_tiers=1
[ "${1:-}" = "--no-run" ] && run_tiers=0

export GOSSAMER_ROOT="${GOSSAMER_ROOT:-/Users/glenn/src/github.com/danpozmanter/gossamer}"

fails=()

fail() { fails+=("$1"); }

say() { printf '%s\n' "$*"; }
hdr() { printf '\n=== %s ===\n' "$*"; }

# ---------------------------------------------------------------------------
# 1. A licence, stated.
# ---------------------------------------------------------------------------
hdr "1. licence"

if [ -f LICENSE ]; then
  lic="$(head -3 LICENSE | tr -s ' ' | grep -oE 'Apache License|MIT License|BSD' | head -1)"
  if grep -qi 'licence: not yet stated\|license: not yet stated' README.md; then
    fail "1. a licence: LICENSE is present but README still says none is stated"
  else
    say "  ok     LICENSE present (${lic:-unknown}) and README does not disclaim it"
  fi
else
  fail "1. a licence: there is no LICENSE at the repository root"
fi

# ---------------------------------------------------------------------------
# 2. A dependency declaration, not a copy.
#
#    The scratch project below relates to this one in exactly one way: a
#    `[rust-bindings]` path entry for `gpui` and a `[dependencies]` entry with
#    a `module = "ui"` override for `ui`. Nothing is copied out of this tree.
# ---------------------------------------------------------------------------
hdr "2. dependence"

mkdir -p "$scratch/src"

# The repo path inside the scratch manifest differs by environment: on the Mac
# it is this checkout; in the container the staged checkout is at /work.
write_manifest() {
  local repo_path="$1"
  # Quoted heredoc: the comments name `gpui` and `ui` in backticks, which an
  # unquoted heredoc would run as commands. The repo path goes in with sed.
  cat > "$scratch/project.toml" <<'EOF'
[project]
id      = "example.com/toolkit-scratch"
version = "0.1.0"
gossamer-version = "^v0.71.1"

# gpui: the binding crate, reached as an ordinary path dependency.
[rust-bindings]
gpui-binding = { path = "@REPO@/gpui-binding" }

# ui: the chaining layer, reached as a Gossamer package whose root module is
# named ui by this override. This is the line that a stranger writes.
[dependencies]
"github.com/gmlewis/gos-gpui" = { path = "@REPO@", module = "ui" }
EOF
  sed -i '' "s|@REPO@|$repo_path|g" "$scratch/project.toml"
}

cat > "$scratch/src/main.gos" <<'EOF'
// The smallest program that is still a program: a window, a label, a button,
// a click handled, and a close. It quits itself so it can be a check.
use gpui
use std::process
use ui

fn handle(events: Vec<String>) {
    for event in events {
        let id, _data = gpui::event_parts(event)
        if id == "ok" {
            println("OK TOOLKIT clicked on frame {}", gpui::frame())
            gpui::quit()
        }
    }
}

fn main() {
    let outcome = gpui::run("toolkit", 320.0, 180.0, || {
        gpui::take_events() |> handle
        // Nothing clicks a button in a headless run, so the program raises the
        // event a click would, once, through the same queue and handler. The
        // store, not a captured variable, remembers that it has been armed: a
        // Gossamer closure captures a scalar by value.
        if !gpui::state_bool("armed", false) {
            gpui::set_bool("armed", true)
            gpui::delay("ok", 900)
        }
        ui::column("p=24 gap=12 size=full")
            .text("toolkit works", "font=2xl color=#cdd6f4")
            .button("ok", "close", "px=14 py=8 rounded=8 bg=#89b4fa")
            |> ui::root
    })

    if let Err(problem) = outcome {
        eprintln(f"gpui::run stopped with an error: {problem}")
        process::exit(1)
    }
}
EOF

write_manifest "$root"

dependence_ok=0
if gos check "$scratch/src/main.gos" >/tmp/zz_toolkit.check.log 2>&1; then
  dependence_ok=1
  say '  ok     a project outside this repo resolves `use gpui` and `use ui` from a declaration'
else
  # Name the module that did not resolve, which is the whole point of the check.
  why="$(grep -oE "module \`[a-z_]+\` does not exist" /tmp/zz_toolkit.check.log | head -1)"
  fail "2. a dependency declaration: the scratch project does not build (${why:-see /tmp/zz_toolkit.check.log})"
  sed 's/^/         /' /tmp/zz_toolkit.check.log | head -6
fi

# ---------------------------------------------------------------------------
# 4. Every callable function documented.   (checked here so the report is one
# 5. What it cannot do, stated.             list, but implemented in docs.sh)
# ---------------------------------------------------------------------------
hdr "4. the reference"
if ./scripts/docs.sh >/tmp/zz_toolkit.docs.log 2>&1; then
  say "  ok     $(tail -1 /tmp/zz_toolkit.docs.log)"
else
  fail "4. every callable function documented: scripts/docs.sh is red (see /tmp/zz_toolkit.docs.log)"
  grep -E 'STALE|README names' /tmp/zz_toolkit.docs.log | sed 's/^/         /' | head -8
fi

hdr "5. what it cannot do"
if grep -q '^## What this cannot do' README.md; then
  say "  ok     README carries a \"What this cannot do\" section"
else
  fail "5. what it cannot do: README has no \"What this cannot do\" section"
fi

# ---------------------------------------------------------------------------
# 7. Checks that run without being asked.
# ---------------------------------------------------------------------------
hdr "7. checks without being asked"

if compgen -G ".github/workflows/*.yml" >/dev/null || compgen -G ".github/workflows/*.yaml" >/dev/null; then
  say "  ok     a CI workflow is in the tree"
else
  fail "7. checks that run without being asked: there is no CI workflow under .github/workflows/"
fi

if gos test tests >/tmp/zz_toolkit.test.log 2>&1; then
  say "  ok     gos test tests"
else
  fail "7. checks that run without being asked: \`gos test tests\` is red"
fi

if ./scripts/pair-census.sh >/tmp/zz_toolkit.census.log 2>&1; then
  say "  ok     scripts/pair-census.sh"
else
  fail "7. checks that run without being asked: scripts/pair-census.sh is red"
fi

# ---------------------------------------------------------------------------
# 3. A window on the first attempt; 6. a native binary that runs.
#
#    All five tiers, or as many as this machine can run.
# ---------------------------------------------------------------------------
hdr "3. a window / 6. a native binary"

if [ "$run_tiers" = 1 ] && [ "$dependence_ok" = 1 ]; then
  window_ok=0
  if ./scripts/window-guard.sh; then window_ok=1; fi

  if [ "$window_ok" = 1 ]; then
    if ./scripts/tiers.sh "$scratch/src/main.gos" >/tmp/zz_toolkit.tiers.log 2>&1; then
      say "  ok     all five tiers on the Mac"
    else
      fail "6. a native binary that runs: a tier failed on the Mac (see /tmp/zz_toolkit.tiers.log)"
      grep -E '^===|FAIL|Bus error|error\[' /tmp/zz_toolkit.tiers.log | sed 's/^/         /' | head -12
    fi
  else
    # The container. Stage this tree, mount the scratch beside it, and run the
    # five tiers against the staged checkout.
    stage="$cache/docker-stage"; dhome="$cache/docker-home"
    if ! docker info >/dev/null 2>&1; then
      fail "3. a window: needs either a window on this Mac or a running Docker (the container)"
    elif [ ! -d "$cache/$toolchain" ] || [ ! -d "$cache/$gossamer" ]; then
      fail "3. a window: the container toolchains are missing under $cache (see scripts/gosbox.sh)"
    else
      mkdir -p "$stage" "$dhome"
      rsync -a --delete \
        --exclude 'tools/upstream/' --exclude 'target/' --exclude '.gos-cache/' \
        --exclude 'gpui-binding/target/' --exclude '.git/' \
        . "$stage/"
      # The runner cache in $dhome is keyed on the binding's path, so a changed
      # binding has to be touched or the staged copy reuses a stale build.
      touch "$stage"/gpui-binding/Cargo.toml "$stage"/gpui-binding/src/*.rs
      # Regenerate the scratch manifest for the container's paths.
      write_manifest "/work"
      rsync -a --delete "$scratch/" "$dhome/scratch/"
      bin="toolkit-scratch"
      # Every tier's own exit status decides the run's: a `docker run` that
      # merely started is not a pass.
      script="export PATH=/usr/local/cargo/bin:/tool/bin:/usr/sbin:/usr/bin:/sbin:/bin; \
        cd /root/scratch; export CC=/work/tools/cc-gpui.sh; failed=0; \
        t() { printf '=== %s ===\\n' \"\$1\"; shift; timeout -k 5 $timeout_s \"\$@\"; \
              s=\$?; echo \"(exit \$s)\"; [ \"\$s\" -eq 0 ] || failed=1; }; \
        t check gos check src/main.gos; \
        t run gos run --main-thread src/main.gos; \
        t 'run --no-jit' gos run --main-thread --no-jit src/main.gos; \
        printf '=== build ===\\n'; gos build src/main.gos || failed=1; \
        t 'build -> ./target/debug/$bin' ./target/debug/$bin; \
        printf '=== build --release ===\\n'; gos build --release src/main.gos || failed=1; \
        t 'release -> ./target/release/$bin' ./target/release/$bin; \
        exit \$failed"
      docker run --rm -i \
        -v "$stage":/work -v "$dhome":/root \
        -v "$cache/$gossamer":/gos -v "$cache/$toolchain":/tool \
        -e DISPLAY=:99 -e GOSBOX=1 -e GOSSAMER_ROOT=/gos -e GOSSAMER_NOTED_ERRORS=1 \
        -e CC=/work/tools/cc-gpui.sh \
        -e GOS_LINUX_LIBS="-Wl,--no-as-needed -lxkbcommon-x11 -lxkbcommon -lX11 -lxcb -lfontconfig -lfreetype -ldl -lpthread -lm" \
        -e PATH=/usr/local/cargo/bin:/tool/bin:/usr/sbin:/usr/bin:/sbin:/bin \
        -e CARGO_HOME=/usr/local/cargo -e RUSTUP_HOME=/usr/local/rustup \
        -w /work "$image" bash -lc "$script" >/tmp/zz_toolkit.tiers.log 2>&1
      tier_status=$?
      # A tier is a pass only if its own exit was 0, the program that ran said
      # `OK `, and nothing aborted. A `docker run` that merely started is not
      # evidence, and a `SIGBUS` on the compiled tiers is the compiled-tier
      # crash, not a pass.
      if [ "$tier_status" -eq 0 ] \
         && grep -q '^OK TOOLKIT' /tmp/zz_toolkit.tiers.log \
         && ! grep -qE 'Bus error|exit 13[0-9]|FAIL' /tmp/zz_toolkit.tiers.log; then
        say "  ok     all five tiers in the container"
      else
        fail "3. a window / 6. a native binary: a tier failed in the container (see /tmp/zz_toolkit.tiers.log)"
        grep -E '^===|FAIL|Bus error|error\[|panicked|exit ' /tmp/zz_toolkit.tiers.log | sed 's/^/         /' | head -14
      fi
    fi
  fi
else
  say "  skipped (--no-run, or the dependency declaration (2) is red, so there is nothing to run)"
fi

# ---------------------------------------------------------------------------
# Verdict.
# ---------------------------------------------------------------------------
hdr "verdict"
if [ "${#fails[@]}" -eq 0 ]; then
  if [ "$run_tiers" = 1 ]; then
    say "toolkit: a stranger can build on this."
  else
    # `--no-run` is the headless half. It must not claim a whole toolkit: the
    # two things only a run can prove were not proven.
    say "toolkit: the headless claims hold (1, 2, 4, 5, 7), and 3 and 6 were NOT run."
    say "         A window on the first attempt and a native binary that runs are"
    say "         unproven here — run it without --no-run to decide those."
  fi
  exit 0
fi
say "toolkit: NOT yet a toolkit. ${#fails[@]} thing(s) missing:"
for f in "${fails[@]}"; do
  say "  - $f"
done
exit 1
