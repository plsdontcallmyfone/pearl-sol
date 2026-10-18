# Replay raw terminal bytes (from capture.py) on a tiny screen model (\r, \n, ESC[2K,
# ESC[K, SGR 0/1/2/22/30-37/39/90-97/38;5;n/48;5;n/107) and write an HTML page on a black
# background. Each prompt is shown as `user@host ~/pearl-sol <command>` (path in yellow).
# Usage: python3 ansi2html.py OUT.html COLS "command line" RAW [ "command line" RAW ...]
import sys, html, re, getpass, socket
out, cols = sys.argv[1], int(sys.argv[2])
pairs = list(zip(sys.argv[3::2], sys.argv[4::2]))

def xterm256(n):
    base = ["#000000", "#cd3131", "#2fbf71", "#e5c07b", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
            "#666666", "#f14c4c", "#3ddc84", "#f5d76e", "#3b8eea", "#d670d6", "#29b8db", "#ffffff"]
    if n < 16: return base[n]
    if n < 232:
        n -= 16; r, g, b = n // 36, (n // 6) % 6, n % 6
        c = lambda v: 0 if v == 0 else 55 + 40 * v
        return "#%02x%02x%02x" % (c(r), c(g), c(b))
    v = 8 + 10 * (n - 232)
    return "#%02x%02x%02x" % (v, v, v)

DEFAULT = {"fg": None, "bg": None, "bold": False, "dim": False}

def replay(data):
    lines, cur, col, st = [], [], 0, dict(DEFAULT)
    i, s = 0, data.decode("utf-8", "replace")
    while i < len(s):
        ch = s[i]
        if ch == "\x1b":
            m = re.match(r"\x1b\[([0-9;?]*)([A-Za-z])", s[i:])
            if not m: i += 1; continue
            args, cmd = m.group(1), m.group(2)
            i += m.end()
            if cmd == "m":
                ps = [int(x) if x else 0 for x in args.split(";")] if args else [0]
                j = 0
                while j < len(ps):
                    p = ps[j]
                    if p == 0: st = dict(DEFAULT)
                    elif p == 1: st["bold"] = True
                    elif p == 2: st["dim"] = True
                    elif p == 22: st["bold"] = st["dim"] = False
                    elif 30 <= p <= 37: st["fg"] = xterm256(p - 30)
                    elif 90 <= p <= 97: st["fg"] = xterm256(p - 90 + 8)
                    elif p == 39: st["fg"] = None
                    elif 40 <= p <= 47: st["bg"] = xterm256(p - 40)
                    elif 100 <= p <= 107: st["bg"] = xterm256(p - 100 + 8)
                    elif p == 49: st["bg"] = None
                    elif p in (38, 48) and j + 2 < len(ps) and ps[j + 1] == 5:
                        st["fg" if p == 38 else "bg"] = xterm256(ps[j + 2]); j += 2
                    j += 1
            elif cmd == "K":
                if args in ("2",): cur = []; 
                else: cur = cur[:col]
            continue
        if ch == "\r": col = 0; i += 1; continue
        if ch == "\n": lines.append(cur); cur, col = [], 0; i += 1; continue
        cell = (ch, dict(st))
        if col < len(cur): cur[col] = cell
        else:
            cur += [(" ", dict(DEFAULT))] * (col - len(cur)); cur.append(cell)
        col += 1; i += 1
    if cur: lines.append(cur)
    return lines

def span(text, st):
    css = []
    if st["fg"]: css.append("color:" + st["fg"])
    if st["bg"]: css.append("background:" + st["bg"])
    if st["bold"]: css.append("font-weight:700")
    if st["dim"]: css.append("opacity:.6")
    t = html.escape(text)
    return f'<span style="{";".join(css)}">{t}</span>' if css else t

def line_html(cells):
    outp, run, rst = [], "", None
    for ch, st in cells:
        if st != rst and run: outp.append(span(run, rst)); run = ""
        rst = st; run += ch
    if run: outp.append(span(run, rst))
    return "".join(outp)

user, host = getpass.getuser(), socket.gethostname().split(".")[0]
rows = []
for cmdline, raw in pairs:
    rows.append(f'<span style="color:#e5e5e5">{user}@{host}</span> <span style="color:#e5c07b">~/pearl-sol</span> '
                f'<span style="color:#ffffff">{html.escape(cmdline)}</span>')
    rows += [line_html(l) for l in replay(open(raw, "rb").read())]
    rows.append("")
page = f"""<!doctype html><meta charset="utf-8"><style>
html,body{{margin:0;background:#000}}
pre{{margin:0;padding:22px 26px;color:#e5e5e5;font:14px/1.25 Menlo,'SF Mono',Monaco,monospace;white-space:pre}}
</style><pre>{chr(10).join(rows)}</pre>"""
open(out, "w").write(page)
print(out, len(rows), "rows")
