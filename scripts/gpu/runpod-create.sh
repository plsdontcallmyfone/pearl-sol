#!/usr/bin/env bash
# Create ONE RunPod GPU pod for the Cadre int7 worker (refuses if any pod already exists).
#   GPU_TYPES   preference list, default "NVIDIA A40" then 48 GB cards under the price cap
#   MAX_PRICE   $/hr cap (default 1.00)    CLOUD  SECURE (default) or COMMUNITY
#   IMAGE       default runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04
#   DISK_GB     container disk (default 120; no network volume: weights are re-downloaded)
# Writes .gpu/pod-id, .gpu/ssh-host, .gpu/ssh-port; ssh uses a throwaway key made in .gpu/.
source "$(dirname "$0")/runpod-lib.sh"
GPU_TYPES="${GPU_TYPES:-NVIDIA A40|NVIDIA RTX A6000|NVIDIA L40|NVIDIA RTX 6000 Ada Generation}"
MAX_PRICE="${MAX_PRICE:-1.00}"
CLOUD="${CLOUD:-SECURE}"
IMAGE="${IMAGE:-runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04}"
DISK_GB="${DISK_GB:-120}"
NAME="${NAME:-cadre-gpu-worker}"

n=$(account | jq_py 'len(d["data"]["myself"]["pods"])')
[ "$n" = 0 ] || { echo "refusing: $n pod(s) already exist; run runpod-stop.sh first"; spend_line; exit 1; }
[ -f "$GPU_STATE/id_ed25519" ] || ssh-keygen -q -t ed25519 -N '' -C cadre-gpu-throwaway -f "$GPU_STATE/id_ed25519"
PUB="$(cat "$GPU_STATE/id_ed25519.pub")"

IFS='|' read -ra TYPES <<< "$GPU_TYPES"
for t in "${TYPES[@]}"; do
  price=$(gql '{ gpuTypes(input:{id:"'"$t"'"}) { securePrice communityPrice } }' | jq_py 'd["data"]["gpuTypes"][0]["'"$( [ "$CLOUD" = SECURE ] && echo securePrice || echo communityPrice )"'"]')
  python3 -c "import sys; sys.exit(0 if 0 < float('$price') <= float('$MAX_PRICE') else 1)" || { echo "skip $t: \$$price/hr"; continue; }
  echo "trying $t ($CLOUD, \$$price/hr)"
  vars=$(python3 - "$t" "$PUB" <<PY
import json,sys
print(json.dumps({"input":{"cloudType":"$CLOUD","gpuCount":1,"gpuTypeId":sys.argv[1],"name":"$NAME","imageName":"$IMAGE",
  "containerDiskInGb":int("$DISK_GB"),"volumeInGb":0,"minVcpuCount":8,"minMemoryInGb":48,"ports":"22/tcp","supportPublicIp":True,
  "startSsh":True,"env":[{"key":"PUBLIC_KEY","value":sys.argv[2]}]}}))
PY
)
  r=$(gql 'mutation($input: PodFindAndDeployOnDemandInput) { podFindAndDeployOnDemand(input: $input) { id costPerHr machine { gpuDisplayName } } }' "$vars")
  id=$(echo "$r" | jq_py '(d.get("data") or {}).get("podFindAndDeployOnDemand") and d["data"]["podFindAndDeployOnDemand"]["id"] or ""')
  if [ -n "$id" ]; then
    echo "$id" > "$GPU_STATE/pod-id"; echo "$t" > "$GPU_STATE/gpu-type"
    echo "$r" | jq_py '"created: costPerHr=%s gpu=%s" % (d["data"]["podFindAndDeployOnDemand"]["costPerHr"], d["data"]["podFindAndDeployOnDemand"]["machine"]["gpuDisplayName"])'
    break
  fi
  echo "  not available: $(echo "$r" | jq_py 'd.get("errors",[{}])[0].get("message","?")')"
done
[ -n "$(pod_id)" ] || { echo "no pod created"; exit 1; }
spend_line

echo "waiting for ssh endpoint..."
for i in $(seq 1 60); do
  ep=$(gql '{ pod(input:{podId:"'"$(pod_id)"'"}) { runtime { uptimeInSeconds ports { ip isIpPublic privatePort publicPort type } } } }' \
    | jq_py '" ".join("%s %s" % (p["ip"], p["publicPort"]) for p in (((d["data"]["pod"] or {}).get("runtime") or {}).get("ports") or []) if p["privatePort"]==22 and p["isIpPublic"])')
  if [ -n "$ep" ]; then
    set -- $ep; echo "$1" > "$GPU_STATE/ssh-host"; echo "$2" > "$GPU_STATE/ssh-port"
    if ssh_pod true 2>/dev/null; then echo "ssh ready after $((i*10)) s"; exit 0; fi
  fi
  sleep 10
done
echo "ssh not ready after 600 s (pod left running: stop it with runpod-stop.sh)"; exit 1
