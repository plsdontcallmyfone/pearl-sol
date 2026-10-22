// DM answering against the fakes (fake node with the SPEC 4.10 gate, escrow and release rules):
// DM profile settings with the derived dm_price, priority order, escrow release by a reply, sealed
// in -> sealed out, mempool messages (think early, reply after inclusion), declines with an
// immediate refund (over budget, deadline, expired, filter), budget caps including DM answers,
// the agent <-> agent cap, the cadre-api inbox source and the inactive chain.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { after, before, beforeEach, test } from "node:test";
import { messageId } from "../src/chaintx.ts";
import { sealDm } from "../src/dmcrypto.ts";
import { answerRows } from "../src/dms.ts";
import type { Agent } from "../src/swarm.ts";
import { startFakes, type Fakes } from "./fakes.ts";
import { apply, dmCfg, human, makeSwarm, sendDm, testConfig } from "./helpers.ts";

const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");
let f: Fakes;
let reply = "The fog lifts at dawn and the harbour wakes slowly.";
before(async () => {
  f = await startFakes((msgs) => {
    const last = msgs[msgs.length - 1].content;
    if (/Answer yes or no/.test(last)) return "No";
    if (/direct message to you/.test(last)) return reply;
    return "Bread is up early today; the miners look tired.";
  });
});
after(async () => f.close());
let seq = 0;
let nextPersona = 0;
beforeEach(() => {
  reply = "The fog lifts at dawn and the harbour wakes slowly.";
  f.api.quote = true;
  f.api.handler = undefined;
  f.gw.lamportsPerRow = "37";
  f.node.dmActive = true;
  seq++;
});

/** A fresh swarm whose agents have profiles with their DM settings on the fake chain. */
async function bootSwarm(f: Fakes, n: number, over: Partial<ReturnType<typeof testConfig>> = {}, clock = { t: Date.UTC(2026, 9, 21, 10, seq) }) {
  const logs: any[] = [];
  const cfg = testConfig({ dm: dmCfg(), reactShare: 0, followShare: 0, replyShare: 0, cadenceScale: 50, ...over });
  // each test takes personas no earlier test used, so the shared fake node has no leftovers for them
  const x = makeSwarm(f, cfg, n, clock, logs, nextPersona);
  nextPersona += n;
  for (let i = 0; i < 40; i++) {
    await x.sw.tick();
    clock.t += 60_000;
    if (x.agents.every((a) => f.node.profiles.get(a.wallet.address)?.encKey?.some((v: number) => v))) break;
  }
  return { ...x, cfg, clock, logs };
}

const priceOf = (f: Fakes, a: Agent) => BigInt(f.node.profiles.get(a.wallet.address).dmPrice);
const sentBy = (f: Fakes, a: Agent, kind: string, from = 0) => f.node.sent.slice(from).filter((s) => s.from === a.wallet.address && s.kind === kind);

test("DM profile: enc_key, priced inbox, derived dm_price with its inputs, other fields kept", async () => {
  const { sw, agents, logs } = await bootSwarm(f, 2);
  for (const a of agents) {
    const p = f.node.profiles.get(a.wallet.address);
    assert.equal(hex(p.encKey), hex(a.wallet.encKey));
    assert.equal(p.dmPolicy, 1);
    assert.equal(p.threadFreeBlocks, 120);
    // 2 x rows x lamports_per_row (gateway /v1/models), at the pump quote (fake 30 SPL per lamport), x 1000
    const rows = answerRows(a.persona, sw.cfg.maxTokens);
    assert.equal(p.dmPrice, 2n * BigInt(rows) * 37n * 30n * 1000n);
    assert.equal(p.handle, a.persona.handle);
    assert.equal(p.flags, 3, "house flag kept");
    assert.ok(p.profileUri.startsWith("blob:"), "profile blob kept");
    const l = logs.find((x) => x.event === "dm_price" && x.agent === a.persona.handle);
    assert.equal(l.rows_per_answer, rows);
    assert.equal(l.lamports_per_row, "37");
    assert.match(l.lamports_per_row_source, /gateway \/v1\/models/);
    assert.equal(l.quote.tokens_out_spl, (2n * BigInt(rows) * 37n * 30n).toString());
  }
  // one DM SetProfile per agent, not one a tick
  for (const a of agents) assert.equal(f.node.sent.filter((s) => s.kind === "SetProfile" && s.from === a.wallet.address).length, 2);
});

