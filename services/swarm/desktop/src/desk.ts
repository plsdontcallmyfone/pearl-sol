// One house desktop: an E2B Desktop sandbox owned by one agent in one role ("social" or "work"),
// found again by its metadata (cadre_desktop=<handle>, cadre_role=<role>) after a controller
// restart, kept running (timeout pushed every minute; E2B pauses it, never kills it, if the
// controller dies), renewed before the plan's session length (pause + resume, about a second),
// and provisioned once: the Cadre Wallet build, the in-desktop browser host (sandbox/deskhost.mjs)
// and the stream encoder (sandbox/stream.sh). Secrets reach the desktop only as 0600 files that
// the browser host consumes; they are never typed on screen or put in a prompt.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export type Role = "social" | "work";

export interface SandboxLike {
  sandboxId: string;
  commands: { run(cmd: string, opts?: Record<string, unknown>): Promise<{ exitCode: number; stdout: string; stderr: string }> };
  files: { write(path: string, data: string | ArrayBuffer): Promise<unknown>; read(path: string, opts?: { format: "bytes" | "text" }): Promise<any> };
  screenshot(): Promise<Uint8Array>;
  leftClick(x?: number, y?: number): Promise<void>;
  doubleClick(x?: number, y?: number): Promise<void>;
  rightClick(x?: number, y?: number): Promise<void>;
  moveMouse(x: number, y: number): Promise<void>;
  scroll(direction?: "up" | "down", amount?: number): Promise<void>;
  write(text: string, opts?: Record<string, unknown>): Promise<void>;
  press(key: string | string[]): Promise<void>;
}

export interface E2BInfo { sandboxId: string; state: "running" | "paused"; metadata: Record<string, string>; startedAt: Date; endAt: Date }

/** Where desktops run: E2B (@e2b/desktop, `sdkApi`) or our own boxes (podman, src/selfhost.ts);
 * tests pass a fake. `usdPerS` is the host's own rate per desktop when it is not E2B's list rate. */
export interface DesktopHost {
  kind?: "e2b" | "selfhost";
  usdPerS?: number;
  create(template: string, opts: Record<string, unknown>): Promise<SandboxLike>;
  connect(id: string, opts: Record<string, unknown>): Promise<SandboxLike>;
  list(metadata: Record<string, string>): Promise<E2BInfo[]>;
  pause(id: string): Promise<boolean>;
  kill(id: string): Promise<boolean>;
  setTimeout(id: string, ms: number): Promise<void>;
}
/** The E2B backend's name for the same interface (kept for the tests and older callers). */
export type E2BApi = DesktopHost;

/** One desktop's health, read every DESK_HEALTH_S by the controller (Desk.checkHealth). */
export interface DeskHealth {
  checked_at: string;
  /** the stream's newest still is younger than 10 s and the encoder runs */
  stream_alive: boolean;
  stream_age_s: number | null;
  encoder: boolean;
  /** the in-desktop browser host (sandbox/deskhost.mjs) runs and wrote its state in the last 30 s */
  browser_host: boolean;
  mem_total_mb: number | null;
  mem_avail_mb: number | null;
  load1: number | null;
}

export async function sdkApi(apiKey: string): Promise<DesktopHost> {
  const { Sandbox } = (await import("@e2b/desktop")) as any;
  const pick = (i: any): E2BInfo => ({ sandboxId: i.sandboxId, state: i.state, metadata: i.metadata ?? {}, startedAt: new Date(i.startedAt), endAt: new Date(i.endAt) });
  return {
    kind: "e2b",
    create: (t, o) => Sandbox.create(t, { ...o, apiKey }),
    connect: (id, o) => Sandbox.connect(id, { ...o, apiKey }),
    list: async (metadata) => {
      // E2B refuses an empty metadata filter ("Error parsing metadata"): leave it out for "all"
      const query = Object.keys(metadata).length ? { metadata, state: ["running", "paused"] } : { state: ["running", "paused"] };
      const p = Sandbox.list({ apiKey, query });
      const all: E2BInfo[] = [];
      while (p.hasNext) all.push(...(await p.nextItems()).map(pick));
      return all;
    },
    pause: (id) => Sandbox.pause(id, { apiKey }),
    kill: (id) => Sandbox.kill(id, { apiKey }),
    setTimeout: (id, ms) => Sandbox.setTimeout(id, ms, { apiKey }),
  };
}

