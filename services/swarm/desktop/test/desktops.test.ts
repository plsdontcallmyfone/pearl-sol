// House desktops: unit tests with a fake E2B account, a fake vision model and a fake chain node.
// No network, no ports. The real E2B / OpenRouter path is covered by the live runs (README).
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createKeyFile, loadWallet } from "../../src/wallet.ts";
import { key32 } from "../../src/b58.ts";
import { REACTION } from "../../src/chaintx.ts";
import { Desk, type E2BApi, type E2BInfo, type SandboxLike } from "../src/desk.ts";
import { Ledger } from "../src/spend.ts";
import { chainTask, workTask, browseTask } from "../src/tasks.ts";
import { runTask, type Chat } from "../src/vision.ts";
import { checkStreamToken, mintStreamToken } from "../src/token.ts";
import { Controller, DEFAULT_HANDLES, DeskFailed, deskIssues, modeAt, secretB58 } from "../src/controller.ts";
import { PERSONAS } from "../../src/personas.ts";
import { containerSandbox, type Runner, selfHostApi, xKey } from "../src/selfhost.ts";
import { deskConfig } from "../src/config.ts";

const SITE = "https://cadre.example";

// ------------------------------------------------------------------ fakes

class FakeSbx implements SandboxLike {
  files_ = new Map<string, string>();
  actions: string[] = [];
  cmds: string[] = [];
  seeded = false;
  healthLine: string | null = null;
  sandboxId: string;
  constructor(id: string) { this.sandboxId = id; }
  commands = {
    run: async (cmd: string) => {
      this.cmds.push(cmd);
      let out = "";
      if (cmd.includes("test -f /home/user/.cadre/provisioned")) out = this.files_.has("/home/user/.cadre/provisioned") ? "yes\nhost\nenc\n" : "";
      if (cmd.includes("deskhost.mjs") && cmd.includes("nohup")) { this.files_.delete("/home/user/.cadre/agent-secret"); this.seeded = true; }
      if (cmd.startsWith("touch /home/user/.cadre/provisioned")) this.files_.set("/home/user/.cadre/provisioned", "");
      if (cmd.startsWith("test -f /home/user/.cadre/agent-secret")) out = this.files_.has("/home/user/.cadre/agent-secret") ? "pending" : "";
      if (cmd.includes("tar -xzf")) out = "ok";
      if (cmd.startsWith("cat /home/user/.cadre/stream.sh")) out = this.files_.get("/home/user/.cadre/stream.sh") ?? "";
      if (cmd.startsWith("cat /home/user/.cadre/deskhost.mjs")) out = this.files_.get("/home/user/.cadre/deskhost.mjs") ?? "";
      if (cmd.startsWith("touch /home/user/.cadre/console")) this.files_.set("/home/user/.cadre/console", "");
      if (cmd.startsWith("rm -f /home/user/.cadre/console")) this.files_.delete("/home/user/.cadre/console");
      if (cmd.includes("/proc/meminfo")) { const n = Math.floor(Date.now() / 1000); out = this.healthLine ?? `${n} ${n - 1} ${n - 3} 1 1 1024 300 0.42`; }
      if (cmd.startsWith("cat /home/user/.cadre/state.json")) out = JSON.stringify({ ext: "extid0000", tabs: [] });
      return { exitCode: 0, stdout: out, stderr: "" };
    },
  };
  files = {
    write: async (p: string, d: string | ArrayBuffer) => { this.files_.set(p, typeof d === "string" ? d : `<${(d as ArrayBuffer).byteLength} bytes>`); },
    read: async (p: string) => new TextEncoder().encode(this.files_.get(p) ?? ""),
  };
  async screenshot() { return new Uint8Array([137, 80, 78, 71]); }
  async leftClick(x?: number, y?: number) { this.actions.push(`click ${x},${y}`); }
  async doubleClick(x?: number, y?: number) { this.actions.push(`dbl ${x},${y}`); }
  async rightClick(x?: number, y?: number) { this.actions.push(`right ${x},${y}`); }
  async moveMouse(x: number, y: number) { this.actions.push(`move ${x},${y}`); }
  async scroll(d?: string, n?: number) { this.actions.push(`scroll ${d} ${n}`); }
  async write(t: string) { this.actions.push(`type ${t}`); }
  async press(k: string | string[]) { this.actions.push(`key ${Array.isArray(k) ? k.join("+") : k}`); }
}

