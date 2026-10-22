// Practice trading for user agents, spec v2 practice fields, the real-swap flag, the planner, and
// the quick-execution runtime path (owner 2026-10-22): a create signed only by the spec, a handle
// taken on this runtime, CADRE self-funding (buy + bridge from the agent's own SOL), creator
// attribution by the signed spec (no OperatorLink wait), the first action at once, practice
// started for a practising spec, timings in status.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { CadreApi, NodeRpc, latestBlockhash } from "../../src/clients.ts";
import { mulberry32 } from "../../src/swarm.ts";
import { walletFromSeed, type Wallet } from "../../src/wallet.ts";
import { STYLE_IDS } from "../../strategies/src/styles.ts";
import { GatewayAt } from "../src/gateway.ts";
import { checkChoices, parsePlan, Planner } from "../src/plan.ts";
import { practiceCfg, practicePlan, realSwapsOn } from "../src/practice.ts";
import { FEATURES, Runtime, VERSION } from "../src/runtime.ts";
import { seal } from "../src/seal.ts";
import { defaultSpec, defaultSpecV2, defaultTrade, PRACTICE_STYLES, specMessage, specSha256, validatePractice, validateSpec, validateTrade, type AgentSpec } from "../src/spec.ts";
import { PORTS, startFakes2, type Fakes2 } from "./fakes.ts";

const seedOf = (s: string) => createHash("sha256").update(s).digest();
const op = walletFromSeed(seedOf("pr-operator"));
const MINT = "BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e";
const sign = (s: AgentSpec, w: Wallet = op) => { const sha256 = specSha256(s); return { spec: s, sha256, sig: Buffer.from(w.sign(Buffer.from(specMessage(s, sha256)))).toString("hex") }; };

function practising(agent: string, over: Partial<AgentSpec> = {}): AgentSpec {
  return defaultSpecV2(agent, op.address, {
    handle: "tide_practice", name: "Tide Practice", persona: "Practises momentum with pretend money.", created_ms: Date.now(),
    skills: ["town", "x402", "practice"], practice: { style: "momentum", balance_lamports: "10000000000", strategy: "ride strength", take_profit_bps: 3000, stop_loss_bps: 1500 },
    template: "momentum", schedule: { cadence_min: 60, quiet_utc: null }, ...over,
  });
}

// ---------------------------------------------------------------- spec

test("spec v2 practice section: valid, bounded, needs the practice skill; absent keeps every earlier hash", () => {
  const a = walletFromSeed(seedOf("pr-a")).address;
  assert.deepEqual(validateSpec(practising(a)), []);
  const bad = (o: Partial<AgentSpec>) => validateSpec(practising(a, o));
  assert.ok(bad({ skills: ["town", "x402"] }).some((x) => /practice skill/.test(x)));
  assert.ok(bad({ practice: null }).some((x) => /practice section/.test(x)));
  assert.ok(validatePractice({ style: "yolo", balance_lamports: "10000000000", strategy: "" }).some((x) => /style/.test(x)));
  assert.ok(validatePractice({ style: "momentum", balance_lamports: "1", strategy: "" }).some((x) => /balance/.test(x)));
  assert.ok(validatePractice({ style: "momentum", balance_lamports: "1000000000", max_trade_lamports: "2000000000", strategy: "" }).some((x) => /max_trade/.test(x)));
  assert.ok(validatePractice({ style: "momentum", balance_lamports: "1000000000", strategy: "", stop_loss_bps: 9000 }).some((x) => /stop_loss/.test(x)));
  assert.ok(validatePractice({ style: "momentum", balance_lamports: "1000000000", strategy: "", extra: 1 }).some((x) => /only/.test(x)));
  assert.ok(bad({ template: "Not Ok" }).some((x) => /template/.test(x)));
  // v1 cannot carry it
  assert.ok(validateSpec({ ...defaultSpec(a, op.address, { handle: "abc", name: "A" }), practice: practising(a).practice } as any).some((x) => /v2 field/.test(x)));
  // the pinned v2 vector of spec.test.ts is unchanged: the defaults carry no practice, template or real_swaps
  const v = defaultSpecV2("11111111111111111111111111111112", "11111111111111111111111111111113", { handle: "abc", name: "A", created_ms: 1 });
  assert.ok(!("practice" in v) && !("template" in v));
  assert.ok(!("real_swaps" in defaultTrade(MINT)));
});

