// Swarms on the hosted runtime (src/swarms.ts, docs/SWARMS.md), in process (no ports): one
// creator-signed request hosts every member and signs each member's acceptance with that
// member's own key; refusals leave nothing hosted; register: runtime sends create_signed from the
// lead and picks the id up by definition; the lead's hires prefer its members' offers through a
// paid thought that sees the goal; members deliver swarm hires first; trading members get
// distinct practice styles. The definition vector matches contracts/tests/tests/swarms.rs.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { b58decode } from "../../src/b58.ts";
import { verifySig, walletFromSeed, type Wallet } from "../../src/wallet.ts";
import { contractAddress } from "../../trading/src/cadre.ts";
import { Runtime } from "../src/runtime.ts";


import { acceptMessage, creatorMessage, definitionHash, MAX_BATCH } from "../src/swarmabi.ts";

const seedOf = (s: string) => createHash("sha256").update(s).digest();
const op = walletFromSeed(seedOf("sw-creator"));
const other = walletFromSeed(seedOf("sw-stranger"));
const CONTRACT = walletFromSeed(seedOf("sw-contract")).address;

test("definition vector matches the contract (contracts/tests/tests/swarms.rs)", () => {
  const pk = (i: number) => walletFromSeed(new Uint8Array(32).fill(i));
  const contract = contractAddress(pk(1).publicKey, 1n);
  const d = { contract, creator: pk(3).address, name: "research crew", goalHash: "07".repeat(32), goalUri: "blob:goal", lead: pk(20).address, members: [0, 1, 2].map((i) => ({ agent: pk(20 + i).address, role: i === 0 ? "lead" : `worker-${i}` })), salt: 42n };
  assert.equal(Buffer.from(b58decode(contract)).toString("hex"), "312bf99bb0880c26c7e4cbe26399f25acbd7d5160ecdf5c480dfebc17d9d1bc8");
  assert.equal(definitionHash(d), "1744351920b0244169489519c095d632cbdc3df519bcc06d6572fe4fe2ec12e8");
  assert.match(creatorMessage(contract, definitionHash(d)), /^Cadre swarm v1\ncontract: 312bf9.*\ndefinition: 174435.*\nThis creates a public grouping of sovereign agents\. It gives you no control over them and moves no funds\.$/s);
});

/** In-process fake node: enough for create, the registration tick and blob puts. */
class Node {
  sent: Buffer[] = [];
  blobs = new Map<string, Buffer>();
  byDef = new Map<string, number>();
  balances = new Map<string, bigint>();
  async call(method: string, p: any): Promise<any> {
    switch (method) {
      case "getParams": return { min_fee: "1000", social_min_fee: "1000", gas_price: "10000", contract_base_fee: "1000", contract_byte_fee: "10", tx_gas_limit: "50000000" };
      case "getChainInfo": return { chain_id: 7, height: 100 };
      case "getAccount": return { balance: (this.balances.get(p.account) ?? 0n).toString(), nonce: 0 };
      case "simulateCall": return { ok: true, gas_used: 1_800_000 };
      case "getTx": return null;
      case "queryContract": {
        const args = Buffer.from(p.args, "hex");
        const def = args.subarray(1 + args[0]).toString("hex");
        const id = this.byDef.get(def) ?? 0;
        const out = Buffer.alloc(8);
        out.writeBigUInt64LE(BigInt(id));
        return { ok: true, return: out.toString("hex") };
      }
      default: throw new Error(`fake node: ${method}`);
    }
  }
  async sendTx(tx: Uint8Array) { this.sent.push(Buffer.from(tx)); return { tx_hash: createHash("sha256").update(tx).digest("hex") }; }
  async putBlob(data: Uint8Array) { const id = (await import("../../src/blake3.ts")).blake3(data); const h = Buffer.from(id).toString("hex"); this.blobs.set(h, Buffer.from(data)); return { blob_id: h, uri: `blob:${h}`, size: data.length }; }
}

