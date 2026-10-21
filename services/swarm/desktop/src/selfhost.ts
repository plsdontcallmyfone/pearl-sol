// The self-hosted desktop backend (docs/DESKTOPS-AT-SCALE.md section 5): the same house desktop as
// on E2B, as a podman container from selfhost/Containerfile (Xvfb + XFCE + Chromium + the Cadre
// Wallet, view-only VNC inside), on a box we run. It implements the same DesktopHost interface as
// the E2B backend (src/desk.ts), so the controller can move one agent at a time between the two
// (DESK_SELFHOST_AGENTS). The controller talks to a container only through `podman exec`
// (commands, files, screenshots with ImageMagick, mouse and keyboard with xdotool); nothing in the
// container listens outside it, so the stream goes out the same way as on E2B (the controller
// copies /tmp/stream files on request).
//
// The podman command runs locally (`podman`) or on the desktop host over ssh: DESK_SELFHOST_CMD,
// e.g. "ssh -i /etc/lode/keys/desk-host -o IdentitiesOnly=yes root@<host> podman". Each container
// gets its own CPU and memory limit (DESK_SELFHOST_CPUS, DESK_SELFHOST_MEM_MB) and, optionally, a
// shared cgroup slice for the whole fleet (DESK_SELFHOST_SLICE, CPUQuota/MemoryMax on the host).
// A self-hosted desktop costs a flat share of its box, booked in the ledger at
// DESK_SELFHOST_USD_PER_HOUR (the box's hourly price / the desktops it holds, measured in section 5).
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import type { DesktopHost, E2BInfo, SandboxLike } from "./desk.ts";

export interface RunResult { code: number; stdout: Buffer; stderr: string }
export type Runner = (args: string[], input?: Buffer | string, timeoutMs?: number) => Promise<RunResult>;

