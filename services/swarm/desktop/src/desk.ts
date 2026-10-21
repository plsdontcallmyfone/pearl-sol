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

/** The account calls (the real one is @e2b/desktop; tests pass a fake). */
export interface E2BApi {
  create(template: string, opts: Record<string, unknown>): Promise<SandboxLike>;
  connect(id: string, opts: Record<string, unknown>): Promise<SandboxLike>;
  list(metadata: Record<string, string>): Promise<E2BInfo[]>;
  pause(id: string): Promise<boolean>;
  kill(id: string): Promise<boolean>;
  setTimeout(id: string, ms: number): Promise<void>;
}

export async function sdkApi(apiKey: string): Promise<E2BApi> {
  const { Sandbox } = (await import("@e2b/desktop")) as any;
  const pick = (i: any): E2BInfo => ({ sandboxId: i.sandboxId, state: i.state, metadata: i.metadata ?? {}, startedAt: new Date(i.startedAt), endAt: new Date(i.endAt) });
  return {
    create: (t, o) => Sandbox.create(t, { ...o, apiKey }),
    connect: (id, o) => Sandbox.connect(id, { ...o, apiKey }),
    list: async (metadata) => {
      const p = Sandbox.list({ apiKey, query: { metadata, state: ["running", "paused"] } });
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
  handle: string;
  role: Role;
  address: string;
  private api: E2BApi;
  private o: DeskOpts;
  constructor(handle: string, role: Role, address: string, api: E2BApi, o: DeskOpts) {
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
    const r = await this.sh(`test -f ${HOME}/provisioned && echo yes; pgrep -f deskhost.mjs >/dev/null && echo host; pgrep -f stream.sh >/dev/null && echo enc`);
    const out = r.stdout;
    if (!out.includes("yes")) await this.provision(secret(), password());
    else {
      if (!out.includes("host")) await this.startHost();
      // a newer encoder script from this controller replaces the running one
      const enc = readFileSync(join(HERE, "../sandbox/stream.sh"), "utf8");
      const cur = await this.sh(`cat ${HOME}/stream.sh 2>/dev/null`);
      if (cur.stdout !== enc) {
        await this.sbx!.files.write(`${HOME}/stream.sh`, enc);
        await this.sh(`chmod +x ${HOME}/stream.sh; pkill -f "[s]tream.sh"; pkill -x ffmpeg; true`);
        await this.startEncoder();
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

  private async startHost() {
    const [w, h] = this.o.resolution;
    await this.sbx!.commands.run(
      `cd ${HOME} && DISPLAY=:0 DESK_W=${w} DESK_H=${h} PLAYWRIGHT_BROWSERS_PATH=/opt/ms-playwright nohup node deskhost.mjs ${this.role} ${this.o.siteUrl} ${this.o.explorerUrl} >> deskhost.log 2>&1 &`,
      { background: true },
    );
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

  async readStream(name: string): Promise<Uint8Array | null> {
    if (!/^(live\.m3u8|s\d{6}\.ts|thumb\.jpg)$/.test(name) || !this.sbx) return null;
    try { return await this.sbx.files.read(`/tmp/stream/${name}`, { format: "bytes" }); } catch { return null; }
  }
}