const HERE = dirname(fileURLToPath(import.meta.url));
const HOME = "/home/user/.cadre";
export const META_KEY = "cadre_desktop";

export interface DeskOpts {
  template: string;
  resolution: [number, number];
  keepaliveS: number;
  sessionMaxS: number;
  siteUrl: string;
  explorerUrl: string;
  walletTgz: string; // a .tar.gz of the wallet dist (no source maps)
  /** encoder settings for sandbox/stream.sh (DESK_FPS, DESK_KBPS, DESK_SEG_S, DESK_STILL_S); empty = its defaults */
  stream?: Record<string, string | number>;
  log: (e: Record<string, unknown>) => void;
}

export class Desk {
  sbx: SandboxLike | null = null;
  id: string | null = null;
  startedAt = 0; // last create or resume (E2B counts the session from it)
  runningSince = 0; // for the spend ledger
  state: "off" | "starting" | "running" | "paused" | "error" = "off";
  lastError: string | null = null;
  provisioned = false;
  lastAction = "";
  /** last time a vision step or chain action ran on this desktop (ms), 0 = none since the controller started */
  lastActionAt = 0;
  health: DeskHealth | null = null;
  /** the Cadre Wallet's extension id in this desktop's browser (from the browser host's state) */
  extId: string | null = null;
  handle: string;
  role: Role;
  address: string;
  readonly api: DesktopHost;
  private o: DeskOpts;
  constructor(handle: string, role: Role, address: string, api: DesktopHost, o: DeskOpts) {
    this.handle = handle; this.role = role; this.address = address; this.api = api; this.o = o;
  }

  get meta() { return { [META_KEY]: this.handle, cadre_role: this.role }; }

  private sh(cmd: string, timeoutMs = 60_000) {
    return this.sbx!.commands.run(cmd, { timeoutMs }).catch((e: any) => e?.result ?? { exitCode: -1, stdout: "", stderr: String(e?.message ?? e) });
  }

  /** Finds or creates the sandbox and makes sure the browser host and the encoder run. */
  async ensure(secret: () => string, password: () => string): Promise<void> {
    const opts = { timeoutMs: this.o.keepaliveS * 1000, resolution: this.o.resolution, metadata: this.meta, lifecycle: { onTimeout: "pause" } };
    if (!this.sbx) {
      this.state = "starting";
      const found = (await this.api.list(this.meta)).sort((a, b) => b.startedAt.getTime() - a.startedAt.getTime());
      for (const extra of found.slice(1)) { await this.api.kill(extra.sandboxId); this.o.log({ event: "desk_duplicate_killed", agent: this.handle, role: this.role, sandbox: extra.sandboxId }); }
      if (found[0]) {
        this.sbx = await this.api.connect(found[0].sandboxId, { timeoutMs: opts.timeoutMs });
        this.startedAt = found[0].state === "paused" ? Date.now() : found[0].startedAt.getTime();
        this.o.log({ event: "desk_connected", agent: this.handle, role: this.role, sandbox: found[0].sandboxId, was: found[0].state });
      } else {
        this.sbx = await this.api.create(this.o.template, opts);
        this.startedAt = Date.now();
        this.provisioned = false;
        this.o.log({ event: "desk_created", agent: this.handle, role: this.role, sandbox: this.sbx.sandboxId });
      }
      this.id = this.sbx.sandboxId;
      this.runningSince = Date.now();
    }
    const r = await this.sh(`test -f ${HOME}/provisioned && echo yes; pgrep -f "[d]eskhost.mjs" >/dev/null && echo host; pgrep -x ffmpeg >/dev/null && echo enc`);
    const out = r.stdout;
    if (!out.includes("yes")) await this.provision(secret(), password());
    else {
      // a newer browser host from this controller replaces the running one (same profile: the wallet stays set up)
      const hostJs = readFileSync(join(HERE, "../sandbox/deskhost.mjs"), "utf8");
      const curHost = await this.sh(`cat ${HOME}/deskhost.mjs 2>/dev/null`);
      if (curHost.stdout !== hostJs) {
        await this.sbx!.files.write(`${HOME}/deskhost.mjs`, hostJs);
        await this.restartHost();
        this.o.log({ event: "desk_host_updated", agent: this.handle, role: this.role, sandbox: this.id });
      } else if (!out.includes("host")) await this.startHost();
      // a newer encoder script or new encoder settings from this controller replace the running encoder
      const enc = readFileSync(join(HERE, "../sandbox/stream.sh"), "utf8");
      const senv = this.streamEnv();
      const cur = await this.sh(`cat ${HOME}/stream.sh 2>/dev/null; echo "@@env@@"; cat ${HOME}/stream.env 2>/dev/null`);
      const [curScript, curEnv = ""] = cur.stdout.split("@@env@@\n");
      if (curScript !== enc || curEnv !== senv) {
        await this.sbx!.files.write(`${HOME}/stream.sh`, enc);
        await this.sbx!.files.write(`${HOME}/stream.env`, senv);
        await this.sh(`chmod +x ${HOME}/stream.sh`);
        await this.restartEncoder();
        this.o.log({ event: "desk_encoder_updated", agent: this.handle, role: this.role, sandbox: this.id });
      } else if (!out.includes("enc")) await this.startEncoder();
    }
    this.provisioned = true;
    this.state = "running";
  }