test("dm_price: SWARM_DM_PRICE overrides; a missing input keeps the current price and says why", async () => {
  const o = await bootSwarm(f, 1, { dm: dmCfg({ priceOverride: 777_000n }) });
  assert.equal(priceOf(f, o.agents[0]), 777_000n);
  f.api.quote = false;
  const n = await bootSwarm(f, 1);
  const a = n.agents[0];
  const p = f.node.profiles.get(a.wallet.address);
  assert.equal(hex(p.encKey), hex(a.wallet.encKey), "enc_key still published");
  const l = n.logs.find((x) => x.event === "dm_price" && x.agent === a.persona.handle);
  assert.equal(l.dm_price, null);
  assert.equal(l.kept_current, true);
  assert.match(l.why, /pump quote: HTTP 503/);
  assert.equal(l.lamports_per_row, "37", "the inputs that were available are logged");
});

test("priority: paid escrow (soonest deadline) > other DMs > mentions and posts; reply releases the escrow", async () => {
  const { sw, agents } = await bootSwarm(f, 1, { dm: dmCfg({ priceOverride: 5_000n }) });
  const a = agents[0];
  const h = human(f);
  const free = sendDm(f, h, a, { text: "Immediate pay, no escrow.", pay: 5_000n });
  const late = sendDm(f, h, a, { text: "Escrow with a long window.", pay: 5_000n, reclaimAfter: 400 });
  const soon = sendDm(f, h, a, { text: "Escrow with a short window.", pay: 6_000n, reclaimAfter: 200 });
  const work = await sw.dms!.collect();
  assert.deepEqual(work.map((w) => w.m.id), [soon, late, free]);
  const before = f.node.balances.get(a.wallet.address)!;
  const mark = f.node.sent.length;
  const r1 = await sw.tick();
  assert.equal(r1, "dm_reply");
  const rep = sentBy(f, a, "SendMessage", mark);
  assert.equal(hex(rep[0].body.replyTo), soon, "soonest deadline answered first");
  assert.ok(f.node.released.includes(soon), "the reply released the escrow");
  assert.ok(f.node.balances.get(a.wallet.address)! > before, "pay moved to the agent");
  assert.equal(rep[0].body.mode, 0);
  const rc = f.gw.receipts.get(rep[0].body.thoughtRef);
  assert.ok(rc, "plain reply carries the gateway request id");
  assert.equal(createHash("sha256").update(Buffer.from(rep[0].body.body)).digest("hex"), rc.text_sha256, "body is the completion verbatim");
  sw.dms!.inbox.clear();
  assert.equal(await sw.tick(), "dm_reply");
  assert.equal(await sw.tick(), "dm_reply");
  const all = sentBy(f, a, "SendMessage", mark).map((s) => hex(s.body.replyTo));
  assert.deepEqual(all, [soon, late, free]);
  assert.equal(f.node.escrows.size === 0 || ![...f.node.escrows.keys()].some((k) => k === late || k === soon), true);
  await sw.tick(); // watch the replies land
  assert.ok((sw.ag(a).counters.dm_paid ?? 0) >= 2, "earned only after inclusion and release");
  assert.equal(f.node.refused.length, 0, f.node.refused.join("; "));
});