class FakeE2B implements E2BApi {
  boxes = new Map<string, { info: E2BInfo; sbx: FakeSbx }>();
  n = 0;
  log: string[] = [];
  async create(_t: string, o: any) {
    const id = `sbx${++this.n}`;
    const sbx = new FakeSbx(id);
    this.boxes.set(id, { info: { sandboxId: id, state: "running", metadata: o.metadata, startedAt: new Date(), endAt: new Date(Date.now() + o.timeoutMs) }, sbx });
    this.log.push(`create ${id}`);
    return sbx;
  }
  async connect(id: string) { const b = this.boxes.get(id)!; b.info.state = "running"; this.log.push(`connect ${id}`); return b.sbx; }
  async list(m: Record<string, string>) { return [...this.boxes.values()].map((b) => b.info).filter((i) => Object.entries(m).every(([k, v]) => i.metadata[k] === v)); }
  async pause(id: string) { this.boxes.get(id)!.info.state = "paused"; this.log.push(`pause ${id}`); return true; }
  async kill(id: string) { this.log.push(`kill ${id}`); return this.boxes.delete(id); }
  async setTimeout(id: string) { this.log.push(`timeout ${id}`); }
}

/** A vision model that clicks once, types what the task quotes, then calls done. */
function scriptedChat(seen: string[]): Chat {
  let i = 0;
  return async (body: any) => {
    seen.push(JSON.stringify(body.messages));
    const task = body.messages[1].content[0].text as string;
    const m = /<text>([\s\S]*?)<\/text>/.exec(task);
    const step = i++ % 3;
    const call = (name: string, args: unknown) => ({ id: `c${i}`, type: "function", function: { name, arguments: JSON.stringify(args) } });
    const msg = step === 0 ? { content: "Clicking the box", tool_calls: [call("computer", { action: "click", x: 100, y: 200 })] }
      : step === 1 ? { content: "Typing", tool_calls: [body.tools.some((t: any) => t.function.name === "type_given_text") ? call("type_given_text", {}) : call("computer", { action: "type", text: m ? m[1] : "x" }), call("computer", { action: "key", keys: "Return" })] }
      : { content: "", tool_calls: [call("done", { success: true, summary: "Posted on the square" })] };
    return { status: 200, json: { choices: [{ message: msg }], usage: { prompt_tokens: 3000, completion_tokens: 60, cost: 0.0004 } } };
  };
}

const deskOpts = (log: Record<string, unknown>[] = []) => ({ template: "t", resolution: [1280, 800] as [number, number], keepaliveS: 900, sessionMaxS: 86400, siteUrl: SITE, explorerUrl: `${SITE}/explorer`, walletTgz: join(tmpdir(), "cadre-desk-wallet.tgz"), log: (e: Record<string, unknown>) => log.push(e) });
writeFileSync(join(tmpdir(), "cadre-desk-wallet.tgz"), "fake");

// ------------------------------------------------------------------ tests

test("stream token: names one desktop, expires, rejects forgeries", () => {
  const s = "k".repeat(32);
  const t = mintStreamToken(s, "baker_rue", "work", 60);
  assert.ok(checkStreamToken(s, "baker_rue", "work", t));
  assert.ok(!checkStreamToken(s, "baker_rue", "social", t));
  assert.ok(!checkStreamToken(s, "night_miner", "work", t));
  assert.ok(!checkStreamToken("other".repeat(7), "baker_rue", "work", t));
  assert.ok(!checkStreamToken(s, "baker_rue", "work", t, Date.now() + 61_000));
  assert.ok(!checkStreamToken(s, "baker_rue", "work", "garbage"));
});

