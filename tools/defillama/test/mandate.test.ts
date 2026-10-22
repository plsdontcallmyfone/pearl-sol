// The mandate (delegated trading) part of fees/cadre: Settled events decoded from their Borsh layout
// and added to fees, supply-side revenue and holders revenue when CADRE_MANDATE_PROGRAM is set.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";

process.env.CADRE_MANDATE_PROGRAM = "BG5WkxZyYwWXTY2YSAJGhwL7JcxChRXx8UqHEAMAC5yc";
process.env.CADRE_STOREFRONT_PROGRAM = "9XjvGk2xJdByPL6NmUCQg7RdBLyeDjWjikZBnwuWvAKu";
const { default: adapter, mandateFlowsFromLogs } = await import("../fees/cadre/index.ts");
const { createBalances } = await import("../adapters/types.ts");
const { setConnection } = await import("../helpers/solana.ts");

const disc = (n: string) => createHash("sha256").update(`event:${n}`).digest().subarray(0, 8);
const n64 = (v: bigint, signed = false) => { const b = Buffer.alloc(8); signed ? b.writeBigInt64LE(v) : b.writeBigUInt64LE(v); return b; };
const settled = (agent: bigint, proto: bigint) => `Program data: ${Buffer.concat([disc("Settled"), Buffer.alloc(32, 3), n64(-5n, true), n64(10n, true), n64(agent), n64(proto), n64(999n)]).toString("base64")}`;
const placed = (solSpent: bigint) => `Program data: ${Buffer.concat([disc("OrderPlaced"), Buffer.alloc(32, 1), n64(1n), Buffer.alloc(96, 2), Buffer.from([0]), n64(solSpent), n64(0n), n64(0n), n64(0n)]).toString("base64")}`;

test("Settled: fee_agent and fee_protocol at their Borsh offsets", () => {
  const f = mandateFlowsFromLogs([settled(900n, 100n), "Program log: settle", settled(90n, 10n)], { orderSol: 0n, shareHeld: 0n, shareReleased: 0n, orders: 0 });
  assert.equal(f.mandateAgentFee, 990n);
  assert.equal(f.mandateProtocolFee, 110n);
  assert.equal(f.settles, 2);
});

test("fetch with the mandate program: trading fees in fees, the protocol share in holders revenue", async () => {
  const sigs: Record<string, { signature: string; blockTime: number; err: null; logs: string[] }[]> = {
    "9XjvGk2xJdByPL6NmUCQg7RdBLyeDjWjikZBnwuWvAKu": [{ signature: "s1", blockTime: 1500, err: null, logs: [placed(1000n)] }],
    "BG5WkxZyYwWXTY2YSAJGhwL7JcxChRXx8UqHEAMAC5yc": [{ signature: "m1", blockTime: 1600, err: null, logs: [settled(450n, 50n)] }],
  };
  const all = new Map(Object.values(sigs).flat().map((t) => [t.signature, t]));
  setConnection({
    async getSignaturesForAddress(p: any, o: { before?: string }) { return o.before ? [] : sigs[p.toBase58()].map((t) => ({ signature: t.signature, blockTime: t.blockTime, err: t.err })); },
    async getTransaction(sig: string) { return { meta: { logMessages: all.get(sig)!.logs } }; },
  });
  const r = await adapter.adapter.solana.fetch({ startTimestamp: 1000, endTimestamp: 2000, chain: "solana", createBalances });
  const SOL = "So11111111111111111111111111111111111111112";
  assert.equal(r.dailyFees.getBalances()[SOL], "1500");
  assert.equal(r.dailySupplySideRevenue.getBalances()[SOL], "1450");
  assert.equal(r.dailyHoldersRevenue.getBalances()[SOL], "50");
  assert.equal(r.dailyRevenue.getBalances()[SOL], "50");
  assert.ok(adapter.breakdownMethodology?.HoldersRevenue);
});
