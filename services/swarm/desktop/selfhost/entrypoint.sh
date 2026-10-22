#!/bin/bash
# PID 1 of a self-hosted house desktop: an X display :0 (Xvfb at DESK_W x DESK_H), an XFCE session
# like E2B's desktop template, and a view-only VNC server on 127.0.0.1:5900 inside the container
# (noVNC's websockify on :6080 only when DESK_NOVNC=1). The controller provisions the rest
# (wallet, sandbox/deskhost.mjs, sandbox/stream.sh) with `podman exec`, exactly as on E2B.
W=${DESK_W:-1280}; H=${DESK_H:-800}
Xvfb :0 -screen 0 "${W}x${H}x24" -nolisten tcp -ac >/tmp/xvfb.log 2>&1 &
for i in $(seq 50); do xdpyinfo -display :0 >/dev/null 2>&1 && break; sleep 0.1; done
dbus-launch --exit-with-session startxfce4 >/tmp/xfce.log 2>&1 &
x11vnc -display :0 -viewonly -shared -forever -nopw -localhost -rfbport 5900 -quiet -noxdamage >/tmp/x11vnc.log 2>&1 &
if [ "${DESK_NOVNC:-0}" = 1 ]; then websockify --web /usr/share/novnc 6080 127.0.0.1:5900 >/tmp/novnc.log 2>&1 & fi
touch /tmp/desk-ready
trap 'exit 0' TERM INT
sleep infinity &
wait
