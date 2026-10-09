#!/usr/bin/env bash
#
# Does the documentation still describe the binding?
#
#   ./scripts/docs.sh            # check this tree
#   ./scripts/docs.sh <root>     # check another checkout — e.g. a pristine one
#
# Drift is the subject: a doc comment or a README that describes behaviour the
# binding does not have. It cannot be a `#[test]` (nothing runs) and it cannot be
# a GUI probe (nothing is drawn), so it is this: a list of the claims the
# documentation makes, each paired with the check that would catch it going
# wrong, so that a claim and the code cannot part company quietly.
#
# Every check names the claim it is defending. Exit status is non-zero if any of
# them is stale.

set -uo pipefail

# `grep X >/dev/null`, never `grep -q X`, wherever grep reads a pipe. `-q` exits
# at the first match and the writer then dies of SIGPIPE; under `pipefail` that
# failure becomes the pipeline's status, so the check reads a match as a miss —
# intermittently, by timing: a name can read as unregistered on one run and fine
# on the next.

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
#    `paint()` implements a fourth painter — `prism`, reproducing `gradient.rs`'s
#    trapezoid — so the doc and the README have to name all four.
painters="$(sed -n '/^fn paint(/,/^}$/p' gpui-binding/src/view.rs \
  | sed -n 's/^        "\([a-z]*\)" =>.*/\1/p' | sort -u)"
drawdoc="$(sed -n '/A directly painted surface/,/fn draw(/p' gpui-binding/src/lib.rs)"
missing=""
for painter in $painters; do
  printf '%s' "$drawdoc" | grep "\`\"$painter\"\`" >/dev/null || missing="$missing $painter"
done
if [ -z "$missing" ]; then
  ok "every painter paint() implements is named in draw()'s doc ($(echo "$painters" | tr '\n' ' '))"
else
  stale "draw()'s doc does not name:$missing"
fi

# 2. `store.rs` does not claim the process has exactly one window.
#
#    A single-window claim in its module doc is stale: `lib.rs`, `examples/25` and
#    `examples/27` open many.
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
     | grep 'focusable' >/dev/null; then
  ok "is_focused()'s doc names the focusable-element gap"
else
  stale "is_focused()'s doc does not mention that a focusable element is invisible to it"
fi

# 5. `observe_keystrokes` promises the keys an element handled, and says how.
#
#    The doc promises every keystroke an element handled, so it has to name the
#    phase that makes that true: the observer is a capture-phase listener on the
#    window root. `cx.observe_keystrokes` could not carry that promise — gpui skips
#    it the moment an element stops propagation.
if sed -n '/Raise keystrokes as/,/^    fn observe_keystrokes/p' gpui-binding/src/lib.rs \
     | sed 's|///||g' | tr '\n' ' ' | tr -s ' ' | grep 'capture' >/dev/null; then
  ok "observe_keystrokes()'s doc promises the keys an element handled, and names the capture phase"
else
  stale "observe_keystrokes()'s doc does not say that keys an element handled arrive, or how"
fi

# 6. `set_theme` chooses a palette, and says what it does *not* touch.
#
#    `set_theme` is wired to the named colour table, so the doc has to name the
#    three choices and say that the OS appearance is left alone: a program that
#    reads `theme()` and expects `appearance()` to follow it has the wrong idea of
#    both.
theme_doc="$(sed -n '/Choose which of the platform/,/^    fn theme()/p' gpui-binding/src/lib.rs \
  | sed 's|///||g' | tr '\n' ' ' | tr -s ' ')"
if printf '%s' "$theme_doc" | grep '"system"' >/dev/null \
   && printf '%s' "$theme_doc" | grep -i 'palette' >/dev/null \
   && printf '%s' "$theme_doc" | grep -i 'appearance.*still reports\|still reports what the' >/dev/null; then
  ok "set_theme()'s doc names the palette it chooses and says the OS appearance is untouched"
else
  stale "set_theme()'s doc does not name the palette it chooses, or does not say the appearance is untouched"
fi

# ---------------------------------------------------------------------------
# The README's names, against the binding's. The checks above cover `lib.rs` and
# `store.rs` and not the README — this closes that gap.
# ---------------------------------------------------------------------------
#
# Every `gpui::name` the README writes has to be a function the binding actually
# registers, or a name the line says belongs to gpui itself. The two mistakes this
# catches: a name the binding spells differently — `gpui::styled_text` for a function
# that is `gpui::text_styled`, so a program copying the name gets an unresolved
# symbol — and a name that is gpui's own, `gpui::gpu_specs()` for gpui's
# `Window::gpu_specs`.
#
# Upstream names are allowed by this list, and each has to say in the README why it
# is there — the line has to be about gpui's own API rather than about this binding's.
# Empty on purpose. The README says whose method an upstream name is rather than
# relying on an exception here. A line that genuinely needs an entry has to earn it
# in the README first.
readme_upstream=""

