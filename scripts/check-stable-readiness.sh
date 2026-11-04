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

if [[ -n "${GITHUB_REF_NAME:-}" ]]; then
  workspace_version="$(
    awk '
      /^\[workspace\.package\]/ { in_package = 1; next }
      /^\[/ { in_package = 0 }
      in_package && /^version[[:space:]]*=/ {
        gsub(/"/, "", $3)
        print $3
        exit
      }
    ' Cargo.toml
  )"
  tag_version="${GITHUB_REF_NAME#v}"
  if [[ -z "$workspace_version" || "$tag_version" != "$workspace_version" ]]; then
    echo "Stable release blocked: tag ${GITHUB_REF_NAME} does not match workspace version ${workspace_version:-unknown}." >&2
    exit 1
  fi
fi

echo "Compatibility ledger and release version checks passed."
