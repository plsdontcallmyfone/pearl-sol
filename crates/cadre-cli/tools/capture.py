# Run one `cadre` command in a pseudo-terminal (so colors and spinners are real) and keep
# the raw bytes. Usage: python3 capture.py OUT.raw COLS -- CMD ARGS...
import os, pty, sys, select, struct, fcntl, termios, time
out, cols = sys.argv[1], int(sys.argv[2])
cmd = sys.argv[sys.argv.index("--") + 1:]
pid, fd = pty.fork()
if pid == 0:
    os.environ["TERM"] = "xterm-256color"
    os.environ.pop("NO_COLOR", None)
    os.execvp(cmd[0], cmd)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 60, cols, 0, 0))
buf = b""
while True:
    r, _, _ = select.select([fd], [], [], 0.2)
    if r:
        try:
            d = os.read(fd, 65536)
        except OSError:
            break
        if not d:
            break
        buf += d
    else:
        p, _ = os.waitpid(pid, os.WNOHANG)
        if p:
            break
try:
    _, status = os.waitpid(pid, 0)
except ChildProcessError:
    status = 0
open(out, "wb").write(buf)
print(f"{out}: {len(buf)} bytes, exit {os.WEXITSTATUS(status) if status else 0}")