test("ledger: books E2B seconds and vision dollars per agent, caps the day, rolls at midnight UTC", () => {
  const dir = mkdtempSync(join(tmpdir(), "desk-ledger-"));
  let now = Date.UTC(2026, 9, 21, 23, 0, 0);
  const l = new Ledger(join(dir, "s.json"), 1, () => now);
  l.e2b("a", 3600, 0.0000185);
  l.vision("a", { usd: 0.01, steps: 3, inTokens: 9000, outTokens: 100 });
  assert.equal(Number(l.spent.toFixed(4)), Number((0.0666 + 0.01).toFixed(4)));
  assert.equal(l.capped, false);
  const m = l.measured();
  assert.equal(m.agents, 1);
  assert.equal(Number(m.usd_per_desktop_hour!.toFixed(4)), 0.0766);
  // one desktop for the last hour: headroom = cap - spent - rate x seconds left
  assert.equal(Number(l.visionHeadroom(0.0000185).toFixed(4)), Number((1 - 0.0766 - 0.0666).toFixed(4)));
  l.e2b("a", 3600 * 20, 0.0000185);
  assert.equal(l.capped, true, "over the cap");
  l.save();
  now += 2 * 3600_000;
  const l2 = new Ledger(join(dir, "s.json"), 1, () => now);
  assert.equal(l2.spent, 0, "a new UTC day starts at zero");
  assert.equal(l2.capped, false);
  assert.equal(l2.history.length, 1);
  rmSync(dir, { recursive: true, force: true });
});

test("tasks: a post carries the paid text verbatim and the thought id; tips and follows name their target", () => {
  const post = chainTask(SITE, { kind: "Post", postKind: 0, body: "Fog over the quay at block 41.", thoughtRef: "req-abc" } as any)!;
  assert.equal(post.kind, "post");
  assert.ok(post.text.includes("<text>Fog over the quay at block 41.</text>"));
  assert.ok(post.url!.endsWith("/town?thought=req-abc"));
  const pid = "ab".repeat(32);
  const reply = chainTask(SITE, { kind: "Post", postKind: 1, parent: key32(pid), body: "Agreed.", thoughtRef: "req-r" } as any)!;
  assert.ok(reply.url!.includes(`reply=${pid}`) && reply.url!.includes("thought=req-r"));
  const tip = chainTask(SITE, { kind: "React", items: [{ postId: key32(pid), reaction: REACTION.tip, tip: 1_000_000n }] }, "0.001")!;
  assert.equal(tip.kind, "tip");
  assert.ok(tip.text.includes("0.001"));
  const like = chainTask(SITE, { kind: "React", items: [{ postId: key32(pid), reaction: REACTION.like, tip: 0n }] })!;
  assert.equal(like.kind, "like");
  const target = "GDcEUhU4fXRziN3YQyrwyoZJhsGhSRytJooX7kr7hC4s";
  const fol = chainTask(SITE, { kind: "Follow", items: [{ target: key32(target), on: true }] })!;
  assert.equal(fol.url, `${SITE}/u/${target}`);
  assert.equal(chainTask(SITE, { kind: "Transfer", to: key32(target), amount: 1n }), null, "desktops never move funds");
  for (let i = 0; i < 5; i++) assert.equal(workTask(SITE, `${SITE}/explorer`, target, i).signs, false, "work tasks are read-only");
  for (let i = 0; i < 3; i++) assert.equal(browseTask(SITE, i, [{ handle: "baker_rue" }]).signs, false);
});

test("vision loop: executes the model's actions in order, books its cost, keeps at most two screenshots", async () => {
  const sbx = new FakeSbx("s");
  const seen: string[] = [];
  const r = await runTask(sbx, scriptedChat(seen), { model: "m", system: "sys", task: "Post <text>hello town</text>", maxSteps: 10, maxUsd: 1 });
  assert.equal(r.ok, true);
  assert.equal(r.summary, "Posted on the square");
  assert.deepEqual(sbx.actions, ["click 100,200", "type hello town", "key enter"]);
  assert.equal(Number(r.usd.toFixed(4)), 0.0012);
  assert.equal(r.steps, 3);
  const last = JSON.parse(seen[seen.length - 1]!);
  const imgs = last.filter((m: any) => Array.isArray(m.content) && m.content.some((c: any) => c.type === "image_url")).length;
  assert.ok(imgs <= 2, `at most two screenshots in a request, saw ${imgs}`);
});

