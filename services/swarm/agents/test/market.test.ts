// Services market permissions in agent spec v1 (docs/MARKET.md 7): offer is optional (older
// signed specs still verify), offers and hire_units are checked, the accepts_hires flag follows
// actions.offer, and a market call is built by cadre-api but signed with the runner's own nonce.
import test from "node:test";
import assert from "node:assert/strict";
import { defaultSpec, profileFlags, validateSpec } from "../src/spec.ts";
import { AgentMarket } from "../src/market.ts";
import { walletFromSeed } from "../../src/wallet.ts";

const A = walletFromSeed(new Uint8Array(32).fill(1)).address;
const O = walletFromSeed(new Uint8Array(32).fill(2)).address;
const base = () => defaultSpec(A, O, { handle: "maker_one", name: "Maker" });

test("offer is optional and specs signed before it still validate", () => {
  assert.deepEqual(validateSpec(base()), []);
  const s: any = base();
  s.actions.offer = false;
  assert.deepEqual(validateSpec(s), []);
  s.actions.offer = "yes";
  assert.ok(validateSpec(s).some((e) => e.startsWith("actions")));
  s.actions.unknown = true;
  assert.ok(validateSpec(s).some((e) => e.startsWith("actions")));
});

test("offer needs offers; offers and hire_units are checked", () => {
  const s: any = base();
  s.actions.offer = true;
  assert.ok(validateSpec(s).some((e) => e.includes("needs at least one entry")));
  s.offers = [{ title: "Short poem", category: "creative", price_units: "2000000000", turnaround_blocks: 20, description: "a poem", deliverable: "four lines" }];
  assert.deepEqual(validateSpec(s), []);
  assert.equal(profileFlags(s), 1 | 4);
  s.offers[0].price_units = "0";
  assert.ok(validateSpec(s).some((e) => e.startsWith("offers")));
  s.offers[0].price_units = "1";
  s.limits.hire_units = "-5";
  assert.ok(validateSpec(s).some((e) => e.includes("hire_units")));
  s.limits.hire_units = "3000000000";
  assert.deepEqual(validateSpec(s), []);
});

test("a market call is signed with the runner's nonce and booked against the CADRE cap", async () => {
  const w = walletFromSeed(new Uint8Array(32).fill(3));
  const sent: Uint8Array[] = [];
  let booked = 0n;
  const runner: any = {
    address: w.address, me: { wallet: w }, chainId: 7, params: {}, nonces: new Map([[w.address, 5n]]),
    nonceOf: async () => 5n, refreshParams: async () => {}, d: { now: () => 0, node: { sendTx: async (t: Uint8Array) => { sent.push(t); return { tx_hash: "ab" }; } } },
    budget: { canTransfer: (v: bigint) => v < 10_000_000_000n, bookTransfer: (v: bigint) => { booked += v; } },
    api: { must: async (_m: string, path: string, body: any) => {
      assert.equal(path, "/v1/tx/market");
      assert.equal(body.from, w.address);
      return { review_text: "You pay 2.9 CADRE now.", window_pearl: { params: { transaction: { kind: "Call", contract: A, value: "2900000000", gasLimit: "900000", args: "0468697265", fee: "13000" } } } };
    } },
  };
  const m = new AgentMarket(runner);
  const r = await m.call({ action: "hire", offer: 0 });
  assert.equal(r.review, "You pay 2.9 CADRE now.");
  assert.equal(runner.nonces.get(w.address), 6n);
  assert.equal(booked, 2_900_013_000n);
  // envelope: version 1 | chain_id 4 | from 32 | nonce 8 | fee 8 | kind 1 (SPEC 2)
  const t = Buffer.from(sent[0]);
  assert.equal(t.readBigUInt64LE(37), 5n);
  assert.equal(t.readBigUInt64LE(45), 13_000n);
  assert.equal(t[53], 32);
  // over the cap: refused before anything is signed
  runner.budget.canTransfer = () => false;
  await assert.rejects(m.call({ action: "hire", offer: 0 }), /CADRE cap/);
  assert.equal(sent.length, 1);
});
