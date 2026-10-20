#!/usr/bin/env bash
# Shared helpers for scripts/gpu/runpod-*.sh. The API key is read from a file into a variable
# and only ever sent as the Authorization header; it is never printed or written elsewhere.
#   RUNPOD_KEY_FILE   default ~/.config/runpod/api-key
#   GPU_STATE         default <repo>/.gpu (gitignored): pod id, throwaway ssh key, ssh endpoint
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GPU_STATE="${GPU_STATE:-$REPO/.gpu}"
RUNPOD_KEY_FILE="${RUNPOD_KEY_FILE:-$HOME/.config/runpod/api-key}"
mkdir -p "$GPU_STATE" && chmod 700 "$GPU_STATE"

# gql '<query>' [variables-json]  -> JSON reply on stdout
gql() {
  local key body vars="${2:-}"
  [ -n "$vars" ] || vars='{}'
  key="$(tr -d '[:space:]' < "$RUNPOD_KEY_FILE")"
  body="$(python3 -c 'import json,sys;print(json.dumps({"query":sys.argv[1],"variables":json.loads(sys.argv[2])}))' "$1" "$vars")"
  curl -sS --max-time 60 https://api.runpod.io/graphql -H "Authorization: Bearer $key" -H 'Content-Type: application/json' -d "$body"
}

jq_py() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }

account() {
  gql '{ myself { clientBalance currentSpendPerHr pods { id name desiredStatus costPerHr gpuCount machine { gpuDisplayName } } } }'
}

spend_line() {
  account | jq_py '"balance=%s spend_per_hr=%s pods=%s" % (d["data"]["myself"]["clientBalance"], d["data"]["myself"]["currentSpendPerHr"], [(p["name"], p["desiredStatus"], p["costPerHr"]) for p in d["data"]["myself"]["pods"]])'
}

pod_id() { cat "$GPU_STATE/pod-id" 2>/dev/null || true; }

ssh_pod() {
  local host port
  host="$(cat "$GPU_STATE/ssh-host")"; port="$(cat "$GPU_STATE/ssh-port")"
  ssh -i "$GPU_STATE/id_ed25519" -o StrictHostKeyChecking=no -o UserKnownHostsFile="$GPU_STATE/known_hosts" \
      -o ServerAliveInterval=30 -o ConnectTimeout=20 -p "$port" "root@$host" "$@"
}

scp_pod() { # scp_pod <src...> <dst-on-pod>
  local host port n; host="$(cat "$GPU_STATE/ssh-host")"; port="$(cat "$GPU_STATE/ssh-port")"
  n=$#; local dst="${!n}"; set -- "${@:1:$((n-1))}"
  scp -i "$GPU_STATE/id_ed25519" -o StrictHostKeyChecking=no -o UserKnownHostsFile="$GPU_STATE/known_hosts" -P "$port" -r "$@" "root@$host:$dst"
}
