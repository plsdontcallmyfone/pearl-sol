// Live check of fees/cadre against a running stack (local demo or the test VPS; never a
// public chain needed). Runs the adapter's fetch over a window and, for the all-time window,
// cross-checks it with the storefront program's own Config counters:
//   OrderPlaced.sol_spent summed      == Config.total_sol
//   orders seen                        == Config.orders
//   BuybackShareHeld summed            == bb_locked + bb_accrued + bb_refunded (every share ever held)
//   BuybackAccrued summed (holders rev) == bb_accrued
// A mismatch means the RPC no longer has part of the history (a pruned ledger), and is reported.
//
//   SOLANA_RPC=http://127.0.0.1:10600 npm run live [-- --from UNIX --to UNIX] [--strict]
import { Connection, PublicKey } from "@solana/web3.js";
import adapter from "../fees/cadre/index.ts";
import { createBalances } from "../adapters/types.ts";

const arg = (k: string) => { const i = process.argv.indexOf(k); return i > 0 ? process.argv[i + 1] : undefined; };
const now = Math.floor(Date.now() / 1000) + 60;
const from = Number(arg("--from") ?? 0), to = Number(arg("--to") ?? now);
const rpc = process.env.SOLANA_RPC ?? "http://127.0.0.1:10600";
const program = process.env.CADRE_STOREFRONT_PROGRAM ?? "DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr";
const SOL = "So11111111111111111111111111111111111111112";

const r = await adapter.adapter.solana.fetch({ startTimestamp: from, endTimestamp: to, chain: "solana", createBalances });
const get = (k: string) => BigInt(r[k].getBalances()[SOL] ?? "0");
const out: Record<string, unknown> = {
  rpc, program, window: { from, to },
  dailyFees: get("dailyFees").toString(), dailyUserFees: get("dailyUserFees").toString(), dailyRevenue: get("dailyRevenue").toString(),
  dailyHoldersRevenue: get("dailyHoldersRevenue").toString(), dailySupplySideRevenue: get("dailySupplySideRevenue").toString(),
};
let ok = true;
if (from === 0) {
  const conn = new Connection(rpc, "confirmed");
  const cfg = PublicKey.findProgramAddressSync([Buffer.from("config")], new PublicKey(program))[0];
  const a = await conn.getAccountInfo(cfg);
  if (!a) throw new Error(`storefront config ${cfg.toBase58()} not found on ${rpc}`);
  const d = a.data;
  const at = 146 + 32 * d.readUInt32LE(142), b = at + 59;
  const u = (o: number) => d.readBigUInt64LE(o);
  const cfgTotals = { orders: u(at), total_sol: u(at + 8), bb_accrued: u(b + 99), shares_held_ever: u(b + 171) + u(b + 99) + u(b + 179) };
  const checks = {
    supply_side_eq_total_sol: get("dailySupplySideRevenue") === cfgTotals.total_sol,
    fees_eq_total_sol_plus_shares: get("dailyFees") === cfgTotals.total_sol + cfgTotals.shares_held_ever,
    holders_revenue_eq_bb_accrued: get("dailyHoldersRevenue") === cfgTotals.bb_accrued,
  };
  ok = Object.values(checks).every(Boolean);
  out.config = Object.fromEntries(Object.entries(cfgTotals).map(([k, v]) => [k, v.toString()]));
  out.checks = checks;
}
console.log(JSON.stringify(out, null, 2));
if (!ok && process.argv.includes("--strict")) process.exit(1);
