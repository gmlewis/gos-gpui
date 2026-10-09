#!/usr/bin/env bash
#
# Run GUI programs headless, in a container, **with no window on this Mac**.
#
#   ./scripts/gosbox.sh run tests/gui/w3_events.gos          # one probe, bounded
#   ./scripts/gosbox.sh run tests/gui/w3_events.gos 60       # ... with its own bound
#   ./scripts/gosbox.sh probes                               # every probe, the bulk pass
#   ./scripts/gosbox.sh probes w3 w7                         # only those whose name matches
#   ./scripts/gosbox.sh check tests/gui/w3_events.gos        # compile only, no window
#   ./scripts/gosbox.sh tiers tests/gui/w3_events.gos        # all five tiers, when the image has LLVM
#   ./scripts/gosbox.sh shell                                # a shell in the same environment
#
# # Why this exists
#
# On 2026-10-06 the owner of this machine asked that nothing pop up on their screen while they
# work, and `scripts/window-guard.sh` made that the default. The runtime tiers — running a
# program, not merely compiling it — went with it, because on this Mac a run *is* a window.
#
# This is the way back. `gosbox:2` is a Linux image with Xvfb, a window manager, a software
# Vulkan device and LLVM 22, so a GPUI window opens onto a virtual display that belongs to the
# container and never reaches the Mac's screen. `scripts/pointer.sh` — which drives a window from
# outside the process through the macOS event tap — has no counterpart here; a probe that needs a
# click is listed as such by its own `// needs:` line and skipped, exactly as `scripts/probes.sh`
# does.
#
# # What is mounted
#
#   docker-stage/   this working tree, rsynced in (no tools/upstream/, target/, .gos-cache/ — 11MB
#                   of source; /work/target/ is excluded from the rsync but survives it, so it
#                   grows to ~130MB once a build tier has run — that is cargo's cache, kept on
#                   purpose, and it is in the cache directory and never in this tree)
#   docker-home/    /root, so the cargo and runner caches survive between runs
#   gossamer-068/   /gos, the pristine toolchain source the binding builds against
#   linux068/       /tool, the Linux `gos` and its runtime archive
#
# The toolchain is chosen by `GOSBOX_TOOLCHAIN` (default `linux068`) and the source by
# `GOSBOX_GOSSAMER` (default `gossamer-068`), so a second toolchain can be exercised without
# editing this file. Both track the pin in `project.toml`, which moved to 0.68.0 in round 88: the
# container has to build the binding against the version the project asks for, or cargo stops at
# version resolution before any of this matters.
#
# # The build tiers, and why `tools/` is staged
#
# `gos build` links the binary itself and its linker plan has no key for linker arguments, so a
# GPUI binary comes out with the windowing symbols undefined. `tools/cc-gpui.sh` is a `cc` that
# supplies them; it is read by `gos` through `$CC`. It lives under `tools/`, which used to be
# excluded from the stage — so the container could run a window but never link one. It is now
# staged (everything under `tools/` but the 5.9GB `tools/upstream/` checkout), and `inbox` sets
# `CC` and `GOS_LINUX_LIBS` for every run, which is what lets `build` and `build --release` be
# witnessed here instead of skipped.
#
# `GOS_LINUX_LIBS` is passed because `cc-gpui.sh` puts its libraries *before* the objects, and
# Debian's `ld` defaults to `--as-needed`: without `-Wl,--no-as-needed` a library nothing has
# asked for yet is dropped on the floor. The set itself was measured, not guessed — the first
# symbol the link cannot find is `xkb_x11_state_new_from_device`, which is in `libxkbcommon-x11`,
# not `libxkbcommon`.
#
# # What this does not do
#
# The programs run against the **Linux** build of the runtime, so the macOS-only parts of a row —
# a window's traffic lights, `Display::bounds()` reporting a real origin — are not what is being
# checked here. This proves the program runs and prints its verdict; `scripts/tiers.sh` on the Mac
# remains the check for anything the platform itself answers.

