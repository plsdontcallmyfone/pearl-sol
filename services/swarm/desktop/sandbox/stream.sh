#!/bin/bash
# Runs INSIDE one house desktop: encodes the X display :0 to a rolling HLS playlist in
# /tmp/stream (a 6-segment window, 2 s segments). The controller copies the newest segments out
# only while someone watches; nothing here listens on a port. Restarts ffmpeg if it ever exits.
# A 640 px JPEG still (thumb.jpg, every 2 s) serves browsers without H.264 and the poster frame.
W=${DESK_W:-1280}; H=${DESK_H:-800}; FPS=${DESK_FPS:-4}; KBPS=${DESK_KBPS:-350}
mkdir -p /tmp/stream
while true; do
  nice -n 5 ffmpeg -nostdin -loglevel error -f x11grab -draw_mouse 1 -framerate "$FPS" -video_size "${W}x${H}" -i :0 \
    -c:v libx264 -preset ultrafast -tune zerolatency -pix_fmt yuv420p -profile:v baseline \
    -b:v "${KBPS}k" -maxrate "${KBPS}k" -bufsize "$((KBPS * 2))k" -g "$((FPS * 2))" -keyint_min "$((FPS * 2))" -sc_threshold 0 \
    -f hls -hls_time 2 -hls_list_size 6 -hls_flags delete_segments+omit_endlist+independent_segments \
    -hls_segment_filename '/tmp/stream/s%06d.ts' /tmp/stream/live.m3u8 \
    -vf "fps=1/2,scale=640:-2" -q:v 6 -f image2 -update 1 /tmp/stream/thumb.jpg
  sleep 2
done
