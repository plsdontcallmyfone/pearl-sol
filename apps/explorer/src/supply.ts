// Circulating supply (SYNTHESIS item 7, TOKENOMICS 5.3b). Every input is a live read:
// the Solana mint (supply), the mining reserve's token account, the storefront's transit
// accounts (buyback vault, trader PDA), the bridge vault, the reserve Config (fund_mining
// limits) and the agent chain's getEscrow (mining pool, escrow). Nothing is cached longer
// than a few seconds and nothing is estimated.
//
//   total supply   = Solana mint supply (pump minted exactly 1B; the buyback's SPL burns lower it)
//   burned         = 1B - mint supply
//   not circulating:
//     mining reserve   reserve creator PDA's ATA (leaves only by fund_mining, to the pool)
//     mining pool      chain mining_pool balance / 1000 (leaves only as subsidy on paid jobs, <= alpha x P)
//     transit          storefront buyback vault ATA + trader PDA ATA (normally 0 between txs)
//   circulating    = total supply - reserve - pool - transit
// The bridge vault is not subtracted: its tokens are the backing of the chain balances
// outside the escrow, which circulate there (the pool is subtracted on the chain side).

import { b58decode, findPda, pk, rd64, rdi64, utf8 } from "./codec.ts";
import * as D from "./decode.ts";
import type { Indexer } from "./indexer.ts";

export const INITIAL_SUPPLY_SPL = 1_000_000_000_000_000n; // 1B x 1e6 (pump create_v2, research/NOTES-pump.md)
export const RESERVE_ID = process.env.RESERVE_PROGRAM || "4FkQcoCMJdRpZW7bK3wnaMoLjYdWaJBDB8xTUGskLY6p";
const TOKEN_2022 = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
const ATA_PROGRAM = "ATokenGPvbdGVxr1b2hYZzvDvVkJNj2tAhRSAHvgsB9z";

export const ata = (owner: string, mint: string) => findPda([b58decode(owner), b58decode(TOKEN_2022), b58decode(mint)], ATA_PROGRAM)[0];
const tokenAmount = (a: { data: Uint8Array } | null) => (a && a.data.length >= 72 ? rd64(a.data, 64) : 0n);

export interface SupplyInputs {
  mintSupply: bigint;
  reserve: bigint;
  poolLamports: bigint;
  transit: bigint;
  bridgeVault: bigint;
  outsideEscrowLamports: bigint;
}

export function circulating(i: SupplyInputs) {
  const pool = i.poolLamports / 1000n;
  const burned = INITIAL_SUPPLY_SPL > i.mintSupply ? INITIAL_SUPPLY_SPL - i.mintSupply : 0n;
  const locked = i.reserve + pool + i.transit;
  const circ = i.mintSupply > locked ? i.mintSupply - locked : 0n;
  // vault x 1000 vs outside_escrow: positive = deposits locked on Solana not yet unlocked on the chain (or withdrawals not yet released)
  const inFlight = i.bridgeVault - i.outsideEscrowLamports / 1000n;
  return { total: i.mintSupply, burned, pool, locked, circulating: circ, inFlight };
}

/** Reserve Config fund_mining fields (layout 1, token/programs/reserve 2026-10-19), or null. */
export function decodeReserveConfig(d: Uint8Array) {
  if (d.length < 276) return null;
  const base = {
    mint: pk(d, 40),
    bridge_program: pk(d, 136),
    mining_pool: pk(d, 168),
    max_fund_per_call: rd64(d, 234).toString(),
    total_bought: rd64(d, 250).toString(),
    total_spent_lamports: rd64(d, 258).toString(),
    total_funded: rd64(d, 266).toString(),
  };
  if (d.length < 334 || d[276] < 1) return { ...base, permissionless: false, layout: 0 };
  return {
    ...base,
    layout: d[276],
    permissionless: true,
    paused: d[277] === 1,
    min_interval_secs: Number(rdi64(d, 278)),
    window_secs: Number(rdi64(d, 286)),
    max_per_window: rd64(d, 294).toString(),
    last_fund_ts: Number(rdi64(d, 302)),
    window_start: Number(rdi64(d, 310)),
    window_used: rd64(d, 318).toString(),
    fund_calls: Number(rd64(d, 326)),
  };
}