set -uo pipefail

cd "$(dirname "$0")/.."

CACHE="${GOSBOX_CACHE:-$HOME/.cache/learn-gpui}"
IMAGE="${GOSBOX_IMAGE:-gosbox:2}"
TOOLCHAIN="${GOSBOX_TOOLCHAIN:-linux068}"
GOSSAMER="${GOSBOX_GOSSAMER:-gossamer-068}"
STAGE="$CACHE/docker-stage"
DHOME="$CACHE/docker-home"

# A run is bounded because a probe that leaves its window open is not a check.
timeout_s="${GOSBOX_TIMEOUT:-30}"

die() { echo "gosbox: $*" >&2; exit 2; }

for d in "$CACHE/$TOOLCHAIN" "$CACHE/$GOSSAMER"; do
  [ -d "$d" ] || die "no such toolchain or source: $d (see the header of this script)"
done

# `docker run` on a stopped Docker Desktop fails with a message nobody reads twice; say it once.
docker info >/dev/null 2>&1 || die "Docker is not running"

# The tree is rsynced rather than mounted so that the container's writes — the runner cache under
# it, cargo's fingerprints — cannot land in the working tree. `--delete` keeps it exactly the tree
# as it stands, so a run can never silently use yesterday's program.
#
# **`--delete` is not enough, and the binding is where it showed.** `$DHOME` is a persistent
# volume, and the runner cache inside it is keyed on the `[rust-bindings]` path declared in
# `project.toml` — the same string in every checkout of this project. So a second tree staged here
# does not get a second cache entry, and `rsync -a` (which preserves mtimes, and must, or every run
# would rebuild) hands the runner the second tree's binding sources with **older** mtimes than the
# build it already has. It keeps the build. Measured round 103, on `mr4_stop_propagation.gos`,
# which is green in the working tree and red against `329133c`: the working tree staged straight
# after a `329133c` run and without touching its binding ran the **`329133c`** binding, three times
# over; touching `gpui-binding/src/*.rs` and staging again ran the working tree's, with nothing
# else changed. The same trap produced a false red on that probe earlier in the round.
#
# The hash below is what makes the guarantee at the top of this comment true for the binding as
# well. It is taken before and after the copy, and a binding that **changed** is touched so the
# next `gos` rebuilds it; a tree staged twice in a row hashes the same and pays nothing.
binding_hash() {
  [ -d "$STAGE/gpui-binding/src" ] || { echo none; return; }
  cksum "$STAGE"/gpui-binding/Cargo.toml "$STAGE"/gpui-binding/src/*.rs 2>/dev/null | cksum
}

stage() {
  mkdir -p "$STAGE" "$DHOME"
  local before after
  before="$(binding_hash)"
  rsync -a --delete \
    --exclude 'tools/upstream/' --exclude 'target/' --exclude '.gos-cache/' \
    --exclude 'gpui-binding/target/' --exclude '.git/' \
    . "$STAGE/"
  after="$(binding_hash)"
  [ "$before" = "$after" ] || touch "$STAGE"/gpui-binding/Cargo.toml "$STAGE"/gpui-binding/src/*.rs
}

# A probe whose claim this environment cannot witness says so on its own `// linux:` line — a window
# manager that places windows itself, a keyboard with no ⌘, a desktop portal to put a panel in — and
# is skipped with that reason rather than being reported as a failure. macOS does not read the line,
# so it keeps the coverage; each probe's own header says what was measured to earn it.
#
# A login shell re-reads /etc/profile and throws away the PATH `-e PATH=` set, taking `tool/bin`
# — the `gos` this script exists to run — with it. So the path is put back inside the command
# rather than in the container's environment, where the profile cannot reach it.
CONTAINER_PATH='/usr/local/cargo/bin:/tool/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'

# The libraries `tools/cc-gpui.sh` is told to hand the linker, and the reason it has to be told:
# `-Wl,--no-as-needed` because the wrapper's `-l` flags precede the objects and Debian's `ld` would
# otherwise discard them, and `-lxkbcommon-x11` before `-lxkbcommon` because that is where
# `xkb_x11_*` lives. Everything after those two measured as unnecessary to link this program; a
# symbol this set cannot find is a line to add here, not a reason to touch `tools/`.
GOS_LINUX_LIBS="${GOSBOX_LIBS:--Wl,--no-as-needed -lxkbcommon-x11 -lxkbcommon -lX11 -lxcb -lfontconfig -lfreetype -ldl -lpthread -lm}"

# The environment every container run gets. `CARGO_HOME` and `RUSTUP_HOME` are set because /root is
# a mounted volume, which hides the ones the rust image baked in. `CC` points at the staged copy of
# `tools/cc-gpui.sh`, so the two build tiers have a linker plan at all — see the header.
inbox() {
  docker run --rm -i \
    -v "$STAGE":/work -v "$DHOME":/root \
    -v "$CACHE/$GOSSAMER":/gos -v "$CACHE/$TOOLCHAIN":/tool \
    -e DISPLAY=:99 -e GOSBOX=1 -e GOSSAMER_ROOT=/gos \
    -e GOSSAMER_NOTED_ERRORS=1 \
    -e CC=/work/tools/cc-gpui.sh -e GOS_LINUX_LIBS="$GOS_LINUX_LIBS" \
    -e PATH=/usr/local/cargo/bin:/tool/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    -e CARGO_HOME=/usr/local/cargo -e RUSTUP_HOME=/usr/local/rustup \
    -w /work "$IMAGE" "$@"
}

# `incmd` is `inbox` with the path restored, which is what every actual program run wants.
incmd() { inbox bash -lc "export PATH=$CONTAINER_PATH; $1"; }


# Reap a stale runner build lock before it costs a silent timeout.
#
# `gos` guards each runner workdir with `~/.cache/gossamer/runners/<hash>/.gos-build.lock`, holding
# the pid of the `gos` that made it; a lock whose owner is gone is meant to be reclaimed, but when
# it is not, the next `gos` polls it and then errors — which under a `timeout 30` is
# indistinguishable from a program that never quits. `scripts/unlock-stale-build.sh` does this for
# the Mac's own cache, and `scripts/probes.sh` and `scripts/tiers.sh` call it too. This does
# the same for the container's, which is *not* the Mac's: `/root` is a persistent volume, so a lock
# left by a container that was killed mid-build survives to hang the next run.
#
# Measured the hard way in round 87: a throwaway container killed at a 120s timeout left
# `runners/eb43bebcecce/.gos-build.lock` holding pid 40, after which two runs of a probe that had
# just been green answered `exited 124 — no verdict` and printed nothing at all.
#
# **`kill -0` is the wrong test, and round 96 measured why.** The lock holds a pid from
# whichever container made it; this reaper runs in a *fresh* container, whose own low pids are
# taken by the display server and the window manager. A lock holding 40 therefore looked
# "alive" to `kill -0 40` because *this* container's pid 40 existed, and it was never reaped —
# every probe in the suite then answered `cache i/o error: another gos process holds … for >5
# min` and the run came back **0/49**. So the pid has to be checked against `/proc/<pid>/cmdline`
# and be a `gos`; anything else, including an empty cmdline for a pid that is gone, is stale.
reap_locks() {
  inbox bash -lc 'for l in /root/.cache/gossamer/runners/*/.gos-build.lock; do
      [ -f "$l" ] || continue
      p="$(cat "$l" 2>/dev/null)"
      case "$p" in ""|*[!0-9]*) continue ;; esac
      owner="$(cat "/proc/$p/cmdline" 2>/dev/null | tr "\0" " ")"
      case "$owner" in
        *gos*) ;;
        *) rm -f "$l"
           echo "gosbox: reaped stale build lock $l (pid $p is ${owner:-gone}, not a gos)" ;;
      esac
    done' 2>&1 | sed 's/^/       /'
}