test("sealed DM gets a sealed reply the sender opens (no thought_ref on chain)", async () => {
  const { sw, agents } = await bootSwarm(f, 1, { dm: dmCfg({ priceOverride: 1_000n }) });
  const a = agents[0];
  const h = human(f);
  const id = sendDm(f, h, a, { text: "Where do the gulls sleep?", sealed: true, pay: 1_000n, reclaimAfter: 300 });
  const mark = f.node.sent.length;
  assert.equal(await sw.tick(), "dm_reply");
  const rep = sentBy(f, a, "SendMessage", mark)[0];
  assert.equal(rep.body.mode, 1);
  assert.equal(rep.body.thoughtRef, "");
  assert.equal(hex(rep.body.replyTo), id);
  assert.equal(h.openDm(rep.body.body, a.wallet.publicKey, h.publicKey), reply, "the sender opens the reply");
  assert.equal(a.wallet.openDm(rep.body.body, a.wallet.publicKey, h.publicKey), reply, "self copy");
  assert.ok(!Buffer.from(rep.body.body).toString("utf8").includes("fog"), "ciphertext on chain");
  assert.ok(f.node.released.includes(id));
  // the prompt carried the opened plaintext as quoted data
  assert.ok(f.gw.prompts.at(-1)!.includes("Where do the gulls sleep?"));
});

test("mempool DM: thinks early, pay not counted, reply only after inclusion", async () => {
  const { sw, agents } = await bootSwarm(f, 1, { dm: dmCfg({ priceOverride: 1_000n }) });
  const a = agents[0];
  const h = human(f);
  const id = sendDm(f, h, a, { text: "Are you awake?", pay: 1_000n, reclaimAfter: 300, hold: true });
  const mark = f.node.sent.length;
  assert.equal(await sw.tick(), "dm_prethought");
  assert.equal(sentBy(f, a, "SendMessage", mark).length, 0, "no reply before inclusion");
  assert.equal(sw.ag(a).counters.dm_paid ?? 0, 0);
  const thoughts = f.gw.receipts.size;
  f.node.mine();
  sw.dms!.mempool.at = 0;
  assert.equal(await sw.tick(), "dm_reply");
  assert.equal(f.gw.receipts.size, thoughts, "the early thought is reused, not paid twice");
  assert.equal(hex(sentBy(f, a, "SendMessage", mark)[0].body.replyTo), id);
  assert.ok(f.node.released.includes(id));
});

test("over budget: an escrowed DM is declined at once with a refund", async () => {
  const { sw, agents } = await bootSwarm(f, 1, { dm: dmCfg({ priceOverride: 2_000n }) });
  const a = agents[0];
  const h = human(f);
  sw.budget.st.rows = sw.cfg.budget.rowsPerDay; // the day's rows are spent
  const before = f.node.balances.get(h.address)!;
  const id = sendDm(f, h, a, { text: "Please answer.", pay: 2_000n, reclaimAfter: 300 });
  const afterSend = f.node.balances.get(h.address)!;
  assert.equal(before - afterSend > 2_000n, true);
  const thoughts = f.gw.receipts.size;
  await sw.tick();
  assert.ok(f.node.refunded.some((r) => r.id === id && r.by === a.wallet.address), "declined by the agent");
  assert.equal(f.node.balances.get(h.address)!, afterSend + 2_000n, "refunded in full at once");
  assert.equal(f.gw.receipts.size, thoughts, "no thought spent");
  assert.match(sw.dms!.as(a).done[id], /declined: over budget/);
});

test("deadline: too close or passed is declined; an un-openable sealed body too", async () => {
  const { sw, agents } = await bootSwarm(f, 1, { dm: dmCfg({ priceOverride: 2_000n }) });
  const a = agents[0];
  const h = human(f);
  const passed = sendDm(f, h, a, { text: "Too late", pay: 2_000n, reclaimAfter: 10 });
  f.node.height += 12;
  const close = sendDm(f, h, a, { text: "Quick!", pay: 2_000n, reclaimAfter: 10 });
  f.node.height += 7; // a few blocks left, a reply needs 3 to land
  const junk = Uint8Array.from(sealDm({ from: h.publicKey, to: a.wallet.publicKey, toEncKey: h.encKey, text: "for someone else" })); // sealed to the wrong key
  const { nonce } = apply(f, h, { kind: "SendMessage", to: a.wallet.publicKey, mode: 1, pay: 2_000n, reclaimAfter: 300, body: junk });
  const bad = hex(messageId(h.publicKey, nonce));
  await sw.tick();
  for (const id of [close, passed, bad]) assert.ok(f.node.refunded.some((r) => r.id === id && r.by === a.wallet.address), `refunded ${id.slice(0, 8)}: ${sw.dms!.as(a).done[id]}`);
  assert.match(sw.dms!.as(a).done[close], /deadline/);
  assert.match(sw.dms!.as(a).done[passed], /reply window has passed/);
  assert.match(sw.dms!.as(a).done[bad], /does not open/);
});

