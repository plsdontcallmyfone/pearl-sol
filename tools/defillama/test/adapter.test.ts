// Unit test of fees/cadre against a fake Solana connection carrying the storefront program's
// real event layouts (built here from the Borsh field order in token/programs/storefront).
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import adapter, { flowsFromLogs } from "../fees/cadre/index.ts";
import { createBalances } from "../adapters/types.ts";
import { setConnection } from "../helpers/solana.ts";

const disc = (n: string) => createHash("sha256").update(`event:${n}`).digest().subarray(0, 8);
const u64 = (...v: bigint[]) => Buffer.concat(v.map((x) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(x); return b; }));
const log = (name: string, body: Buffer) => `Program data: ${Buffer.concat([disc(name), body]).toString("base64")}`;
const placed = (nonce: bigint, solSpent: bigint) => log("OrderPlaced", Buffer.concat([Buffer.alloc(32, 1), u64(nonce), Buffer.alloc(96, 2), Buffer.from([0]), u64(solSpent, 5n, 6n, 7n)]));
const held = (nonce: bigint, lamports: bigint) => log("BuybackShareHeld", Buffer.concat([Buffer.alloc(32, 1), u64(nonce, lamports, lamports, 1n)]));
const accrued = (nonce: bigint, lamports: bigint) => log("BuybackAccrued", Buffer.concat([Buffer.alloc(32, 1), u64(nonce, lamports, 0n, lamports)]));

test("event decoding: OrderPlaced sol_spent, BuybackShareHeld and BuybackAccrued lamports", () => {
  const f = flowsFromLogs([placed(1n, 900n), held(1n, 100n), "Program log: x", accrued(0n, 40n)], { orderSol: 0n, shareHeld: 0n, shareReleased: 0n, orders: 0 });
  assert.deepEqual(f, { orderSol: 900n, shareHeld: 100n, shareReleased: 40n, orders: 1 });
});

test("fetch: only successful txs inside [start, end), paged by signature", async () => {
  // newest first, as getSignaturesForAddress returns them; 2 pages of 1000 + a tail
  const txs: { signature: string; blockTime: number; err: any; logs: string[] }[] = [];
  txs.push({ signature: "after", blockTime: 2000, err: null, logs: [placed(9n, 1_000_000n)] }); // at end: excluded
  for (let i = 0; i < 1500; i++) txs.push({ signature: `in${i}`, blockTime: 1999 - (i % 999), err: null, logs: i % 2 ? [accrued(BigInt(i), 10n)] : [placed(BigInt(i), 100n), held(BigInt(i), 11n)] });
  txs.push({ signature: "failed", blockTime: 1500, err: { InstructionError: [0, "x"] }, logs: [placed(7n, 5_000_000n)] });
  txs.sort((a, b) => b.blockTime - a.blockTime);
  txs.push({ signature: "before", blockTime: 999, err: null, logs: [placed(8n, 1_000_000n)] }); // before start: stops the walk
  const byId = new Map(txs.map((t) => [t.signature, t]));
  let pages = 0;
  setConnection({
    async getSignaturesForAddress(_p: any, o: { before?: string; limit: number }) {
      pages++;
      const i = o.before ? txs.findIndex((t) => t.signature === o.before) + 1 : 0;
      return txs.slice(i, i + o.limit).map((t) => ({ signature: t.signature, blockTime: t.blockTime, err: t.err }));
    },
    async getTransaction(sig: string) {
      return { meta: { logMessages: byId.get(sig)!.logs } };
    },
  });
  const r = await adapter.adapter.solana.fetch({ startTimestamp: 1000, endTimestamp: 2000, chain: "solana", createBalances });
  const SOL = "So11111111111111111111111111111111111111112";
  // 750 orders x (100 + 11) lamports, 750 releases x 10
  assert.equal(r.dailyFees.getBalances()[SOL], String(750 * 111));
  assert.equal(r.dailyUserFees.getBalances()[SOL], String(750 * 111));
  assert.equal(r.dailySupplySideRevenue.getBalances()[SOL], String(750 * 100));
  assert.equal(r.dailyHoldersRevenue.getBalances()[SOL], String(750 * 10));
  assert.equal(r.dailyRevenue.getBalances()[SOL], String(750 * 10));
  assert.equal(pages, 2, "paged past the first 1000, stopped at the first tx before start");
  assert.ok(adapter.methodology?.HoldersRevenue);
  assert.equal(adapter.version, 2);
});
