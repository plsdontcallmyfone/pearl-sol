#!/usr/bin/env python3
"""Bytes per second a view-only VNC viewer receives from x11vnc (ZRLE, like noVNC asks for), over
T seconds of a busy screen. Prints bytes/s.  python3 rfbbytes.py HOST PORT T"""
import socket, struct, sys, time
host, port, T = sys.argv[1], int(sys.argv[2]), float(sys.argv[3])
s = socket.create_connection((host, port)); f = s.makefile("rb")
got = 0
def rd(n):
    global got
    b = f.read(n)
    if len(b) < n: raise EOFError
    got += n; return b
rd(12); s.sendall(b"RFB 003.008\n"); n = rd(1)[0]; rd(n); s.sendall(b"\x01"); rd(4); s.sendall(b"\x01")
w, h = struct.unpack(">HH", rd(4)); rd(16); nl = struct.unpack(">I", rd(4))[0]; rd(nl)
s.sendall(struct.pack(">BxxxBBBBHHHBBBxxx", 0, 32, 24, 0, 1, 255, 255, 255, 16, 8, 0))
s.sendall(struct.pack(">BxHii", 2, 2, 16, 0))  # ZRLE, raw
got = 0; t0 = time.time(); inc = 0
while time.time() - t0 < T:
    s.sendall(struct.pack(">BBHHHH", 3, inc, 0, 0, w, h)); inc = 1
    t = rd(1)[0]
    if t == 2: continue
    if t == 3: rd(3); ln = struct.unpack(">I", rd(4))[0]; rd(ln); continue
    rd(1); nr = struct.unpack(">H", rd(2))[0]
    for _ in range(nr):
        x, y, rw, rh, enc = struct.unpack(">HHHHi", rd(12))
        if enc == 16: ln = struct.unpack(">I", rd(4))[0]; rd(ln)
        elif enc == 0: rd(rw * rh * 4)
        else: raise SystemExit(f"enc {enc}")
print(round(got / (time.time() - t0)))
