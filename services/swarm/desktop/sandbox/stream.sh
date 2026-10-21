#!/bin/bash
# Runs INSIDE one house desktop: encodes the X display :0 to a rolling HLS playlist in
# /tmp/stream (a 6-segment window). The controller copies the newest segments out only while
# someone watches; nothing here listens on a port. Restarts ffmpeg if it ever exits.
# A 640 px JPEG still (thumb.jpg, every DESK_STILL_S) serves the /live grid tiles and browsers
# without H.264. Settings come from ~/.cadre/stream.env (written by the controller) or the env:
#   DESK_FPS (4), DESK_KBPS (350), DESK_SEG_S (2, HLS segment seconds), DESK_STILL_S (2)
[ -f "$HOME/.cadre/stream.env" ] && . "$HOME/.cadre/stream.env"
W=${DESK_W:-1280}; H=${DESK_H:-800}; FPS=${DESK_FPS:-4}; KBPS=${DESK_KBPS:-350}; SEG=${DESK_SEG_S:-2}; STILL=${DESK_STILL_S:-2}
GOP=$(awk -v f="$FPS" -v s="$SEG" 'BEGIN{g=int(f*s+0.5); if(g<1)g=1; print g}')
mkdir -p /tmp/stream
while true; do
  nice -n 5 ffmpeg -nostdin -loglevel error -f x11grab -draw_mouse 1 -framerate "$FPS" -video_size "${W}x${H}" -i :0 \
    -c:v libx264 -preset ultrafast -tune zerolatency -pix_fmt yuv420p -profile:v baseline \
    -b:v "${KBPS}k" -maxrate "${KBPS}k" -bufsize "$((KBPS * 2))k" -g "$GOP" -keyint_min "$GOP" -sc_threshold 0 \
    -f hls -hls_time "$SEG" -hls_list_size 6 -hls_flags delete_segments+omit_endlist+independent_segments \
    -hls_segment_filename '/tmp/stream/s%06d.ts' /tmp/stream/live.m3u8 \
    -vf "fps=1/$STILL,scale=640:-2" -q:v 6 -f image2 -update 1 /tmp/stream/thumb.jpg
  sleep 2
done