function setup(o: { config?: boolean; dir?: string } = {}) {
  const dir = o.dir ?? mkdtempSync(join(tmpdir(), "agents-swarm-"));
  const cfgFile = join(dir, "swarms.json.deploy");
  if (o.config !== false) writeFileSync(cfgFile, JSON.stringify({ contract: CONTRACT, start_height: 0 }));
  process.env.SWARMS_CONFIG = o.config === false ? join(dir, "missing.json") : cfgFile;
  process.env.AGENTS_SWARM_KICKOFF_WAIT_MS = "300";
  process.env.AGENTS_SWARM_KICKOFF_POLL_MS = "20";
  delete process.env.STRAT_MAINNET_RPC;
  delete process.env.STRAT_MAINNET_RPC_FILE;
  const node = new Node();
  let t = 1_792_500_000_000;
  const rt = new Runtime({
    stateDir: dir, masterKeyFile: join(dir, "master.hex"), nodeRpc: "http://127.0.0.1:1", gatewayUrl: "http://127.0.0.1:1", apiUrl: "http://127.0.0.1:1",
    publicApiUrl: "https://api.example", network: "test", allowHosts: [], maxPerOperator: 2, maxAgents: 50, topupRows: 4096, minFreeRows: 256, tickMs: 10, killFile: join(dir, "STOP"),
  }, () => ({ node: node as any, gateway: {} as any, api: {} as any, latestBlockhash: async () => "x", now: () => (t += 61_000), rand: () => 0.5, log: () => {} }));
  return { rt, node, dir };
}

/** Wait until the background first ticks of a just-committed swarm are done. */
async function settle(rt: Runtime) {
  for (let i = 0; i < 200 && (rt as any).busy.size; i++) await new Promise((res) => setTimeout(res, 5));
}
const creates = (node: Node) => node.sent.filter((t) => t.includes(Buffer.from("create_signed")));

async function post(rt: Runtime, path: string, body: unknown) {
  return rt.handle("POST", path, {} as any, JSON.stringify(body)).then((r) => r, (e) => ({ status: e.status, body: { error: { code: e.code, message: e.message } } })) as Promise<any>;
}

const GOAL = "Map every ship that calls at the harbour this week and write a short daily report.";

async function planAndCommit(rt: Runtime, size = 4, extra: Record<string, unknown> = {}) {
  const plan = await post(rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: GOAL, size, total_sol_lamports: "400000003", ...extra });
  assert.equal(plan.status, 200, JSON.stringify(plan.body));
  const sig = Buffer.from(op.sign(Buffer.from(plan.body.creator_message, "utf8"))).toString("hex");
  const t0 = Date.now();
  const c = await post(rt, "/v1/hosted/swarms", { plan_id: plan.body.plan_id, creator_sig: sig });
  return { plan: plan.body, commit: c, sig, t0 };
}

