#!/bin/bash
# Watching-delay probe, run INSIDE one desktop (podman exec): shows a red square at a known time and
# measures when it appears (1) in a VNC framebuffer update (what noVNC gets), (2) in a finished HLS
# segment listed in the playlist (what the site's video player can fetch), (3) in the 2 s still.
# Prints one JSON line per trial. Usage: latency-probe.sh TRIALS
N=${1:-5}; export DISPLAY=:0
cd /tmp && convert -size 300x300 xc:red /tmp/red.png
declare -A checked
seen_in_ts() { # a finished (listed) segment, not checked before, with red at (250,250)
  for f in $(grep -v '^#' /tmp/stream/live.m3u8 2>/dev/null | tail -2); do
    [ -n "${checked[$f]:-}" ] && continue; checked[$f]=1
    ffmpeg -nostdin -loglevel error -i "/tmp/stream/$f" -vf "crop=2:2:250:250" -f rawvideo -pix_fmt rgb24 - 2>/dev/null | od -An -tu1 -w3 | awk '$1>200 && $2<60 && $3<60{found=1} END{exit !found}' && return 0
  done; return 1; }
for i in $(seq "$N"); do
  pkill -f "display.*red.png" 2>/dev/null; sleep 4
  python3 /tmp/rfbwatch.py 127.0.0.1 5900 250 250 20 > /tmp/rfb.out 2>&1 &
  rp=$!; for k in $(seq 50); do grep -q ready /tmp/rfb.out && break; sleep 0.1; done
  for f in $(grep -v '^#' /tmp/stream/live.m3u8 2>/dev/null); do checked[$f]=1; done
  t0=$(date +%s.%N)
  display -geometry 300x300+100+100 -title probe /tmp/red.png &
  hls=""; still=""
  for k in $(seq 200); do
    if [ -z "$hls" ]; then seen_in_ts && hls=$(date +%s.%N); fi
    if [ -z "$still" ]; then convert /tmp/stream/thumb.jpg -crop 1x1+125+125 -depth 8 txt:- 2>/dev/null | tail -1 | grep -qiE '#(F|E|D)[0-9A-F]0[0-9A-F]{3}' && still=$(date +%s.%N); fi
    [ -n "$hls" ] && [ -n "$still" ] && break
    sleep 0.1
  done
  wait $rp 2>/dev/null; vnc=$(awk '/^red/{print $2}' /tmp/rfb.out)
  python3 -c "import json,sys; t0=float(sys.argv[1]); f=lambda v: round(float(v)-t0,3) if v else None; print(json.dumps({'trial':int(sys.argv[5]),'vnc_s':f(sys.argv[2]),'hls_listed_s':f(sys.argv[3]),'still_s':f(sys.argv[4])}))" "$t0" "$vnc" "$hls" "$still" "$i"
done
pkill -f "display.*red.png" 2>/dev/null; true
