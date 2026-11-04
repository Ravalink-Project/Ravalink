#!/usr/bin/env bash
set -Eeuo pipefail

binary="${1:-target/release/ravalink}"
[[ -x "$binary" ]] || { echo "Binary not found or not executable: $binary" >&2; exit 1; }

port="${RAVALINK_SMOKE_PORT:-23391}"
password="ravalink-smoke-test-password"
log_file="$(mktemp)"
pid=""

cleanup() {
  if [[ -n "$pid" ]]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -f "$log_file"
}
trap cleanup EXIT

RAVALINK_ADDRESS=127.0.0.1 \
RAVALINK_PORT="$port" \
RAVALINK_PASSWORD="$password" \
RAVALINK_LOG_LEVEL=debug \
"$binary" >"$log_file" 2>&1 &
pid=$!

base="http://127.0.0.1:$port"
ready=0
for _ in $(seq 1 40); do
  if curl --silent --fail --header "Authorization: $password" "$base/version" >/dev/null; then
    ready=1
    break
  fi
  if ! kill -0 "$pid" 2>/dev/null; then
    cat "$log_file" >&2
    exit 1
  fi
  sleep 0.25
done
if [[ "$ready" != 1 ]]; then
  cat "$log_file" >&2
  echo "RavaLink did not become ready in time." >&2
  exit 1
fi

version="$(curl --silent --show-error --fail --header "Authorization: $password" "$base/version")"
[[ "$version" == "4.0.0" ]] || { echo "Unexpected /version response: $version" >&2; exit 1; }

unauthorized_status="$(curl --silent --output /dev/null --write-out '%{http_code}' "$base/version")"
[[ "$unauthorized_status" == "401" ]] || { echo "Expected unauthenticated /version to return 401, got $unauthorized_status" >&2; exit 1; }

info="$(curl --silent --show-error --fail --header "Authorization: $password" "$base/v4/info")"
grep -q '"sourceManagers"' <<< "$info" || { echo "The /v4/info response is missing sourceManagers." >&2; exit 1; }

stats="$(curl --silent --show-error --fail --header "Authorization: $password" "$base/v4/stats")"
grep -q '"uptime"' <<< "$stats" || { echo "The /v4/stats response is missing uptime." >&2; exit 1; }

echo "RavaLink API smoke test passed."