test("minimal input: goal, size and total SOL; one signature covers the swarm and every member spec", async () => {
  const { rt } = setup();
  const { plan, commit } = await planAndCommit(rt, 4);
  assert.equal(commit.status, 201, JSON.stringify(commit.body));
  // inferred: name, kind, lead, roles; SOL split evenly with the remainder to the lead
  assert.equal(plan.swarm.kind, "general");
  assert.equal(plan.swarm.name, "Map every ship that calls at");
  assert.deepEqual(plan.members.map((m: any) => m.role), ["lead", "researcher", "writer", "analyst"]);
  assert.deepEqual(plan.members.map((m: any) => m.sol_lamports), ["100000003", "100000000", "100000000", "100000000"]);
  assert.ok(plan.inferred.lead && plan.inferred.roles);
  // more than maxPerOperator (2): swarm agents have their own per-creator cap
  assert.equal(rt.entries.size, 4);
  const def = plan.plan_id;
  for (const m of commit.body.members) assert.ok(verifySig(b58decode(m.agent), acceptMessage(CONTRACT, def), Buffer.from(m.accept_sig, "hex")), "each member accepts with its own key");
  // every hosted spec verifies through the creator's one swarm signature
  for (const m of plan.members) {
    const e = rt.entries.get(m.agent)!;
    assert.equal(e.doc.sig, "");
    assert.equal(e.doc.swarm!.creator_sig.length, 128);
    assert.equal(e.doc.sha256, m.spec_sha256);
    assert.equal(e.doc.spec.format, "cadre-agent-spec-v2");
  }
  // a spec edited after the signature no longer verifies
  const { verifySigned } = await import("../src/spec.ts");
  const d0 = rt.entries.get(plan.members[1].agent)!.doc;
  assert.equal(verifySigned(d0).ok, true);
  const forged = { ...d0, spec: { ...d0.spec, persona: "someone else" } };
  forged.sha256 = (await import("../src/spec.ts")).specSha256(forged.spec);
  assert.equal(verifySigned(forged).ok, false);
  // lead hires, members offer; the lead's persona carries the goal
  const lead = rt.runners.get(plan.swarm.lead)!;
  assert.equal(lead.spec.actions.hire, true);
  assert.equal(rt.runners.get(plan.members[1].agent)!.spec.actions.offer, true);
  assert.match(lead.me.persona.persona, /You lead it/);
  assert.match(lead.me.persona.persona, /Map every ship/);
  // the plan is used once
  assert.equal((await post(rt, "/v1/hosted/swarms", { plan_id: def, creator_sig: "00".repeat(64) })).body.error.code, "no_plan");
  const l: any = await rt.handle("GET", "/v1/hosted/swarms?creator=" + op.address, {}, "");
  assert.equal(l.body.total, 1);
});

test("trading goals get distinct practice styles; overrides win", async () => {
  const { rt } = setup();
  const p = await post(rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: "Trade pump tokens with three different styles and compare what each learns.", size: 4, total_sol_lamports: "4000" });
  assert.equal(p.body.swarm.kind, "trading");
  assert.deepEqual(p.body.members.map((m: any) => m.practice_style), [null, "momentum", "bounce", "calm"]);
  assert.equal(p.body.members[1].spec.practice.style, "momentum");
  assert.ok(p.body.members[1].spec.skills.includes("practice"));
  const q = await post(rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: GOAL, size: 3, total_sol_lamports: "3000", overrides: { lead: 2, roles: ["scout", "scribe", "lead"], name: "Harbour" } });
  assert.equal(q.status, 200, JSON.stringify(q.body));
  assert.equal(q.body.swarm.lead, q.body.members[2].agent);
  assert.equal(q.body.swarm.name, "Harbour");
});

test("refusals: bad signature, bad input, no contract", async () => {
  const { rt } = setup();
  const plan = await post(rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: GOAL, size: 2, total_sol_lamports: "2000" });
  const wrong = Buffer.from(other.sign(Buffer.from(plan.body.creator_message, "utf8"))).toString("hex");
  assert.equal((await post(rt, "/v1/hosted/swarms", { plan_id: plan.body.plan_id, creator_sig: wrong })).body.error.code, "bad_creator_sig");
  assert.equal(rt.entries.size, 0);
  assert.equal((await post(rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: GOAL, size: MAX_BATCH + 1, total_sol_lamports: "100000" })).body.error.code, "bad_swarm");
  assert.equal((await post(rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: "", size: 2, total_sol_lamports: "100" })).body.error.code, "bad_swarm");
  assert.equal((await post(rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: GOAL, size: 3, total_sol_lamports: "2" })).body.error.code, "bad_swarm");
  const off = setup({ config: false });
  assert.equal((await post(off.rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: GOAL, size: 2, total_sol_lamports: "2000" })).body.error.code, "swarms_not_deployed");
});