bound_names="$(sed -n '/register_module!/,/^);/p' gpui-binding/src/lib.rs \
  | grep -oE '^    (fn|cb_fn) [a-z_0-9]+' | awk '{print $NF}' | sort -u)"

readme_names="$(grep -oE 'gpui::[a-z_0-9]+' README.md | sed 's/^gpui:://' | sort -u)"

readme_bad=0
for name in $readme_names; do
  if printf '%s\n' "$bound_names" | grep -x "$name" >/dev/null; then
    continue
  fi
  if [ -n "$readme_upstream" ] && printf '%s\n' "$readme_upstream" | grep -x "$name" >/dev/null; then
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
# **The count is checked as well as the list.** An awk that dies at parse time
# prints nothing, and the check reads that as "no undocumented functions" and
# passes. An instrument that fails silently reads exactly like a clean surface, so a
# run that examined no functions at all is a failure here, not a pass.
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

# ---------------------------------------------------------------------------
# The reference accounts for the whole surface, not just the names it happens
# to mention.
# ---------------------------------------------------------------------------
#
# The README's tables name the functions a program reaches for. The ones they
# leave out are listed in the README as patterns, under `### Everything else the
# binding registers`, and this holds both directions to the binding: a registered
# function that is in no table and matches no pattern has no documented entry
# point at all, and a pattern that matches nothing is a leftover from a rename.
#
# The list lives in the README rather than in this script on purpose: a reader
# sees what the check sees, and an omission has to be written down where they
# will find it. A missing list is a failure here, not an empty pass.
exception_patterns="$(awk '
  /^### Everything else the binding registers/ { in_section = 1; next }
  in_section && /^```/ { fence++; next }
  in_section && fence == 1 { print }
  in_section && fence == 2 { exit }
' README.md | sed 's/#.*//; s/[[:space:]]//g' | sed '/^$/d')"

uncovered=""
for name in $(comm -23 <(printf '%s\n' "$bound_names") <(printf '%s\n' "$readme_names")); do
  hit=0
  while IFS= read -r pat; do
    [ -n "$pat" ] || continue
    case "$name" in $pat) hit=1; break ;; esac
  done <<EOF
$exception_patterns
EOF
  [ "$hit" = 1 ] || uncovered="$uncovered $name"
done

stale_patterns=""
while IFS= read -r pat; do
  [ -n "$pat" ] || continue
  hit=0
  for name in $bound_names; do
    case "$name" in $pat) hit=1; break ;; esac
  done
  [ "$hit" = 1 ] || stale_patterns="$stale_patterns $pat"
done <<EOF
$exception_patterns
EOF

bound_total="$(printf '%s\n' "$bound_names" | grep -c .)"
if [ -z "$exception_patterns" ]; then
  stale "the README has no \"Everything else the binding registers\" list, so functions in no table are undocumented by default ($bound_total registered)"
elif [ -n "$uncovered" ]; then
  stale "these registered functions are in no README table and match no pattern:$uncovered"
elif [ -n "$stale_patterns" ]; then
  stale "these README patterns match no registered function:$stale_patterns"
else
  ok "every one of the $bound_total registered functions is named in the README or covered by its omit-list"
fi

# ---------------------------------------------------------------------------
# Every path the README links to is a path in the tree.
# ---------------------------------------------------------------------------
#
# A link that resolves to nothing is the same wrong claim as a wrong name: it costs
# the reader the same time. `examples/ui/` is not in the tree — the chaining layer is
# `src/lib.gos` — and a `target/` path is not built in a fresh checkout.
bad_links=""
for target in $(grep -oE '\]\([^)]+\)' README.md | sed 's/^](//; s/)$//'); do
  case "$target" in
    \#*|http://*|https://*|mailto:*) continue ;;
  esac
  link_path="${target%%#*}"
  [ -n "$link_path" ] || continue
  [ -e "$link_path" ] || bad_links="$bad_links $link_path"
done

if [ -z "$bad_links" ]; then
  ok "every path the README links to is in the tree"
else
  stale "the README links to path(s) that are not in the tree:$bad_links"
fi

echo "docs: $((total - fails))/$total claims still accurate"
[ "$fails" -eq 0 ] || exit 1
