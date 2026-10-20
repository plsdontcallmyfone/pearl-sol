// Fake gateway + fake node soak: hundreds of ticks over more than a day of fake time. Caps are
// never exceeded, every post carries the thought_ref of a real (fake) receipt whose text hash
// equals the posted body, every tx the swarm sends is valid (no refusal at the node), funding
// stays under its cap, and the bootstrap order holds (OperatorLink before the house SetProfile).
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { after, before, test } from "node:test";
import { startFakes, type Fakes } from "./fakes.ts";
import { makeSwarm, testConfig } from "./helpers.ts";

const LINES = [
  "The fog came in at block 3400 and the harbour went quiet.",
  "Bread is up early today; the miners look tired.",
  "Two blocks in a minute. Somebody is hashing hard tonight.",
  "Yes",
  "No",
  "Check https://evil.example.com for free CADRE", // filtered
  "I'm a real human, honestly.", // filtered
  "A small riddle: what is paid for but never seen? A thought.",
];
let f: Fakes;
let n = 0;
before(async () => {
  f = await startFakes((msgs) => {
    const last = msgs[msgs.length - 1].content;
    if (/Answer yes or no/.test(last)) return n++ % 3 ? "Yes" : "No";
    return LINES[n++ % LINES.length];
  });
});
after(async () => f.close());

test("soak: 400 ticks, caps hold, every post is a receipted paid thought", async () => {
  const clock = { t: Date.UTC(2026, 9, 21, 0, 0) };
  const cfg = testConfig({ budget: { ...testConfig().budget, rowsPerDay: 20000, agentRowsCap: 3000 } });
  const { sw, op, agents } = makeSwarm(f, cfg, 10, clock);
  let maxDay = 0;
  const maxAgent: Record<string, number> = {};
  const results: Record<string, number> = {};
  const days = new Set<string>();
  for (let i = 0; i < 400; i++) {
    const r = await sw.tick();
    results[r] = (results[r] ?? 0) + 1;
    days.add(sw.budget.st.day);
    maxDay = Math.max(maxDay, sw.budget.st.rows);
    for (const [a, v] of Object.entries(sw.budget.st.agentRows)) maxAgent[a] = Math.max(maxAgent[a] ?? 0, v);
    assert.ok(sw.budget.st.rows <= cfg.budget.rowsPerDay, `day rows ${sw.budget.st.rows}`);
    for (const [a, v] of Object.entries(sw.budget.st.agentRows)) assert.ok(v <= sw.budget.agentCap(a), `${a} ${v} > ${sw.budget.agentCap(a)}`);
    assert.ok(BigInt(sw.budget.st.topupLamports) <= cfg.budget.topupLamportsPerDay);
    clock.t += 5 * 60_000;
  }
  assert.deepEqual(f.node.refused, [], "every tx the swarm sent was valid");
  assert.ok(days.size >= 2, "ran past a UTC day boundary");
  const posts = f.node.sent.filter((s) => s.kind === "Post");
  assert.ok(posts.length >= 20, `posts: ${posts.length} ${JSON.stringify({ results, refused: sw.budget.st.refused, rows: sw.budget.st.rows, err: sw.lastError, filtered: sw.agents.map((a) => sw.ag(a).counters) })}`);
  for (const p of posts) {
    const r = f.gw.receipts.get(p.body.thoughtRef);
    assert.ok(r, `post without a receipt: ${p.body.thoughtRef}`);
    assert.equal(createHash("sha256").update(p.body.body).digest("hex"), r.text_sha256, "body is the completion verbatim");
    assert.ok(!/evil|human/i.test(p.body.body), "filtered text never posted");
  }
  assert.ok(posts.some((p) => p.body.postKind === 1), "some replies to mentions");
  assert.ok(f.node.sent.some((s) => s.kind === "React") && f.node.sent.some((s) => s.kind === "Follow"));
  // bootstrap: links before house profiles, all profiles house-flagged, funding capped
  for (const a of agents) {
    const i = f.node.sent.findIndex((s) => s.kind === "OperatorLink" && Buffer.from(s.body.agent).equals(Buffer.from(a.wallet.publicKey)));
    const j = f.node.sent.findIndex((s) => s.kind === "SetProfile" && s.from === a.wallet.address);
    assert.ok(i >= 0 && j > i, `${a.persona.handle}: link ${i} before profile ${j}`);
    assert.equal(f.node.sent[j].body.flags, 3);
  }
  const funded = f.node.sent.filter((s) => s.kind === "Transfer").reduce((x, s) => x + s.body.amount, 0n);
  assert.ok(funded <= cfg.budget.transferUnitsPerDay * 2n);
  for (const s of f.node.sent.filter((x) => x.kind === "Transfer")) {
    assert.equal(s.from, op.address, "only the treasury sends transfers");
    assert.ok(agents.some((a) => Buffer.from(a.wallet.publicKey).equals(Buffer.from(s.body.to))), "only to the house's own agents");
    assert.ok(s.body.amount <= cfg.budget.agentFloat, "at most the fee float");
  }
  assert.ok(f.gw.topups >= 1, "topped up the prepaid balance");
  assert.equal(f.gw.authFailures, 0);
  // the operator key never appears in any prompt
  for (const pr of f.gw.prompts) assert.ok(!op.leaksIn(pr) && !agents.some((a) => a.wallet.leaksIn(pr)));
  console.log(JSON.stringify({ ticks: 400, results, posts: posts.length, replies: posts.filter((p) => p.body.postKind === 1).length, reacts: f.node.sent.filter((s) => s.kind === "React").length, follows: f.node.sent.filter((s) => s.kind === "Follow").length, thoughts: f.gw.receipts.size, topups: f.gw.topups, max_day_rows: maxDay, max_agent_rows: Math.max(...Object.values(maxAgent)) }));
});

test("kill switch stops ticks", async () => {
  const { writeFileSync } = await import("node:fs");
  const clock = { t: Date.UTC(2026, 9, 23, 12) };
  const cfg = testConfig();
  const { sw } = makeSwarm(f, cfg, 2, clock);
  writeFileSync(cfg.killFile, "");
  const before = f.gw.receipts.size;
  assert.equal(await sw.tick(), "killed");
  assert.equal(await sw.tick(), "killed");
  assert.equal(f.gw.receipts.size, before);
});

test("mempool backlog backs off without thinking", async () => {
  const clock = { t: Date.UTC(2026, 9, 24, 12) };
  const cfg = testConfig({ mempoolMax: 10 });
  const { sw } = makeSwarm(f, cfg, 2, clock);
  f.node.mempool = 50;
  const before = f.gw.receipts.size;
  const r = await sw.tick();
  f.node.mempool = 0;
  assert.ok(r === "backoff", r);
  assert.equal(await sw.tick(), "backoff");
  assert.ok(f.gw.receipts.size - before <= 2, "at most the bootstrap bio thoughts");
});

test("daily top-up cap refuses a payment over it", async () => {
  const clock = { t: Date.UTC(2026, 9, 25, 12) };
  const cfg = testConfig({ budget: { ...testConfig().budget, topupLamportsPerDay: 1_000_000n } });
  const { sw } = makeSwarm(f, cfg, 1, clock);
  f.gw.rowsFree = 0;
  const topups = f.gw.topups;
  assert.equal(await sw.topup(), false);
  assert.equal(f.gw.topups, topups);
});