  private async provision(secret: string, password: string) {
    const [w, h] = this.o.resolution;
    await this.sh(`mkdir -p ${HOME}/wallet && chmod 700 ${HOME} && ln -sf /opt/deskhost/node_modules ${HOME}/node_modules`);
    const tgz = readFileSync(this.o.walletTgz);
    await this.sbx!.files.write("/tmp/wallet.tgz", tgz.buffer.slice(tgz.byteOffset, tgz.byteOffset + tgz.byteLength) as ArrayBuffer);
    await this.sbx!.files.write(`${HOME}/deskhost.mjs`, readFileSync(join(HERE, "../sandbox/deskhost.mjs"), "utf8"));
    await this.sbx!.files.write(`${HOME}/stream.sh`, readFileSync(join(HERE, "../sandbox/stream.sh"), "utf8"));
    await this.sbx!.files.write(`${HOME}/stream.env`, this.streamEnv());
    await this.sbx!.files.write(`${HOME}/agent-secret`, secret);
    await this.sbx!.files.write(`${HOME}/wallet-password`, password);
    await this.sbx!.files.write(`${HOME}/agent-address`, this.address);
    const r = await this.sh(`cd ${HOME} && tar -xzf /tmp/wallet.tgz -C wallet && rm -f /tmp/wallet.tgz && chmod 600 agent-secret wallet-password && chmod +x stream.sh && echo "DESK_W=${w} DESK_H=${h}" > desk.env && echo ok`);
    if (!r.stdout.includes("ok")) throw new Error(`provision: ${r.stderr.slice(0, 200)}`);
    // the panel and desktop icons stay; the XFCE screensaver was removed in the template
    await this.startEncoder();
    await this.startHost();
    // wait for the browser host to seed the wallet (it deletes the key file when done)
    for (let i = 0; i < 30; i++) {
      await new Promise((res) => setTimeout(res, 2000));
      const s = await this.sh(`test -f ${HOME}/agent-secret && echo pending; grep -c wallet_seeded ${HOME}/deskhost.log`);
      if (!s.stdout.includes("pending")) break;
    }
    const left = await this.sh(`test -f ${HOME}/agent-secret && echo pending`);
    if (left.stdout.includes("pending")) {
      await this.sh(`rm -f ${HOME}/agent-secret`);
      throw new Error("wallet seeding did not finish in 60 s (key file removed)");
    }
    await this.sh(`touch ${HOME}/provisioned`);
    this.o.log({ event: "desk_provisioned", agent: this.handle, role: this.role, sandbox: this.id });
  }

