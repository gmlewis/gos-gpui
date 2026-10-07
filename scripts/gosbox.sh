#!/usr/bin/env bash
#
# Run GUI programs headless, in a container, **with no window on this Mac**.
#
#   ./scripts/gosbox.sh run tests/gui/w3_events.gos          # one probe, bounded
#   ./scripts/gosbox.sh run tests/gui/w3_events.gos 60       # ... with its own bound
#   ./scripts/gosbox.sh probes                               # every probe, the bulk pass
#   ./scripts/gosbox.sh probes w3 w7                         # only those whose name matches
#   ./scripts/gosbox.sh check tests/gui/w3_events.gos        # compile only, no window
#   ./scripts/gosbox.sh tiers tests/gui/w3_events.gos        # the five tiers — see "tiers" below:
#                                                            # check + both runs here, both builds skipped
#   ./scripts/gosbox.sh shell                                # a shell in the same environment
#
# # Why this exists
#
# On 2026-10-06 the owner of this machine asked that nothing pop up on their screen while they
# work, and `scripts/window-guard.sh` made that the default. The runtime tiers — running a
# program, not merely compiling it — went with it, because on this Mac a run *is* a window.
#
# This is the way back. `gosbox:1` is a Linux image with Xvfb, a window manager and a software
# Vulkan device, so a GPUI window opens onto a virtual display that belongs to the container and
# never reaches the Mac's screen. `scripts/pointer.sh` — which drives a window from outside the
# process through the macOS event tap — has no counterpart here; a probe that needs a click is
# listed as such by its own `// needs:` line and skipped, exactly as `scripts/probes.sh` does.
#
# # What is mounted
#
#   docker-stage/   this working tree, rsynced in (no tools/, target/, .gos-cache/ — 9MB)
#   docker-home/    /root, so the cargo and runner caches survive between runs
#   gossamer-head/  /gos, the pristine toolchain source the binding builds against
#   linux-head/     /tool, the Linux `gos` and its runtime archive
#
# The toolchain is chosen by `GOSBOX_TOOLCHAIN` (default `linux-head`) and the source by
# `GOSBOX_GOSSAMER` (default `gossamer-head`), so a second toolchain can be exercised without
# editing this file.
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
IMAGE="${GOSBOX_IMAGE:-gosbox:1}"
TOOLCHAIN="${GOSBOX_TOOLCHAIN:-linux-head}"
GOSSAMER="${GOSBOX_GOSSAMER:-gossamer-head}"
STAGE="$CACHE/docker-stage"
DHOME="$CACHE/docker-home"

# A run is bounded because a probe that leaves its window open is not a check.
timeout_s="${GOSBOX_TIMEOUT:-30}"

die() { echo "gosbox: $*" >&2; exit 2; }

for d in "$CACHE/$TOOLCHAIN" "$CACHE/$GOSSAMER"; do
  [ -d "$d" ] || die "no such toolchain or source: $d (see CAPABILITIES.md, 'The container')"
done

# `docker run` on a stopped Docker Desktop fails with a message nobody reads twice; say it once.
docker info >/dev/null 2>&1 || die "Docker is not running"