test("practice styles are the strategies module's styles", () => {
  assert.deepEqual([...PRACTICE_STYLES], STYLE_IDS);
});

test("trade.real_swaps: a boolean; only true turns the real-swap engine on", () => {
  assert.deepEqual(validateTrade({ ...defaultTrade(MINT), real_swaps: true }), []);
  assert.ok(validateTrade({ ...defaultTrade(MINT), real_swaps: "yes" }).some((x) => /real_swaps/.test(x)));
  const a = walletFromSeed(seedOf("pr-b")).address;
  const base = defaultSpecV2(a, op.address, { handle: "abc", name: "A", skills: ["town", "x402", "trade"], actions: { ...defaultSpec(a, op.address).actions, trade: true }, trade: defaultTrade(MINT, { strategy: "ride", take_profit_bps: 40_000, stop_loss_bps: 9_000 }) });
  assert.equal(realSwapsOn(base), false);
  const p = practicePlan(base)!;
  assert.equal(p.source, "trade");
  assert.equal(p.style, "momentum");
  assert.equal(p.balance, 10_000_000_000n);
  assert.equal(p.target_bps, 20_000, "clamped to the strategies module's bounds");
  assert.equal(p.stop_bps, 5_000);
  const real = { ...base, trade: { ...base.trade!, real_swaps: true } };
  assert.equal(realSwapsOn(real), true);
  assert.equal(practicePlan(real), null, "real swaps on: no practice");
  assert.equal(practicePlan(defaultSpec(a, op.address)), null, "v1: nothing");
});

test("practice config: the style's rules, the pretend balance, pinned exits", async () => {
  const a = walletFromSeed(seedOf("pr-c")).address;
  const s = practising(a, { practice: { style: "bounce", balance_lamports: "5000000000", max_trade_lamports: "500000000", strategy: "buy the first bounce", take_profit_bps: 2500 } });
  const c = await practiceCfg(s, practicePlan(s)!);
  assert.equal(c.base.practice_balance_lamports, 5_000_000_000);
  assert.equal(c.base.size.max_lamports, 500_000_000);
  assert.ok(c.base.conditions.some((x: any) => x.signal === "ret_15m" && x.op === "<="));
  assert.deepEqual(c.pin, { stop_bps: undefined, target_bps: 2500 });
  assert.match(c.persona, /buy the first bounce/);
  const none = practising(a, { practice: { style: "calm", balance_lamports: "1000000000", strategy: "" } });
  const c2 = await practiceCfg(none, practicePlan(none)!);
  assert.equal(c2.pin, undefined);
  assert.equal(c2.base.size.max_lamports, 100_000_000, "a tenth of the balance by default");
});

test("pinned exits hold for every strategy version (strategies module)", async () => {
  const { PracticeAgent } = await import("../../strategies/src/agent.ts");
  const a = new PracticeAgent({ handle: "x", name: "X", persona: "x", base: { conditions: [], size: { equity_fraction_bps: 1000, max_lamports: 1, max_pool_share_bps: 1 }, max_risk_bps: 100, max_slippage_bps: 100, practice_balance_lamports: 1 }, pin: { stop_bps: 1500, target_bps: 3000 } }, { stateFile: join(mkdtempSync(join(tmpdir(), "pin-")), "s.json") } as any);
  const out = a.pinned({ stop_bps: 800, target_bps: 1500, edge: { win_rate: 0.4, avg_win_bps: 1500, avg_loss_bps: 800 } } as any);
  assert.equal(out.stop_bps, 1500);
  assert.equal(out.target_bps, 3000);
  assert.deepEqual(out.edge, { win_rate: 0.4, avg_win_bps: 3000, avg_loss_bps: 1500 });
});

// ---------------------------------------------------------------- planner

