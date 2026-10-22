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
// With CADRE_API set (e.g. https://api.64-95-12-116.sslip.io, the public devnet) the all-time window
// is also checked against what cadre-api reports from its own index of the same chain:
//   holders revenue (storefront part) == /v1/buyback accrued_lamports
//   holders revenue (mandate part)    == sum of fees_protocol over every Mandate account (the program's own
//                                        counters, decoded with cadre-api's decodeMandate)
// so the adapter and the trader board (/v1/traderboard) agree before the adapter is pointed at mainnet.
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
  rpc: rpc.replace(/([?&](api[-_]?key|token)=)[^&]+/i, "$1REDACTED"), program, window: { from, to },
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
const api = process.env.CADRE_API?.replace(/\/$/, "");
if (api && from === 0) {
  const { flowsInWindow } = await import("../fees/cadre/index.ts");
  const f = await flowsInWindow(from, to);
  const j = async (p: string) => { const r = await globalThis.fetch(api + p); if (!r.ok) throw new Error(`${p}: HTTP ${r.status}`); return r.json() as Promise<any>; };
  const bb = await j("/v1/buyback");
  let mandateProto: bigint | null = null;
  const mp = process.env.CADRE_MANDATE_PROGRAM;
  if (mp) {
    const { decodeMandate } = await import("../../../services/cadre-api/src/routes/trading.ts");
    const accts = await new Connection(rpc, "confirmed").getProgramAccounts(new PublicKey(mp));
    mandateProto = 0n;
    for (const a of accts) {
      const m = decodeMandate(a.pubkey.toBase58(), a.account.data);
      if (m) mandateProto += BigInt(m.fees_protocol);
    }
  }
  const apiChecks = {
    storefront_holders_revenue_eq_api_buyback_accrued: f.shareReleased === BigInt(bb.accrued_lamports ?? -1),
    mandate_protocol_fees_eq_mandate_accounts: mandateProto === null ? "skipped: CADRE_MANDATE_PROGRAM not set" : (f.mandateProtocolFee ?? 0n) === mandateProto,
  };
  out.api = { url: api, buyback_accrued_lamports: bb.accrued_lamports, mandate_protocol_lamports: mandateProto?.toString() ?? null, adapter_storefront_holders: f.shareReleased.toString(), adapter_mandate_protocol: (f.mandateProtocolFee ?? 0n).toString(), settles: f.settles ?? 0, checks: apiChecks };
  if (Object.values(apiChecks).some((v) => v === false)) ok = false;
}
console.log(JSON.stringify(out, null, 2));
if (!ok && process.argv.includes("--strict")) process.exit(1);