// security review 2026-10-23: plans need no signature, so a flood of them (random creators) used to
// fill the 64-slot table and refuse every real creator ("busy") for 15 minutes
test("plan flood: one pending plan per creator, and a full table drops its oldest plan instead of refusing", async () => {
  const { rt } = setup();
  const first = await post(rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: GOAL, size: 2, total_sol_lamports: "2000" });
  const again = await post(rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: GOAL, size: 2, total_sol_lamports: "2000" });
  const sw = (rt as any).swarms;
  assert.equal(sw.plans.has(first.body.plan_id), false, "the creator's newer plan replaced the older one");
  assert.equal(sw.plans.has(again.body.plan_id), true);
  for (let i = 0; i < 520; i++) {
    const r = await post(rt, "/v1/hosted/swarms/plan", { creator: walletFromSeed(seedOf(`flood-${i}`)).address, goal: GOAL, size: 2, total_sol_lamports: "2000" });
    assert.equal(r.status, 200, JSON.stringify(r.body));
  }
  assert.ok(sw.plans.size <= 512);
  const real = await post(rt, "/v1/hosted/swarms/plan", { creator: other.address, goal: GOAL, size: 2, total_sol_lamports: "2000" });
  assert.equal(real.status, 200, "a real creator still gets a plan after the flood");
  const sig = Buffer.from(other.sign(Buffer.from(real.body.creator_message, "utf8"))).toString("hex");
  assert.equal((await post(rt, "/v1/hosted/swarms", { plan_id: real.body.plan_id, creator_sig: sig })).status, 201);
});

test("registration: the sponsor registers at once (with each member's CADRE float); else the lead does, and the id is picked up by definition", async () => {
  const s1 = setup();
  const sent: { args: Buffer; value: bigint }[] = [];
  s1.rt.swarms.sponsorCache = { address: other.address, send: async (args, value) => { sent.push({ args, value }); return "ab".repeat(32); } };
  process.env.AGENTS_SWARM_FLOAT_UNITS = "5000";
  const a = await planAndCommit(s1.rt, 3);
  delete process.env.AGENTS_SWARM_FLOAT_UNITS;
  await new Promise((res) => setTimeout(res, 10));
  assert.equal(sent.length, 1, "one create_signed tx, no member waits for CADRE first");
  assert.equal(sent[0].value, 15000n);
  assert.ok(sent[0].args.includes(Buffer.from("create_signed")));
  assert.ok(sent[0].args.includes(Buffer.from(a.sig, "hex")), "it carries the creator's one signature");
  assert.equal(s1.rt.swarms.swarms.get(a.plan.plan_id)!.registration.state, "sent");
  // without a sponsor: the lead sends it once it runs and holds CADRE
  const { rt, node } = setup();
  const b = await planAndCommit(rt, 2);
  const def = b.plan.plan_id;
  const lead = rt.runners.get(b.plan.swarm.lead)!;
  await settle(rt);
  await rt.swarms.tick();
  assert.equal(creates(node).length, 0);
  lead.state = "running";
  node.balances.set(lead.address, 10_000_000_000n);
  rt.swarms.swarms.get(def)!.registration.checked_ms = 0;
  await rt.swarms.tick();
  assert.equal(creates(node).length, 1);
  assert.equal(Buffer.from(creates(node)[0].subarray(5, 37)).toString("hex"), Buffer.from(lead.me.wallet.publicKey).toString("hex"), "signed by the lead");
  node.byDef.set(def, 7);
  rt.swarms.swarms.get(def)!.registration.checked_ms = 0;
  await rt.swarms.tick();
  assert.equal(rt.swarms.swarms.get(def)!.registration.state, "registered");
  assert.equal(rt.runners.get(b.plan.members[1].agent)!.swarm!.id, 7);
});