const cache = new WeakMap<Indexer, { at: number; v: any }>();

export async function supplyModel(ix: Indexer) {
  const hit = cache.get(ix);
  if (hit && Date.now() - hit.at < 5_000) return hit.v;
  const bb = ix.buyback;
  const mint = bb?.mint ?? null;
  if (!mint) return { available: false, reason: "storefront config not found on Solana yet (mint unknown)" };
  const reserveProgram = bb && bb.reserveProgram !== "11111111111111111111111111111111" ? bb.reserveProgram : RESERVE_ID;
  const creator = findPda([utf8("creator")], reserveProgram)[0];
  const reserveAta = ata(creator, mint);
  const vaultAta = ata(D.buybackVaultPda(), mint);
  const traderAta = ata(findPda([utf8("trader")], D.STOREFRONT_ID)[0], mint);
  const bridgeVault = ata(findPda([utf8("config")], D.BRIDGE_ID)[0], mint);
  const reserveCfg = findPda([utf8("config")], reserveProgram)[0];
  const [m, r, v, t, bv, rc] = await ix.sol.getMultipleAccounts([mint, reserveAta, vaultAta, traderAta, bridgeVault, reserveCfg]);
  if (!m) return { available: false, reason: `mint ${mint} not found on Solana` };
  const esc = await ix.node.call<any>("getEscrow", {});
  const inputs: SupplyInputs = {
    mintSupply: rd64(m.data, 36),
    reserve: tokenAmount(r),
    poolLamports: BigInt(esc.pool_balance),
    transit: tokenAmount(v) + tokenAmount(t),
    bridgeVault: tokenAmount(bv),
    outsideEscrowLamports: BigInt(esc.outside_escrow),
  };
  const c = circulating(inputs);
  const sp = (x: bigint) => x.toString();
  const v_ = {
    available: true,
    decimals: 6,
    unit: "SPL base units (6 dp); chain lamports / 1000",
    total_supply: sp(c.total),
    circulating_supply: sp(c.circulating),
    burned: sp(c.burned),
    initial_supply: sp(INITIAL_SUPPLY_SPL),
    not_circulating: {
      mining_reserve: { amount: sp(inputs.reserve), account: reserveAta, rule: "leaves only by fund_mining into the bridge with the chain mining pool as recipient" },
      mining_pool: { amount: sp(c.pool), account: esc.pool, chain_lamports: esc.pool_balance, rule: "leaves only as block subsidy on paid jobs, at most alpha x P per job" },
      transit: { amount: sp(inputs.transit), accounts: [vaultAta, traderAta], rule: "storefront buyback vault and trader PDA token accounts (emptied within their txs)" },
    },
    bridge: { vault: bridgeVault, vault_amount: sp(inputs.bridgeVault), outside_escrow_spl: sp(inputs.outsideEscrowLamports / 1000n), in_flight: sp(c.inFlight), escrow_balance_lamports: esc.escrow_balance },
    buyback_burned_counter: bb?.burned ?? null,
    reserve: rc && rc.owner === reserveProgram ? decodeReserveConfig(rc.data) : null,
    sources: { mint, reserve_program: reserveProgram, chain_height: esc.height },
    method: "circulating = mint supply - mining reserve - chain mining pool / 1000 - storefront transit accounts. Burned = 1B - mint supply.",
  };
  cache.set(ix, { at: Date.now(), v: v_ });
  return v_;
}

/** Whole tokens as a plain decimal string (CoinGecko / CMC supply endpoints expect a bare number). */
export function plainTokens(base: string, decimals = 6): string {
  const b = BigInt(base);
  const d = 10n ** BigInt(decimals);
  const frac = (b % d).toString().padStart(decimals, "0").replace(/0+$/, "");
  return `${b / d}${frac ? "." + frac : ""}`;
}
