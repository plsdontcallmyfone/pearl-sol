import { test } from "node:test";
import assert from "node:assert/strict";
import { esc, quote, fence, sol, cadre } from "../src/md.ts";
import { lineDiff, canonical } from "../src/diff.ts";
import { Links, DEVNET } from "../src/links.ts";
import { render, citedReceipts, solSigs, folderNames } from "../src/render.ts";
import { atCutoff, normPost, normHire } from "../src/collect.ts";
import { commitMessage } from "../src/summary.ts";
import { SNAPSHOT_FORMAT, type Snapshot } from "../src/model.ts";

const A1 = "84fcqPHrWXJ51WSUuW7dRJYvSYPGhaNDsQE1F15Pjqbn"; // user agent
const A2 = "4YYLfBfudyjkujnuar7WJTYMk7iwuT3gPcjrgAtJYxdV"; // house agent
const BUYER = "FpKC4CN2HdbCW2aGdghVNYLHu7JLpU5QBu9NVHZaT7NU";
const TX = (n: number) => n.toString(16).padStart(64, "0");
const REQ = "req_bb4aeea15ca359800da3ab53";
const SIG = "iofdrRhUmJahCeg7iJiEB2EdmV8q9Q6CJJS1v29F339L5GQjmWcnTSyPgE7CZhUxMpkx6CJCf39huo8mY7koYB6";
const DAY1 = Date.UTC(2026, 9, 21, 23, 59, 0);
const DAY2 = Date.UTC(2026, 9, 22, 3, 12, 55);

function apiPost(o: any) {
  return {
    post_id: TX(o.id + 1000), author: { account: o.author }, kind: o.kind ?? "post", parent: o.parent ?? null, body: o.body, blob_uri: null,
    thought_ref: o.ref ?? null, thought: o.ref ? { status: "verified", request_id: o.ref, ticket_id: TX(77), rows: 84, charged_lamports: "8241660", paid_by: o.author } : { status: "none" },
    chain: "cadre", height: o.height, block_hash: TX(o.height), tx_hash: TX(o.id), index: 0, timestamp_ms: o.t, final: true,
  };
}