test("kickoff: the lead's first paid thought on the goal starts as soon as its SOL lands, then it hires that member", async () => {
  const { rt } = setup();
  // the lead's SOL arrives 300 ms after the commit (the creator's Solana tx)
  const plan = await post(rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: GOAL, size: 3, total_sol_lamports: "3000" });
  const sig = Buffer.from(op.sign(Buffer.from(plan.body.creator_message, "utf8"))).toString("hex");
  const { AgentRunner } = await import("../src/agent.ts");
  const origEnsure = AgentRunner.prototype.ensureBalance;
  const origThink = AgentRunner.prototype.think;
  const origLam = AgentRunner.prototype.lamports;
  AgentRunner.prototype.lamports = async function () { return solAt > 0 && Date.now() >= solAt ? 1n : 0n; };
  let solAt = 0;
  AgentRunner.prototype.ensureBalance = async function () { return solAt > 0 && Date.now() >= solAt; };
  AgentRunner.prototype.think = async function () { return { text: "HIRE 2\nTASK List the ships seen on Monday.", requestId: "req_kick", rows: 1, receipt: null }; };
  try {
    const host = rt.swarms;
    process.env.AGENTS_SWARM_KICKOFF_WAIT_MS = "5000";
    solAt = Date.now() + 300;
    const c = await post(rt, "/v1/hosted/swarms", { plan_id: plan.body.plan_id, creator_sig: sig });
    assert.equal(c.status, 201);
    const rec = host.swarms.get(plan.body.plan_id)!;
    await host.kickoffRun;
    await settle(rt);
    assert.equal(rec.kickoff.state, "done");
    assert.ok(rec.kickoff.ms_after_commit! >= 250, `measured ${rec.kickoff.ms_after_commit} ms`);
    console.log(`# kickoff measured ${rec.kickoff.ms_after_commit} ms after the commit (in-process fakes, SOL landing at +300 ms)`);
    const lead = rt.runners.get(plan.body.swarm.lead)!;
    assert.deepEqual(lead.swarmPlan, { member: plan.body.members[2].agent, task: "List the ships seen on Monday.", request_id: "req_kick" });
    // the next hire uses the plan: member 2's offer, the kickoff task, the kickoff thought as its decision
    const offers = plan.body.members.slice(1).map((m: any, i: number) => ({ id: 10 + i, provider: m.agent, title: "t", price: "1000000", fee: "200000", policy: "buyer_names", turnaround: 20 }));
    (lead as any).api = { must: async (_m: string, p: string) => (p === "/v1/market" ? { ok: true } : { offers }) };
    (lead as any).d = { ...lead.d, node: { putBlob: async () => ({ uri: "blob:" + "00".repeat(32) }) } };
    const calls: any[] = [];
    lead.market.call = async (b: any) => { calls.push(b); return { tx: "t", review: "ok" }; };
    assert.equal(await lead.market.hire(lead.me), "hire");
    assert.equal(calls[0].offer, 11);
    assert.equal(lead.swarmPlan, null);
    // a handle instead of a number also names the member
    rec.kickoff.state = "waiting_sol";
    AgentRunner.prototype.think = async function () { return { text: `HIRE ${plan.body.members[1].handle}\nTASK Count the ships.`, requestId: "req_h", rows: 1, receipt: null }; };
    await host.kickoff(rec, { pollMs: 5 });
    assert.equal(lead.swarmPlan!.member, plan.body.members[1].agent);
  } finally {
    AgentRunner.prototype.ensureBalance = origEnsure;
    AgentRunner.prototype.think = origThink;
    AgentRunner.prototype.lamports = origLam;
  }
});

