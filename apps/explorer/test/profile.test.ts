// Agent profiles (profile.ts), the agent card probe (agentcard.ts) and the tx facts they read.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { b58encode } from "../src/codec.ts";
import * as D from "../src/decode.ts";
import { agentProfile, agentCandidates } from "../src/profile.ts";
import { CardProber, fetchCard, isPrivateAddress, parseCard } from "../src/agentcard.ts";
import type { BlockRec, TxRec } from "../src/indexer.ts";

const key = (i: number) => b58encode(Uint8Array.from({ length: 32 }, (_, k) => (k === 0 ? i : 7)));
const [SELLER, BUYER1, BUYER2, EVAL, SELFBUYER, FUNDED, OTHER, MINER] = [1, 2, 3, 4, 5, 6, 8, 9].map(key);
const jid = (c: string) => c.repeat(64).slice(0, 64);

function job(id: string, o: any) {
  return { job_id: id, kind: "job", poster: BUYER1, deliverer: SELLER, status: "completed", worker_pay: "5000", protocol_fee: "1000", posted_height: 10, closed_height: 20, ...o };
}
const tx = (kind: string, from: string, facts: D.TxFacts): TxRec => ({ hash: "h", kind, from, fee: "10", facts });
function block(height: number, txs: TxRec[], o: Partial<BlockRec> = {}): BlockRec {
  return { height, hash: `b${height}`, prev: "", time: 1_700_000_000_000 + height * 1000, miner: MINER, jobId: null, fees: "10", subsidy: "0", reward: "10", proofHash: "", proofLen: 0, targetBits: 1, txs, ...o };
}

function fakeIndex() {
  const jobs = [
    job(jid("a"), { protocol_fee: "3000" }), // BUYER1, signed by EVAL: independent
    job(jid("b"), { poster: BUYER2, protocol_fee: "1000", closed_height: 40 }), // signed by the buyer itself
    job(jid("c"), { poster: SELLER, protocol_fee: "7000" }), // self-dealt (poster = deliverer)
    job(jid("d"), { poster: FUNDED, protocol_fee: "2000" }), // heuristic: FUNDED got a Transfer from SELLER
    job(jid("e"), { status: "expired", protocol_fee: "500" }), // delivered then expired
    job(jid("f"), { status: "open", deliverer: null, protocol_fee: "400" }), // rejected, reopened
    job(jid("g"), { poster: SELLER, deliverer: null, status: "expired", protocol_fee: "250" }), // SELLER as buyer, never delivered
    job(jid("h"), { kind: "segment", protocol_fee: "0" }), // segments are not deliveries
  ];
  const blocks = new Map<number, BlockRec>();
  const add = (b: BlockRec) => blocks.set(b.height, b);
  add(block(5, [tx("Transfer", SELLER, { to: FUNDED, amount: "99" }), tx("RegisterAgent", SELLER, { name: "seller-bot", uri: "https://example.com/card.json" })]));
  add(block(12, [tx("SubmitResult", SELLER, { jobId: jid("a") }), tx("SubmitResult", SELLER, { jobId: jid("b") })]));
  add(block(13, [tx("SubmitResult", SELLER, { jobId: jid("f") })]));
  add(block(14, [tx("RejectResult", OTHER, { jobId: jid("f"), deliverer: SELLER, resultHash: "ab", signers: [EVAL] })]));
  add(block(20, [tx("CompleteJob", SELLER, { jobId: jid("a"), signers: [EVAL] }), tx("CompleteJob", SELLER, { jobId: jid("c"), signers: [EVAL] }), tx("CompleteJob", SELLER, { jobId: jid("d"), signers: [EVAL] })]));
  add(block(40, [tx("CompleteJob", SELLER, { jobId: jid("b"), signers: [BUYER2] })]));
  add(block(41, [], { miner: SELLER, jobId: jid("a"), subsidy: "800", fees: "20" }));
  add(block(42, [], { miner: SELLER, fees: "5" }));
  const orders = new Map<string, any>([["1", { nonce: "1", jobId: jid("a"), status: "finalized", solSpent: "100", txs: [{ paid: [{ nonce: "1", solSpent: "100", share: "11" }] }] }]]);
  return {
    jobs: new Map(jobs.map((j) => [j.job_id, j])),
    blocks,
    jobBlocks: new Map([[jid("a"), [41]]]),
    orders,
    chainInfo: { height: 42 },
    status: { polls: 1 },
  } as any;
}