test("vision loop: stops at the step limit and at the per-task spend cap", async () => {
  const loop: Chat = async () => ({ status: 200, json: { choices: [{ message: { content: "wait", tool_calls: [{ id: "w", type: "function", function: { name: "computer", arguments: '{"action":"move","x":1,"y":1}' } }] } }], usage: { cost: 0.01 } } });
  const a = await runTask(new FakeSbx("s"), loop, { model: "m", system: "s", task: "t", maxSteps: 3, maxUsd: 1 });
  assert.equal(a.error, "step_limit");
  const b = await runTask(new FakeSbx("s"), loop, { model: "m", system: "s", task: "t", maxSteps: 50, maxUsd: 0.025 });
  assert.equal(b.error, "task_usd_cap");
  assert.ok(b.usd <= 0.035);
});

test("desk: created once, found again by metadata after a restart, duplicates killed, renewed before the session limit", async () => {
  const e2b = new FakeE2B();
  const log: Record<string, unknown>[] = [];
  const d1 = new Desk("baker_rue", "social", "addr", e2b, deskOpts(log));
  await d1.ensure(() => "SECRET58", () => "pw");
  assert.equal(d1.state, "running");
  const sbx = e2b.boxes.get(d1.id!)!.sbx;
  assert.ok(sbx.seeded, "the browser host started and consumed the key file");
  assert.ok(!sbx.files_.has("/home/user/.cadre/agent-secret"), "the key file is gone after seeding");
  // a restarted controller finds the same sandbox (and kills an accidental duplicate)
  await e2b.create("t", { metadata: { cadre_desktop: "baker_rue", cadre_role: "social" }, timeoutMs: 1 });
  e2b.boxes.get("sbx2")!.info.startedAt = new Date(Date.now() - 3600_000);
  const d2 = new Desk("baker_rue", "social", "addr", e2b, deskOpts(log));
  await d2.ensure(() => "SECRET58", () => "pw");
  assert.equal(d2.id, d1.id);
  assert.ok(e2b.log.includes("kill sbx2"));
  // renewal: near the plan's session length the desktop is paused and resumed
  d2.startedAt = Date.now() - (86400 - 1000) * 1000;
  await d2.keepalive();
  assert.ok(e2b.log.includes(`pause ${d1.id}`) && e2b.log.filter((x) => x === `connect ${d1.id}`).length >= 2);
  await d2.keepalive();
  assert.ok(e2b.log.includes(`timeout ${d1.id}`), "otherwise the timeout is pushed");
});

test("controller: a post is typed by the social desktop from the paid thought and resolves to the agent's tx; keys never reach the model", async () => {
  const dir = mkdtempSync(join(tmpdir(), "desk-ctl-"));
  const keys = join(dir, "keys");
  createKeyFile(join(keys, "operator.json"));
  const op = loadWallet(join(keys, "operator.json"));
  const ag = createKeyFile(join(keys, "agent-baker_rue.json"));
  const secret = secretB58(join(keys, "agent-baker_rue.json"));
  let included = false;
  const node = {
    call: async (m: string, p: any) => {
      if (m === "getAccount") return { nonce: 7 };
      if (m === "getAccountTxs") return { txs: included ? [{ tx_hash: "h".repeat(64), from: p.account, nonce: 7 }] : [] };
      if (m === "getMempool") return { size: 0, txs: [] };
      return null;
    },
  };
  const seen: string[] = [];
  const base = scriptedChat(seen);
  const chat: Chat = async (b) => { const r = await base(b); if (JSON.stringify(r.json).includes('"done"')) included = true; return r; };
  const cfg = { ...deskConfig({}), stateDir: dir, killFile: join(dir, "STOP"), keysDir: keys, handles: ["baker_rue"], siteUrl: SITE };
  const e2b = new FakeE2B();
  const ctl = new Controller(cfg, { budget: { rowsPerDay: 1000, agentRowsCap: 1000, topupLamportsPerDay: 0n, transferUnitsPerDay: 0n, agentFloat: 0n }, maxTokens: 48, judgeMaxTokens: 2, topupRows: 0, minFreeRows: 0, mempoolMax: 500, cadenceScale: 1, killFile: join(dir, "STOP"), stateFile: join(dir, "sw.json"), allowHosts: [], house: true, reactShare: 0, followShare: 0, replyShare: 0 },
    { node: node as any, gateway: {} as any, api: {} as any, latestBlockhash: async () => "", now: () => Date.now(), rand: () => 0.5, log: () => {} },
    op, e2b, chat,
    { roles: ["social", "work"], agentsN: 1, parallel: 2, browseGapS: 999, workGapS: 999, tipShare: 0, tipCadre: "0.001", tipUnits: 1n, tipsPerDay: 0, walletTgz: join(tmpdir(), "cadre-desk-wallet.tgz"), passwordFor: () => "pw-not-for-models" });
  await ctl.ensureAll();
  assert.equal(ctl.runningDesks().length, 2, "two desktops for one agent");
  const da = ctl.byHandle.get("baker_rue")!;
  assert.equal(da.wallet.address, ag.address);
  const h = await ctl.viaDesktop(da, { kind: "Post", postKind: 0, body: "Bread at dawn, blocks at noon.", thoughtRef: "req-9" } as any);
  assert.equal(h, "h".repeat(64));
  const sbx = e2b.boxes.get(da.desks.social.id!)!.sbx;
  assert.ok(sbx.actions.includes("type Bread at dawn, blocks at noon."), "typed verbatim");
  assert.ok(da.log.some((l) => l.kind === "tx" && l.tx === h));
  for (const s of seen) {
    assert.ok(!s.includes(secret), "the agent key never reaches the vision model");
    assert.ok(!s.includes("pw-not-for-models"), "the wallet password never reaches the vision model");
  }
  // the status a public API serves carries no sandbox ids and no secrets
  const st = JSON.stringify(ctl.status());
  assert.ok(!st.includes(da.desks.social.id!) && !st.includes(secret));
  // a desktop that produces no tx fails cleanly (the swarm logs it and moves on)
  included = false;
  ctl.findTx = async () => null;
  await assert.rejects(ctl.viaDesktop(da, { kind: "Post", postKind: 0, body: "x", thoughtRef: "r" } as any), DeskFailed);
  // the day cap pauses every desktop
  ctl.ledger.e2b("baker_rue", 1e9, 1);
  await ctl.spendTick();
  assert.equal(ctl.capped, true);
  assert.equal(ctl.runningDesks().length, 0);
  assert.ok([...e2b.boxes.values()].every((b) => b.info.state === "paused"));
  rmSync(dir, { recursive: true, force: true });
});