test("planner: parses the first listed id, checks the choices, caps per day, times out", async () => {
  const ch = [{ id: "momentum", line: "practice-trades strength" }, { id: "explainer", line: "explains things for pay" }];
  assert.equal(parsePlan("explainer", ch), "explainer");
  assert.equal(parsePlan("ID: momentum. (not explainer)", ch), "momentum");
  assert.equal(parsePlan("momentumish", ch), null);
  assert.equal(typeof checkChoices([{ id: "Bad Id", line: "" }]), "string");
  assert.ok(Array.isArray(checkChoices(ch)));
  const fast = new Planner({ ensureRows: async () => {}, think: async () => ({ text: "explainer", requestId: "req_1" }) }, 1);
  assert.equal((await fast.plan("explain rust lifetimes for money", ch))?.template, "explainer");
  assert.equal(await fast.plan("again", ch), null, "per-day cap");
  const slow = new Planner({ ensureRows: async () => {}, think: () => new Promise((r) => setTimeout(() => r({ text: "explainer", requestId: "r" }), 200)) }, 5, 20);
  assert.equal(await slow.plan("x", ch), null, "timeout");
  const noReceipt = new Planner({ ensureRows: async () => {}, think: async () => ({ text: "explainer", requestId: null }) }, 5);
  assert.equal(await noReceipt.plan("x", ch), null, "no receipt, no plan");
});

// ---------------------------------------------------------------- runtime: quick execution

let f: Fakes2;
let rt: Runtime;
let server: any;
const U = (p: string) => `http://127.0.0.1:${PORTS.runtime}${p}`;
const ensured: string[] = [];
const stubPractice = {
  cfg: { tickMs: 1_000_000 },
  ensure: async (r: any) => { if (!ensured.includes(r.address) && practicePlan(r.spec)) { ensured.push(r.address); r.note("practice", "practice trading started (stub)", { ok: true }); return true; } return false; },
  status: (a: string) => (ensured.includes(a) ? { mode: "practice", label: "practice trading: pretend money at real prices, every trade and lesson recorded on Cadre", version: null } : null),
  tick: async () => {},
  info: () => ({ enabled: true, stub: true }),
};

before(async () => {
  f = await startFakes2((msgs) => (/yes or no/i.test(msgs.at(-1)!.content) ? "yes" : "A short post about the tide."));
  const dir = mkdtempSync(join(tmpdir(), "agents-pr-"));
  rt = new Runtime({
    stateDir: dir, masterKeyFile: join(dir, "master.hex"), nodeRpc: `http://127.0.0.1:${PORTS.node}`, gatewayUrl: `http://127.0.0.1:${PORTS.gateway}`,
    apiUrl: `http://127.0.0.1:${PORTS.api}`, publicApiUrl: "https://api.example", solRpc: `http://127.0.0.1:${PORTS.sol}`, network: "test",
    allowHosts: [], maxPerOperator: 5, maxAgents: 10, topupRows: 4096, minFreeRows: 256, tickMs: 10, killFile: join(dir, "STOP"),
  }, (w) => ({
    node: new NodeRpc(`http://127.0.0.1:${PORTS.node}`), gateway: new GatewayAt(`http://127.0.0.1:${PORTS.gateway}`, w), api: new CadreApi(`http://127.0.0.1:${PORTS.api}`),
    latestBlockhash, now: () => Date.now(), rand: mulberry32(5), log: () => {},
  }), { practice: stubPractice as any, planner: null });
  server = rt.listen(PORTS.runtime);
});
after(async () => {
  server.close();
  await f.close();
});

const post = (path: string, body: unknown) => fetch(U(path), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }).then(async (r) => ({ status: r.status, body: await r.json() }));

test("info: version 0.3.0, features, practice and planner state", async () => {
  const i = await fetch(U("/v1/hosted")).then((x) => x.json());
  assert.equal(i.version, VERSION);
  assert.equal(VERSION, "0.3.0");
  assert.deepEqual(i.features, [...FEATURES]);
  assert.equal(i.plan.enabled, false);
  assert.equal((await post("/v1/hosted/plan", { text: "explain things", choices: [{ id: "explainer", line: "explains" }] })).body.error.code, "plan_off");
  assert.equal((await post("/v1/hosted/plan", { text: "", choices: [] })).status, 400);
});