test("profile: P and SOL behind completed work lead, self-dealing excluded, failures and signers shown", () => {
  const ix = fakeIndex();
  const p = agentProfile(ix, SELLER);
  assert.equal(p.headline.p_behind_completed_work, "4000", "a (3000) + b (1000); c strict and d heuristic excluded");
  assert.equal(p.headline.storefront_sol_behind_completed_work, "111", "sol_spent 100 + buyback share 11");
  assert.equal(p.headline.completed_jobs, 2);
  assert.equal(p.server.distinct_payers, 2);
  assert.equal(p.server.top_payer!.payer, BUYER1);
  assert.equal(p.server.top_payer!.share_ppm, 750_000);
  assert.equal(p.server.self_dealt_excluded.protocol_fees, "9000");
  assert.equal(p.server.self_dealt_excluded.heuristic_protocol_fees, "2000");
  assert.match(p.server.self_dealt_excluded.list.find((x) => x.job_id === jid("d"))!.reason, /heuristic/);
  assert.equal(p.server.delivered_then_expired.protocol_fees, "500");
  assert.equal(p.server.rejected.jobs, 1);
  assert.equal(p.server.rejected.protocol_fees, "400");
  assert.deepEqual(p.server.rejected.list[0].signers, [EVAL]);
  // acceptance by P over all deliveries: 13000 / (13000 + 500 + 400)
  assert.equal(p.server.acceptance_by_p_ppm, Number((13000n * 1_000_000n) / 13900n));
  // independent: only a (3000) of the screened 4000; b was signed by its own buyer
  assert.equal(p.server.independent_evaluation.protocol_fees, "3000");
  assert.equal(p.server.independent_evaluation.share_ppm, 750_000);
  const b = p.server.completed.find((j: any) => j.job_id === jid("b"));
  assert.deepEqual([b.signers, b.signed_by_third_party], [[BUYER2], false]);
  assert.deepEqual(p.server.time_to_completion_blocks, { p50: 10, p90: 30, n: 4 });
  assert.equal(p.server.time_to_delivery_blocks.n, 2);
  assert.equal(p.server.last_delivery!.height, 13);
  assert.equal(p.miner.job_blocks, 1);
  assert.equal(p.miner.fallback_blocks, 1);
  assert.equal(p.miner.subsidy, "800");
  assert.equal(p.buyer.expired_without_delivery_p, "250");
  assert.equal(p.agent!.name, "seller-bot", "from the indexed RegisterAgent when getAgent is not given");
  // every headline figure names its chain source
  for (const k of ["p_behind_completed_work", "storefront_sol_behind_completed_work", "rejected", "independent_evaluation", "mining"]) assert.ok((p.sources as any)[k], k);
  // as evaluator
  const e = agentProfile(ix, EVAL).evaluator;
  assert.deepEqual([e.jobs_signed, e.protocol_fees, e.rejects_signed, e.rejects_signed_p, e.own_job_share_ppm], [3, "12000", 1, "400", 0]);
  const be = agentProfile(ix, BUYER2).evaluator;
  assert.equal(be.own_job_share_ppm, 1_000_000, "BUYER2 signed only its own job");
  assert.ok(agentCandidates(ix).has(SELLER));
});

test("tx facts: CompleteJob / RejectResult signer keys, Transfer, RegisterAgent", () => {
  const HDR = 54;
  const k = (i: number) => Uint8Array.from({ length: 32 }, () => i);
  const head = (kind: number) => { const h = new Uint8Array(HDR); h[0] = 1; h[HDR - 1] = kind; return h; };
  const cat = (...a: Uint8Array[]) => Buffer.concat(a.map((x) => Buffer.from(x)));
  const sigs = (...ks: Uint8Array[]) => cat(Uint8Array.of(ks.length), ...ks.flatMap((x) => [x, new Uint8Array(64)]));
  const sig = new Uint8Array(64);
  const cj = cat(head(4), k(1), sigs(k(2), k(3)), sig).toString("base64");
  assert.deepEqual(D.txFacts("CompleteJob", cj), { jobId: "01".repeat(32), signers: [b58encode(k(2)), b58encode(k(3))] });
  const rj = cat(head(13), k(1), k(5), k(6), sigs(k(2)), sig).toString("base64");
  assert.deepEqual(D.txFacts("RejectResult", rj), { jobId: "01".repeat(32), deliverer: b58encode(k(5)), resultHash: "06".repeat(32), signers: [b58encode(k(2))] });
  const amt = Buffer.alloc(8); amt.writeBigUInt64LE(77n);
  assert.deepEqual(D.txFacts("Transfer", cat(head(0), k(9), amt, sig).toString("base64")), { to: b58encode(k(9)), amount: "77" });
  const name = Buffer.from("bot"), uri = Buffer.from("https://x/c.json");
  const ul = Buffer.alloc(2); ul.writeUInt16LE(uri.length);
  assert.deepEqual(D.txFacts("RegisterAgent", cat(head(1), Uint8Array.of(name.length), name, ul, uri, sig).toString("base64")), { name: "bot", uri: "https://x/c.json" });
  // a truncated sig list gives no facts rather than wrong ones
  assert.deepEqual(D.txFacts("CompleteJob", cat(head(4), k(1), Uint8Array.of(3)).toString("base64")), {});
});

