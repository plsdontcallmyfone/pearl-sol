// House desktops: unit tests with a fake E2B account, a fake vision model and a fake chain node.
// No network, no ports. The real E2B / OpenRouter path is covered by the live runs (README).
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
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
import { Controller, DeskFailed, secretB58 } from "../src/controller.ts";
import { deskConfig } from "../src/config.ts";

const SITE = "https://cadre.example";

// ------------------------------------------------------------------ fakes

class FakeSbx implements SandboxLike {
  files_ = new Map<string, string>();
  actions: string[] = [];
  cmds: string[] = [];
  seeded = false;
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
      : step === 1 ? { content: "Typing", tool_calls: [call("computer", { action: "type", text: m ? m[1] : "x" }), call("computer", { action: "key", keys: "Return" })] }
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
