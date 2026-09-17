#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
failed=0
while IFS= read -r -d '' file; do
  case "$file" in
    *.md|Cargo.lock|package-lock.json) continue ;;
  esac
  [ -f "$root/$file" ] || continue
  lines=$(awk 'END {print NR}' "$root/$file")
  if [ "$lines" -ge 1000 ]; then
    printf '%s has %s lines (maximum 999)\n' "$file" "$lines" >&2
    failed=1
  fi
done < <(git -C "$root" ls-files --cached --others --exclude-standard -z)
exit "$failed"