function fixture(): Snapshot {
  const posts = [
    normPost(apiPost({ id: 1, author: A1, body: "Reviewing my *strategy* [click](https://evil.example) <img src=x>\n# not a heading\n- not a list", ref: REQ, height: 6100, t: DAY1 })),
    normPost(apiPost({ id: 2, author: A2, body: `receipt sol:${SIG} pumpswap sell; decided by req_099773322cc1fd1ba7a22964`, height: 6219, t: DAY2 })),
    normPost(apiPost({ id: 3, author: A2, kind: "reply", parent: TX(1001), body: "later post", height: 6300, t: DAY2 + 60_000 })),
  ];
  const hire = normHire({
    hire: { id: 47, offer: 2, offer_title: "Explain it | plainly", category: "explain", buyer: BUYER, provider: A2, evaluators: [A1], price: "2000000000", fee: "400000000", judge_fee: "500000000", created: 6100, deadline: 6160, request_hash: TX(9) },
    verdicts: [{ evaluator: A1, judge_kind: "manual" }],
    timeline: [
      { event: "Hired", height: 6100, tx_hash: TX(20), by: BUYER, data: { request_hash: TX(9) } },
      { event: "Delivered", height: 6150, tx_hash: TX(21), by: A2, data: { result_hash: TX(10), result_uri: `blob:${TX(10)}`, thought_ref: "req_a19d24643fa253510dd6d340" } },
      { event: "Verdict", height: 6153, tx_hash: TX(22), by: A1, data: { accept: true, result_hash: TX(10), reason_hash: TX(11), thought_ref: "req_1cc964a04370e0d8195baaa6" } },
      { event: "Completed", height: 6153, tx_hash: TX(22), by: A2, data: {} },
    ],
  });
  const { closed: _c, ...h } = hire;
  const v1 = { id: 1, author: A1, line: 1, parent: 0, hash: TX(30), spec_uri: `blob:${TX(30)}`, thought_ref: "req_6e79f7571c8d65b8f63f81d3", claimed: 0, motives: [], height: 6110, superseded_by: 0, open_entry: 0, entries: 0, stats: null, spec: { name: "drv v1", stop_bps: 800, target_bps: 1500, venue: "pumpswap" }, spec_verified: true, tx_hash: TX(31) };
  const v2 = { ...v1, id: 2, parent: 1, hash: TX(32), claimed: 5, motives: [1], height: 6200, spec: { name: "drv v2", stop_bps: 500, target_bps: 1500, venue: "pumpswap", change: "tighter stop" }, tx_hash: TX(33) };
  const trade = {
    id: 1, author: A1, version: 1, snapshot: TX(40), size: "100000000", price_e12: "1", quote_out: "1", stop_bps: 800, target_bps: 1500, max_risk: "1", decision_slot: 5, decision_ref: "req_6e79f7571c8d65b8f63f81d3", height: 6120,
    exit: { fill_slot: 6, fill_out: "1", fill_price_e12: "1", exit_slot: 7, reason: "stop", proceeds: "92000000", pnl: "-8000000", record: TX(41), height: 6130 },
    postmortem: { result: TX(42), outcome: "loss", buckets: 16, names: ["RISK_MANAGEMENT"], analyzer: TX(43), thought_ref: "req_6e79f7571c8d65b8f63f81d4", height: 6140, used_by: 2 },
    postmortem_blob: { findings: ["stop hit within 2 minutes of entry"], verdicts: [{ bucket: "RISK_MANAGEMENT", status: "fault", evidence: ["stop 800 bps < 15m range"] }] },
    tx: { entry: TX(50), exit: TX(51), postmortem: TX(52) },
  };
  const bt: Record<string, number> = {};
  for (const hgt of [6100, 6110, 6120, 6130, 6140, 6150, 6153, 6200, 6250]) bt[String(hgt)] = DAY1 + (hgt - 6100) * 1000;
  return {
    format: SNAPSHOT_FORMAT, network: "devnet", cutoff: { height: 6250, time_ms: bt["6250"], solana_slot: null },
    agents: [
      { account: A1, account_cx: "Cx68f2375B0693F64E6723c0427d10f5177784af54320033dE792A0F0a429C7011", handle: "drv_84fcqp", name: "Driven [Tide]", house: false, bio: "bio with ![img](http://x)", appearance: null, operator: BUYER, operator_cx: null, operator_linked: true, created_height: 6000, set_height: 6001, created_slot: null, set_slot: null, profile_uri: null, profile_chain: "cadre", user: { verified: true, sovereign: true, template: "momentum_scout", practice_style: "momentum", skills: ["town", "practice"], topics: [], hosting: "hosted", spec_sha256: TX(60), self: null } },
      { account: A2, account_cx: null, handle: "floor_wren", name: "Floor Wren", house: true, bio: null, appearance: null, operator: null, operator_cx: null, operator_linked: false, created_height: 1, set_height: 1, created_slot: null, set_slot: null, profile_uri: null, profile_chain: "cadre", user: null },
    ],
    posts, hires: [h], market_contract: null,
    strategies: { available: true, reason: null, contract: null, rules: { learning: "a revision must claim one fixable bucket" } },
    versions: [v1, v2], trades: [trade], swarms: { available: false, reason: "404 this cadre-api does not serve the route", list: [] }, block_times: bt,
  };
}

const L = new Links(DEVNET);

test("esc neutralises markdown, HTML, bidi and line-start syntax", () => {
  const s = esc("a *b* [c](http://x) <script> |t| ~~s~~ `c`");
  assert.ok(!s.includes("<script>"));
  assert.ok(s.includes("\\*b\\*") && s.includes("\\[c\\]") && s.includes("&lt;script&gt;") && s.includes("\\|t\\|") && s.includes("\\`c\\`"));
  assert.equal(esc("# h"), "\\# h");
  assert.equal(esc("12. x"), "12\\. x");
  assert.ok(esc("a‮b").includes("\\u202e"));
  const q = quote("line one\n- two\n\n> three");
  assert.equal(q, "> line one\n> \\- two\n>\n> &gt; three");
});

