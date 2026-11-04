#!/usr/bin/env bash
set -euo pipefail

ledger="${1:-COMPATIBILITY.md}"
if [[ ! -f "$ledger" ]]; then
  echo "Compatibility ledger not found: $ledger" >&2
  exit 2
fi

unresolved="$(
  awk -F'|' '
    /^\|/ && $1 == "" {
      status = tolower($4)
      if (status ~ /(pending|initial implementation|scaffold|not implemented|not yet|still need|still require|acceptance tests require)/) {
        print NR ":" $0
      }
    }
  ' "$ledger"
)"

if [[ -n "$unresolved" ]]; then
  echo "Stable release blocked: unresolved compatibility rows remain in $ledger:" >&2
  printf '%s\n' "$unresolved" >&2
  exit 1
fi

echo "Compatibility ledger contains no known scaffold or pending rows."