const shq = (s: string) => (/^[A-Za-z0-9_@%+=:,./-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`);

/** Runs `prefix... args` (prefix = DESK_SELFHOST_CMD split on spaces; over ssh the args are shell-quoted). */
export function podmanRunner(prefix = (process.env.DESK_SELFHOST_CMD ?? "podman").split(/\s+/).filter(Boolean)): Runner {
  const viaSsh = prefix[0] === "ssh";
  return (args, input, timeoutMs = 60_000) =>
    new Promise((resolve) => {
      const argv = viaSsh ? [...prefix.slice(1, -1), [prefix[prefix.length - 1]!, ...args].map(shq).join(" ")] : [...prefix.slice(1), ...args];
      const p = spawn(prefix[0]!, argv, { stdio: ["pipe", "pipe", "pipe"] });
      const out: Buffer[] = [];
      let err = "";
      const t = setTimeout(() => p.kill("SIGKILL"), timeoutMs);
      p.stdout.on("data", (b: Buffer) => out.push(b));
      p.stderr.on("data", (b: Buffer) => { if (err.length < 4000) err += b.toString(); });
      p.on("error", (e) => { clearTimeout(t); resolve({ code: -1, stdout: Buffer.alloc(0), stderr: String(e.message) }); });
      p.on("close", (code) => { clearTimeout(t); resolve({ code: code ?? -1, stdout: Buffer.concat(out), stderr: err }); });
      if (input !== undefined) p.stdin.end(input);
      else p.stdin.end();
    });
}

/** xdotool key names for the keys the vision driver sends (E2B's names, lower case). */
const KEYMAP: Record<string, string> = {
  enter: "Return", return: "Return", tab: "Tab", escape: "Escape", esc: "Escape", backspace: "BackSpace", delete: "Delete",
  space: "space", up: "Up", down: "Down", left: "Left", right: "Right", home: "Home", end: "End",
  page_down: "Next", page_up: "Prior", pagedown: "Next", pageup: "Prior", ctrl: "ctrl", control: "ctrl", alt: "alt", shift: "shift",
  meta: "super", super: "super", cmd: "super",
};
export const xKey = (k: string | string[]) => (Array.isArray(k) ? k : [k]).map((p) => KEYMAP[p.toLowerCase()] ?? (p.length === 1 ? p : p.replace(/^f(\d+)$/i, "F$1"))).join("+");

export interface SelfHostOpts {
  image: string;
  cpus: number;
  memoryMb: number;
  /** podman --cgroup-parent (a systemd slice on the host that caps the whole fleet), or "" */
  slice: string;
  /** podman --network (slirp4netns: user-mode networking, no iptables rules and no ports on the host) */
  network: string;
  usdPerHour: number;
  run?: Runner;
}

export function selfHostOptsFromEnv(env = process.env): SelfHostOpts {
  const n = (k: string, d: number) => (env[k] ? Number(env[k]) : d);
  return {
    image: env.DESK_SELFHOST_IMAGE ?? "localhost/cadre-desktop-selfhost:latest",
    cpus: n("DESK_SELFHOST_CPUS", 1),
    memoryMb: n("DESK_SELFHOST_MEM_MB", 1024),
    slice: env.DESK_SELFHOST_SLICE ?? "",
    network: env.DESK_SELFHOST_NETWORK ?? "slirp4netns",
    usdPerHour: n("DESK_SELFHOST_USD_PER_HOUR", 0),
  };
}

/** One container as the SandboxLike the desk and the vision driver use. */
export function containerSandbox(name: string, run: Runner): SandboxLike {
  const exec = (args: string[], input?: Buffer | string, timeoutMs?: number) => run(["exec", ...(input !== undefined ? ["-i"] : []), "-e", "DISPLAY=:0", name, ...args], input, timeoutMs);
  const xdo = async (...a: string[]) => {
    const r = await exec(["xdotool", ...a], undefined, 20_000);
    if (r.code !== 0) throw new Error(`xdotool ${a[0]}: ${r.stderr.slice(0, 120)}`);
  };
  return {
    sandboxId: name,
    commands: {
      async run(cmd, opts = {}) {
        if (opts.background) {
          const r = await run(["exec", "-d", "-e", "DISPLAY=:0", name, "bash", "-lc", cmd], undefined, 20_000);
          return { exitCode: r.code, stdout: "", stderr: r.stderr };
        }
        const r = await exec(["bash", "-lc", cmd], undefined, Number(opts.timeoutMs ?? 60_000));
        return { exitCode: r.code, stdout: r.stdout.toString("utf8"), stderr: r.stderr };
      },
    },
    files: {
      async write(path, data) {
        const buf = typeof data === "string" ? Buffer.from(data) : Buffer.from(new Uint8Array(data));
        const dir = path.replace(/\/[^/]*$/, "") || "/";
        const r = await exec(["sh", "-c", `mkdir -p ${shq(dir)} && cat > ${shq(path)}`], buf, 120_000);
        if (r.code !== 0) throw new Error(`write ${path}: ${r.stderr.slice(0, 120)}`);
        return { path };
      },
      async read(path, opts) {
        const r = await exec(["cat", path], undefined, 30_000);
        if (r.code !== 0) throw new Error(`read ${path}: ${r.stderr.slice(0, 120)}`);
        return opts?.format === "bytes" ? new Uint8Array(r.stdout) : r.stdout.toString("utf8");
      },
    },
    async screenshot() {
      const r = await exec(["import", "-window", "root", "-display", ":0", "png:-"], undefined, 30_000);
      if (r.code !== 0 || !r.stdout.length) throw new Error(`screenshot: ${r.stderr.slice(0, 120)}`);
      return new Uint8Array(r.stdout);
    },
    leftClick: (x, y) => (x === undefined ? xdo("click", "1") : xdo("mousemove", String(x), String(y), "click", "1")),
    doubleClick: (x, y) => (x === undefined ? xdo("click", "--repeat", "2", "1") : xdo("mousemove", String(x), String(y), "click", "--repeat", "2", "1")),
    rightClick: (x, y) => (x === undefined ? xdo("click", "3") : xdo("mousemove", String(x), String(y), "click", "3")),
    moveMouse: (x, y) => xdo("mousemove", String(x), String(y)),
    scroll: (direction = "down", amount = 3) => xdo("click", "--repeat", String(Math.max(1, Math.min(10, amount))), direction === "up" ? "4" : "5"),
    async write(text, opts = {}) {
      const size = Number(opts.chunkSize ?? 40);
      const delay = String(Number(opts.delayInMs ?? 20));
      for (let i = 0; i < text.length; i += size) await xdo("type", "--delay", delay, "--", text.slice(i, i + size));
    },
    press: (key) => xdo("key", "--", xKey(key)),
  };
}

const parseTime = (s: unknown) => {
  const d = new Date(String(s ?? "").replace(/ \+0000 UTC$/, "Z").replace(/\.\d+Z$/, "Z"));
  return Number.isNaN(d.getTime()) ? new Date(0) : d;
};

/** The self-hosted DesktopHost (podman). `template` is ignored: every desktop uses `o.image`. */
export function selfHostApi(o: SelfHostOpts): DesktopHost {
  const run = o.run ?? podmanRunner();
  const must = async (args: string[], timeoutMs?: number) => {
    const r = await run(args, undefined, timeoutMs);
    if (r.code !== 0) throw new Error(`podman ${args[0]}: ${r.stderr.trim().slice(0, 200)}`);
    return r.stdout.toString("utf8");
  };
  const waitReady = async (name: string) => {
    for (let i = 0; i < 60; i++) {
      const r = await run(["exec", name, "test", "-f", "/tmp/desk-ready"], undefined, 10_000);
      if (r.code === 0) return;
      await new Promise((res) => setTimeout(res, 500));
    }
    throw new Error(`desktop ${name} did not start its display in 30 s`);
  };
  return {
    kind: "selfhost",
    usdPerS: o.usdPerHour / 3600,
    async create(_template, opts) {
      const meta = (opts.metadata ?? {}) as Record<string, string>;
      const [w, h] = (opts.resolution as [number, number] | undefined) ?? [1280, 800];
      const tag = `${meta.cadre_desktop ?? "desk"}-${meta.cadre_role ?? "x"}`.replace(/[^a-z0-9_-]/gi, "").slice(0, 40);
      const name = `cadre-desk-${tag}-${randomBytes(3).toString("hex")}`;
      const labels = Object.entries(meta).flatMap(([k, v]) => ["--label", `${k}=${v}`]);
      await must([
        "run", "-d", "--name", name, ...labels, "--label", "cadre_selfhost=1",
        "--cpus", String(o.cpus), "--memory", `${o.memoryMb}m`, "--memory-swap", `${o.memoryMb}m`, "--shm-size", "256m", "--pids-limit", "2048",
        ...(o.slice ? ["--cgroup-parent", o.slice] : []), "--network", o.network,
        "-e", `DESK_W=${w}`, "-e", `DESK_H=${h}`, o.image,
      ], 120_000);
      await waitReady(name);
      return containerSandbox(name, run);
    },
    async connect(id) {
      const st = (await must(["inspect", "-f", "{{.State.Status}}", id])).trim();
      if (st === "paused") await must(["unpause", id]);
      else if (st !== "running") { await must(["start", id]); await waitReady(id); }
      return containerSandbox(id, run);
    },
    async list(metadata) {
      const filters = ["--filter", "label=cadre_selfhost=1", ...Object.entries(metadata).flatMap(([k, v]) => ["--filter", `label=${k}=${v}`])];
      const rows = JSON.parse((await must(["ps", "-a", ...filters, "--format", "json"])) || "[]") as any[];
      const out: E2BInfo[] = [];
      for (const r of rows) {
        const state = String(r.State ?? "").toLowerCase();
        if (state !== "running" && state !== "paused") continue;
        const started = r.StartedAt ? new Date(Number(r.StartedAt) * 1000) : parseTime(r.CreatedAt);
        out.push({ sandboxId: (r.Names?.[0] ?? r.Id) as string, state, metadata: r.Labels ?? {}, startedAt: started, endAt: new Date(8.64e15) });
      }
      return out;
    },
    pause: async (id) => (await run(["pause", id])).code === 0,
    kill: async (id) => (await run(["rm", "-f", "-t", "5", id], undefined, 60_000)).code === 0,
    // no idle timeout on our own boxes: the controller's kill switch and day cap pause them
    setTimeout: async () => {},
  };
}
