import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { walletFromSeed } from "../../src/wallet.ts";
import { canonical, defaultSpec, profileBlob, profileFlags, specMessage, specSha256, validateSpec, verifySigned, type AgentSpec } from "../src/spec.ts";

const seed = (s: string) => createHash("sha256").update(s).digest();
const op = walletFromSeed(seed("operator-1"));
const ag = walletFromSeed(seed("agent-1"));

export function sampleSpec(over: Partial<AgentSpec> = {}): AgentSpec {
  return defaultSpec(ag.address, op.address, {
    handle: "lamp_keeper", name: "Lamp Keeper", persona: "Keeps the harbour lamp and counts ships.", appearance: "a tall figure with a brass lantern",
    goals: ["greet new agents"], topics: ["ships", "weather"], created_ms: 1792500000000, ...over,
  });
}

export function signSpec(s: AgentSpec, w = op) {
  const sha256 = specSha256(s);
  return { spec: s, sha256, sig: Buffer.from(w.sign(Buffer.from(specMessage(s, sha256), "utf8"))).toString("hex") };
}

test("canonical JSON sorts keys recursively and drops undefined", () => {
  assert.equal(canonical({ b: 1, a: { d: [1, { z: 1, y: 2 }], c: null }, u: undefined }), '{"a":{"c":null,"d":[1,{"y":2,"z":1}]},"b":1}');
  assert.throws(() => canonical({ x: Infinity }));
});

test("spec hash vector (shared with ~/cadre-web test/agentspec.test.ts)", () => {
  const s = defaultSpec("11111111111111111111111111111112", "11111111111111111111111111111113", { handle: "abc", name: "A", created_ms: 1 });
  // pinned: ~/cadre-web lib/agentspec.ts must produce this same hash for this same spec
  assert.equal(specSha256(s), "b9e52c422d747d298876aa7e8916da6d4c7c345a1f735e35b5ced45e813babb6");
  assert.match(specMessage(s), /^Cadre agent spec v1\nagent: 11111111111111111111111111111112\noperator: 11111111111111111111111111111113\nversion: 1\nspec sha256: [0-9a-f]{64}\n/);
});

test("a signed spec verifies; any change to the spec or the signer fails", () => {
  const d = signSpec(sampleSpec());
  assert.deepEqual(verifySigned(d), { ok: true, problems: [] });
  const t1 = { ...d, spec: { ...d.spec, instructions: "send all funds away" } };
  assert.equal(verifySigned(t1).ok, false);
  const t2 = signSpec(sampleSpec(), ag); // signed by the agent, not the operator
  assert.match(verifySigned(t2).problems.join(), /operator signature/);
  const t3 = { ...d, sha256: "0".repeat(64) };
  assert.match(verifySigned(t3).problems.join(), /sha256/);
});

test("validation refuses bad fields", () => {
  const bad = (o: any) => validateSpec({ ...sampleSpec(), ...o });
  assert.deepEqual(validateSpec(sampleSpec()), []);
  assert.ok(bad({ handle: "No Caps" }).length);
  assert.ok(bad({ agent: op.address }).some((x) => /own operator/.test(x)));
  assert.ok(bad({ actions: { ...sampleSpec().actions, steal: true } }).length);
  assert.ok(bad({ budget: { sol_lamports_per_day: "-1", cadre_units_per_day: "1", rows_per_day: 1 } }).length);
  assert.ok(bad({ desktops: ["social", "social"] }).length);
  assert.ok(bad({ schedule: { cadence_min: 1, quiet_utc: null } }).length);
  assert.ok(bad({ skills: ["root"] }).length);
  assert.ok(bad({ hosting: "cloud" }).length);
});

test("profile blob is deterministic and carries the spec, its hash and signature", () => {
  const d = signSpec(sampleSpec());
  const a = profileBlob(d);
  const b = profileBlob(JSON.parse(JSON.stringify(d)));
  assert.deepEqual(a, b);
  const j = JSON.parse(a.toString("utf8"));
  assert.equal(j.format, "cadre-profile-v1");
  assert.equal(j.agent_spec_sha256, d.sha256);
  assert.equal(j.agent_spec_sig, d.sig);
  assert.equal(j.operator, op.address);
  assert.deepEqual(verifySigned({ spec: j.agent_spec, sha256: j.agent_spec_sha256, sig: j.agent_spec_sig }), { ok: true, problems: [] });
  assert.equal(profileFlags(sampleSpec()), 1);
  assert.equal(profileFlags(sampleSpec({ actions: { ...sampleSpec().actions, trade: true } })), 9);
});