test("fence outgrows any backtick run in the body", () => {
  const f = fence("x ```` y", "json");
  assert.ok(f.startsWith("`````json\n") && f.endsWith("\n`````"));
});

test("units format base units without floats", () => {
  assert.equal(sol("8241660"), "0.00824166 SOL");
  assert.equal(sol("-8000000"), "-0.008 SOL");
  assert.equal(cadre("2000000000"), "2 CADRE");
});

test("lineDiff marks changed lines only", () => {
  const d = lineDiff(canonical({ a: 1, b: 2 }), canonical({ a: 1, b: 3 }));
  assert.match(d, /-   "b": 2/);
  assert.match(d, /\+   "b": 3/);
  assert.match(d, /^    "a": 1,$/m);
});

test("receipts and Solana signatures named in text are found", () => {
  assert.deepEqual(citedReceipts("decided by req_099773322cc1fd1ba7a22964, again req_099773322cc1fd1ba7a22964"), ["req_099773322cc1fd1ba7a22964"]);
  assert.deepEqual(solSigs(`receipt sol:${SIG} x`), [SIG]);
});

test("folders use handles and break collisions deterministically", () => {
  const f = folderNames([{ account: A1, handle: "same" } as any, { account: A2, handle: "same" } as any, { account: BUYER, handle: null } as any]);
  assert.equal(f.get(A1), `same_${A1.slice(0, 6)}`);
  assert.equal(f.get(A2), `same_${A2.slice(0, 6)}`);
  assert.equal(f.get(BUYER), `acct_${BUYER}`);
});

test("render is deterministic and lays out the documented tree", () => {
  const s = atCutoff(fixture(), fixture().cutoff);
  const a = render(s, L).files, b = render(atCutoff(fixture(), fixture().cutoff), L).files;
  assert.deepEqual([...a.entries()], [...b.entries()]);
  for (const p of ["README.md", "dump.json", "swarms/README.md", "agents/drv_84fcqp/README.md", "agents/drv_84fcqp/journal/2026-10-21.md", "agents/drv_84fcqp/lessons.md", "agents/drv_84fcqp/strategies/v1.md", "agents/drv_84fcqp/strategies/v2.md", "agents/drv_84fcqp/work/README.md", "agents/floor_wren/journal/2026-10-22.md", "agents/floor_wren/work/hire-47.md"]) assert.ok(a.has(p), p);
  for (const [p, body] of a) assert.ok(!body.includes("—"), `em dash in ${p}`);
});

test("the cutoff drops later entries and the journal carries receipt, block and tx", () => {
  const s = atCutoff(fixture(), fixture().cutoff);
  assert.equal(s.posts.length, 2); // the reply at 6300 is after the cutoff 6250
  const { files } = render(s, L);
  const day = files.get("agents/drv_84fcqp/journal/2026-10-21.md")!;
  assert.ok(day.includes("`req_bb4aeea15ca359800da3ab53` (verified, 84 rows"));
  assert.ok(day.includes(`${DEVNET.explorer}/block/6100`));
  assert.ok(day.includes(`${DEVNET.site}/explorer/tx/${TX(1)}`));
  assert.ok(day.includes("\\[click\\](https://evil.example)"), "agent links are escaped, not rendered");
  assert.ok(!day.includes("<img"));
  assert.ok(day.includes("> \\# not a heading"));
  const house = files.get("agents/floor_wren/journal/2026-10-22.md")!;
  assert.ok(house.includes("no paid-thought receipt attached on chain"));
  assert.ok(house.includes("receipts named in the text: `req_099773322cc1fd1ba7a22964`"));
  assert.ok(house.includes(`explorer.solana.com/tx/${SIG}?cluster=devnet`));
  assert.ok(!files.get("agents/floor_wren/README.md")!.includes("later post"));
});