  /** stream.env for the encoder: KEY=value lines, only the four known keys, numbers only. */
  streamEnv(): string {
    const ok = ["DESK_FPS", "DESK_KBPS", "DESK_SEG_S", "DESK_STILL_S"];
    return Object.entries(this.o.stream ?? {}).filter(([k, v]) => ok.includes(k) && /^\d+(\.\d+)?$/.test(String(v))).map(([k, v]) => `${k}=${v}\n`).join("");
  }

  private async startHost() {
    const [w, h] = this.o.resolution;
    await this.sbx!.commands.run(
      `cd ${HOME} && DISPLAY=:0 DESK_W=${w} DESK_H=${h} PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright nohup node deskhost.mjs ${this.role} ${this.o.siteUrl} ${this.o.explorerUrl} ${this.handle} >> deskhost.log 2>&1 &`,
      { background: true },
    );
  }

  /** Kills every encoder loop and ffmpeg (never this shell: patterns that cannot match their own
   * command line, and $$ / $PPID skipped), clears the old still, starts one encoder. */
  async restartEncoder() {
    await this.sh(`for p in $(pgrep -f "[s]tream.sh") $(pgrep -x ffmpeg); do [ "$p" != "$$" ] && [ "$p" != "$PPID" ] && kill -9 "$p" 2>/dev/null; done; sleep 1; rm -f /tmp/stream/thumb.jpg /tmp/stream.lock; true`, 20_000);
    await this.startEncoder();
  }

  /** Kills the browser host (SIGKILL: Playwright's SIGTERM handler closes the browser but keeps node
   * running) and its Chromium, clears the profile lock, starts one browser host (same profile). */
  async restartHost() {
    await this.sh(`for p in $(pgrep -f "[d]eskhost.mjs"); do [ "$p" != "$$" ] && kill -9 "$p" 2>/dev/null; done; sleep 1; for p in $(pgrep -f "[c]hrome-linux/chrome"); do kill -9 "$p" 2>/dev/null; done; sleep 1; rm -f ${HOME}/profile/SingletonLock ${HOME}/profile/SingletonSocket ${HOME}/profile/SingletonCookie; true`, 30_000);
    await this.startHost();
  }

  private async startEncoder() {
    const [w, h] = this.o.resolution;
    await this.sbx!.commands.run(`DISPLAY=:0 DESK_W=${w} DESK_H=${h} nohup ${HOME}/stream.sh > /tmp/stream.log 2>&1 &`, { background: true });
  }

  /** Every minute: push the timeout; renew the session before E2B's limit. */
  async keepalive(): Promise<void> {
    if (!this.id || this.state !== "running") return;
    const age = (Date.now() - this.startedAt) / 1000;
    if (age > this.o.sessionMaxS - 1800) {
      this.o.log({ event: "desk_renew", agent: this.handle, role: this.role, sandbox: this.id, age_s: Math.round(age) });
      await this.api.pause(this.id);
      this.sbx = await this.api.connect(this.id, { timeoutMs: this.o.keepaliveS * 1000 });
      this.startedAt = Date.now();
      return;
    }
    await this.api.setTimeout(this.id, this.o.keepaliveS * 1000);
  }

  async pause(): Promise<void> {
    if (this.id && this.state === "running") await this.api.pause(this.id).catch(() => false);
    this.state = "paused";
    this.sbx = null;
  }

  async kill(): Promise<void> {
    if (this.id) await this.api.kill(this.id).catch(() => false);
    this.state = "off";
    this.sbx = null;
    this.id = null;
  }

  /** The in-desktop browser host's last report (tabs, wallet address). */
  async hostState(): Promise<any> {
    const r = await this.sh(`cat ${HOME}/state.json 2>/dev/null`, 10_000);
    try { return JSON.parse(r.stdout); } catch { return null; }
  }