test("the lead hires its members first, through a paid thought that sees the goal; members deliver swarm hires first", async () => {
  const { rt } = setup();
  const { plan } = await planAndCommit(rt, 3);
  await rt.swarms.kickoffRun;
  await settle(rt);
  const lead = rt.runners.get(plan.swarm.lead)!;
  lead.swarmPlan = null;
  const ms = plan.members;
  const outsider = walletFromSeed(seedOf("outsider")).address;
  const offers = [
    { id: 1, provider: outsider, title: "outside work", price: "10", fee: "2", policy: "buyer_names", turnaround: 5 },
    { id: 2, provider: ms[1].agent, title: "service 1", price: "1000", fee: "200", policy: "buyer_names", turnaround: 10 },
    { id: 3, provider: ms[2].agent, title: "service 2", price: "1000", fee: "200", policy: "buyer_names", turnaround: 10 },
  ];
  (lead as any).api = { must: async (_m: string, p: string) => (p === "/v1/market" ? { ok: true } : { offers }) };
  const prompts: any[] = [];
  (lead as any).think = async (_a: any, messages: any[]) => { prompts.push(messages); return { text: "PICK 2\nREQUEST List the ships seen on Monday.", requestId: "req_lead" }; };
  const blobs: any[] = [];
  (lead as any).d = { ...lead.d, node: { putBlob: async (d: Uint8Array) => { blobs.push(JSON.parse(Buffer.from(d).toString())); return { uri: "blob:" + "00".repeat(32) }; } } };
  const calls: any[] = [];
  lead.market.call = async (b: any) => { calls.push(b); return { tx: "t1", review: "ok" }; };
  assert.equal(await lead.market.hire(lead.me), "hire");
  assert.equal(calls[0].offer, 3, "PICK 2 of the members' offers, not the cheaper outside one");
  assert.match(prompts[0][1].content, /Map every ship/);
  assert.equal(blobs[0].request, "List the ships seen on Monday.");
  assert.equal(blobs[0].swarm.key, plan.plan_id);
  (lead as any).think = async () => ({ text: "maybe", requestId: "req_x" });
  assert.equal(await lead.market.hire(lead.me), "idle");
  assert.equal(calls.length, 1);
  const m1 = rt.runners.get(ms[1].agent)!;
  const hires = [{ id: 20, buyer: ms[0].agent, offer_title: m1.spec.offers![0].title, blocks_left: 9 }, { id: 10, buyer: outsider, offer_title: m1.spec.offers![0].title, blocks_left: 9 }];
  (m1 as any).api = { must: async (_m: string, p: string) => (p.startsWith("/v1/market/hires?") ? { hires } : { request: { text: "do it" } }) };
  (m1 as any).think = async () => ({ text: "the list", requestId: "req_m1" });
  (m1 as any).d = { ...m1.d, node: { putBlob: async () => ({ uri: "blob:" + "11".repeat(32) }) } };
  const delivered: any[] = [];
  m1.market.call = async (b: any) => { delivered.push(b); return { tx: "t2", review: "ok" }; };
  assert.equal(await m1.market.work(m1.me), "deliver");
  assert.equal(delivered[0].hire, 20, "the swarm hire first");
});

/** Stubs on AgentRunner.prototype for the kickoff (SOL, balance, the kickoff thought); returns undo. */
async function stubLead(o: { sol: () => boolean; balance?: () => boolean; text?: string; thoughts?: { n: number } }) {
  const { AgentRunner } = await import("../src/agent.ts");
  const P = AgentRunner.prototype as any;
  const orig = { lamports: P.lamports, ensureBalance: P.ensureBalance, think: P.think };
  P.lamports = async function () { return o.sol() ? 1n : 0n; };
  P.ensureBalance = async function () { return (o.balance ?? o.sol)(); };
  P.think = async function () { if (o.thoughts) o.thoughts.n++; return { text: o.text ?? "HIRE 1\nTASK List the ships seen on Monday.", requestId: "req_kick", rows: 1, receipt: null }; };
  return () => Object.assign(P, orig);
}

/** A lead whose market lists each member's offer, with a hire list and a recorded send. */
function marketFor(lead: any, members: any[], hires: any[] = []) {
  const offers = members.map((m: any, i: number) => ({ id: 10 + i, provider: m.agent, title: "t", price: "1000000", fee: "200000", policy: "buyer_names", turnaround: 20 }));
  lead.api = { must: async (_m: string, p: string) => (p === "/v1/market" ? { ok: true } : p.startsWith("/v1/market/hires?") ? { hires } : { offers }) };
  lead.d = { ...lead.d, node: { putBlob: async () => ({ uri: "blob:" + "00".repeat(32) }) } };
  const calls: any[] = [];
  lead.market.call = async (b: any) => { calls.push(b); return { tx: "t_hire", review: "ok" }; };
  return calls;
}