# One program's transcript is a pass on the same terms `scripts/probes.sh` uses: it exited 0,
# it said `OK `, it said no `FAIL `/`error[`/panic, and it noted no error. Anything else is not
# evidence, however good it looked.
verdict() {
  local log="$1" needs="$2"
  grep -q "^OK " "$log" || { echo "no verdict"; return 1; }
  grep -qi "^FAIL \|panicked\|error\[" "$log" && { echo "said FAIL or errored"; return 1; }
  [ "$needs" = "noted" ] || ! grep -q "^gpui: noted:" "$log" || { echo "noted an error"; return 1; }
  return 0
}

# A **compile** tier's transcript is a pass on different terms, and demanding `^OK ` of it was a
# bug in this script: `gos check` answers `check: ok (N items typed)` and a build answers
# `build: NB native executable at …`, neither of which is the running program's `OK ` verdict. So
# the rule for those tiers is the one that is actually about them — the command exited 0 and the
# transcript carries no error — and it is deliberately not weaker: an `error[` or a `FAIL ` still
# fails it, and a *run* tier never reaches this function.
compile_verdict() {
  local log="$1"
  grep -qi "error\[\|^FAIL \|panicked\|build failed\|linker failed" "$log" && { echo "errored"; return 1; }
  return 0
}

