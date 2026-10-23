#!/usr/bin/env bash
#
# Check that all examples typecheck cleanly with `gos check`.
#
#   ./scripts/check-all.sh
#

set -euo pipefail

cd "$(dirname "$0")/.."

echo "Linting all sources..."
gos lint .
echo

echo "Typechecking all examples..."
failed=0
count=0

for f in examples/[0-9][0-9]_*.gos; do
  [ -f "$f" ] || continue
  count=$((count + 1))
  name="$(basename "$f")"
  if ! output=$(gos check "$f" 2>&1); then
    echo "  FAIL  $name"
    echo "$output" | sed 's/^/        /'
    failed=$((failed + 1))
  else
    echo "  ok    $name"
  fi
done

echo
if [ "$failed" -eq 0 ]; then
  echo "All $count examples passed gos check."
  exit 0
else
  echo "$failed example(s) failed."
  exit 1
fi