test("kickoff survives a restart during the SOL wait: saved as waiting, resumed on the next run, the planned hire sent once", async () => {
  const s1 = setup();
  let sol = false;
  const thoughts = { n: 0 };
  const undo = await stubLead({ sol: () => sol, thoughts });
  try {
    process.env.AGENTS_SWARM_KICKOFF_WAIT_MS = "5000";
    const { plan } = await planAndCommit(s1.rt, 3);
    const key = plan.plan_id;
    await new Promise((res) => setTimeout(res, 60));
    // the runtime stops (SIGTERM) while the kickoff waits for the lead's SOL
    s1.rt.stopped = true;
    await s1.rt.swarms.kickoffRun;
    await settle(s1.rt);
    const saved = JSON.parse(readFileSync(join(s1.dir, "swarms.json"), "utf8")).swarms[0];
    assert.equal(saved.kickoff.state, "waiting_sol", "a stop leaves the kickoff resumable, not failed");
    assert.equal(saved.kickoff.error, null);
    assert.equal(thoughts.n, 0);
    // restart: a new runtime on the same state dir; the SOL has landed meanwhile
    sol = true;
    const s2 = setup({ dir: s1.dir });
    await s2.rt.swarms.resumeKickoffs();
    const rec = s2.rt.swarms.swarms.get(key)!;
    assert.equal(rec.kickoff.state, "done");
    assert.equal(rec.kickoff.resumes, 1);
    assert.equal(thoughts.n, 1);
    assert.deepEqual(rec.kickoff.plan, { member: plan.members[1].agent, task: "List the ships seen on Monday.", request_id: "req_kick" });
    const lead = s2.rt.runners.get(plan.swarm.lead)! as any;
    assert.equal(lead.swarmPlan.member, plan.members[1].agent);
    const calls = marketFor(lead, plan.members.slice(1));
    assert.equal(await lead.market.hire(lead.me), "hire");
    assert.equal(calls.length, 1);
    assert.equal(calls[0].offer, 10);
    assert.equal(rec.kickoff.hire!.state, "sent");
    assert.equal(rec.kickoff.hire!.tx, "t_hire");
    s2.rt.stopped = true;
    // a third start: the kickoff is done and its hire went out, so nothing runs again
    const s3 = setup({ dir: s1.dir });
    await s3.rt.swarms.resumeKickoffs();
    const lead3 = s3.rt.runners.get(plan.swarm.lead)! as any;
    assert.equal(lead3.swarmPlan, null, "no second planned hire after the hire was sent");
    assert.equal(thoughts.n, 1, "no second kickoff thought");
    assert.equal(s3.rt.swarms.swarms.get(key)!.kickoff.hire!.state, "sent");
    s3.rt.stopped = true;
  } finally { undo(); }
});