# Does this image carry the LLVM the native backend needs? `gos build` shells out to `opt` and
# `llc`, and an image without them answers `error: build: native backend failed: opt (LLVM
# toolchain) not found`. That is a property of the environment, not of the program, so it is
# reported as a skip, with the reason, rather than as a failure — the same reason a probe's
# `// linux:` line is a skip. A skip is never counted as a pass.
#
# `gosbox:2` does have them (Debian's `llvm-22`, `1:22.1.8-1~deb12u1`), so this normally answers
# yes and both build tiers run. `GOSBOX_IMAGE=gosbox:1` is the case it exists for: that image has
# neither LLVM nor the staged `tools/`, and there the two build tiers still skip, by name.
have_llvm() { incmd 'command -v opt >/dev/null 2>&1'; }

cmd="${1:-}"; shift || true

case "$cmd" in
  shell)
    stage
    exec docker run --rm -it \
      -v "$STAGE":/work -v "$DHOME":/root \
      -v "$CACHE/$GOSSAMER":/gos -v "$CACHE/$TOOLCHAIN":/tool \
      -e DISPLAY=:99 -e GOSBOX=1 -e GOSSAMER_ROOT=/gos \
      -e GOSSAMER_NOTED_ERRORS=1 \
      -e CC=/work/tools/cc-gpui.sh -e GOS_LINUX_LIBS="$GOS_LINUX_LIBS" \
      -e PATH=/usr/local/cargo/bin:/tool/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
      -e CARGO_HOME=/usr/local/cargo -e RUSTUP_HOME=/usr/local/rustup \
      -w /work "$IMAGE" bash -l
    ;;

  check)
    [ $# -ge 1 ] || die "check needs a program"
    stage
    reap_locks
    rc=0
    for p in "$@"; do
      out="$(incmd "gos check '$p' 2>&1 | tail -20")"
      if echo "$out" | grep -q "check: ok"; then
        echo "ok     $p"
      else
        echo "FAIL   $p"; echo "$out" | sed 's/^/       /'; rc=1
      fi
    done
    exit "$rc"
    ;;

  run)
    [ $# -ge 1 ] || die "run needs a program"
    prog="$1"; secs="${2:-$timeout_s}"
    stage
    reap_locks
    log="$(mktemp -t gosbox.XXXXXX)"
    incmd "timeout -k 5 $secs gos run --main-thread '$prog'" > "$log" 2>&1
    status=$?
    if reason="$(verdict "$log" no)"; then
      echo "ok     $prog: $(grep -m1 '^OK ' "$log")"
      # A green run prints one line and throws the transcript away, which is right for a suite and
      # wrong for a record: the per-point lines a probe prints *are* the evidence, and a `FAIL`
      # run gets them printed for free while a passing one has to ask. `GOSBOX_TRANSCRIPT=1` is
      # how it asks.
      [ "${GOSBOX_TRANSCRIPT:-0}" = 1 ] && sed 's/^/       /' "$log"
      rm -f "$log"; exit 0
    fi
    echo "FAIL   $prog: exited $status — $reason"
    sed 's/^/       /' "$log" | head -20
    rm -f "$log"; exit 1
    ;;

  tiers)
    # The five tiers, but each one only as far as a Linux run can carry it: check, build and
    # build --release compile; run and run --no-jit execute. `--no-jit` exists on the interpreter
    # tier only, so it is asked for the way the toolchain spells it.
    #
    # `check` is judged by whether it compiled — it prints `check: ok (N items typed)`, not the
    # running program's `OK ` — and the two run tiers exactly as `scripts/probes.sh` judges a
    # probe. The two build tiers compile *and link*, which needs LLVM plus the staged
    # `tools/cc-gpui.sh`; when the image has both they run here like any other tier, and when it
    # has neither they skip, by name and with the reason.
    [ $# -ge 1 ] || die "tiers needs a program"
    prog="$1"
    stage
    reap_locks
    rc=0
    # Asked once, before any tier runs: whether the native backend can run at all here.
    llvm=0; have_llvm && llvm=1
    for t in check run run-nosjit build build-release; do
      case "$t" in
        check)         cmdline="gos check '$prog'"; kind=compile ;;
        run)           cmdline="timeout -k 5 $timeout_s gos run --main-thread '$prog'"; kind=run ;;
        run-nosjit)    cmdline="timeout -k 5 $timeout_s gos run --main-thread --no-jit '$prog'"; kind=run ;;
        build)         cmdline="timeout -k 5 600 gos build '$prog'"; kind=compile ;;
        build-release) cmdline="timeout -k 5 600 gos build --release '$prog'"; kind=compile ;;
      esac
      # A build tier needs LLVM; an image without it is skipped by name, with the reason, rather
      # than having the image's own gap reported as the program's failure. Skipped is never
      # counted as a pass.
      if [ "$kind" = compile ] && [ "$t" != check ] && [ "$llvm" = 0 ]; then
        echo "skip   $t: this image cannot link a GPUI binary — no LLVM (\`opt\`/\`llc\`); witnessed headless on the Mac instead"
        continue
      fi
      log="$(mktemp -t gosbox.XXXXXX)"
      incmd "$cmdline" > "$log" 2>&1
      status=$?
      if [ "$kind" = compile ]; then
        if [ "$status" = 0 ] && reason="$(compile_verdict "$log")"; then
          echo "ok     $t: $(grep -m1 -E '^(check: ok|build: .*)' "$log" | cut -c1-100)"
        else
          [ "$status" = 0 ] && detail="$reason" || detail="exited $status"
          echo "FAIL   $t: $detail"
          sed 's/^/       /' "$log" | head -8
          rc=1
        fi
      elif reason="$(verdict "$log" no)"; then
        # The verdict in full, not the first hundred characters of it. A verdict is a list of
        # readings — the numbers a row is judged on — and a transcript that stops in the middle
        # of that list is a transcript a reader has to go back to the container to finish. The
        # `check` and `build` lines above keep their cut because those lines are a count and a
        # byte size; a verdict has no such shape.
        echo "ok     $t: $(grep -m1 '^OK ' "$log")"
      else
        echo "FAIL   $t: exited $status — $reason"
        sed 's/^/       /' "$log" | head -8
        rc=1
      fi
      rm -f "$log"
    done
    exit "$rc"
    ;;

  probes)
    # Every probe, or those whose name matches an argument — the same selection `scripts/probes.sh`
    # makes. A probe that declares `// needs:` is skipped rather than failed: it names something
    # this container cannot supply (a click through the macOS event tap, a person), and reporting
    # that as a failure would be the suite crying wolf.
    stage
    reap_locks
    want=("$@")
    probes=()
    for p in tests/gui/*.gos; do
      [ -f "$p" ] || continue
      if [ "${#want[@]}" -gt 0 ]; then
        keep=0
        for w in "${want[@]}"; do case "$(basename "$p")" in *"$w"*) keep=1 ;; esac; done
        [ "$keep" = 1 ] || continue
      fi
      probes+=("$p")
    done
    [ "${#probes[@]}" -gt 0 ] || die "nothing matched"

    fails=0; skipped=0; flaky=0
    for p in "${probes[@]}"; do
      name="$(basename "$p" .gos)"
      needs_line="$(sed -n '1,20p' "$p" | sed -n 's|^// needs: *||p' | head -1)"
      # The one probe whose subject *is* the noted-error channel declares it, exactly as
      # `scripts/probes.sh` reads it; without this its own banner would fail it.
      noted="$(sed -n '1,20p' "$p" | sed -n 's|^// noted: *expected.*|noted|p' | head -1)"
      noted="${noted:-no}"
      if [ -n "$needs_line" ]; then
        echo "skip   $name: needs $needs_line"
        skipped=$((skipped + 1)); continue
      fi
      # `// linux:` names a claim the container cannot witness, where `// needs:` names something no
      # run could supply (a person, the macOS pointer harness). Both are skips, for the same reason.
      #
      # `// container:` is deliberately *not* read here. It is the mirror of `// linux:` — a probe
      # the Mac cannot run and the container can — and `scripts/probes.sh` and `scripts/tiers.sh`
      # are the ones that skip it. Reading it here would skip the only place it means anything.
      linux_line="$(sed -n '1,20p' "$p" | sed -n 's|^// linux: *||p' | head -1)"
      if [ -n "$linux_line" ]; then
        echo "skip   $name: $linux_line"
        skipped=$((skipped + 1)); continue
      fi
      # Every probe gets its own container (`inbox` is `docker run --rm`), and `/root` is a
      # persistent volume — so **any** build lock still on disk when this container starts
      # belongs to a container that no longer exists, and is stale by construction. Reaping once
      # per sweep is not enough and round 101 measured what that costs: a probe that timed out
      # mid-sweep left `runners/*/.gos-build.lock` behind, and every probe after it answered an
      # empty transcript and `failed twice` — the sweep came back **32/52** with 13 skips and
      # every failure above `cl9_clipboard` in alphabetical order, which is a lock, not a
      # regression. The blunt rule is safe *here* and nowhere else: nothing in this container is
      # running `gos` at the moment this line runs, because the line is the thing that starts it.
      run_one() {
        log="$(mktemp -t gosbox.XXXXXX)"
        incmd "rm -f /root/.cache/gossamer/runners/*/.gos-build.lock; timeout -k 5 $timeout_s gos run --main-thread '$p'" > "$log" 2>&1
        verdict "$log" "$noted" >/dev/null
      }
      if run_one; then
        echo "ok     $name: $(grep -m1 '^OK ' "$log")"
      else
        first="$(sed -n '1p' "$log")"
        if run_one; then
          echo "FLAKY  $name: passed on retry; the first attempt said: ${first:-no output}"
          flaky=$((flaky + 1))
        else
          echo "FAIL   $name: failed twice"
          sed 's/^/       /' "$log" | head -4
          fails=$((fails + 1))
        fi
      fi
      rm -f "$log"
    done
    echo "gosbox: $(( ${#probes[@]} - fails - skipped ))/$(( ${#probes[@]} - skipped )) passed"
    [ "$skipped" -gt 0 ] && echo "gosbox: $skipped skipped (each names what it needs)"
    [ "$flaky" -gt 0 ] && echo "gosbox: $flaky flaky (passed only on a retry)"
    exit $(( fails > 0 ))
    ;;

  stage)
    stage
    du -sh "$STAGE"
    ;;

  *)
    sed -n '2,50p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac
