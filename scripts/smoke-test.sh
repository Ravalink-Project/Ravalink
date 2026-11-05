#!/usr/bin/env bash
set -Eeuo pipefail

binary="${1:-target/release/ravalink}"
[[ -x "$binary" ]] || { echo "Binary not found or not executable: $binary" >&2; exit 1; }

port="${RAVALINK_SMOKE_PORT:-23391}"
password="ravalink-smoke-test-password"
log_file="$(mktemp)"
tmpdir="$(mktemp -d)"
pid=""

cleanup() {
  if [[ -n "$pid" ]]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -f "$log_file"
  rm -rf "$tmpdir"
}
trap cleanup EXIT

RAVALINK_ADDRESS=127.0.0.1 \
RAVALINK_PORT="$port" \
RAVALINK_PASSWORD="$password" \
RAVALINK_LOG_LEVEL=debug \
"$binary" >"$log_file" 2>&1 &
pid=$!

base="http://127.0.0.1:$port"
auth=(-H "Authorization: $password")
ready=0
for _ in $(seq 1 40); do
  if curl --silent --fail "${auth[@]}" "$base/version" >/dev/null; then
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

assert_status() {
  local expected="$1"
  shift
  local actual
  actual="$(curl --silent --show-error --output /tmp/ravalink-smoke-response --write-out '%{http_code}' "$@")"
  if [[ "$actual" != "$expected" ]]; then
    echo "Expected HTTP $expected, got $actual for: $*" >&2
    cat /tmp/ravalink-smoke-response >&2 || true
    exit 1
  fi
}

version="$(curl --silent --show-error --fail "${auth[@]}" "$base/version")"
[[ "$version" == "4.0.0" ]] || { echo "Unexpected /version response: $version" >&2; exit 1; }

unauthorized_status="$(curl --silent --output /dev/null --write-out '%{http_code}' "$base/version")"
[[ "$unauthorized_status" == "401" ]] || { echo "Expected unauthenticated /version to return 401, got $unauthorized_status" >&2; exit 1; }

info="$(curl --silent --show-error --fail "${auth[@]}" "$base/v4/info")"
for field in '"version"' '"buildTime"' '"git"' '"jvm"' '"lavaplayer"' '"sourceManagers"' '"filters"' '"plugins"'; do
  grep -q "$field" <<< "$info" || { echo "The /v4/info response is missing $field." >&2; exit 1; }
done

stats="$(curl --silent --show-error --fail "${auth[@]}" "$base/v4/stats")"
for field in '"players"' '"playingPlayers"' '"uptime"' '"memory"' '"cpu"'; do
  grep -q "$field" <<< "$stats" || { echo "The /v4/stats response is missing $field." >&2; exit 1; }
done
if grep -q '"frameStats"' <<< "$stats"; then
  echo "Lavalink v4 /v4/stats must omit frameStats." >&2
  exit 1
fi

# Route planner disabled-state contract.
assert_status 204 "${auth[@]}" "$base/v4/routeplanner/status"
assert_status 500 -X POST -H "Content-Type: application/json" "${auth[@]}" \
  --data '{"address":"203.0.113.1"}' "$base/v4/routeplanner/free/address"
assert_status 500 -X POST "${auth[@]}" "$base/v4/routeplanner/free/all"

# Decode endpoints must return Lavalink-shaped client errors for missing input.
assert_status 400 "${auth[@]}" "$base/v4/decodetrack"
assert_status 400 -X POST -H "Content-Type: application/json" "${auth[@]}" \
  --data '[]' "$base/v4/decodetracks"

# Unsupported identifiers resolve to the Lavalink empty-result shape.
load_result="$(curl --silent --show-error --fail "${auth[@]}" \
  --get --data-urlencode "identifier=ravalink-smoke-unsupported:track" "$base/v4/loadtracks")"
grep -q '"loadType":"empty"' <<< "$load_result" || {
  echo "Unsupported source did not return loadType=empty: $load_result" >&2
  exit 1
}
grep -q '"data":null' <<< "$load_result" || {
  echo "Empty load result did not return data=null: $load_result" >&2
  exit 1
}

# Missing sessions must serialize the standard Lavalink API error object.
assert_status 404 "${auth[@]}" "$base/v4/sessions/ravalink-smoke-missing/players"
grep -q '"status":404' /tmp/ravalink-smoke-response
grep -q '"error":"Not Found"' /tmp/ravalink-smoke-response
grep -q '"path":"/v4/sessions/ravalink-smoke-missing/players"' /tmp/ravalink-smoke-response

# Disabled metrics are not exposed.
assert_status 404 "$base/metrics"

rm -f /tmp/ravalink-smoke-response
echo "RavaLink API smoke test passed."

# Exercise an actual supported Lavalink client against the running server.
python3 -m venv "$tmpdir/wavelink-venv"
"$tmpdir/wavelink-venv/bin/python" -m pip install --disable-pip-version-check --quiet "wavelink==3.5.2"
"$tmpdir/wavelink-venv/bin/python" scripts/wavelink-smoke.py "http://127.0.0.1:$port" "$password"

# Exercise the real Shoukaku 4.3.0 Node.js client as well.
npm install --prefix "$tmpdir/shoukaku-venv" --no-audit --no-fund --silent "shoukaku@4.3.0"
cp scripts/shoukaku-smoke.mjs "$tmpdir/shoukaku-venv/shoukaku-smoke.mjs"
node "$tmpdir/shoukaku-venv/shoukaku-smoke.mjs" "http://127.0.0.1:$port" "$password"