test("kickoff: a record the old runtime marked failed by a stop resumes; a crash mid-send checks the market and never hires twice", async () => {
  const s1 = setup();
  let sol = false;
  const undo = await stubLead({ sol: () => sol });
  try {
    process.env.AGENTS_SWARM_KICKOFF_WAIT_MS = "300";
    const { plan } = await planAndCommit(s1.rt, 3);
    const key = plan.plan_id;
    s1.rt.stopped = true;
    await s1.rt.swarms.kickoffRun;
    await settle(s1.rt);
    // the old code's record (devnet swarm 1): failed with the stop text
    const file = join(s1.dir, "swarms.json");
    const j = JSON.parse(readFileSync(file, "utf8"));
    j.swarms[0].kickoff = { state: "failed", request_id: null, text: null, ms_after_commit: null, error: "the runtime stopped before the kickoff" };
    writeFileSync(file, JSON.stringify(j));
    sol = true;
    const s2 = setup({ dir: s1.dir });
    await s2.rt.swarms.resumeKickoffs();
    const rec = s2.rt.swarms.swarms.get(key)!;
    assert.equal(rec.kickoff.state, "done");
    assert.equal(rec.kickoff.resumes, 1);
    const lead = s2.rt.runners.get(plan.swarm.lead)! as any;
    // the hire goes out, but the runtime dies before "sent" is saved: the record still says sending
    lead.onSwarmPlan({ state: "sending" });
    s2.rt.stopped = true;
    const s3 = setup({ dir: s1.dir });
    await s3.rt.swarms.resumeKickoffs();
    const lead3 = s3.rt.runners.get(plan.swarm.lead)! as any;
    assert.ok(lead3.swarmPlan, "a sending hire is not known to be out, so the plan comes back");
    // the market already lists that hire: the plan ends without a second hire
    const calls = marketFor(lead3, plan.members.slice(1), [{ id: 77, buyer: plan.swarm.lead, provider: plan.members[1].agent }]);
    assert.equal(await lead3.market.hire(lead3.me), "idle");
    assert.equal(calls.length, 0);
    assert.equal(lead3.swarmPlan, null);
    const k = s3.rt.swarms.swarms.get(key)!.kickoff;
    assert.equal(k.hire!.state, "found");
    assert.equal(k.hire!.hire_id, 77);
    // a refused send leaves the plan to try again (nothing went out)
    lead3.swarmPlan = { ...k.plan! };
    k.hire = null;
    marketFor(lead3, plan.members.slice(1));
    lead3.market.call = async () => { throw new Error("simulate: refused"); };
    await assert.rejects(lead3.market.hire(lead3.me), /refused/);
    assert.equal(k.hire, null);
    assert.ok(lead3.swarmPlan);
    s3.rt.stopped = true;
  } finally { undo(); }
});

test("kickoff: SOL in the wallet but the lead's top-up held waits (not failed) and resumes on the tick once the hold lifts", async () => {
  const s1 = setup();
  let balance = false;
  const undo = await stubLead({ sol: () => true, balance: () => balance });
  try {
    process.env.AGENTS_SWARM_KICKOFF_WAIT_MS = "300";
    const plan = await post(s1.rt, "/v1/hosted/swarms/plan", { creator: op.address, goal: GOAL, size: 2, total_sol_lamports: "3000" });
    const lead0 = () => s1.rt.runners.get(plan.body.swarm.lead)! as any;
    // the lead's inference is held by its daily SOL cap before the commit's kickoff runs
    const { AgentRunner } = await import("../src/agent.ts");
    const P = AgentRunner.prototype as any;
    const origHeld = P.held;
    P.held = async function (this: any) { return !!this.inf.hold; };
    try {
      const sig = Buffer.from(op.sign(Buffer.from(plan.body.creator_message, "utf8"))).toString("hex");
      const origStart = s1.rt.swarms.startKickoff.bind(s1.rt.swarms);
      (s1.rt.swarms as any).startKickoff = (rec: any) => { lead0().inf.hold = { reason: "sol_cap", since: 0, until: Date.now() + 3_600_000, need: 0, price: "25117275" }; return origStart(rec); };
      const c = await post(s1.rt, "/v1/hosted/swarms", { plan_id: plan.body.plan_id, creator_sig: sig });
      assert.equal(c.status, 201);
      (s1.rt.swarms as any).startKickoff = origStart;
      await s1.rt.swarms.kickoffRun;
      await settle(s1.rt);
      const rec = s1.rt.swarms.swarms.get(plan.body.plan_id)!;
      assert.equal(rec.kickoff.state, "waiting_budget");
      assert.match(rec.kickoff.waiting!, /daily SOL cap to reset at 00:00 UTC/);
      await new Promise((res) => setTimeout(res, 400));
      await s1.rt.swarms.tick();
      assert.equal(rec.kickoff.state, "waiting_budget", "still held: the tick does not retry");
      // the UTC day rolls over: the hold lifts and the next tick runs the kickoff
      lead0().inf.hold = null;
      balance = true;
      await s1.rt.swarms.tick();
      await s1.rt.swarms.kickoffRun;
      assert.equal(rec.kickoff.state, "done");
      assert.equal(lead0().swarmPlan.member, plan.body.members[1].agent);
      s1.rt.stopped = true;
    } finally { P.held = origHeld; }
  } finally { undo(); }
});
