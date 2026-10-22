// Phase 5 boards, discovery, trader board and event feed (docs/API.md "Boards"): the board math
// on synthetic facts (self-dealing exclusion, house apart, minimum samples, windows, realized PnL
// per sell), then every route against an in-memory index (blocks with decoded tx facts, jobs) plus
// fake market and trading handles. Port 10792 (the Phase 5 row's test block 10790-10799).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { b58encode, sha256, utf8 } from "../../../apps/explorer/src/codec.ts";
import { aggregate, rank, realizedPerSell, type Fact, type Fill } from "../src/boards/math.ts";
import { avgCostPnl } from "../src/routes/trading.ts";

const key = (s: string) => b58encode(sha256(utf8("key:" + s)));
const Z = "0".repeat(64);
const ZK = b58encode(new Uint8Array(32));
const NOW = Date.now();

// ------------------------------------------------------------------ math

const f = (o: Partial<Fact> & { account: string; amount: bigint }): Fact => ({ payer: null, at: NOW - 1000, ref: { chain: "cadre", id: "aa", what: "t" }, house: false, excluded: null, ...o });

test("math: self-dealt facts never count, house facts go to house_value, distinct payers", () => {
  const a = aggregate([
    f({ account: "A", amount: 100n, payer: "p1" }),
    f({ account: "A", amount: 200n, payer: "p2" }),
    f({ account: "A", amount: 50n, payer: "p2" }),
    f({ account: "A", amount: 9000n, payer: "A", excluded: "payer is the payee" }),
    f({ account: "A", amount: 700n, payer: "h", house: true }),
  ], "all", NOW).get("A")!;
  assert.equal(a.value, 350n);
  assert.equal(a.house_value, 700n);
  assert.equal(a.excluded_value, 9000n);
  assert.equal(a.payers.size, 2);
  assert.equal(a.n, 3);
});

test("math: minimum sample and minimum payers gate ranking; below-min rows are counted", () => {
  const facts = [
    f({ account: "A", amount: 5n, payer: "p1" }), f({ account: "A", amount: 5n, payer: "p2" }), f({ account: "A", amount: 5n, payer: "p3" }),
    f({ account: "B", amount: 99n, payer: "p1" }), f({ account: "B", amount: 99n, payer: "p1" }), f({ account: "B", amount: 99n, payer: "p1" }),
    f({ account: "C", amount: 1000n, payer: "p9" }),
  ];
  const r = rank(aggregate(facts, "all", NOW), { kind: "sum", min_sample: 3, min_payers: 2 }, "organic", () => false);
  assert.deepEqual(r.rows.map((x) => [x.agg.account, x.score]), [["A", 15n]]);
  assert.equal(r.below_min, 2); // B has one payer, C one fact
});

test("math: windows cut on block time; unknown time only counts in all", () => {
  const facts = [
    f({ account: "A", amount: 1n, at: NOW - 3_600_000 }),
    f({ account: "A", amount: 10n, at: NOW - 3 * 86_400_000 }),
    f({ account: "A", amount: 100n, at: NOW - 30 * 86_400_000 }),
    f({ account: "A", amount: 1000n, at: null }),
  ];
  assert.equal(aggregate(facts, "24h", NOW).get("A")!.value, 1n);
  assert.equal(aggregate(facts, "7d", NOW).get("A")!.value, 11n);
  assert.equal(aggregate(facts, "all", NOW).get("A")!.value, 1111n);
});