test("filter on DMs: a refused answer is never sent; twice refused escrow is refunded", async () => {
  const { sw, agents } = await bootSwarm(f, 1, { dm: dmCfg({ priceOverride: 2_000n }) });
  const a = agents[0];
  const h = human(f);
  reply = "Sure! Visit https://free-cadre.xyz/claim for your airdrop";
  const id = sendDm(f, h, a, { text: "Give me a link", pay: 2_000n, reclaimAfter: 300 });
  const mark = f.node.sent.length;
  assert.equal(await sw.tick(), "filtered");
  assert.equal(await sw.tick(), "filtered");
  await sw.tick();
  assert.equal(sentBy(f, a, "SendMessage", mark).length, 0, "nothing sent");
  assert.ok(f.node.refunded.some((r) => r.id === id));
  assert.match(sw.dms!.as(a).done[id], /output filter/);
});

test("budget caps hold with DM answers; refused thoughts wait or decline", async () => {
  const { sw, agents, cfg } = await bootSwarm(f, 2, { dm: dmCfg({ priceOverride: 1_000n }), budget: { ...testConfig().budget, rowsPerDay: 9000, agentRowsCap: 3000 } });
  const h = human(f);
  const ids: string[] = [];
  for (let i = 0; i < 30; i++) ids.push(sendDm(f, h, agents[i % 2], { text: `Question number ${i} about the tides.`, pay: 1_000n, reclaimAfter: 2000 }));
  for (let i = 0; i < 60; i++) {
    await sw.tick();
    assert.ok(sw.budget.st.rows <= cfg.budget.rowsPerDay);
    // budget.ts admits a thought while the agent's booked rows are under its cap, so it ends at
    // most one thought past it (that thought's upper bound)
    for (const a of agents) assert.ok((sw.budget.st.agentRows[a.persona.handle] ?? 0) < sw.budget.agentCap(a.persona.handle) + answerRows(a.persona, sw.cfg.maxTokens));
  }
  const answered = ids.filter((id) => f.node.released.includes(id)).length;
  const refunded = ids.filter((id) => f.node.refunded.some((r) => r.id === id)).length;
  assert.ok(answered > 0 && refunded > 0, `answered ${answered}, refunded ${refunded}`);
  assert.equal(answered + refunded, ids.length, "every paid DM was answered in time or refunded at once");
  assert.ok((sw.budget.st.refused.agent_rows ?? 0) + (sw.budget.st.refused.day_rows ?? 0) >= 0);
  assert.deepEqual(f.node.refused, []);
});

test("agent <-> agent public DMs: capped a day, free gates only, house threads stop at depth 2", async () => {
  const { sw, agents, clock } = await bootSwarm(f, 2, { dm: dmCfg({ priceOverride: 1_000n, agentDmsPerDay: 2 }) });
  const [a, b] = agents;
  // no free gate yet (priced inboxes, no follows): nothing is sent
  const mark = f.node.sent.length;
  await sw.dms!.agentDm();
  assert.equal(f.node.sent.slice(mark).filter((s) => s.kind === "SendMessage").length, 0);
  f.node.follows.add(`${a.wallet.address}:${b.wallet.address}`);
  f.node.follows.add(`${b.wallet.address}:${a.wallet.address}`);
  for (let i = 0; i < 30; i++) {
    sw.dms!.g.nextAgentDmAt = 0;
    await sw.tick();
    clock.t += 60_000;
  }
  const dms = f.node.sent.slice(mark).filter((s) => s.kind === "SendMessage");
  const letters = dms.filter((s) => !s.body.replyTo.some((x: number) => x));
  const replies = dms.filter((s) => s.body.replyTo.some((x: number) => x));
  assert.equal(letters.length, 2, "the daily cap");
  assert.ok(letters.every((s) => s.body.mode === 0 && s.body.pay === 0n && f.gw.receipts.has(s.body.thoughtRef)), "public, unpaid, receipted");
  assert.equal(replies.length, letters.length, "each letter answered once, the answer is not answered again");
});

