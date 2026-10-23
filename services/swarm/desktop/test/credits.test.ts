// The vision model's credit gate: classifier, probe and the pause/resume state machine, on the
// responses recorded on box 0 (test/fixtures/openrouter-credit.json). No network, fake clock.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { CreditGate, creditError, probeCredits } from "../src/credits.ts";
import { runTask, type Chat } from "../src/vision.ts";

const F = JSON.parse(readFileSync(new URL("./fixtures/openrouter-credit.json", import.meta.url), "utf8"));
const rec = (k: string) => ({ status: F[k].status as number, json: structuredClone(F[k].json) });
const ok200 = (cost = 0.00002) => ({ status: 200, json: { choices: [{ message: { content: "ok" } }], usage: { prompt_tokens: 10, completion_tokens: 2, cost } } });

test("classifier: the recorded 402s are credit errors (with the affordable tokens); rate limits and 5xx are not", () => {
  const a = creditError(F.afford_137.status, F.afford_137.json)!;
  assert.equal(a.status, 402);
  assert.equal(a.afford, 137);
  assert.equal(a.requested, 400);
  assert.equal(creditError(F.afford_301.status, F.afford_301.json)!.afford, 301);
  const p = creditError(F.prompt_limit.status, F.prompt_limit.json)!;
  assert.ok(p && p.afford === null && /Prompt tokens limit exceeded/.test(p.message));
  assert.ok(creditError(F.key_limit_403.status, F.key_limit_403.json), "a credit message with another code");
  assert.equal(creditError(F.rate_limit_429.status, F.rate_limit_429.json), null);
  assert.equal(creditError(F.server_500.status, F.server_500.json), null);
  assert.equal(creditError(0, { error: "fetch failed" }), null);
  assert.equal(creditError(200, { choices: [] }), null);
  assert.ok(creditError(402, null), "a bare 402 is still out of credit");
});

test("vision driver: a 402 is not retried and comes back marked as a credit error; a halted gate stops a task before its next model call", async () => {
  const sbx: any = { screenshot: async () => new Uint8Array([1]), press: async () => {}, write: async () => {} };
  let calls = 0;
  const chat: Chat = async () => { calls++; return rec("afford_137"); };
  const r = await runTask(sbx, chat, { model: "m", system: "s", task: "t", maxSteps: 5, maxUsd: 1 });
  assert.equal(calls, 1, "no retry: it cannot succeed until credit is added");
  assert.equal(r.ok, false);
  assert.equal(r.credit?.afford, 137);
  assert.match(r.error!, /^model 402: This request requires more credits/);
  // the prompt-limit shape arrives on a later step and carries the spend already made
  let n = 0;
  const later: Chat = async () => (n++ === 0
    ? { status: 200, json: { choices: [{ message: { content: "Looking", tool_calls: [{ id: "c1", type: "function", function: { name: "computer", arguments: JSON.stringify({ action: "wait", seconds: 1 }) } }] } }], usage: { cost: 0.000463, prompt_tokens: 2638, completion_tokens: 399 } } }
    : rec("prompt_limit"));
  const r2 = await runTask(sbx, later, { model: "m", system: "s", task: "t", maxSteps: 5, maxUsd: 1 });
  assert.ok(r2.credit && r2.steps === 1 && Math.abs(r2.usd - 0.000463) < 1e-9);
  let c3 = 0;
  const r3 = await runTask(sbx, async () => { c3++; return ok200(); }, { model: "m", system: "s", task: "t", maxSteps: 5, maxUsd: 1, halted: () => true });
  assert.equal(c3, 0);
  assert.equal(r3.error, "vision_paused");
});

test("probe: /key with no per-key limit proves nothing, so the tiny request decides; an empty per-key limit or a 401 stays paused without spending", async () => {
  const sent: any[] = [];
  const chatWith = (r: { status: number; json: any }): Chat => async (b) => { sent.push(b); return r; };
  let r = await probeCredits({ keyInfo: async () => rec("key_limit_empty"), chat: chatWith(ok200()), model: "m", maxTokens: 1600, minUsd: 0.01 });
  assert.equal(r.ok, false);
  assert.match(r.why, /limit_remaining 0/);
  assert.equal(sent.length, 0, "no tokens spent when /key already says empty");
  r = await probeCredits({ keyInfo: async () => rec("key_rejected"), chat: chatWith(ok200()), model: "m", maxTokens: 1600, minUsd: 0.01 });
  assert.equal(r.ok, false);
  assert.equal(sent.length, 0);
  r = await probeCredits({ keyInfo: async () => rec("key_unlimited"), chat: chatWith(rec("afford_137")), model: "m", maxTokens: 1600, minUsd: 0.01 });
  assert.equal(r.ok, false);
  assert.equal(r.afford, 137);
  assert.equal(sent[0].max_tokens, 1600, "the probe asks for a whole step's worth so a pass means a step is affordable");
  assert.equal(sent[0].messages.length, 1);
  r = await probeCredits({ keyInfo: async () => rec("key_unlimited"), chat: chatWith(ok200(0.00003)), model: "m", maxTokens: 1600, minUsd: 0.01 });
  assert.equal(r.ok, true);
  assert.equal(r.usd, 0.00003);
  r = await probeCredits({ keyInfo: async () => { throw new Error("down"); }, chat: chatWith(rec("server_500")), model: "m", maxTokens: 1600, minUsd: 0.01 });
  assert.equal(r.ok, false);
  assert.match(r.why, /inconclusive/);
  r = await probeCredits({ chat: async () => { throw new Error("ECONNRESET"); }, model: "m", maxTokens: 1600, minUsd: 0.01 });
  assert.equal(r.ok, false, "a dropped connection is not a resume");
});