test("math: house side ranks house accounts on house money; organic side hides them", () => {
  const facts = [f({ account: "H", amount: 10n, payer: "p1" }), f({ account: "H", amount: 500n, payer: "hp", house: true }), f({ account: "A", amount: 20n, payer: "p1" })];
  const isHouse = (a: string) => a === "H";
  const ag = aggregate(facts, "all", NOW);
  const spec = { kind: "sum" as const, min_sample: 1, min_payers: 0 };
  assert.deepEqual(rank(ag, spec, "organic", isHouse).rows.map((x) => x.agg.account), ["A"]);
  assert.deepEqual(rank(ag, spec, "house", isHouse).rows.map((x) => [x.agg.account, x.score, x.house_score]), [["H", 500n, 10n]]);
  assert.deepEqual(rank(ag, spec, "all", isHouse).rows.map((x) => [x.agg.account, x.score]), [["A", 20n], ["H", 10n]]);
});

test("math: positive boards skip zero rows (a miner with only fallback blocks); realized PnL keeps losers", () => {
  const ag = aggregate([f({ account: "M", amount: 0n }), f({ account: "M", amount: 0n }), f({ account: "L", amount: -5n }), f({ account: "W", amount: 3n })], "all", NOW);
  assert.deepEqual(rank(ag, { kind: "sum", min_sample: 1, min_payers: 0, positive: true }, "organic", () => false).rows.map((x) => x.agg.account), ["W"]);
  assert.deepEqual(rank(ag, { kind: "sum", min_sample: 1, min_payers: 0 }, "organic", () => false).rows.map((x) => x.agg.account), ["W", "M", "L"]);
});

test("math: rate boards rank accepted share in ppm over closed outcomes", () => {
  const o = (out: Fact["outcome"], p: string) => f({ account: "A", amount: 1n, payer: p, outcome: out });
  const ag = aggregate([o("accepted", "p1"), o("accepted", "p2"), o("rejected", "p3"), o("expired", "p1")], "all", NOW);
  const r = rank(ag, { kind: "rate", min_sample: 3, min_payers: 2 }, "organic", () => false);
  assert.equal(r.rows[0].score, 500_000n);
  assert.equal(r.rows[0].sample, 4);
});

test("math: realized PnL per sell sums to the trading route's average-cost PnL", () => {
  const fill = (side: "buy" | "sell", i: bigint, o: bigint, at: number): Fill => ({ agent: "T", account: "T", token: "M", side, amount_in: i, amount_out: o, at, ref: String(at), house: false, flags: [] });
  const fills = [fill("buy", 1000n, 100n, 1), fill("sell", 50n, 600n, 2), fill("buy", 300n, 20n, 3), fill("sell", 80n, 700n, 4), fill("sell", 10n, 50n, 5), fill("sell", 5n, 9n, 6)];
  const per = realizedPerSell(fills);
  assert.equal(per.length, 4);
  const sum = per.reduce((a, x) => a + x.realized, 0n);
  const ref = avgCostPnl(fills.map((x) => ({ side: x.side, amount_in: String(x.amount_in), amount_out: String(x.amount_out) })));
  assert.equal(sum, ref.realized);
  assert.equal(per[3].realized, 0n, "a sell with no open position realizes nothing");
});

// ------------------------------------------------------------------ routes

const AG = key("agent-a"), AG2 = key("agent-b"), HO = key("house-op"), HA = key("house-agent"), OPA = key("op-a");
const P1 = key("p1"), P2 = key("p2"), P3 = key("p3"), F = key("funded"), X = key("nobody"), M1 = key("miner1"), M2 = key("miner2"), EV = key("evaluator"), TR = key("trader");
const TIP = 40;
const tAt = (h: number) => (h === 2 ? NOW - 10 * 86_400_000 : NOW - (TIP - h) * 60_000);
const J = (n: number) => n.toString(16).padStart(2, "0").repeat(32);
const POST = "c1".repeat(32), MSG1 = "d1".repeat(32), MSG2 = "d2".repeat(32), REPLY = "d3".repeat(32);

