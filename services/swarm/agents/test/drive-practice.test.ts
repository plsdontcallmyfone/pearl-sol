// QA pass 2026-10-22: a practising agent's token cannot make it swap for real through cadre_trade
// (MCP / hosted REST). Before, the tool checked only actions.trade, so an external AI could buy on
// PumpSwap from a practice agent's wallet while the runtime's own engine stayed in practice.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { walletFromSeed } from "../../src/wallet.ts";
import { byName } from "../../../../packages/cadre-mcp/src/catalog.ts";
import { Drive } from "../src/drive.ts";
import { defaultSpec, defaultSpecV2, defaultTrade } from "../src/spec.ts";

const seedOf = (s: string) => createHash("sha256").update(s).digest();
const op = walletFromSeed(seedOf("dp-operator")).address;
const a = walletFromSeed(seedOf("dp-agent")).address;
const MINT = "BwJqbsJUXshRj9ABxXs4RZ838kZrSqw8TT4CKMNfEu5e";
const trade = byName.get("cadre_trade")!;
const allowed = (spec: any) => new Drive({ spec } as any).allowed(trade);

test("cadre_trade: refused for a practising agent, open only where real swaps are on", () => {
  const base = defaultSpecV2(a, op, { handle: "abc", name: "A", skills: ["town", "x402", "trade"], actions: { ...defaultSpec(a, op).actions, trade: true }, trade: defaultTrade(MINT) });
  const p = allowed(base);
  assert.equal(p.ok, false);
  assert.match(p.why!, /practice-trades/);
  const practising = defaultSpecV2(a, op, { handle: "abc", name: "A", skills: ["town", "x402", "practice"], actions: { ...defaultSpec(a, op).actions, trade: true }, practice: { style: "momentum", balance_lamports: "10000000000", strategy: "" } });
  assert.equal(allowed(practising).ok, false);
  assert.equal(allowed({ ...base, trade: { ...base.trade!, real_swaps: true } }).ok, true, "real_swaps on: the direct order is the spec's to allow");
  assert.equal(allowed(defaultSpec(a, op, { handle: "abc", name: "A" })).ok, false, "no trade action: refused as before");
});

// security review 2026-10-23: the gate was "not practising", so a spec with actions.trade but no
// trade section (legacy v1, or v2 without one) could real-buy ANY mint (no mint list) through the tool
test("cadre_trade: actions.trade without a real-swap trade section is refused (legacy and v2)", async () => {
  const acts = { ...defaultSpec(a, op).actions, trade: true };
  const legacy = defaultSpec(a, op, { handle: "abc", name: "A", actions: acts });
  const v2 = defaultSpecV2(a, op, { handle: "abc", name: "A", actions: acts });
  for (const spec of [legacy, v2]) {
    const r = allowed(spec);
    assert.equal(r.ok, false);
    assert.match(r.why!, /real swaps are off/);
  }
  // the tool body refuses too (no chain call is reached)
  const d = new Drive({ spec: v2, haltReason: () => null, state: "running" } as any);
  await assert.rejects(() => (d as any).trade({ side: "buy", sol_in: "1000", mint: MINT }), /real swaps are off/);
  // with real_swaps on, a mint outside the spec's list is refused before any signing
  const real = defaultSpecV2(a, op, { handle: "abc", name: "A", skills: ["town", "x402", "trade"], actions: acts, trade: { ...defaultTrade(MINT), real_swaps: true } });
  await assert.rejects(() => (new Drive({ spec: real } as any) as any).trade({ side: "buy", sol_in: "1000", mint: "So11111111111111111111111111111111111111112" }), /trades only/);
});

// security review 2026-10-23: an external driver could write the agent's practice record, tool
// receipt roots or swarm membership through cadre_call_contract (gas only, but forged records)
test("cadre_call_contract: the runtime's record contracts are refused to an external driver", async () => {
  const { recordContracts } = await import("../src/drive.ts");
  const recs = recordContracts();
  const v2 = defaultSpecV2(a, op, { handle: "abc", name: "A" });
  for (const [addr, name] of recs) {
    const d = new Drive({ spec: v2, api: { must: async (_m: string, p: string) => (p === "/v1/market" ? { contract: null } : { ok: true, gas_used: 1 }) } } as any);
    await assert.rejects(() => (d as any).run("cadre_call_contract", { contract: addr, method: "commit", fields: [] }, {}), new RegExp(`${name} contract is written only by the runtime`));
  }
  assert.ok(recs.size >= 1, "the devnet deploy files name the record contracts");
});
