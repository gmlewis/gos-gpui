#!/usr/bin/env bash
#
# Does the documentation still describe the binding?
#
#   ./scripts/docs.sh            # check this tree
#   ./scripts/docs.sh <root>     # check another checkout — e.g. a pristine one
#
# MR7 of the register is a row about drift: a doc comment or a README that
# describes behaviour the binding no longer has, or never had. It cannot be a
# `#[test]` (nothing runs) and it cannot be a GUI probe (nothing is drawn), so it
# is this: a list of the specific claims that were wrong, each turned into a
# check, so that the next person to make one wrong cannot do it quietly.
#
# Every check names the claim it is defending. Exit status is non-zero if any of
# them is stale.

set -uo pipefail

root="${1:-}"
if [ -z "$root" ]; then
  root="$(cd "$(dirname "$0")/.." && pwd)"
fi
cd "$root"

if [ ! -f gpui-binding/src/lib.rs ]; then
  echo "docs: no binding at $root" >&2
  exit 2
fi

fails=0
total=0

ok() {
  total=$((total + 1))
  echo "  ok     $1"
}

stale() {
  total=$((total + 1))
  fails=$((fails + 1))
  echo "  STALE  $1"
}

# 1. The painters `view.rs` implements are the painters `lib.rs` documents.
#
#    `paint()` grew a fourth one — `prism`, reproducing `gradient.rs`'s
#    trapezoid — and the doc, and the README, went on saying three.
painters="$(sed -n '/^fn paint(/,/^}$/p' gpui-binding/src/view.rs \
  | sed -n 's/^        "\([a-z]*\)" =>.*/\1/p' | sort -u)"
drawdoc="$(sed -n '/A directly painted surface/,/fn draw(/p' gpui-binding/src/lib.rs)"
missing=""
for painter in $painters; do
  printf '%s' "$drawdoc" | grep -q "\`\"$painter\"\`" || missing="$missing $painter"
done
if [ -z "$missing" ]; then
  ok "every painter paint() implements is named in draw()'s doc ($(echo "$painters" | tr '\n' ' '))"
else
  stale "draw()'s doc does not name:$missing"
fi

# 2. `store.rs` no longer claims the process has exactly one window.
#
#    It carried that claim in its module doc while `lib.rs`, `examples/25` and
#    `examples/27` opened many.
if grep -q "exactly one window" gpui-binding/src/store.rs; then
  stale "store.rs still says the process has exactly one window"
else
  ok "store.rs no longer claims a single window"
fi

# 3. The undocumented checkbox tick key is documented.
#
#    `view.rs` reads `"<id>#tick"` out of the store to colour a checkbox's tick,
#    and nothing said so.
if grep -q '#tick' gpui-binding/src/lib.rs; then
  ok "the \"<id>#tick\" checkbox key is documented"
else
  stale "the \"<id>#tick\" checkbox key is undocumented in lib.rs"
fi

# 4. `is_focused` admits what it cannot see.
#
#    It reports only the handles this binding builds. An element made focusable
#    with the `focusable` token has a handle GPUI builds internally and never
#    hands over, so it is invisible here — which the doc did not say.
if sed -n '/Whether the element with this id/,/^    fn is_focused/p' gpui-binding/src/lib.rs \
     | grep -q 'focusable'; then
  ok "is_focused()'s doc names the focusable-element gap"
else
  stale "is_focused()'s doc does not mention that a focusable element is invisible to it"
fi

# 5. `observe_keystrokes` promises the keys an element handled, and says how.
#
#    This claim used to be the opposite — that the doc *admits* the keys it misses —
#    and it was accurate while the observer was `cx.observe_keystrokes`, which gpui
#    skips the moment an element stops propagation. KM5 changed the mechanism to a
#    capture-phase listener on the window root, so the doc now promises every
#    keystroke and has to name the phase that makes that true.
if sed -n '/Raise keystrokes as/,/^    fn observe_keystrokes/p' gpui-binding/src/lib.rs \
     | sed 's|///||g' | tr '\n' ' ' | tr -s ' ' | grep -q 'capture'; then
  ok "observe_keystrokes()'s doc promises the keys an element handled, and names the capture phase"
else
  stale "observe_keystrokes()'s doc does not say that keys an element handled arrive, or how"
fi