# The tree is rsynced rather than mounted so that the container's writes — the runner cache under
# it, cargo's fingerprints — cannot land in the working tree. `--delete` keeps it exactly the tree
# as it stands, so a run can never silently use yesterday's program.
stage() {
  mkdir -p "$STAGE" "$DHOME"
  rsync -a --delete \
    --exclude 'tools/' --exclude 'target/' --exclude '.gos-cache/' \
    --exclude 'gpui-binding/target/' --exclude '.git/' \
    . "$STAGE/"
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

# The environment every container run gets. `CARGO_HOME` and `RUSTUP_HOME` are set because /root is
# a mounted volume, which hides the ones the rust image baked in.
inbox() {
  docker run --rm -i \
    -v "$STAGE":/work -v "$DHOME":/root \
    -v "$CACHE/$GOSSAMER":/gos -v "$CACHE/$TOOLCHAIN":/tool \
    -e DISPLAY=:99 -e GOSBOX=1 -e GOSSAMER_ROOT=/gos \
    -e GOSSAMER_NOTED_ERRORS=1 \
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
# the Mac's own cache, and `scripts/regress.sh`, `probes.sh` and `tiers.sh` all call it. This does
# the same for the container's, which is *not* the Mac's: `/root` is a persistent volume, so a lock
# left by a container that was killed mid-build survives to hang the next run.
#
# Measured the hard way in round 87: a throwaway container killed at a 120s timeout left
# `runners/eb43bebcecce/.gos-build.lock` holding pid 40, after which two runs of a probe that had
# just been green answered `exited 124 — no verdict` and printed nothing at all.
reap_locks() {
  inbox bash -lc 'for l in /root/.cache/gossamer/runners/*/.gos-build.lock; do
      [ -f "$l" ] || continue
      p="$(cat "$l" 2>/dev/null)"
      case "$p" in ""|*[!0-9]*) continue ;; esac
      kill -0 "$p" 2>/dev/null || { rm -f "$l"; echo "gosbox: reaped stale build lock $l (pid $p)"; }
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
# `llc`; the image has no LLVM, so both build tiers answer
# `error: build: native backend failed: opt (LLVM toolchain) not found`. That is a property of the
# environment, not of the program, and reporting it as a failure would be this harness crying
# wolf — the same reason a probe's `// linux:` line is a skip. It is reported as a skip, with the
# reason, and a skip is never counted as a pass.
have_llvm() { incmd 'command -v opt >/dev/null 2>&1'; }

cmd="${1:-}"; shift || true

case "$cmd" in
  shell)
    stage
    exec docker run --rm -it \
      -v "$STAGE":/work -v "$DHOME":/root \
      -v "$CACHE/$GOSSAMER":/gos -v "$CACHE/$TOOLCHAIN":/tool \
      -e DISPLAY=:99 -e GOSBOX=1 -e GOSSAMER_ROOT=/gos \
      -e PATH=/usr/local/cargo/bin:/tool/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
      -e CARGO_HOME=/usr/local/cargo -e RUSTUP_HOME=/usr/local/rustup \
      -w /work "$IMAGE" bash -l
    ;;

  check)
    [ $# -ge 1 ] || die "check needs a program"
    stage
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
    # Two of the five do not carry as far as the others, and this says so rather than pretending:
    # `check` is judged by whether it compiled (it prints `check: ok (N items typed)`, not the
    # running program's `OK `), and both build tiers are **skipped** because this image cannot link
    # a GPUI binary at all. What the container is *for* — running a window headlessly, which is the
    # thing the Mac may not do in bulk — is the two run tiers, and they are judged exactly as
    # `scripts/probes.sh` judges a probe.
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
      # A compile tier needs LLVM, and this image has none: skip it by name, with the reason,
      # rather than report the image's own gap as the program's failure. Skipped is never counted
      # as a pass, and both build tiers are witnessed on the Mac, headless (`gos build` compiles
      # and links; it does not run, so no window opens) — see the register's "The container".
      #
      # Measured, not assumed: installing llvm-22 into this image gets the compile further and the
      # *link* then fails on GPUI's Linux libraries (`undefined reference to xkb_state_unref` and
      # friends), because `tools/cc-gpui.sh` — the wrapper that supplies them — lives under
      # `tools/`, which `stage()` excludes. So the container would need both LLVM and a staged
      # Linux shim, and until it has them this tier is not witnessable here.
      if [ "$kind" = compile ] && [ "$t" != check ] && [ "$llvm" = 0 ]; then
        echo "skip   $t: this image cannot link a GPUI binary — no LLVM (\`opt\`/\`llc\`) and \`tools/\` is not staged, so \`cc-gpui.sh\` is absent; witnessed headless on the Mac instead"
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
        echo "ok     $t: $(grep -m1 '^OK ' "$log" | cut -c1-100)"
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
      linux_line="$(sed -n '1,20p' "$p" | sed -n 's|^// linux: *||p' | head -1)"
      if [ -n "$linux_line" ]; then
        echo "skip   $name: $linux_line"
        skipped=$((skipped + 1)); continue
      fi
      run_one() {
        log="$(mktemp -t gosbox.XXXXXX)"
        incmd "timeout -k 5 $timeout_s gos run --main-thread '$p'" > "$log" 2>&1
        verdict "$log" "$noted" >/dev/null
      }
      if run_one; then
        echo "ok     $name: $(grep -m1 '^OK ' "$log" | cut -c1-100)"
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
