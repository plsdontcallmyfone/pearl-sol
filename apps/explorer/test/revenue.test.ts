import { test } from "node:test";
import assert from "node:assert/strict";
import { findPda, utf8 } from "../src/codec.ts";
import * as D from "../src/decode.ts";
import { screenRevenue, isSelfDealt, type RevJob } from "../src/revenue.ts";
import { INITIAL_SUPPLY_SPL, ata, circulating, decodeReserveConfig, plainTokens, supplyModel } from "../src/supply.ts";

const job = (o: Partial<RevJob> & { job_id: string; poster: string }): RevJob => ({
  deliverer: "D1", status: "completed", worker_pay: "8000", protocol_fee: "2000", miners: ["M1"], order: null, ...o,
});

test("self-dealing: payer = deliverer or a miner of its job is excluded; concentration and seller screen shown", () => {
  assert.deepEqual(isSelfDealt(job({ job_id: "a", poster: "P1" })), { self: false, reason: null });
  assert.equal(isSelfDealt(job({ job_id: "b", poster: "D1" })).reason, "payer delivered the result");
  assert.equal(isSelfDealt(job({ job_id: "c", poster: "M1", deliverer: "D2" })).reason, "payer mined a block on its own job");
  const jobs = [
    job({ job_id: "a", poster: "P1", protocol_fee: "3000", order: { nonce: "1", status: "finalized", sol_in: "110", sol_spent: "100" } }),
    job({ job_id: "b", poster: "P2", protocol_fee: "1000", status: "expired" }),
    job({ job_id: "c", poster: "P1", protocol_fee: "1000", order: { nonce: "2", status: "refunded", sol_in: "55", sol_spent: "50" } }),
    // self-dealt: the payer mined its own job; its storefront SOL is excluded too
    job({ job_id: "d", poster: "M1", protocol_fee: "5000", order: { nonce: "3", status: "finalized", sol_in: "220", sol_spent: "200" } }),
    // finalized order whose buy tx was not recorded: counted at sol_spent
    job({ job_id: "e", poster: "P3", deliverer: "D2", miners: ["M2"], protocol_fee: "1000", order: { nonce: "4", status: "finalized", sol_in: null, sol_spent: "40" } }),
  ];
  const r = screenRevenue(jobs, { accrued: "10", refunded: "5" });
  assert.equal(r.gross.protocol_fees, "11000");
  assert.equal(r.gross.unique_payers, 4);
  assert.equal(r.gross.storefront_sol_lamports, "370", "finalized only: 110 + 220 + 40");
  assert.equal(r.self_dealt_excluded.jobs, 1);
  assert.equal(r.self_dealt_excluded.protocol_fees, "5000");
  assert.equal(r.self_dealt_excluded.storefront_sol_lamports, "220");
  assert.equal(r.self_dealt_excluded.jobs_list[0].payer, "M1");
  assert.equal(r.screened.protocol_fees, "6000");
  assert.equal(r.screened.storefront_sol_lamports, "150");
  assert.equal(r.screened.storefront_orders, 2);
  assert.equal(r.screened.worker_pay_delivered, "24000", "expired job's W excluded");
  assert.equal(r.screened.unique_payers, 3);
  assert.equal(r.screened.top_payer!.payer, "P1");
  assert.equal(r.screened.top_payer!.share_ppm, 666_666);
  assert.equal(r.screened.top_two_share_ppm, 833_333);
  assert.equal(r.screened.deliverers, 2);
  assert.equal(r.screened.from_deliverers_under_10_payers.protocol_fees, "6000", "both deliverers have fewer than 10 payers");
  assert.equal(r.buyback!.released_shares_lamports, "10");
  assert.match(r.notes[1], /1 finalized order/);
  // the sum of the parts is the whole
  assert.equal(BigInt(r.screened.protocol_fees) + BigInt(r.self_dealt_excluded.protocol_fees), BigInt(r.gross.protocol_fees));
});

