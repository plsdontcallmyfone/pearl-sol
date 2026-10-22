# Drive the real dashboard in a pseudo-terminal (140x46): Mining tab, a row detail,
# mine start / status / stop, quit. Usage: python3 pty-smoke.py CADRE_BIN KEY_FILE RAW_OUT
# Needs a running stack (DEMO_KEEP_UP=1 scripts/demo.sh). Prints found/MISSING per check.
import os, pty, sys, time, select, struct, fcntl, termios, re
exe, key, out = sys.argv[1], sys.argv[2], sys.argv[3]
pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.execv(exe, [exe, "--key", key, "dashboard"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 46, 140, 0, 0))
buf = b""
def pump(sec):
    global buf
    end = time.time() + sec
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try: buf += os.read(fd, 65536)
            except OSError: return
def send(s, wait=1.0):
    os.write(fd, s.encode()); pump(wait)
pump(4)
send("2"); send("\x1b[B"); send("\r"); send("\x1b", 1.0)
send("mine start --threads 1\r", 4)
send("mine status\r", 4)
send("mine stop\r", 2)
send("quit\r", 2)
_, status = os.waitpid(pid, 0)
open(out, "wb").write(buf)
txt = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", buf).decode("utf-8", "replace")
for needle in ["[2]Mining", "Wallet Connected_", "miner started (pid", "of the last", "stopped", "Close(); Tap Esc"]:
    print(f"{needle!r}: {'found' if needle in txt else 'MISSING'}")
print("exit status", os.WEXITSTATUS(status))