// ------------------------------------------------------------------ Phase A (docs/DESKTOPS-AT-SCALE.md)

const swarmCfg = (dir: string) => ({ budget: { rowsPerDay: 1000, agentRowsCap: 1000, topupLamportsPerDay: 0n, transferUnitsPerDay: 0n, agentFloat: 0n }, maxTokens: 48, judgeMaxTokens: 2, topupRows: 0, minFreeRows: 0, mempoolMax: 500, cadenceScale: 1, killFile: join(dir, "STOP"), stateFile: join(dir, "sw.json"), allowHosts: [], house: true, reactShare: 0, followShare: 0, replyShare: 0 });

test("encoder script: one loop per desktop and ffmpeg overwrites the old still after a crash", () => {
  const sh = readFileSync(new URL("../sandbox/stream.sh", import.meta.url), "utf8");
  assert.match(sh, /flock -n 9 \|\| exit 0/);
  assert.match(sh, /ffmpeg -y /);
});

test("split: desktop agents come from the end of the persona list, so the headless swarm's first 30 - N never overlap", () => {
  assert.equal(PERSONAS.length, 30);
  const all = DEFAULT_HANDLES();
  assert.equal(all.length, 30, "every house persona can have a desktop");
  assert.equal(all[0], PERSONAS[29]!.handle);
  for (let n = 0; n <= 30; n++) {
    const desk = new Set(all.slice(0, n));
    const swarm = PERSONAS.slice(0, 30 - n).map((p) => p.handle);
    assert.ok(swarm.every((h) => !desk.has(h)), `no agent driven twice at DESK_AGENTS=${n}`);
    assert.equal(desk.size + swarm.length, 30, `every agent is driven by one of them at DESK_AGENTS=${n}`);
  }
});

test("mode scheduler: alternates Social and Work per slot, staggered across agents", () => {
  const slot = 1800_000;
  const t0 = Date.UTC(2026, 9, 21, 20, 0, 0);
  const at = (t: number) => Array.from({ length: 30 }, (_, i) => modeAt(i, t, slot));
  const m0 = at(t0);
  assert.equal(m0.filter((m) => m === "social").length, 15, "half the agents in each mode");
  assert.deepEqual(at(t0 + slot), m0.map((m) => (m === "social" ? "work" : "social")), "every agent flips at the next slot");
  assert.deepEqual(at(t0 + slot - 1), m0, "stable within a slot");
});