test("reserve ATA matches the one the programs use on the test VPS (spl-token getAssociatedTokenAddressSync)", () => {
  const creator = findPda([utf8("creator")], "4FkQcoCMJdRpZW7bK3wnaMoLjYdWaJBDB8xTUGskLY6p")[0];
  assert.equal(creator, "Bspy5czBMnzWJfYJoLkzxni1Ah7Do2EbErSFvp57i1of");
  assert.equal(ata(creator, "BRtyAHcjS55k99sfPrLFNgAyhqfgwmrrjdhPgdiaE9s8"), "CNsyLAnDWAo9F9rNiboGYDrzgV9AJkw3BdiVH5T5xSPF");
});

test("circulating supply = mint supply - reserve - pool - transit; burned = 1B - supply", () => {
  const c = circulating({
    mintSupply: INITIAL_SUPPLY_SPL - 5_000_000n,
    reserve: 100_000_000_000n,
    poolLamports: 30_000_000_000_000n, // 30e9 SPL units on the chain side
    transit: 7n,
    bridgeVault: 40_000_000_000n,
    outsideEscrowLamports: 39_000_000_000_000n,
  });
  assert.equal(c.burned, 5_000_000n);
  assert.equal(c.pool, 30_000_000_000n);
  assert.equal(c.circulating, INITIAL_SUPPLY_SPL - 5_000_000n - 100_000_000_000n - 30_000_000_000n - 7n);
  assert.equal(c.inFlight, 1_000_000_000n);
  assert.equal(plainTokens("1000000000000000"), "1000000000");
  assert.equal(plainTokens("1234500000"), "1234.5");
});

function reserveCfg(layout1: boolean) {
  const d = new Uint8Array(layout1 ? 334 : 276);
  const dv = Buffer.from(d.buffer);
  d.set(new Uint8Array(32).fill(9), 168); // mining_pool
  dv.writeBigUInt64LE(10_000_000_000_000n, 234);
  dv.writeBigUInt64LE(26n, 266);
  if (layout1) {
    d[276] = 1;
    d[277] = 1;
    dv.writeBigInt64LE(3600n, 278);
    dv.writeBigInt64LE(86400n, 286);
    dv.writeBigUInt64LE(20_000_000_000_000n, 294);
    dv.writeBigUInt64LE(3n, 326);
  }
  return d;
}

test("reserve config: fund_mining limits decode at the program's offsets; an old layout says so", () => {
  const r = decodeReserveConfig(reserveCfg(true))!;
  assert.equal(r.permissionless, true);
  assert.equal(r.paused, true);
  assert.equal(r.min_interval_secs, 3600);
  assert.equal(r.window_secs, 86400);
  assert.equal(r.max_per_window, "20000000000000");
  assert.equal(r.max_fund_per_call, "10000000000000");
  assert.equal(r.total_funded, "26");
  assert.equal(r.fund_calls, 3);
  const old = decodeReserveConfig(reserveCfg(false))!;
  assert.equal(old.permissionless, false);
});

const ev = (name: string, body: Buffer) => `Program data: ${Buffer.concat([Buffer.from(D.eventDisc(name)), body]).toString("base64")}`;
const u64 = (...v: bigint[]) => Buffer.concat(v.map((x) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(x); return b; }));

test("events: the reserve cap split and an order's SOL in, from the program's Borsh layouts", () => {
  const logs = [
    ev("BuybackToReserve", u64(4n, 1000n, 5n, 1005n, 1005n)),
    ev("BuybackBurned", Buffer.concat([u64(4n, 77n), Buffer.from([0]), u64(1005n, 1005n, 999n, 77n)])),
    ev("OrderPlaced", Buffer.concat([Buffer.alloc(32, 1), u64(12n), Buffer.alloc(96, 2), Buffer.from([0]), u64(900n, 1n, 2n, 3n)])),
    ev("BuybackShareHeld", Buffer.concat([Buffer.alloc(32, 1), u64(12n, 100n, 100n, 991n)])),
  ];
  const parts = D.buybackParts(logs);
  assert.deepEqual(parts.map((p) => [p.kind, p.buy, p.tokens, p.reason]), [["reserve", 4, "1000", null], ["burn", 4, "77", "above cap"]]);
  assert.equal(parts[1].supplyAfter, "999");
  assert.equal(parts[0].reserveCap, "1005");
  assert.deepEqual(D.orderPaid(logs), [{ nonce: "12", solSpent: "900", share: "100" }]);
});

