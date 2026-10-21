#!/bin/bash
# Stream settings bench on a self-hosted desktop: one fresh container from selfhost/Containerfile at
# 1 vCPU / 1 GiB (E2B's desktop size), then bench-inner.sh inside it (one JSON line per setting).
# Runs on the box as root (podman); removes its container at the end. SLICE=desk-density.slice caps it.
#   streambench.sh "4:350:2:2 10:600:1:1 15:800:1:1 24:1200:1:1 30:1500:1:1" [SITE]
set -u
SETTINGS=${1:-"4:350:2:2 10:600:1:1 15:800:1:1 24:1200:1:1 30:1500:1:1"}
SITE=${2:-https://cadre.64-95-12-116.sslip.io}
HERE=$(cd "$(dirname "$0")" && pwd)
N=cadre-streambench-$$
podman run -d --name $N --label cadre_selfhost=1 --cpus "${CPUS:-1}" --memory 1024m --memory-swap 1024m --shm-size 256m --network slirp4netns ${SLICE:+--cgroup-parent $SLICE} localhost/cadre-desktop-selfhost:latest >/dev/null
trap 'podman rm -f -t 2 $N >/dev/null 2>&1' EXIT
for i in $(seq 60); do podman exec $N test -f /tmp/desk-ready && break; sleep 0.5; done
for f in ../sandbox/stream.sh latency-probe.sh rfbwatch.py rfbbytes.py bench-inner.sh; do podman cp "$HERE/$f" "$N:/tmp/$(basename $f)"; done
podman exec -u root $N chmod 755 /tmp/stream.sh /tmp/latency-probe.sh /tmp/bench-inner.sh
podman exec $N bash /tmp/bench-inner.sh "$SETTINGS" "$SITE"