test("Phase A controller: first agents keep two desktops, the rest one; solo work tasks follow the mode; a chain action runs on the solo desktop in Work mode", async () => {
  const dir = mkdtempSync(join(tmpdir(), "desk-phasea-"));
  const keys = join(dir, "keys");
  createKeyFile(join(keys, "operator.json"));
  const op = loadWallet(join(keys, "operator.json"));
  const handles = ["ferry_nan", "cipher_kit", "chef_ottoline"];
  for (const h of handles) createKeyFile(join(keys, `agent-${h}.json`));
  let included = false;
  const node = { call: async (m: string, p: any) => (m === "getAccount" ? { nonce: 3 } : m === "getAccountTxs" ? { txs: included ? [{ tx_hash: "e".repeat(64), from: p.account, nonce: 3 }] : [] } : m === "getMempool" ? { size: 0, txs: [] } : null) };
  const seen: string[] = [];
  const base = scriptedChat(seen);
  const chat: Chat = async (b) => { const r = await base(b); if (JSON.stringify(r.json).includes('"done"')) included = true; return r; };
  const cfg = { ...deskConfig({}), stateDir: dir, killFile: join(dir, "STOP"), keysDir: keys, handles, siteUrl: SITE, explorerUrl: `${SITE}/explorer` };
  const e2b = new FakeE2B();
  const ctl = new Controller(cfg, swarmCfg(dir), { node: node as any, gateway: {} as any, api: {} as any, latestBlockhash: async () => "", now: () => Date.now(), rand: () => 0.5, log: () => {} }, op, e2b, chat,
    { roles: ["social", "work"], agentsN: 3, parallel: 4, browseGapS: 1, workGapS: 1, soloGapS: 1, modeSlotS: 1800, twoDeskAgents: 1, tipShare: 0, tipCadre: "0.001", tipUnits: 1n, tipsPerDay: 0, walletTgz: join(tmpdir(), "cadre-desk-wallet.tgz"), passwordFor: () => "pw" });
  await ctl.ensureAll();
  assert.equal(ctl.runningDesks().length, 4, "1 agent x 2 + 2 agents x 1");
  const [two, s1, s2] = handles.map((h) => ctl.byHandle.get(h)!);
  assert.deepEqual(two!.roles, ["social", "work"]);
  assert.deepEqual(s1!.roles, ["social"]);
  assert.equal(s1!.solo && s2!.solo && !two!.solo, true);
  assert.notEqual(s1!.mode, s2!.mode, "neighbours are staggered");
  // health first, so the browser host's wallet id is known for a solo Work-mode wallet task
  await ctl.healthTick();
  const solo = s1!.mode === "work" ? s1! : s2!;
  assert.equal(solo.desks.social.walletUrl(), "chrome-extension://extid0000/index.html");
  assert.equal(solo.desks.social.health?.stream_alive, true);
  assert.equal(ctl.healthSummary().unhealthy.length, 0);
  await ctl.background();
  while (ctl.visionInFlight > 0) await new Promise((r) => setTimeout(r, 100));
  const workKinds = ["wallet", "explorer_account", "explorer_blocks", "market", "jobs"];
  const socialKinds = ["read_town", "read_profile", "read_leaderboards"];
  const done = (a: typeof solo) => a.log.filter((l) => l.kind === "task").map((l) => l.text.replace(" ", "_"));
  assert.ok(done(solo).every((k) => workKinds.includes(k)) && done(solo).length === 1, `work-mode solo agent ran a work task: ${done(solo)}`);
  const other = solo === s1 ? s2! : s1!;
  assert.ok(done(other).every((k) => socialKinds.includes(k)) && done(other).length === 1, `social-mode solo agent browsed: ${done(other)}`);
  assert.equal(done(two!).length, 2, "the two-desktop agent ran one task on each desktop");
  // a chain action while in Work mode still runs on the solo agent's only (signing) desktop
  included = false;
  const h = await ctl.viaDesktop(solo, { kind: "Post", postKind: 0, body: "Ledger says hello.", thoughtRef: "req-solo" } as any);
  assert.equal(h, "e".repeat(64));
  assert.ok(solo.log.some((l) => l.kind === "mode" && l.text.includes("paused for a post")));
  assert.ok(seen.some((x) => x.includes("thought=req-solo")), "the post task carries the thought id for thought_ref");
  // the mode flips at the next slot and is logged
  ctl.scheduleModes(Date.now() + 1800_000);
  assert.ok(solo.log.some((l) => l.kind === "mode" && l.text.startsWith("Social mode")));
  // status: layout, per-desktop health and activity, no sandbox ids
  const st: any = ctl.status();
  assert.deepEqual(st.layout, { two_desktops: 1, one_desktop: 2, mode_slot_s: 1800 });
  assert.equal(st.desktops_total, 4);
  const card = st.agents.find((a: any) => a.handle === solo.persona.handle);
  assert.equal(card.layout, "one_desktop");
  assert.deepEqual(Object.keys(card.desktops), ["social"]);
  assert.equal(card.desktops.social.health.mem_avail_mb, 300);
  assert.ok(!JSON.stringify(st).includes(solo.desks.social.id!));
  // the activity console: on the Work desktop and on the solo agent in Work mode, not on Social desktops
  const flagOf = (a: typeof solo, r: "social" | "work") => e2b.boxes.get(a.desks[r].id!)!.sbx.files_.has("/home/user/.cadre/console");
  assert.equal(flagOf(two!, "work"), true);
  assert.equal(flagOf(two!, "social"), false);
  // runtime events (the fast executor's trades) land in the agent's log for the console
  assert.equal(ctl.activity(solo.persona.handle, { kind: "trade", text: "bought 1.2 CADRE for 0.01 SOL", tx: "abc12345" }), true);
  assert.equal(ctl.activity("nobody", { kind: "trade", text: "x" }), false);
  assert.ok(solo.log.some((l) => l.kind === "trade" && l.tx === "abc12345"));
  // a dead encoder is flagged and restarted
  const sbx = e2b.boxes.get(solo.desks.social.id!)!.sbx;
  const n = Math.floor(Date.now() / 1000);
  sbx.healthLine = `${n} ${n - 60} ${n - 2} 0 1 1024 60 0.9`;
  await ctl.healthTick();
  const bad = ctl.healthSummary().unhealthy.find((x) => x.agent === solo.persona.handle)!;
  assert.ok(bad.issues.includes("stream encoder down") && bad.issues.some((i) => i.startsWith("memory low")), JSON.stringify(bad));
  assert.ok(sbx.cmds.some((c) => c.includes("stream.sh") && c.includes("nohup")), "encoder restarted");
  assert.ok(sbx.cmds.some((c) => c.includes("kill -9") && c.includes("rm -f /tmp/stream/thumb.jpg")), "old loops killed and the old still removed first");
  // a frozen stream (encoder running, still 400 s old) is restarted too; low memory restarts the browser when idle
  const before = sbx.cmds.length;
  sbx.healthLine = `${n} ${n - 400} ${n - 2} 1 1 1024 40 0.9`;
  await ctl.healthTick();
  const fresh = sbx.cmds.slice(before);
  assert.ok(fresh.some((c) => c.includes("rm -f /tmp/stream/thumb.jpg")), "frozen stream: encoder restarted");
  assert.ok(fresh.some((c) => c.includes("deskhost.mjs") && c.includes("nohup")), "memory under 60 MB: browser host restarted");
  // no process match can hit its own shell (2026-10-21: pkill -f "[s]tream.sh" in a command naming stream.sh killed itself)
  for (const c of sbx.cmds) {
    for (const m of c.matchAll(/p(?:grep|kill)\s+(?:-\S+\s+)*-f\s+"([^"]+)"/g)) assert.ok(/^\[/.test(m[1]!), `bracketed pattern: ${m[1]}`);
  }
  rmSync(dir, { recursive: true, force: true });
});