# 6. `set_theme` chooses a palette, and says what it does *not* touch.
#
#    This claim used to be the opposite — that the doc admits the name is inert —
#    and it was accurate while nothing consulted it. TH3 wired it to the named colour
#    table, so the doc now has to name the three choices and say that the OS
#    appearance is left alone: a program that reads `theme()` and expects
#    `appearance()` to follow it has the wrong idea of both.
theme_doc="$(sed -n '/Choose which of the platform/,/^    fn theme()/p' gpui-binding/src/lib.rs \
  | sed 's|///||g' | tr '\n' ' ' | tr -s ' ')"
if printf '%s' "$theme_doc" | grep -q '"system"' \
   && printf '%s' "$theme_doc" | grep -qi 'palette' \
   && printf '%s' "$theme_doc" | grep -qi 'appearance.*still reports\|still reports what the'; then
  ok "set_theme()'s doc names the palette it chooses and says the OS appearance is untouched"
else
  stale "set_theme()'s doc does not name the palette it chooses, or does not say the appearance is untouched"
fi

# ---------------------------------------------------------------------------
# The README's names, against the binding's. MR7's own doubt says the check
# covers `lib.rs` and `store.rs` and not the README — this closes that gap.
# ---------------------------------------------------------------------------
#
# Every `gpui::name` the README writes has to be a function the binding actually
# registers, or a name the line says belongs to gpui itself. Two were wrong when
# this check was written: the README offered `gpui::styled_text` for a binding whose
# function is `gpui::text_styled` (so a program copying the name gets an unresolved
# symbol), and it offered `gpui::gpu_specs()` as though the binding had one, when
# that is gpui's own `Window::gpu_specs`.
#
# Upstream names are allowed by this list, and each has to say in the README why it
# is there — the line has to be about gpui's own API rather than about this binding's.
# Empty on purpose. The first version of this check needed one entry — the README wrote
# `gpui::gpu_specs()` for gpui's own `Window::gpu_specs` — and rather than keep the exception,
# the README was rewritten to say whose method it is. A line that genuinely needs an entry has
# to earn it in the README first.
readme_upstream=""

bound_names="$(sed -n '/register_module!/,/^);/p' gpui-binding/src/lib.rs \
  | grep -oE '^    (fn|cb_fn) [a-z_0-9]+' | awk '{print $NF}' | sort -u)"

readme_bad=0
for name in $(grep -oE 'gpui::[a-z_0-9]+' README.md | sed 's/^gpui:://' | sort -u); do
  if printf '%s\n' "$bound_names" | grep -qx "$name"; then
    continue
  fi
  if [ -n "$readme_upstream" ] && printf '%s\n' "$readme_upstream" | grep -qx "$name"; then
    continue
  fi
  echo "  STALE README names gpui::$name, which the binding does not register" >&2
  readme_bad=$((readme_bad + 1))
done

if [ "$readme_bad" -eq 0 ]; then
  ok "every gpui:: name the README writes is one the binding registers"
else
  stale "$readme_bad name(s) in the README are not binding functions"
fi

# ---------------------------------------------------------------------------
# Every function the binding registers documents itself.
# ---------------------------------------------------------------------------
#
# The register's house style is that a binding function says what it is for, what it
# answers, and what it does not cover — a program reads these docs and nothing else.
# This holds the whole surface to that, so a function added in a hurry cannot be the
# one that has none.
#
# **The count is checked as well as the list.** The first version of this awk had a
# stray character in it, died at parse time, and printed nothing — which the check
# read as "no undocumented functions" and passed. An instrument that fails silently
# reads exactly like a clean surface, so a run that examined no functions at all is a
# failure here, not a pass.
doc_scan="$(sed -n '/register_module!/,/^);/p' gpui-binding/src/lib.rs | awk '
  /^    (fn|cb_fn) [a-z_0-9]+/ { total++; if (prev !~ /^ *\/\/\//) { n = $2; sub(/\(.*/, "", n); print "UNDOC " n } next }
  /^ *$/ { next }
  { prev = $0 }
  END { print "TOTAL " (total + 0) }
' 2>&1)"

doc_total="$(printf '%s\n' "$doc_scan" | sed -n 's/^TOTAL //p')"
doc_undoc="$(printf '%s\n' "$doc_scan" | sed -n 's/^UNDOC //p' | tr '\n' ' ')"

if [ -z "$doc_total" ] || [ "$doc_total" -lt 100 ]; then
  stale "the doc-comment scan examined ${doc_total:-no} function(s) — it is broken, not clean"
elif [ -n "$doc_undoc" ]; then
  stale "these registered functions have no doc comment: $doc_undoc"
else
  ok "every registered binding function carries its own doc comment ($doc_total of them)"
fi

echo "docs: $((total - fails))/$total claims still accurate"
[ "$fails" -eq 0 ] || exit 1