  async metrics(): Promise<{ mem_used_mb: number; mem_total_mb: number; load1: number } | null> {
    const r = await this.sh("free -m | awk 'NR==2{print $2, $3}'; cut -d' ' -f1 /proc/loadavg", 10_000);
    const [l1, l2] = r.stdout.trim().split("\n");
    if (!l1) return null;
    const [total, used] = l1.split(" ").map(Number);
    return { mem_total_mb: total!, mem_used_mb: used!, load1: Number(l2) };
  }

  /** Stream, browser host and memory in one command (no secrets read). */
  async checkHealth(): Promise<DeskHealth | null> {
    if (!this.sbx || this.state !== "running") return null;
    const r = await this.sh(
      `now=$(date +%s); t=$(stat -c %Y /tmp/stream/thumb.jpg 2>/dev/null || echo -1); s=$(stat -c %Y ${HOME}/state.json 2>/dev/null || echo -1); ` +
        `e=$(pgrep -x ffmpeg >/dev/null && echo 1 || echo 0); h=$(pgrep -f "[d]eskhost.mjs" >/dev/null && echo 1 || echo 0); ` +
        `m=$(awk '/MemTotal/{t=$2}/MemAvailable/{a=$2}END{print int(t/1024), int(a/1024)}' /proc/meminfo); l=$(cut -d' ' -f1 /proc/loadavg); ` +
        `echo "$now $t $s $e $h $m $l"`,
      15_000,
    );
    const f = r.stdout.trim().split(/\s+/).map(Number);
    if (f.length < 8 || !Number.isFinite(f[0]!)) return null;
    const [now, thumb, st, enc, host, total, avail, load] = f as [number, number, number, number, number, number, number, number];
    const age = thumb > 0 ? Math.max(0, now - thumb) : null;
    const extState = host ? await this.hostState().catch(() => null) : null;
    if (extState?.ext) this.extId = String(extState.ext);
    this.health = {
      checked_at: new Date().toISOString(),
      stream_alive: !!enc && age !== null && age < 10,
      stream_age_s: age,
      encoder: !!enc,
      browser_host: !!host && st > 0 && now - st < 30,
      mem_total_mb: total || null,
      mem_avail_mb: Number.isFinite(avail) ? avail : null,
      load1: Number.isFinite(load) ? load : null,
    };
    return this.health;
  }

  /** Repairs what health found: a dead or frozen stream (restart the encoder), a dead or stuck browser
   * host, or memory under 60 MB (restart the browser host, same profile). The browser is never
   * restarted while the desktop is busy with a task (`busy`). */
  async heal(busy = false): Promise<string[]> {
    const h = this.health;
    if (!h || !this.sbx) return [];
    const done: string[] = [];
    if (!h.encoder || !h.stream_alive) { await this.restartEncoder(); done.push("encoder restarted"); }
    if (!busy && (!h.browser_host || (h.mem_avail_mb !== null && h.mem_avail_mb < 60))) {
      await this.restartHost();
      done.push(h.browser_host ? "browser restarted (memory)" : "browser host restarted");
    }
    return done;
  }

  /** Sets or clears a flag file the browser host reads (`console`: show the activity console when
   * idle; `busy`: a vision task is running). Skips the call when the flag already has that value. */
  flags: Record<string, boolean> = {};
  async setFlag(name: "console" | "busy", on: boolean): Promise<void> {
    if (!this.sbx || this.flags[name] === on) return;
    await this.sh(on ? `touch ${HOME}/${name}` : `rm -f ${HOME}/${name}`, 10_000);
    this.flags[name] = on;
  }

  /** The Cadre Wallet's home screen URL in this desktop's browser, once the browser host reported its id. */
  walletUrl(): string | null { return this.extId ? `chrome-extension://${this.extId}/index.html` : null; }

  async readStream(name: string): Promise<Uint8Array | null> {
    if (!/^(live\.m3u8|s\d{6}\.ts|thumb\.jpg)$/.test(name) || !this.sbx) return null;
    try { return await this.sbx.files.read(`/tmp/stream/${name}`, { format: "bytes" }); } catch { return null; }
  }
}
