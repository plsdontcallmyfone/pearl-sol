#!/bin/bash
# Stream settings bench, run INSIDE one desktop (self-hosted container or E2B sandbox) with the
# encoder script at /tmp/stream.sh and latency-probe.sh, rfbwatch.py, rfbbytes.py in /tmp. Starts
# Chromium on the town page scrolling every 0.5 s (a busy screen), then for each setting
# FPS:KBPS:SEG_S:STILL_S prints one JSON line: fps achieved, desktop CPU (cgroup if it has one, else
# the VM's /proc/stat), CPU throttling, encoder and browser CPU, bytes/s per viewer (HLS and a
# view-only VNC viewer, ZRLE), watching delay (red square -> VNC update, -> listed HLS segment, -> still).
#   bench-inner.sh "4:350:2:2 10:600:1:1" SITE
set -u
SETTINGS=$1; SITE=${2:-https://cadre.64-95-12-116.sslip.io}; export DISPLAY=:0
CH=$(ls -d /opt/ms-playwright/chromium-*/chrome-linux/chrome | head -1)
pgrep -x x11vnc >/dev/null || (x11vnc -display :0 -viewonly -shared -forever -nopw -localhost -rfbport 5900 -quiet -noxdamage >/tmp/x11vnc.log 2>&1 &)
nohup "$CH" --no-sandbox --test-type --no-first-run --user-data-dir=/tmp/prof --window-size=1280,800 --window-position=0,0 "$SITE/town" >/tmp/chrome.log 2>&1 &
sleep 12
nohup bash -c 'while true; do for k in 1 2 3 4 5 6; do xdotool mousemove 640 450 click 5; sleep 0.5; done; for k in 1 2 3 4 5 6; do xdotool click 4; sleep 0.5; done; done' >/dev/null 2>&1 &
SCROLL=$!
cpu_of() { local t=0; for p in $(pgrep -x "$1"); do t=$((t + $(awk '{print $14+$15}' /proc/$p/stat 2>/dev/null || echo 0))); done; echo $t; }
cg() { if grep -q usage_usec /sys/fs/cgroup/cpu.stat 2>/dev/null; then echo "cg $(awk '/usage_usec/{u=$2}/throttled_usec/{t=$2}END{print u+0, t+0}' /sys/fs/cgroup/cpu.stat)"; else echo "vm $(head -1 /proc/stat | awk '{print ($2+$3+$4+$7+$8)*10000, 0}')"; fi; }
for S in $SETTINGS; do
  IFS=: read -r FPS KBPS SEG STILL <<<"$S"
  pkill -f '[s]tream.sh'; pkill -x ffmpeg; sleep 1; rm -rf /tmp/stream; mkdir -p "$HOME/.cadre"
  printf 'DESK_FPS=%s\nDESK_KBPS=%s\nDESK_SEG_S=%s\nDESK_STILL_S=%s\n' "$FPS" "$KBPS" "$SEG" "$STILL" > "$HOME/.cadre/stream.env"
  nohup /tmp/stream.sh >/tmp/stream.log 2>&1 &
  sleep 10
  W=40
  c0=$(cg); f0=$(cpu_of ffmpeg); b0=$(cpu_of chrome); q0=$(grep -m1 MEDIA-SEQUENCE /tmp/stream/live.m3u8 | cut -d: -f2); t0=$(date +%s.%N)
  sleep $W
  c1=$(cg); f1=$(cpu_of ffmpeg); b1=$(cpu_of chrome); q1=$(grep -m1 MEDIA-SEQUENCE /tmp/stream/live.m3u8 | cut -d: -f2); t1=$(date +%s.%N)
  seg=$(cd /tmp/stream; tot=0; fr=0; du=0; for f in $(grep -v '^#' live.m3u8 | tail -4); do b=$(stat -c %s $f); n=$(ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of default=nw=1:nk=1 $f | head -1); d=$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 $f); tot=$((tot+b)); fr=$((fr+n)); du=$(awk -v a=$du -v b=$d 'BEGIN{print a+b}'); done; echo $tot $fr $du)
  lat=$(bash /tmp/latency-probe.sh 3 | tr '\n' ' ')
  vnc=$(python3 /tmp/rfbbytes.py 127.0.0.1 5900 10 2>/dev/null || echo "")
  python3 - "$S" "$c0" "$c1" "$f0" "$f1" "$b0" "$b1" "$q0" "$q1" "$t0" "$t1" "$seg" "$lat" "$vnc" "$(nproc)" <<'PY'
import json, sys
S, c0, c1, f0, f1, b0, b1, q0, q1, t0, t1, seg, lat, vnc, ncpu = sys.argv[1:]
k0, u0, th0 = c0.split(); k1, u1, th1 = c1.split()
wall = float(t1) - float(t0)
fps, kbps, seg_s, still_s = S.split(":")
tot, fr, du = seg.split(); tot, fr, du = int(tot), int(fr), float(du)
real = (int(q1) - int(q0)) * float(seg_s) / wall
trials = [json.loads(x) for x in lat.replace("} {", "}\n{").splitlines() if x.strip()]
def med(k):
    v = sorted(t[k] for t in trials if t.get(k) is not None)
    return v[len(v) // 2] if v else None
print(json.dumps({
  "fps_set": int(fps), "kbps_set": int(kbps), "seg_s": float(seg_s), "still_s": float(still_s), "cpus": int(ncpu), "cpu_source": k0,
  "fps_achieved": round(fr / du * min(1.0, real), 2) if du else None, "realtime": round(real, 3),
  "desktop_cpu": round((int(u1) - int(u0)) / 1e6 / wall, 3), "throttled_share": round((int(th1) - int(th0)) / 1e6 / wall, 3),
  "encoder_cpu": round((int(f1) - int(f0)) / 100 / wall, 3), "browser_cpu": round((int(b1) - int(b0)) / 100 / wall, 3),
  "hls_kbit_per_viewer": round(tot * 8 / du / 1000, 1) if du else None,
  "vnc_kbit_per_viewer": round(float(vnc) * 8 / 1000, 1) if vnc.strip() else None,
  "delay_vnc_s": med("vnc_s"), "delay_hls_listed_s": med("hls_listed_s"), "delay_still_s": med("still_s"), "trials": trials,
}), flush=True)
PY
done
kill $SCROLL 2>/dev/null; pkill -f '[s]tream.sh'; pkill -x ffmpeg; pkill -f "$CH"; true