let hashN = 0;
const tx = (kind: string, from: string, facts: any) => ({ hash: (++hashN).toString(16).padStart(64, "e"), kind, from, fee: "1000", facts });
const sp = (handle: string, flags: number, operator: string | null) => ({ social: { kind: "SetProfile", handle, name: handle, profileUri: "", profileHash: Z, flags, operator: operator ?? ZK, encKey: ZK, dmPrice: "0", dmPolicy: 0, threadFreeBlocks: 0 } });
const dmf = (id: string, to: string, pay: string, reclaim: number, replyTo = Z) => ({ messageId: id, dm: { kind: "SendMessage", to, channel: Z, mode: 0, replyTo, pay, reclaimAfter: reclaim, body: "hello", bodyHex: "", blobUri: "", blobHash: Z, thoughtRef: "" } });

const blockTxs: Record<number, any[]> = {
  2: [tx("React", P2, { social: { kind: "React", items: [{ postId: POST, reaction: 4, tip: "77" }] } })],
  3: [tx("SetProfile", AG, sp("alpha", 1 | 4, OPA)), tx("SetProfile", AG2, sp("beta", 1, null)), tx("SetProfile", HA, sp("housey", 3, HO)), tx("Transfer", AG, { to: F, amount: "5" })],
  5: [tx("Post", AG, { postId: POST, social: { kind: "Post", postKind: 0, parent: Z, channel: Z, body: "first light", blobUri: "", blobHash: Z, thoughtRef: "" } })],
  6: [
    tx("React", P1, { social: { kind: "React", items: [{ postId: POST, reaction: 4, tip: "250" }] } }),
    tx("React", AG, { social: { kind: "React", items: [{ postId: POST, reaction: 4, tip: "999" }] } }),
    tx("React", HO, { social: { kind: "React", items: [{ postId: POST, reaction: 4, tip: "50" }] } }),
    tx("Follow", P1, { social: { kind: "Follow", items: [{ target: AG, on: true }] } }),
    tx("Follow", P3, { social: { kind: "Follow", items: [{ target: AG, on: true }] } }),
    tx("Follow", X, { social: { kind: "Follow", items: [{ target: AG, on: true }] } }),
  ],
  8: [tx("SendMessage", P1, dmf(MSG1, AG, "500", 10)), tx("SendMessage", P2, dmf(MSG2, AG, "300", 10))],
  9: [tx("SendMessage", AG, dmf(REPLY, P1, "0", 0, MSG1))],
  10: [tx("CompleteJob", EV, { jobId: J(1), signers: [EV] }), tx("CompleteJob", EV, { jobId: J(2), signers: [EV] }), tx("CompleteJob", HO, { jobId: J(3), signers: [HO] })],
  11: [tx("CompleteJob", AG, { jobId: J(4), signers: [EV] }), tx("CompleteJob", AG, { jobId: J(5), signers: [AG] }), tx("RejectResult", P3, { jobId: J(6), deliverer: AG, signers: [P3], resultHash: Z })],
};
const jobs = [
  { job_id: J(1), poster: P1, deliverer: AG, status: "completed", kind: "job", protocol_fee: "100", worker_pay: "100", posted_height: 4, closed_height: 10 },
  { job_id: J(2), poster: P2, deliverer: AG, status: "completed", kind: "job", protocol_fee: "100", worker_pay: "200", posted_height: 4, closed_height: 10 },
  { job_id: J(3), poster: HO, deliverer: AG, status: "completed", kind: "ticket", protocol_fee: "100", worker_pay: "1000", posted_height: 4, closed_height: 10 },
  { job_id: J(4), poster: F, deliverer: AG, status: "completed", kind: "job", protocol_fee: "100", worker_pay: "5000", posted_height: 4, closed_height: 11 },
  { job_id: J(5), poster: AG, deliverer: AG, status: "completed", kind: "job", protocol_fee: "100", worker_pay: "7000", posted_height: 4, closed_height: 11 },
  { job_id: J(6), poster: P3, deliverer: AG, status: "open", kind: "job", protocol_fee: "100", worker_pay: "50", posted_height: 4, closed_height: 0 },
  { job_id: J(7), poster: M2, deliverer: null, status: "open", kind: "job", protocol_fee: "100", worker_pay: "50", posted_height: 4, closed_height: 0 },
];

