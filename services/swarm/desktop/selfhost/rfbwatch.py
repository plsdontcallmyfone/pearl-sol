#!/usr/bin/env python3
"""Minimal RFB (VNC 3.8, no auth, raw encoding) watcher for the watching-delay probe: connects to
x11vnc inside a desktop, keeps asking for incremental framebuffer updates, and prints the wall time
(epoch seconds) of the first update in which pixel (X, Y) turns red. What a noVNC viewer gets is
this update plus its websocket hop and a canvas draw.
  python3 rfbwatch.py HOST PORT X Y TIMEOUT_S"""
import socket, struct, sys, time

host, port, px, py, tmo = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), float(sys.argv[5])
s = socket.create_connection((host, port))
f = s.makefile("rb")
def rd(n):
    b = f.read(n)
    if len(b) < n: raise EOFError
    return b
rd(12); s.sendall(b"RFB 003.008\n")
n = rd(1)[0]; types = rd(n)
if 1 not in types: raise SystemExit("no None auth")
s.sendall(b"\x01"); rd(4)
s.sendall(b"\x01")  # shared
w, h = struct.unpack(">HH", rd(4)); rd(16); nl = struct.unpack(">I", rd(4))[0]; rd(nl)
# 32 bpp, depth 24, little endian, true colour, r/g/b max 255 at shifts 16/8/0
s.sendall(struct.pack(">BxxxBBBBHHHBBBxxx", 0, 32, 24, 0, 1, 255, 255, 255, 16, 8, 0))
s.sendall(struct.pack(">BxHi", 2, 1, 0))  # raw only
inc = 0
end = time.time() + tmo
print("ready", flush=True)
while time.time() < end:
    s.sendall(struct.pack(">BBHHHH", 3, inc, 0, 0, w, h)); inc = 1
    t = rd(1)[0]
    if t != 0:  # bell / colour map / cut text: skip what we can
        if t == 2: continue
        if t == 3: rd(3); ln = struct.unpack(">I", rd(4))[0]; rd(ln); continue
        raise SystemExit(f"msg {t}")
    rd(1); nr = struct.unpack(">H", rd(2))[0]
    hit = False
    for _ in range(nr):
        x, y, rw, rh, enc = struct.unpack(">HHHHi", rd(12))
        data = rd(rw * rh * 4)
        if x <= px < x + rw and y <= py < y + rh:
            o = ((py - y) * rw + (px - x)) * 4
            b, g, r = data[o], data[o + 1], data[o + 2]
            if r > 200 and g < 60 and b < 60: hit = True
    if hit:
        print(f"red {time.time():.3f}", flush=True); break
