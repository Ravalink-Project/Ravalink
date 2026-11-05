#!/usr/bin/env bash
set -euo pipefail

checklist="${1:-RC1_ACCEPTANCE.md}"
if [[ ! -f "$checklist" ]]; then
  echo "RC1 acceptance checklist not found: $checklist" >&2
  exit 2
fi

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
tag_version="${GITHUB_REF_NAME:-}"
tag_version="${tag_version#v}"
base_tag_version="${tag_version%%-*}"
if [[ -n "${GITHUB_REF_NAME:-}" && "$base_tag_version" != "$workspace_version" ]]; then
  echo "RC1 tag ${GITHUB_REF_NAME} does not match workspace version $workspace_version." >&2
  exit 1
fi

unresolved="$(
  awk '
    /^## Automated pre-RC1 gates/ { in_gates = 1; next }
    /^## Post-RC1 real-world validation/ { in_gates = 0 }
    in_gates && /^- \[ \]/ { print NR ":" $0 }
  ' "$checklist"
)"

if [[ -n "$unresolved" ]]; then
  echo "RC1 blocked: automated pre-RC1 gates remain unchecked in $checklist:" >&2
  printf '%s\n' "$unresolved" >&2
  exit 1
fi

echo "RC1 automated acceptance gates passed."