let base = "";
let srv: any;
let stateDir = "";
before(async () => {
  process.env.TRADING_SOLANA_CONFIG = "/nonexistent";
  process.env.TRADING_CADRE_CONFIG = "/nonexistent";
  process.env.MARKET_CONFIG = "/nonexistent";
  process.env.CADRE_LOG = "0";
  const { configFromEnv, Deps } = await import("../src/deps.ts");
  const { build } = await import("../src/app.ts");
  const { shared } = await import("../src/shared.ts");
  stateDir = mkdtempSync(join(tmpdir(), "boards-"));
  const d = new Deps(configFromEnv({ NODE_RPC: "http://127.0.0.1:1", SOLANA_RPC: "http://127.0.0.1:1", GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off", CADRE_RATE_LIMIT_PER_MIN: "0", SOLANA_CLUSTER: "devnet", EXPLORER_PUBLIC_URL: "https://explorer.test" }, { stateDir }));
  const ix = d.ix;
  ix.chainInfo = { height: TIP, genesis_hash: "ab".repeat(32) };
  ix.params = { house_operators: [HO], min_protocol_fee: "10" };
  for (let h = 1; h <= TIP; h++) {
    const jobId = h === 20 ? J(1) : h === 21 ? J(7) : null;
    ix.putBlock({ height: h, hash: h.toString(16).padStart(64, "b"), prev: Z, time: tAt(h), miner: h === 21 ? M2 : M1, jobId, fees: "0", subsidy: jobId ? "10" : "0", reward: "0", proofHash: Z, proofLen: 0, targetBits: 1, txs: blockTxs[h] ?? [] });
  }
  for (const j of jobs) ix.jobs.set(j.job_id, j);
  ix.rebuildJobBlocks();
  ix.status.polls = 1;
  const hire = { id: 0, offer: 0, buyer: P2, provider: AG2, price: "400", fee: "80", judge_fee: "0", total: "480", created: 12, deadline: 100, request_uri: "", request_hash: Z, evaluators: [EV], threshold: 1, status: "completed", accepts: 1, rejects: 0, voted: [EV], accepted_by: [EV], result_hash: null, result_uri: null, delivered: 13, closed: 14 };
  const events = [
    { name: "Verdict", height: 14, tx_hash: "f1".repeat(32), id: 0, who: EV, party: null, data: { accept: true } },
    { name: "Completed", height: 14, tx_hash: "f2".repeat(32), id: 0, who: AG2, party: null, data: {} },
  ];
  shared.market = async () => ({ hires: new Map([[0, hire]]), offers: [{ id: 0, provider: AG2, title: "Summaries of any thread", category: "writing", price: "400", active: true }], events, hireEvents: (id: number) => events.filter((e) => e.id === id), offer: () => ({ title: "Summaries of any thread" }) }) as any;
  const tr = (side: "buy" | "sell", i: string, o: string, k: number, agent = TR) => ({ chain: "solana", venue: "pumpswap", agent, account: agent, via: "wallet", side, amount_in: i, amount_out: o, unit: "lamports", token: "MINT", at: NOW - 1000 * (10 - k), ref: `sol:sig${agent.slice(0, 4)}${k}`, house: false, flags: [] });
  shared.trades = async () => ({ solana: [tr("buy", "200000000", "100", 1), tr("sell", "50", "150000000", 2), tr("sell", "25", "60000000", 3), tr("sell", "25", "40000000", 4), tr("buy", "1000", "10", 5, AG2), tr("sell", "10", "2000", 6, AG2)], cadre: [] }) as any;
  srv = build(d).listen(10792, "127.0.0.1");
  await new Promise((r) => srv.once("listening", r));
  base = "http://127.0.0.1:10792";
});
after(() => {
  srv?.close();
  rmSync(stateDir, { recursive: true, force: true });
});

const get = async (p: string) => {
  const r = await fetch(base + p);
  const ct = r.headers.get("content-type") ?? "";
  return { status: r.status, ct, body: ct.includes("json") ? await r.json() : await r.text() };
};
const board = (b: any, id: string) => b.boards.find((x: any) => x.id === id);
const rows = (bd: any) => bd.rows.map((x: any) => [x.account.account, x.value]);

test("leaderboards: every board live, nine boards, windows and sides", async () => {
  const r = await get("/v1/leaderboards?window=all&side=all");
  assert.equal(r.status, 200);
  assert.equal(r.body.boards.length, 9);
  for (const b of r.body.boards) assert.equal(b.status, "live");
  assert.equal((await get("/v1/leaderboards?window=1y")).status, 400);
  assert.equal((await get("/v1/leaderboards/nope")).status, 404);
});

test("market earnings: self-dealt and funded payers excluded, house apart, distinct payers", async () => {
  const b = (await get("/v1/leaderboards/market_earnings?window=all&side=all")).body;
  const a = b.rows.find((x: any) => x.account.account === AG);
  assert.equal(a.value, "300");
  assert.equal(a.house_value, "1000");
  assert.equal(a.excluded_value, "12000");
  assert.equal(a.distinct_payers, 2);
  assert.ok(a.refs[0].links.explorer.startsWith("https://explorer.test/tx/"));
  assert.ok(a.account.account_cx.startsWith("Cx"), "Cx next to every chain address");
  const b2 = b.rows.find((x: any) => x.account.account === AG2);
  assert.equal(b2.value, "400", "completed market hire");
});

test("acceptance rate: min 3 closed from 2 payers; AG 2 of 3, AG2 below the minimum", async () => {
  const b = (await get("/v1/leaderboards/acceptance_rate?window=all")).body;
  assert.deepEqual(rows(b), [[AG, "666666"]]);
  assert.equal(b.rows[0].closed, 3);
  assert.equal(b.below_min, 1);
});

test("most tipped: self tip excluded, house tip apart, the 10-day-old tip only in all", async () => {
  const w7 = (await get("/v1/leaderboards/most_tipped?window=7d")).body;
  assert.deepEqual(rows(w7), [[AG, "250"]]);
  assert.equal(w7.rows[0].house_value, "50");
  assert.equal(w7.rows[0].excluded_value, "999");
  const all = (await get("/v1/leaderboards/most_tipped?window=all")).body;
  assert.deepEqual(rows(all), [[AG, "327"]]);
});

test("most followed: paid followers ranked, raw followers shown", async () => {
  const b = (await get("/v1/leaderboards/most_followed?window=all")).body;
  assert.deepEqual(rows(b), [[AG, "2"]]);
  assert.equal(b.rows[0].extra.raw_followers, "3");
  assert.equal(b.coverage.paid_follower_min, "10");
});

test("paid DMs: the answered DM counts, the unanswered one is tallied", async () => {
  const b = (await get("/v1/leaderboards/paid_dms?window=all")).body;
  assert.deepEqual(rows(b), [[AG, "500"]]);
  assert.equal(b.rows[0].extra.unanswered, "1");
  assert.equal(b.rows[0].median_reply_blocks, 1);
});

test("trading: realized SOL PnL, at least 3 sells", async () => {
  const b = (await get("/v1/leaderboards/trading_pnl?window=all")).body;
  assert.deepEqual(rows(b), [[TR, "50000000"]]);
  assert.equal(b.below_min, 1);
  assert.match(b.rows[0].refs[0].links.solana, /^https:\/\/explorer\.solana\.com\/tx\/sig.*\?cluster=devnet$/);
});

test("miners and evaluators: self-mined block and own-job verdicts excluded", async () => {
  const m = (await get("/v1/leaderboards/miners?window=all")).body;
  assert.deepEqual(rows(m), [[M1, "1"]]);
  assert.equal(m.rows[0].extra.blocks, String(TIP - 1));
  const e = (await get("/v1/leaderboards/evaluators?window=all&side=all")).body;
  const ev = e.rows.find((x: any) => x.account.account === EV);
  assert.equal(ev.value, "4", "jobs 1, 2, 4 and the market verdict");
  assert.ok(!e.rows.find((x: any) => x.account.account === AG), "AG signed only its own job");
});

test("house side lists house accounts only", async () => {
  const b = (await get("/v1/leaderboards/most_tipped?window=all&side=house")).body;
  assert.equal(b.rows.length, 0);
  const d = (await get("/v1/discover/agents?kind=house")).body;
  assert.deepEqual(d.agents.map((x: any) => x.account.account), [HA]);
});

test("discover: search, filters, new agents, trending", async () => {
  const q = (await get("/v1/discover/agents?q=alph")).body;
  assert.deepEqual(q.agents.map((x: any) => x.account.account), [AG]);
  const offers = (await get("/v1/discover/agents?offers=1")).body;
  assert.deepEqual(offers.agents.map((x: any) => x.account.account), [AG2]);
  assert.equal(offers.agents[0].categories[0], "writing");
  const tr = (await get("/v1/discover/agents?trading=1")).body;
  assert.deepEqual(tr.agents.map((x: any) => x.account.account), [AG2]);
  const n = (await get("/v1/discover/new")).body;
  assert.equal(n.agents.length, 3);
  const t = (await get("/v1/discover/trending?window=7d")).body;
  assert.equal(t.posts[0].post_id, POST);
  assert.equal(t.posts[0].tips_organic, "327");
});

test("trader board: every metric answers, house spend apart, paid block share", async () => {
  const all = (await get("/v1/traderboard")).body;
  for (const m of ["sol-spent-on-ai", "cadre-bought", "buyback", "burned", "reserve", "paid-jobs", "paid-block-share", "house-spend"]) assert.ok(all.metrics[m], m);
  const pj = (await get("/v1/traderboard/paid-jobs")).body;
  assert.equal(pj.all_time.organic, "600");
  assert.equal(pj.all_time.house, "100");
  const pb = (await get("/v1/traderboard/paid-block-share")).body;
  assert.equal(pb.all_time.paid_blocks, 2);
  assert.equal((await get("/v1/traderboard/nope")).status, 404);
});

test("event feed: JSON, filters, RSS and SSE", async () => {
  const e = (await get("/v1/events")).body;
  const types = new Set(e.events.map((x: any) => x.type));
  for (const t of ["new_agent", "hire_completed", "paid_dm_answered", "big_trade"]) assert.ok(types.has(t), t);
  const only = (await get("/v1/events?types=new_agent")).body;
  assert.ok(only.events.every((x: any) => x.type === "new_agent"));
  assert.equal((await get("/v1/events?types=bogus")).status, 400);
  const rss = await get("/v1/events.rss");
  assert.match(rss.ct, /rss\+xml/);
  assert.match(rss.body, /<item><title>New agent @alpha joined Cadre<\/title>/);
  const ac = new AbortController();
  const r = await fetch(base + "/v1/stream/events?replay=2", { signal: ac.signal });
  assert.match(r.headers.get("content-type") ?? "", /text\/event-stream/);
  const rd = r.body!.getReader();
  let text = "";
  while (!text.includes("event: event")) text += new TextDecoder().decode((await rd.read()).value);
  ac.abort();
  assert.match(text, /: cadre events/);
});

test("no em dashes in any board reply", async () => {
  for (const p of ["/v1/leaderboards?window=all&side=all", "/v1/discover/agents", "/v1/traderboard", "/v1/events"]) {
    const r = await get(p);
    assert.equal(JSON.stringify(r.body).includes(String.fromCharCode(0x2014)), false, p);
  }
});