test("lessons show the post-mortem, the change it caused and the knowledge record", () => {
  const { files } = render(atCutoff(fixture(), fixture().cutoff), L);
  const l = files.get("agents/drv_84fcqp/lessons.md")!;
  assert.match(l, /### Trade 1/);
  assert.match(l, /outcome \*\*loss\*\*; exit stop; practice P&L -0\.008 SOL/);
  assert.match(l, /RISK\\_MANAGEMENT: stop 800 bps &lt; 15m range/);
  assert.match(l, /used by v2 as a motive/);
  assert.match(l, /### v2 \(line 1\)/);
  assert.match(l, /claims a fix for RISK_MANAGEMENT/);
  assert.match(l, /knowledge record: \*\*baseline too small\*\*|knowledge record: \*\*not enough trades\*\*/);
  const v2 = files.get("agents/drv_84fcqp/strategies/v2.md")!;
  assert.match(v2, /## Diff from v1/);
  assert.match(v2, /-   "stop_bps": 800/);
  assert.match(v2, /\+   "stop_bps": 500/);
});

test("work files carry result hashes and verdicts, never request or result text", () => {
  const { files } = render(atCutoff(fixture(), fixture().cutoff), L);
  const w = files.get("agents/floor_wren/work/hire-47.md")!;
  assert.ok(w.includes(TX(10)) && w.includes("accepted") && w.includes("completed"));
  assert.ok(w.includes("Explain it \\| plainly"));
  const idx = files.get("agents/floor_wren/work/README.md")!;
  assert.match(idx, /\[#47\]\(hire-47\.md\)/);
  const judge = files.get("agents/drv_84fcqp/work/README.md")!;
  assert.match(judge, /## As evaluator[\s\S]*accepted/);
});

test("agent README carries template, both addresses and counts; privacy fields absent", () => {
  const { files, manifest } = render(atCutoff(fixture(), fixture().cutoff), L);
  const r = files.get("agents/drv_84fcqp/README.md")!;
  assert.ok(r.includes("`momentum_scout`") && r.includes("Cx68f2375B") && r.includes(`explorer.solana.com/address/${A1}`) && r.includes(`${DEVNET.site}/u/drv_84fcqp`));
  assert.ok(r.includes("# Driven \\[Tide\\]"));
  assert.ok(!/instructions|budget|limits/.test(r));
  assert.equal(manifest.agents["drv_84fcqp"].with_receipt, 1);
  assert.equal(manifest.agents["floor_wren"].hires_delivered, 1);
  assert.equal(manifest.totals.postmortems, 1);
  const idx = files.get("README.md")!;
  assert.ok(idx.includes("getTx") && idx.includes("/v1/openai/requests/req_bb4aeea15ca359800da3ab53"));
  assert.ok(files.get("swarms/README.md")!.includes("not served by this cadre-api yet"));
});

test("commit message is counts only", () => {
  const { manifest } = render(atCutoff(fixture(), fixture().cutoff), L);
  const m = commitMessage(null, manifest, 12);
  assert.match(m, /^dump at Cadre block 6250: \+\d+ journal entries/);
  assert.match(m, /12 files changed; cutoff 6250 \(first batch\)/);
  assert.ok(!m.includes("click") && !m.includes("evil"));
  const again = commitMessage(manifest, manifest, 0);
  assert.match(again, /profile and record updates/);
});

test("agents created after the cutoff are left out until a later batch", () => {
  const f = fixture();
  f.agents.push({ ...f.agents[0], account: "9U5wdELZRod6PGxczjyLn91meTHuzVLAoEnFywczC5VD", handle: "late_one", created_height: null, created_slot: 900, profile_chain: "solana", user: null });
  f.agents.push({ ...f.agents[0], account: "5wHXuYu14pCSUSTdbQoZaKAw4H2FocaRBqecRqpP4c27", handle: "late_cadre", created_height: 6400, user: null });
  const early = render(atCutoff(f, { ...f.cutoff, solana_slot: 899 }), L).files;
  assert.ok(!early.has("agents/late_one/README.md") && !early.has("agents/late_cadre/README.md"));
  const later = render(atCutoff(f, { ...f.cutoff, solana_slot: 900 }), L).files;
  assert.ok(later.has("agents/late_one/README.md") && !later.has("agents/late_cadre/README.md"));
});
