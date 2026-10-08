#!/usr/bin/env bash
#
# pair-census.sh -- the deviation census for Goal B, and the check that it is
# still true.
#
#   ./scripts/pair-census.sh            # the table, and a pass/fail on the check
#   ./scripts/pair-census.sh --list     # ... and every row, one line each
#   ./scripts/pair-census.sh --pair 22_text.gos
#
# Goal B says that every documented `Deviations:` bullet in `examples/NN_*.gos`
# must be classified, and every `visible` one driven to zero or proved `forced`.
# The classification lives in `tests/evidence/pairs/census.tsv`; this script
# reads the examples themselves and reports any bullet with no row, and any row
# whose line does not begin a deviation.
#
# Exit 0 when the table and the examples agree. Exit 1 when they do not.
#
# No window is opened, no build is run, and no network is touched. This is a
# text comparison and may be run at any time -- it is deliberately not behind
# `scripts/window-guard.sh`.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 2

table="tests/evidence/pairs/census.tsv"
[ -f "$table" ] || { echo "pair-census: no $table" >&2; exit 2; }

list=0
only=""
while [ $# -gt 0 ]; do
  case "$1" in
    --list) list=1 ;;
    --pair) only="${2:-}"; shift ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "pair-census: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done

tmp=$(mktemp -d) || exit 2
trap 'rm -rf "$tmp"' EXIT

# --- what the examples actually say -----------------------------------------
#
# A deviation is:
#   * a `*` bullet inside a section introduced by a line mentioning "deviation"
#     (the headings are not uniform -- "// Deviations:", "// Two deviations, both
#     on the binding's side:", "// Deviations - the list is the point of this
#     file:", ... ), or
#   * a singular prose `// Deviation: ...` line, which files 10 and 23 use.
#
# `///` doc comments are never deviations; several of them *refer* back to one.
find_lines () {
  local f="$1"
  awk '
    /^\/\/ *Deviation:/ { print NR; next }
    /^\/\/.*[Dd]eviation/ && substr($0,1,3)!="///" && $0 !~ /\* / { on=1; next }
    on && substr($0,1,3)=="///" { next }
    on && /^\/\/[[:space:]]*\*[[:space:]]/ { print NR; next }
    on && substr($0,1,2)!="//" { on=0 }
  ' "$f"
}

: > "$tmp/found"
: > "$tmp/heads"
for f in examples/[0-9][0-9]_*.gos; do
  b=$(basename "$f")
  grep -q '^//.*[Dd]eviation' "$f" && echo "$b" >> "$tmp/heads"
  find_lines "$f" | while read -r ln; do printf '%s\t%s\n' "$b" "$ln"; done >> "$tmp/found"
done
sort -o "$tmp/found" "$tmp/found"

# --- what the table claims ---------------------------------------------------
awk -F'\t' '!/^#/ && NF>=4 { print $1 "\t" $2 }' "$table" | sort > "$tmp/claimed"

# --- the comparison ----------------------------------------------------------
comm -23 "$tmp/found" "$tmp/claimed" > "$tmp/missing"   # in the examples, not in the table
comm -13 "$tmp/found" "$tmp/claimed" > "$tmp/extra"     # in the table, not in the examples

# --- the table ---------------------------------------------------------------
awk -F'\t' -v only="$only" -v list="$list" '
  !/^#/ && NF>=4 {
    pair=$1; cls=$3
    if (only!="" && pair!=only) next
    n[pair]++
    c[pair FS cls]++
    if (list) printf "%-28s %5s  %-9s %s\n", pair, $2, cls, $4
  }
  END {
    if (list) print ""
    printf "%-30s %5s %7s %8s %7s %7s\n", "pair", "devs", "visible", "forced", "hidden", "state"
    printf "%-30s %5s %7s %8s %7s %7s\n", "------------------------------", "-----", "-------", "--------", "-------", "-------"
    total=0; tv=0; tf=0; ti=0
    for (p in n) {
      v=c[p FS "visible"]+0; f=c[p FS "forced"]+0; i=c[p FS "invisible"]+0
      # Goal B is met for a pair when nothing visible is left unproved.
      state = (v>0 && f>0) ? "open (mixed)" : (v>0 ? "OPEN" : (f>0 ? "forced only" : "equal"))
      printf "%-30s %5d %7d %8d %7d %7s\n", p, n[p], v, f, i, state
      total+=n[p]; tv+=v; tf+=f; ti+=i
    }
    printf "%-30s %5s %7s %8s %7s %7s\n", "------------------------------", "-----", "-------", "--------", "-------", "-------"
    printf "%-30s %5d %7d %8d %7d\n", "total", total, tv, tf, ti
    printf "\nvisible %d, forced %d, invisible %d, devs %d\n", tv, tf, ti, total
  }
' "$table" > "$tmp/summary"

cat "$tmp/summary"

# --- verdict -----------------------------------------------------------------
echo
echo "examples carrying a deviation section: $(sort -u "$tmp/heads" | wc -l | tr -d ' ')"
echo "deviation anchors found in the examples: $(wc -l < "$tmp/found" | tr -d ' ')"

status=0
if [ -s "$tmp/missing" ]; then
  status=1
  echo
  echo "IN THE EXAMPLES, NOT IN THE TABLE:"
  while IFS=$'\t' read -r f l; do
    echo "  $f:$l"
    sed -n "${l}p" "examples/$f" | sed 's/^/      /'
  done < "$tmp/missing"
fi
if [ -s "$tmp/extra" ]; then
  status=1
  echo
  echo "IN THE TABLE, NOT IN THE EXAMPLES:"
  while IFS=$'\t' read -r f l; do
    echo "  $f:$l"
    [ -f "examples/$f" ] && sed -n "${l}p" "examples/$f" | sed 's/^/      /' || echo "      (no such file)"
  done < "$tmp/extra"
fi

echo
if [ "$status" = 0 ]; then
  echo "pair-census: table and examples agree -- every documented deviation is classified"
else
  echo "pair-census: table and examples DISAGREE -- see above"
fi
exit "$status"