test("gate: pause on the first credit error with one log line, probe on schedule, remind every 30 min, hold desktops after 30 min, resume with one line", async () => {
  let t = Date.UTC(2026, 9, 22, 22, 24, 0);
  const log: Record<string, unknown>[] = [];
  const g = new CreditGate({ probeS: 300, flapS: 600, maxProbeS: 3600, holdDesksS: 1800, remindS: 1800, log: (e) => log.push(e), now: () => t });
  assert.equal(g.paused, false);
  assert.equal(g.probeDue(), false, "nothing to probe while credit is fine");
  const e = creditError(F.afford_137.status, F.afford_137.json)!;
  assert.equal(g.trip(e, "betony83/social read_town"), true);
  assert.equal(g.trip(e, "hyssop58/social explorer_blocks"), false, "parallel failures do not log again");
  assert.deepEqual(log.map((x) => x.event), ["vision_paused"]);
  assert.match(String(log[0]!.reason), /can only afford 137 of the 400 max_tokens/);
  assert.equal(g.status().paused, true);
  assert.equal(g.status().afford_tokens, 137);
  g.skip(); g.skip();
  // not due before the interval
  t += 299_000;
  let probes = 0;
  const empty = async () => { probes++; return { ok: false, usd: 0.00001, why: "still out of credit (402, can only afford 137 tokens)" }; };
  assert.equal(await g.tick(empty), false);
  assert.equal(probes, 0);
  // five hours of failed probes: one every 5 min, a reminder every 30 min, no other lines
  for (let i = 0; i < 60; i++) { t += 300_000; await g.tick(empty); }
  assert.equal(probes, 60);
  assert.equal(log.filter((x) => x.event === "vision_still_paused").length, 10);
  assert.equal(log.length, 11);
  assert.equal(g.holdDue(), true, "past 30 min the desktops should be paused");
  g.desksHeld = true;
  assert.equal(g.holdDue(), false, "held once");
  assert.ok(Math.abs(g.status().probe_usd - 0.0006) < 1e-9);
  assert.equal(g.status().skipped, 2);
  // the owner tops up: the next probe passes and resumes, one line, counters reset
  t += 300_000;
  assert.equal(await g.tick(async () => ({ ok: true, usd: 0.00002, why: "probe answered" })), true);
  const resumed = log[log.length - 1]!;
  assert.equal(resumed.event, "vision_resumed");
  assert.equal(resumed.probes, 61);
  assert.equal(resumed.skipped, 2);
  assert.equal(resumed.desktops_were_held, true);
  assert.equal(g.paused, false);
  assert.equal(g.status().reason, null);
  assert.equal(g.desksHeld, true, "the controller clears it once the desktops run again");
});

test("gate: a pause right after a resume doubles the probe interval (to the max), so a probe that passes but a real step that does not cannot flap every 5 min", async () => {
  let t = 0;
  const g = new CreditGate({ probeS: 300, flapS: 600, maxProbeS: 1200, holdDesksS: -1, remindS: 1800, log: () => {}, now: () => t });
  const e = creditError(F.prompt_limit.status, F.prompt_limit.json)!;
  const pass = async () => ({ ok: true, usd: 0, why: "ok" });
  g.trip(e, "x");
  assert.equal(g.status().probe_every_s, 300);
  for (const want of [600, 1200, 1200]) {
    t += g.intervalMs; await g.tick(pass);
    t += 60_000; g.trip(e, "x");
    assert.equal(g.status().probe_every_s, want);
  }
  assert.equal(g.holdDue(), false, "holdDesksS -1: desktops are never paused by the gate");
  // a pause long after the last resume keeps the current interval (no doubling)
  t += g.intervalMs; await g.tick(pass);
  t += 3600_000; g.trip(e, "x");
  assert.equal(g.status().probe_every_s, 1200);
});

test("gate: a probe that throws counts as not ok and the gate stays paused", async () => {
  let t = 0;
  const g = new CreditGate({ probeS: 60, flapS: 600, maxProbeS: 600, holdDesksS: 0, remindS: 1800, log: () => {}, now: () => t });
  g.trip(creditError(402, null)!, "x");
  t += 60_000;
  assert.equal(await g.tick(async () => { throw new Error("boom"); }), false);
  assert.equal(g.paused, true);
  assert.match(g.status().last_probe!.why, /probe failed: boom/);
  assert.equal(g.holdDue(), true, "holdDesksS 0 holds at once");
});
