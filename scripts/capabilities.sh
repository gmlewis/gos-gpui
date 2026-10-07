#!/usr/bin/env bash
#
# Read CAPABILITIES.md and report the state of the register.
#
#   ./scripts/capabilities.sh          # summary + every non-terminal row
#   ./scripts/capabilities.sh --all    # the summary + the whole register
#
# Exit status is 0 only when every row of the register is terminal — `tested`
# (implemented, and a check that failed before the work and passes after) or
# `unreachable` (documented, with evidence). Anything else is a row still in
# flight, and the run is not complete while one exists.
#
# The table this reads is the one in CAPABILITIES.md: one row per capability,
# columns `id | capability | status | test | failure seen before the fix | fix |
# tiers passed | residual doubt`. A row is a line whose first cell looks like an
# id from the register (`W1`, `SC4`, `DW13`, `MR7`, …); the header, the separator
# and any prose row are skipped.

set -uo pipefail

cd "$(dirname "$0")/.."

ledger=CAPABILITIES.md
if [ ! -f "$ledger" ]; then
  echo "capabilities: no $ledger" >&2
  exit 2
fi

show_all=0
[ "${1:-}" = "--all" ] && show_all=1

# The terminal statuses, and the whole vocabulary the ledger is allowed to use.
terminal_re='^(tested|unreachable)$'
known_re='^(todo|test-written|failing-recorded|implemented|tested|unreachable)$'

total=0
tested=0
unreachable=0
open=0
unknown=0
missing_paths=0
no_tiers=0
no_evidence=0
checked_paths=0
checked_tiers=0

open_rows=()
all_rows=()

