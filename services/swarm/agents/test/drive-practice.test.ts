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