test("create with the signed spec as the only auth (one wallet prompt); stale, v1 and taken handles refused", async () => {
  const pub = Buffer.from((await fetch(U("/v1/hosted")).then((x) => x.json())).seal_pub, "base64");
  const s1 = seedOf("pr-agent-1"), a1 = walletFromSeed(s1);
  const stale = practising(a1.address, { created_ms: Date.now() - 11 * 60_000 });
  assert.equal((await post("/v1/hosted/agents", { sealed: seal(s1, pub), spec: sign(stale) })).body.error.code, "stale_signature");
  const v1 = defaultSpec(a1.address, op.address, { handle: "abc_v1", name: "A", persona: "p", created_ms: Date.now() });
  assert.equal((await post("/v1/hosted/agents", { sealed: seal(s1, pub), spec: sign(v1) })).status, 401);
  const forged = sign(practising(a1.address), walletFromSeed(seedOf("not-the-operator")));
  assert.equal((await post("/v1/hosted/agents", { sealed: seal(s1, pub), spec: forged })).status, 401);
  const ok = await post("/v1/hosted/agents", { sealed: seal(s1, pub), spec: sign(practising(a1.address)) });
  assert.equal(ok.status, 201, JSON.stringify(ok.body));
  assert.equal(ok.body.operator, op.address);
  const again = await post("/v1/hosted/agents", { sealed: seal(s1, pub), spec: sign(practising(a1.address)) });
  assert.equal(again.status, 409, "a replay can only re-create the same agent");
  const s2 = seedOf("pr-agent-2"), a2 = walletFromSeed(s2);
  const taken = await post("/v1/hosted/agents", { sealed: seal(s2, pub), spec: sign(practising(a2.address)) });
  assert.equal(taken.body.error.code, "handle_taken");
});

test("the agent funds its own CADRE, goes live without an OperatorLink, practises and acts at once", async () => {
  const a1 = walletFromSeed(seedOf("pr-agent-1"));
  await rt.loopOnce();
  assert.equal(rt.status(a1.address).state, "waiting_funds");
  assert.match(rt.status(a1.address).why, /SOL/);
  // the creator's one Solana tx: SOL to the agent's wallet
  f.sol.lamports.set(a1.address, 100_000_000n);
  f.gw.rowsFree = 0;
  await rt.loopOnce();
  const sent = f.sol.sent.filter((x) => x.from === a1.address).map((x) => x.builder);
  assert.deepEqual(sent, ["/v1/tx/pump-buy", "/v1/tx/bridge-deposit"], "bought and bridged its own CADRE");
  const buy = f.sol.sent.find((x) => x.builder === "/v1/tx/pump-buy")!;
  assert.equal(buy.body.sol_in, "5000000");
  assert.ok((f.node.balances.get(a1.address) ?? 0n) > 0n, "the fake relayer credited its Cadre account");
  for (let i = 0; i < 8 && rt.status(a1.address).state !== "running"; i++) await rt.loopOnce();
  const st = rt.status(a1.address);
  assert.equal(st.state, "running", st.why);
  assert.ok(!f.node.links.has(`${op.address}:${a1.address}`), "no OperatorLink was needed");
  assert.ok(st.log.some((l: any) => l.kind === "live" && /creator-signed spec/.test(l.text)));
  assert.ok(ensured.includes(a1.address), "practice started");
  assert.equal(st.practice.mode, "practice");
  assert.equal(st.trading, null, "no real-swap engine");
  // its first action ran on the tick it went live: a paid thought and a post, no cadence wait
  await rt.loopOnce();
  const st2 = rt.status(a1.address);
  assert.ok(st2.timings.live_ms && st2.timings.first_thought_ms && st2.timings.first_task_ms, JSON.stringify(st2.timings));
  assert.ok(st2.timings.first_task_ms - st2.timings.live_ms < 5_000);
  assert.equal(st2.cadre_self_funding.state, "bridged");
});

test("a mention wakes an agent before its cadence slot", async () => {
  const a1 = walletFromSeed(seedOf("pr-agent-1"));
  const r: any = rt.runners.get(a1.address)!;
  const s = r.ag(r.me);
  s.nextAt = Date.now() + 3_600_000;
  const mine = f.node.posts.find((p: any) => p.author === a1.address);
  assert.ok(mine, "it posted");
  f.node.posts.push({ ...mine, post_id: "f".repeat(64), kind: "reply", parent: mine.post_id, author: walletFromSeed(seedOf("someone")).address, body: "nice tide", height: 99 });
  r.town = { at: 0, posts: [] };
  r.triggersAt = 0;
  const before = f.node.posts.filter((p: any) => p.author === a1.address && p.kind === "reply").length;
  await rt.loopOnce();
  const after = f.node.posts.filter((p: any) => p.author === a1.address && p.kind === "reply").length;
  assert.equal(after, before + 1, "answered the reply at once");
});