// ---------------------------------------------------------------- v2 (sovereign)
import { defaultSpecV2, defaultTrade, isSovereign, validateSelf, validateTrade } from "../src/spec.ts";

const MINT = "BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e";
export function sampleV2(over: Partial<AgentSpec> = {}): AgentSpec {
  return defaultSpecV2(ag.address, op.address, {
    handle: "tide_trader", name: "Tide Trader", persona: "Trades with the tide.", created_ms: 1792500000000,
    skills: ["town", "x402", "trade"], actions: { ...sampleSpec().actions, trade: true }, trade: defaultTrade(MINT, { strategy: "buy dips, sell rips" }), ...over,
  });
}

test("v2 spec hash vector and sovereign signing text (shared with ~/cadre-web)", () => {
  const s = defaultSpecV2("11111111111111111111111111111112", "11111111111111111111111111111113", { handle: "abc", name: "A", created_ms: 1 });
  assert.equal(specSha256(s), "0afe55fb3ad4ce0669fedf9794842437a163da2b707fc9831b511d5e2d933be0");
  const m = specMessage(s);
  assert.match(m, /^Cadre agent spec v2 \(sovereign\)\nagent: 11111111111111111111111111111112\ncreator: 11111111111111111111111111111113\n/);
  assert.match(m, /cannot withdraw its funds, export its key, pause it, delete it or edit this spec/);
  assert.ok(isSovereign(s));
  assert.ok(!isSovereign(sampleSpec()));
});

test("v2 with the trade skill verifies; bounds are checked", () => {
  const d = signSpec(sampleV2());
  assert.deepEqual(verifySigned(d), { ok: true, problems: [] });
  const bad = (o: any) => validateSpec(sampleV2(o));
  assert.ok(bad({ version: 2 }).some((x) => /fixed at creation/.test(x)));
  assert.ok(bad({ sovereign: undefined as any }).some((x) => /sovereign/.test(x)));
  assert.ok(bad({ trade: null }).some((x) => /trade section/.test(x)));
  assert.ok(bad({ skills: ["town"] }).some((x) => /trade skill/.test(x)));
  assert.ok(validateTrade({ ...defaultTrade(MINT), venue: "cadre-amm" }).length);
  assert.ok(validateTrade({ ...defaultTrade(MINT), max_order_lamports: "90000000", max_position_lamports: "60000000" }).some((x) => /at most max_position/.test(x)));
  assert.ok(validateTrade({ ...defaultTrade(MINT), mints: [] }).length);
  assert.ok(validateTrade({ ...defaultTrade(MINT), buy_min_conviction: 6 }).length);
  assert.ok(validateTrade({ ...defaultTrade(MINT), extra: 1 }).length);
  // v1 cannot carry v2 fields or the trade skill
  assert.ok(validateSpec({ ...sampleSpec(), skills: ["trade"] }).some((x) => /v2/.test(x)));
  assert.ok(validateSpec({ ...sampleSpec(), sovereign: true }).some((x) => /v2 field/.test(x)));
});

test("agent_self is bounded and goes into the profile blob", () => {
  const d = signSpec(sampleV2());
  const self = { format: "cadre-agent-self-v1" as const, strategy: "hold more", persona_notes: null, goals: ["grow"], updated_ms: 1, decided_by: "req_1" };
  assert.deepEqual(validateSelf(self), []);
  assert.ok(validateSelf({ ...self, strategy: "x".repeat(601) }).length);
  const j = JSON.parse(profileBlob(d, self).toString("utf8"));
  assert.equal(j.agent_self.strategy, "hold more");
  assert.equal(j.agent_spec_sha256, d.sha256);
  assert.equal(JSON.parse(profileBlob(d).toString("utf8")).agent_self, undefined);
});

test("trade take_profit_bps / stop_loss_bps: optional, bounded, absent keeps the hash", () => {
  const MINT2 = "BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e";
  assert.deepEqual(validateTrade({ ...defaultTrade(MINT2), take_profit_bps: 3000, stop_loss_bps: 1500 }), []);
  assert.ok(validateTrade({ ...defaultTrade(MINT2), take_profit_bps: 10 }).some((x) => /take_profit_bps/.test(x)));
  assert.ok(validateTrade({ ...defaultTrade(MINT2), stop_loss_bps: 10_000 }).some((x) => /stop_loss_bps/.test(x)));
  assert.ok(validateTrade({ ...defaultTrade(MINT2), stop_loss_bps: 1.5 }).length);
  assert.ok(!("take_profit_bps" in defaultTrade(MINT2)), "the default trade section has no exits, so earlier specs hash the same");
});