while IFS= read -r line; do
  # Only table rows: a leading pipe, then an id cell.
  case "$line" in
    '|'*) ;;
    *) continue ;;
  esac

  # Split on pipes. Cells 1..n, with the leading and trailing pipe as empty
  # fields, so the id is field 2 and the status field 4.
  IFS='|' read -r -a cells <<<"$line"
  id="$(printf '%s' "${cells[1]:-}" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
  status="$(printf '%s' "${cells[3]:-}" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"

  case "$id" in
    ''|'id'|'-'*) continue ;;
    W[0-9]*|SC[0-9]*|TM[0-9]*|CL[0-9]*|KM[0-9]*|TH[0-9]*|WH[0-9]*|BG[0-9]*|MN[0-9]*|DD[0-9]*|DW[0-9]*|AX[0-9]*|MR[0-9]*) ;;
    *) continue ;;
  esac

  total=$((total + 1))
  all_rows+=("$(printf '%-6s %-12s %s' "$id" "$status" "$(printf '%s' "${cells[2]:-}" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')" | cut -c1-110)")

  if ! printf '%s' "$status" | grep -Eq "$known_re"; then
    unknown=$((unknown + 1))
    open=$((open + 1))
    open_rows+=("$(printf '%-6s %-12s %s' "$id" "$status" "$(printf '%s' "${cells[2]:-}" | cut -c1-90)")")
    continue
  fi

  if printf '%s' "$status" | grep -Eq "$terminal_re"; then
    case "$status" in
      tested) tested=$((tested + 1)) ;;
      unreachable) unreachable=$((unreachable + 1)) ;;
    esac

    # The register's own rule: "`test` is a path that exists. Never write a row as tested
    # without one." Every path the test cell names has to be on disk, and a terminal row
    # has to name the tiers it passed — a row whose evidence nobody can follow is not a row.
    while IFS= read -r named; do
      [ -n "$named" ] || continue
      checked_paths=$((checked_paths + 1))
      if [ ! -e "$named" ]; then
        echo "  MISSING TEST PATH on $id: $named" >&2
        missing_paths=$((missing_paths + 1))
      fi
    done < <(printf '%s' "${cells[4]:-}" | grep -oE '`[^`]+`' | tr -d '`' | grep -E '^(tests|scripts)/' || true)

    # **Only a `tested` row has tiers to name.** An `unreachable` row ran nothing, so its
    # `tiers passed` is `—` and that is correct — this check was first written for every terminal
    # row, reported nine false positives on the unreachable ones, and was narrowed by running it.
    # The lesson this run keeps relearning, this time about its own instrument.
    if [ "$status" = "tested" ]; then
      checked_tiers=$((checked_tiers + 1))
      tiers="$(printf '%s' "${cells[7]:-}" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
      if [ -z "$tiers" ] || [ "$tiers" = "—" ] || [ "$tiers" = "-" ]; then
        echo "  NO TIERS on $id: a tested row must name the tiers it passed" >&2
        no_tiers=$((no_tiers + 1))
      fi
    fi

    # Not a failure: three rows carry their evidence in one shared file and cite it in the
    # row. Counted so a reader knows which rows to read rather than grep for.
    if ! ls tests/evidence/ 2>/dev/null | grep -qi "^${id}\."; then
      no_evidence=$((no_evidence + 1))
    fi
  else
    open=$((open + 1))
    open_rows+=("$(printf '%-6s %-12s %s' "$id" "$status" "$(printf '%s' "${cells[2]:-}" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' | cut -c1-90)")")
  fi
done < "$ledger"

echo "register: $total rows"
echo "  tested       $tested"
echo "  unreachable  $unreachable"
echo "  not terminal $open"
# The register's central claim, checked rather than asserted — printed every run, so that
# "65 rows tested" is a number with something behind it.
echo "  checked      $checked_paths test path(s) named by terminal rows, all on disk"
echo "               $checked_tiers tested row(s) naming the tiers they passed"

if [ "$unknown" -gt 0 ]; then
  echo "  INVALID STATUS on $unknown row(s) — the vocabulary is todo, test-written," >&2
  echo "  failing-recorded, implemented, tested, unreachable" >&2
fi

# Every `unreachable` row has to be named in the `## Unreachable` section: the prompt asks that
# section to give, per row, the capability, the evidence and the change that would close it. A row
# marked unreachable with its evidence only in its own cell is a row a reader cannot find, which is
# exactly how MN5 and DD2 were missed until round 66 (MN5 had no entry; DD2 and its neighbours were
# hidden inside a "DD1–DD4" range).
section="$(awk '/^## Unreachable/,/^## Round log/' "$ledger")"
unnamed=0
while IFS= read -r id; do
  [ -n "$id" ] || continue
  if ! grep -qE "(^|[^A-Za-z0-9])${id}([^A-Za-z0-9]|$)" <<<"$section"; then
    echo "  UNNAMED UNREACHABLE $id: marked unreachable with no entry in ## Unreachable" >&2
    unnamed=$((unnamed + 1))
  fi
done < <(awk -F'|' '$4 ~ /unreachable/ {gsub(/ /, "", $2); print $2}' "$ledger")

if [ "$unnamed" -gt 0 ]; then
  echo "  $unnamed unreachable row(s) with no entry of their own in ## Unreachable" >&2
fi

# Terminal rows whose evidence file is not named after them — three rows share a file and cite it.
# The counter was written in round 63 and sat unused until round 67, which is MR7's own complaint
# about dead code, in this run's own tooling.
if [ "$no_evidence" -gt 0 ]; then
  echo "  evidence     $no_evidence terminal row(s) keep their evidence under another name:"
  while IFS= read -r id; do
    [ -n "$id" ] || continue
    if ls tests/evidence/ 2>/dev/null | grep -qi "^${id}\."; then continue; fi
    printf '               %s\n' "$id"
  done < <(awk -F'|' '{gsub(/ /, "", $4)} $4 ~ /^(tested|unreachable)$/ {gsub(/ /, "", $2); print $2}' "$ledger")
fi

# The round log accounts for every round: "one line per round". A *missing* round is a hole in the
# only record of what happened; a round that appears twice is not a failure — round 29 has two
# records, because a resumed session counted it once for its baseline and once for its capability,
# and this log is append-only on purpose (round 67).
log_section="$(awk '/^## Round log/,0' "$ledger")"
highest="$(printf '%s' "$log_section" | sed -n 's/^- \*\*Round \([0-9]*\).*/\1/p' | sort -n | tail -1)"
missing_rounds=0
if [ -n "$highest" ]; then
  for n in $(seq 1 "$highest"); do
    if ! grep -qE "^- \*\*Round ${n}([^0-9]|$)" <<<"$log_section"; then
      echo "  MISSING ROUND $n: the round log has no line for it" >&2
      missing_rounds=$((missing_rounds + 1))
    fi
  done
fi

if [ "$show_all" = 1 ]; then
  echo
  printf '%s\n' "${all_rows[@]}"
elif [ "$open" -gt 0 ]; then
  echo
  echo "not yet terminal:"
  printf '  %s\n' "${open_rows[@]}"
fi

if [ "$open" -gt 0 ] || [ "$unknown" -gt 0 ] || [ "$total" -ne 94 ] || [ "$missing_paths" -gt 0 ] || [ "$no_tiers" -gt 0 ] || [ "$unnamed" -gt 0 ] || [ "$missing_rounds" -gt 0 ]; then
  if [ "$total" -ne 94 ]; then
    echo >&2 "capabilities: expected 94 register rows, counted $total — a row was deleted or the table is malformed"
  fi
  exit 1
fi

echo "  every terminal row names a test that exists, the tiers it passed, and — if unreachable — an entry in ## Unreachable"
echo "  the round log has a line for every round 1..$highest"
echo
echo "every row is accounted for"