test("storefront config: reserve cap read after bb_refunded; zero slack reads as no cap", () => {
  const n = 1;
  const at = 146 + 32 * n;
  const d = new Uint8Array(at + 254 + 40);
  d.set(D.accountDisc("Config"), 0);
  const dv = Buffer.from(d.buffer);
  dv.writeUInt32LE(n, 142);
  d.set(new Uint8Array(32).fill(7), 40); // mint
  const b = at + 59;
  dv.writeUInt16LE(1000, b);
  dv.writeBigUInt64LE(123n, b + 187);
  const s = D.decodeStoreBuyback(d)!;
  assert.equal(s.reserveCap, "123");
  assert.equal(s.bps, 1000);
  dv.writeBigUInt64LE(0n, b + 187);
  assert.equal(D.decodeStoreBuyback(d)!.reserveCap, "0");
});

test("supply endpoints: JSON breakdown and bare numbers from live reads (fake RPCs)", async () => {
  const mint = "So11111111111111111111111111111111111111112"; // any valid key works as the mint here
  const reserveProgram = "4FkQcoCMJdRpZW7bK3wnaMoLjYdWaJBDB8xTUGskLY6p";
  const creator = findPda([utf8("creator")], reserveProgram)[0];
  const mintAcc = new Uint8Array(82);
  Buffer.from(mintAcc.buffer).writeBigUInt64LE(INITIAL_SUPPLY_SPL - 1_000_000n, 36);
  const tokenAcc = (amt: bigint) => { const d = new Uint8Array(165); Buffer.from(d.buffer).writeBigUInt64LE(amt, 64); return d; };
  const accounts: Record<string, Uint8Array> = {
    [mint]: mintAcc,
    [ata(creator, mint)]: tokenAcc(50_000_000_000n),
    [ata(findPda([utf8("config")], D.BRIDGE_ID)[0], mint)]: tokenAcc(20_000_000_000n),
    [findPda([utf8("config")], reserveProgram)[0]]: reserveCfg(true),
  };
  const ix: any = {
    buyback: { mint, reserveProgram, burned: "1000000" },
    sol: { getMultipleAccounts: async (ks: string[]) => ks.map((k) => (accounts[k] ? { address: k, data: accounts[k], owner: k === findPda([utf8("config")], reserveProgram)[0] ? reserveProgram : "x", lamports: 1 } : null)) },
    node: { call: async (m: string) => (m === "getEscrow" ? { pool: "Pool111", pool_balance: "10000000000000", outside_escrow: "20000000000000", escrow_balance: "1", height: 9 } : null) },
    orders: new Map(), jobs: new Map(), jobBlocks: new Map(), blocks: new Map(),
    status: { nodeOk: true }, chainInfo: null, params: null, lc: { config: null, acc: null }, buybackTxs: [], withdrawals: new Map(),
  };
  const sp = await supplyModel(ix);
  assert.equal(sp.available, true);
  assert.equal(sp.total_supply, (INITIAL_SUPPLY_SPL - 1_000_000n).toString());
  assert.equal(sp.burned, "1000000");
  assert.equal(sp.not_circulating.mining_reserve.amount, "50000000000");
  assert.equal(sp.not_circulating.mining_pool.amount, "10000000000");
  assert.equal(sp.circulating_supply, (INITIAL_SUPPLY_SPL - 1_000_000n - 50_000_000_000n - 10_000_000_000n).toString());
  assert.equal(sp.bridge.in_flight, "0");
  assert.equal(sp.reserve.permissionless, true);
});