test("health: issues name a dead stream, a dead browser host, low memory and an idle desktop", () => {
  const h = { checked_at: "", stream_alive: false, stream_age_s: 40, encoder: true, browser_host: false, mem_total_mb: 1024, mem_avail_mb: 50, load1: 1 };
  const t = 10_000_000;
  assert.deepEqual(deskIssues(h, t - 100, 1000, t, 0), ["stream stale (40 s)", "browser host down", "memory low (50 MB free)"]);
  assert.deepEqual(deskIssues({ ...h, stream_alive: true, browser_host: true, mem_avail_mb: 500 }, t - 5000, 1000, t, 0), ["no action for 0 min"]);
  assert.deepEqual(deskIssues({ ...h, stream_alive: true, browser_host: true, mem_avail_mb: 500 }, 0, 1000, t, t - 10), [], "not flagged before the controller ran 3 gaps");
  assert.deepEqual(deskIssues(null, 0, 1000, t, 0), []);
});

test("self-hosted host: podman run with per-desktop limits and labels, list by label, exec for files, screenshots, mouse and keys", async () => {
  const calls: { args: string[]; input?: string }[] = [];
  const run: Runner = async (args, input) => {
    calls.push({ args, input: input === undefined ? undefined : String(input) });
    if (args[0] === "ps") return { code: 0, stdout: Buffer.from(JSON.stringify([{ Names: ["cadre-desk-a-social-1"], State: "running", Labels: { cadre_desktop: "a", cadre_role: "social" }, StartedAt: 1_700_000_000 }, { Names: ["gone"], State: "exited", Labels: {} }])), stderr: "" };
    if (args[0] === "exec" && args.includes("import")) return { code: 0, stdout: Buffer.from([137, 80, 78, 71]), stderr: "" };
    if (args[0] === "exec" && args.includes("cat")) return { code: 0, stdout: Buffer.from("hello"), stderr: "" };
    if (args[0] === "inspect") return { code: 0, stdout: Buffer.from("paused\n"), stderr: "" };
    return { code: 0, stdout: Buffer.from(""), stderr: "" };
  };
  const host = selfHostApi({ image: "img", cpus: 0.5, memoryMb: 1024, slice: "desk.slice", network: "pasta", usdPerHour: 0.036, run });
  assert.equal(host.kind, "selfhost");
  assert.equal(Number((host.usdPerS! * 3600).toFixed(3)), 0.036);
  const sbx = await host.create("ignored", { metadata: { cadre_desktop: "a", cadre_role: "social" }, resolution: [1280, 800] });
  const runArgs = calls.find((c) => c.args[0] === "run")!.args;
  for (const want of ["--cpus", "0.5", "--memory", "1024m", "--cgroup-parent", "desk.slice", "--network", "pasta", "cadre_desktop=a", "cadre_role=social", "cadre_selfhost=1", "DESK_W=1280", "img"]) assert.ok(runArgs.includes(want), `podman run has ${want}`);
  assert.ok(!runArgs.includes("-p") && !runArgs.includes("--publish"), "no port leaves the container");
  const list = await host.list({ cadre_desktop: "a" });
  assert.equal(list.length, 1);
  assert.equal(list[0]!.state, "running");
  assert.ok(calls.some((c) => c.args[0] === "ps" && c.args.includes("label=cadre_desktop=a")));
  await host.connect("cadre-desk-a-social-1", {});
  assert.ok(calls.some((c) => c.args[0] === "unpause"), "a paused desktop is unpaused on connect");
  await sbx.files.write("/home/user/.cadre/agent-secret", "S3CRET");
  const w = calls.find((c) => c.input === "S3CRET")!;
  assert.ok(w.args.includes("-i") && !w.args.join(" ").includes("S3CRET"), "secrets go through stdin, never argv");
  assert.equal(Buffer.from(await sbx.files.read("/x", { format: "bytes" })).toString(), "hello");
  assert.deepEqual([...(await sbx.screenshot())].slice(0, 4), [137, 80, 78, 71]);
  await sbx.leftClick(10, 20);
  await sbx.press(["ctrl", "l"]);
  await sbx.press("enter");
  await sbx.scroll("up", 2);
  const xdo = calls.filter((c) => c.args.includes("xdotool")).map((c) => c.args.slice(c.args.indexOf("xdotool") + 1).join(" "));
  assert.deepEqual(xdo, ["mousemove 10 20 click 1", "key -- ctrl+l", "key -- Return", "click --repeat 2 4"]);
  assert.equal(xKey("page_down"), "Next");
  const c2 = containerSandbox("n", run);
  await c2.commands.run("echo hi", { background: true });
  assert.ok(calls.some((c) => c.args[0] === "exec" && c.args[1] === "-d"));
});
