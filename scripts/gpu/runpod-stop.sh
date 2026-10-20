#!/usr/bin/env bash
# Terminate the pod (default) or only stop it (STOP_ONLY=1), then show the account's spend.
# With ALL=1 it terminates every pod on the account.
source "$(dirname "$0")/runpod-lib.sh"
ids="$(pod_id)"
[ "${ALL:-0}" = 1 ] && ids=$(account | jq_py '" ".join(p["id"] for p in d["data"]["myself"]["pods"])')
for id in $ids; do
  if [ "${STOP_ONLY:-0}" = 1 ]; then
    gql 'mutation { podStop(input:{podId:"'"$id"'"}) { id desiredStatus } }' >/dev/null && echo "stopped"
  else
    gql 'mutation { podTerminate(input:{podId:"'"$id"'"}) }' >/dev/null && echo "terminated"
    rm -f "$GPU_STATE/pod-id" "$GPU_STATE/ssh-host" "$GPU_STATE/ssh-port"
  fi
done
sleep 5
spend_line