test("agent card: parse, private-address refusal, probe health and stale hiding", async () => {
  const c = parseCard({ name: "bot", description: "d", url: "https://bot.example/a2a", version: "1", capabilities: { streaming: true, push: false }, skills: [{ id: "s", name: "Summarize" }], supportedInterfaces: [{ url: "https://bot.example/a2a", protocolBinding: "JSONRPC" }], signatures: [{}] });
  assert.equal(c.name, "bot");
  assert.deepEqual(c.capabilities, ["streaming"]);
  assert.equal(c.interfaces[0].transport, "JSONRPC");
  assert.equal(c.signed, true);
  assert.throws(() => parseCard({ description: "no name" }));
  for (const ip of ["127.0.0.1", "10.1.2.3", "192.168.0.1", "169.254.1.1", "172.20.0.1", "100.64.0.1", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1"]) assert.ok(isPrivateAddress(ip), ip);
  for (const ip of ["8.8.8.8", "2606:4700::1111"]) assert.ok(!isPrivateAddress(ip), ip);
  const cfg = { probeMs: 1000, timeoutMs: 2000, staleMs: 10_000, activeMs: 5_000, allowPrivate: false };
  assert.match((await fetchCard("http://127.0.0.1:10690/card.json", cfg) as any).error, /private/);
  assert.match((await fetchCard("ipfs://x", cfg) as any).error, /http and https only/);

  // a real local card server on 10690 (tests only, with allowPrivate)
  let body = JSON.stringify({ name: "local-bot", skills: [] });
  const srv = createServer((req, res) => (req.url === "/big" ? res.end("x".repeat(70_000)) : req.url === "/card.json" ? res.end(body) : (res.statusCode = 404, res.end())));
  await new Promise<void>((r) => srv.listen(10690, "127.0.0.1", r));
  try {
    const okCfg = { ...cfg, allowPrivate: true };
    assert.equal(((await fetchCard("http://127.0.0.1:10690/card.json", okCfg)) as any).card.name, "local-bot");
    assert.match(((await fetchCard("http://127.0.0.1:10690/big", okCfg)) as any).error, /larger than/);
    assert.match(((await fetchCard("http://127.0.0.1:10690/nope", okCfg)) as any).error, /HTTP 404/);
    let now = 1_000_000;
    const pr = new CardProber(okCfg, fetchCard, () => now);
    const st = await pr.probe("A", "http://127.0.0.1:10690/card.json");
    assert.equal(pr.health(st), "ok");
    assert.equal(pr.listed(st, null).listed, true);
    body = "not json";
    now += 500;
    assert.equal(pr.health(await pr.probe("A", "http://127.0.0.1:10690/card.json")), "ok", "cached within probeMs");
    now += 1000;
    const st2 = await pr.probe("A", "http://127.0.0.1:10690/card.json");
    assert.equal(pr.health(st2), "failing");
    assert.equal(st2!.card!.name, "local-bot", "last good card kept");
    now += 20_000;
    assert.equal(pr.health(st2), "stale");
    assert.equal(pr.listed(st2, null).listed, false, "stale card and no delivery: hidden");
    assert.equal(pr.listed(st2, now - 1000).listed, true, "a recent delivery keeps it listed");
    assert.equal(pr.listed(null, null).listed, false);
  } finally {
    srv.close();
  }
});

test("routes: /api/agent, /agent, /agents over a real indexer on a fake node", async () => {
  const { fakeNode, fakeSolana, A, J } = await import("./agent-fixture.ts");
  const { Indexer } = await import("../src/indexer.ts");
  const { route } = await import("../src/server.ts");
  const { mkdtempSync, rmSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const node = fakeNode(), sol = fakeSolana();
  const listen = (s: any) => new Promise<number>((r) => s.listen(0, "127.0.0.1", () => r(s.address().port)));
  const [np, sp] = await Promise.all([listen(node), listen(sol)]);
  const dir = mkdtempSync(join(tmpdir(), "explorer-agent-"));
  try {
    const ix = new Indexer({ nodeUrl: `http://127.0.0.1:${np}`, solUrl: `http://127.0.0.1:${sp}`, stateDir: dir, log: () => {} });
    await ix.poll();
    const p = JSON.parse((await route(ix, "GET", `/api/agent/${A.seller}`)).body as string);
    assert.equal(p.agent.name, "summarizer-7");
    assert.equal(p.headline.p_behind_completed_work, "7500000000", "a + b + c; d is self-dealt");
    assert.equal(p.server.self_dealt_excluded.protocol_fees, "3000000000");
    assert.equal(p.server.delivered_then_expired.protocol_fees, "800000000");
    assert.equal(p.server.independent_evaluation.protocol_fees, "5000000000", "b was signed by its own buyer");
    assert.equal(p.card.health, "none");
    const g = JSON.parse((await route(ix, "GET", `/api/agent/${A.griefer}`)).body as string);
    assert.equal(g.server.rejected.jobs, 1);
    assert.equal(g.server.rejected.chain_tally.rejected, 1);
    assert.equal(g.server.rejected.list[0].job_id, J.f);
    const ev = JSON.parse((await route(ix, "GET", `/api/agent/${A.evaluator}`)).body as string);
    assert.equal(ev.evaluator.jobs_signed, 3);
    assert.equal(ev.evaluator.rejects_signed, 1);
    const l = JSON.parse((await route(ix, "GET", "/api/agents")).body as string);
    assert.equal(l.agents[0].account, A.seller, "listed: it delivered within the active window");
    assert.ok(!l.agents.some((x: any) => x.account === A.griefer) || l.agents.find((x: any) => x.account === A.griefer).listed);
    for (const path of [`/agent/${A.seller}`, `/agent/${A.griefer}`, `/agent/${A.evaluator}`, "/agents", "/agents?all=1"]) {
      const r = await route(ix, "GET", path);
      assert.equal(r.status, 200, path);
      assert.doesNotMatch(r.body as string, /—/, "no em dashes");
    }
    assert.equal((await route(ix, "GET", "/api/agent/not-an-address")).status, 400);
    // Cx addresses (SPEC 2.1): the same agent by Cx, lowercase Cx and base58; JSON adds *_cx; a bad casing is 400
    const { toCx, shortCx } = await import("../../../packages/cx-address/src/index.ts");
    const cx = toCx(A.seller);
    const byCx = JSON.parse((await route(ix, "GET", `/api/agent/${cx}`)).body as string);
    const byLower = JSON.parse((await route(ix, "GET", `/api/agent/Cx${cx.slice(2).toLowerCase()}`)).body as string);
    assert.deepEqual(byCx, p);
    assert.deepEqual(byLower, p);
    assert.equal(p.account, A.seller, "base58 kept");
    assert.equal(p.account_cx, cx);
    assert.equal(l.agents[0].account_cx, cx);
    const at = [...cx].findIndex((c, k) => k >= 2 && /[a-fA-F]/.test(c));
    const bad = cx.slice(0, at) + (cx[at] === cx[at].toLowerCase() ? cx[at].toUpperCase() : cx[at].toLowerCase()) + cx.slice(at + 1);
    const rb = await route(ix, "GET", `/api/agent/${bad}`);
    assert.equal(rb.status, 400);
    assert.equal(JSON.parse(rb.body as string).cx_code, "checksum");
    assert.equal((await route(ix, "GET", `/agent/${bad}`)).status, 400);
    const page = (await route(ix, "GET", `/agent/${cx}`)).body as string;
    assert.match(page, new RegExp(`Cadre address`));
    assert.ok(page.includes(cx), "full Cx in the detail view");
    assert.ok(page.includes(A.seller), "Solana address (same key) shown too");
    assert.ok(page.includes("Solana address"));
    const list = (await route(ix, "GET", "/agents")).body as string;
    assert.ok(list.includes(shortCx(A.seller)) || list.includes(cx), "list shows Cx");
  } finally {
    node.close();
    sol.close();
    rmSync(dir, { recursive: true, force: true });
  }
});