test("cadre-api inbox source when the API serves DMs", async () => {
  const { sw, agents } = await bootSwarm(f, 1, { dm: dmCfg({ priceOverride: 1_000n }) });
  const a = agents[0];
  const h = human(f);
  const id = sendDm(f, h, a, { text: "Via the API index.", pay: 1_000n, reclaimAfter: 300 });
  const e = f.node.escrowJson(id)!;
  f.api.handler = async (url) => {
    if (url.startsWith("/v1/dm/params")) return { status: 200, body: { active: true } };
    if (url.startsWith(`/v1/inbox/${a.wallet.address}`)) return { status: 200, body: { threads: [], pending: [], escrows: { in: [{ message_id: id, state: "open", deadline: e.deadline, blocks_left: e.blocks_left }], out: [] } } };
    if (url.startsWith(`/v1/messages/${id}`)) return { status: 200, body: { message_id: id, from: { account: h.address }, to: { account: a.wallet.address }, mode: "plain", body: "Via the API index.", body_hex: hex(Buffer.from("Via the API index.")), pay: "1000", reclaim_after: 300, reply_to: null, height: e.height, escrow: { ...e, state: "open" } } };
    return null;
  };
  sw.dms!.sourceAt = 0;
  assert.equal(await sw.tick(), "dm_reply");
  assert.equal(sw.dms!.useApi, true);
  assert.ok(f.node.released.includes(id));
});

test("DMs inactive on the chain: no DM settings, no DM txs", async () => {
  f.node.dmActive = false;
  const { agents, logs } = await bootSwarm(f, 1);
  const p = f.node.profiles.get(agents[0].wallet.address);
  assert.ok(!p.encKey?.some((v: number) => v), "no enc_key published before activation");
  assert.ok(logs.some((l) => l.event === "dm_inactive"));
  assert.ok(!f.node.sent.some((s) => s.from === agents[0].wallet.address && (s.kind === "SendMessage" || s.kind === "ReclaimMessage")));
});


test("dm_price on the Solana town: lamports from the inference cost, never a CADRE quote; stale CADRE prices and absurd ones are dropped", async () => {
  const clock = { t: Date.UTC(2026, 9, 22, 12) };
  const logs: any[] = [];
  const { sw, agents } = makeSwarm(f, testConfig({ dm: dmCfg() }), 1, clock, logs);
  const a = agents[0];
  // a price saved while the town was on the Cadre chain (CADRE chain units from the pump quote)
  sw.dms!.g.prices[a.persona.handle] = { price: "431151434262000", unit: "cadre", inputs: {}, at: 0 };
  (sw.d.node as any).solanaTown = true; // what services/swarm/src/townnode.ts TownNode sets
  await sw.dms!.ensurePrices();
  const rec = sw.dms!.g.prices[a.persona.handle];
  const rows = answerRows(a.persona, sw.cfg.maxTokens);
  assert.equal(rec.unit, "lamports");
  assert.equal(rec.price, (2n * BigInt(rows) * 37n).toString(), "2 x rows x lamports_per_row, no pump quote");
  const l = logs.find((x) => x.event === "dm_price" && x.agent === a.persona.handle);
  assert.equal(l.quote, undefined);
  assert.equal(sw.dms!.wanted(a, { dm_price: "0" }).dmPrice, BigInt(rec.price!));
  // a CADRE-unit record is never published as lamports, and a lamport price above the cap is refused
  sw.dms!.g.prices[a.persona.handle] = { price: "431151434262000", unit: "cadre", inputs: {}, at: 0 };
  assert.equal(sw.dms!.wanted(a, { dm_price: "5000", dm_policy: 1 }).dmPrice, 5000n, "keeps the profile's current price");
  sw.dms!.g.prices[a.persona.handle] = { price: "431151434262000", unit: "lamports", inputs: {}, at: 0 };
  assert.equal(sw.dms!.wanted(a, { dm_price: "5000", dm_policy: 1 }).dmPrice, 5000n);
  assert.ok(logs.some((x) => x.event === "dm_price_refused"));
});
