// The practice market: which mainnet PumpSwap pools are watched, one read of all of them per tick
// (one getMultipleAccounts call, every 20 s by default), the observation series per pool, and the
// "available set": every signal this process can compute at a slot. A strategy inspects a subset of
// it; the post-mortem's INFORMATION bucket diffs the two (docs/STRATEGIES.md 4).
import { isOnCurve, b58decode } from "../../../../apps/explorer/src/codec.ts";
import { type Acc, ReadOnlySolana } from "./mainnet.ts";
import {
  AMM, AMM_FEE_CONFIG, ata, canonicalPool, decodeFeeConfig, decodePool, type FeeConfig, type Fees, feesFor, mcapLamports, mintDecimals, mintSupply,
  type Pool, priceE12, reserves, TOKEN, TOKEN_2022, tokenAmount, tokenOwner, totalBps, TRADE_DISCS, WSOL,
} from "./pump.ts";
import type { Signal, Values } from "./spec.ts";

export interface Obs { slot: number; t: number; base: string; quote: string; dev: string | null }
export interface Watched { pool: Pool; supply: bigint; decimals: number; tokenProgram: string; devAta: string; obs: Obs[]; trades: number }

/** Discovery: pools with the most PumpSwap trades among the AMM's latest signatures. */
export async function discover(sol: ReadOnlySolana, o: { signatures?: number; txs?: number } = {}): Promise<Map<string, { mint: string; trades: number }>> {
  const sigs = await sol.signatures(AMM, o.signatures ?? 100);
  const out = new Map<string, { mint: string; trades: number }>();
  for (const s of sigs.filter((x) => !x.err).slice(0, o.txs ?? 40)) {
    const t = await sol.tx(s.signature).catch(() => null);
    if (!t?.transaction?.message) continue;
    const m = t.transaction.message;
    const keys: string[] = [...m.accountKeys, ...(t.meta?.loadedAddresses?.writable ?? []), ...(t.meta?.loadedAddresses?.readonly ?? [])];
    const ixs = [...m.instructions, ...(t.meta?.innerInstructions ?? []).flatMap((x: any) => x.instructions)];
    for (const ix of ixs) {
      if (keys[ix.programIdIndex] !== AMM || !ix.data) continue;
      let d: Uint8Array;
      try { d = b58decode(ix.data); } catch { continue; }
      if (d.length < 8 || !TRADE_DISCS.has(Buffer.from(d.subarray(0, 8)).toString("hex"))) continue;
      const pool = keys[ix.accounts[0]], mint = keys[ix.accounts[3]], quote = keys[ix.accounts[4]];
      if (!pool || quote !== WSOL) continue;
      const e = out.get(pool) ?? { mint, trades: 0 };
      e.trades++;
      out.set(pool, e);
    }
  }
  return out;
}

export class Market {
  sol: ReadOnlySolana;
  pools = new Map<string, Watched>();
  feeCfg: FeeConfig | null = null;
  keepMs: number;
  /** Pools the last refresh evicted. */
  evicted: string[] = [];
  constructor(sol: ReadOnlySolana, o: { keepMs?: number } = {}) {
    this.sol = sol;
    this.keepMs = o.keepMs ?? 2 * 3600_000;
  }

  /**
   * Drop watched pools that no longer qualify, so discovery can replace them: the pool's SOL
   * fell under `minPoolSol`, or its reserves have not moved for `idleMs` (no trades). Pools in
   * `keep` (an agent's open trade) always stay. Before 2026-10-23 a pool, once watched, stayed for
   * the life of the process and the set froze at `max`, rugged and dead pools included.
   */
  evict(o: { minPoolSol?: number; idleMs?: number; keep?: Set<string>; now?: number } = {}): string[] {
    const now = o.now ?? Date.now();
    const idle = o.idleMs ?? 30 * 60_000;
    const out: string[] = [];
    for (const [address, w] of this.pools) {
      if (o.keep?.has(address)) continue;
      const last = w.obs.at(-1);
      if (!last) continue;
      const low = Number(big(last.quote)) / 1e9 < (o.minPoolSol ?? 10);
      const first = w.obs[0];
      const still = first.t <= now - idle && w.obs.filter((x) => x.t >= now - idle).every((x) => x.base === last.base && x.quote === last.quote);
      if (low || still) {
        this.pools.delete(address);
        out.push(address);
      }
    }
    return out;
  }

  /** Add pools: only canonical pump pools (migrated from the pump curve) quoted in SOL with at least `minPoolSol`; first evict those that no longer qualify (`evict`). */
  async refresh(found: Map<string, { mint: string; trades: number }>, o: { minPoolSol?: number; max?: number; keep?: Set<string> } = {}): Promise<string[]> {
    this.evicted = this.evict({ minPoolSol: o.minPoolSol, keep: o.keep });
    const fc = await this.sol.accounts([AMM_FEE_CONFIG]);
    if (fc.values[0]) this.feeCfg = decodeFeeConfig(fc.values[0].data);
    const cand = [...found.entries()].filter(([p, x]) => !this.pools.has(p) && canonicalPool(x.mint) === p).sort((a, b) => b[1].trades - a[1].trades);
    const added: string[] = [];
    const max = o.max ?? 12;
    for (const [address, x] of cand) {
      if (this.pools.size >= max) break;
      const r = await this.sol.accounts([address, x.mint]);
      const [pa, ma] = r.values;
      if (!pa || !ma) continue;
      const pool = decodePool(address, pa.data);
      const tokenProgram = ma.owner === TOKEN_2022 ? TOKEN_2022 : TOKEN;
      const v = await this.sol.accounts([pool.baseVault, pool.quoteVault]);
      if (!v.values[0] || !v.values[1]) continue;
      const res = reserves(pool, v.values[0].data, v.values[1].data);
      if (Number(res.quote) / 1e9 < (o.minPoolSol ?? 10)) continue;
      this.pools.set(address, { pool, supply: mintSupply(ma.data), decimals: mintDecimals(ma.data), tokenProgram, devAta: ata(pool.coinCreator, pool.baseMint, tokenProgram), obs: [], trades: x.trades });
      added.push(address);
    }
    return added;
  }

  /** One read of every watched pool (pool, both vaults, the dev's token account): one observation each. */
  async tick(now = Date.now()): Promise<number> {
    const list = [...this.pools.values()];
    if (list.length === 0) return 0;
    const keys = list.flatMap((w) => [w.pool.address, w.pool.baseVault, w.pool.quoteVault, w.devAta]);
    const r = await this.sol.accounts(keys);
    list.forEach((w, i) => {
      const [p, b, q, dv] = r.values.slice(i * 4, i * 4 + 4);
      if (!p || !b || !q) return;
      w.pool = decodePool(w.pool.address, p.data);
      const res = reserves(w.pool, b.data, q.data);
      w.obs.push({ slot: r.slot, t: now, base: res.base.toString(), quote: res.quote.toString(), dev: dv ? tokenAmount(dv.data).toString() : "0" });
      while (w.obs.length > 0 && w.obs[0].t < now - this.keepMs) w.obs.shift();
    });
    return r.slot;
  }

  /** A fresh read of one pool (for fills): reserves at a slot at least `minSlot`. */
  async readPool(address: string, minSlot = 0, tries = 8): Promise<{ slot: number; base: bigint; quote: bigint; t: number }> {
    const w = this.pools.get(address);
    if (!w) throw new Error(`pool ${address} is not watched`);
    for (let i = 0; i < tries; i++) {
      const r = await this.sol.accounts([w.pool.address, w.pool.baseVault, w.pool.quoteVault]);
      const [p, b, q] = r.values;
      if (p && b && q && r.slot >= minSlot) {
        w.pool = decodePool(w.pool.address, p.data);
        const res = reserves(w.pool, b.data, q.data);
        return { slot: r.slot, ...res, t: Date.now() };
      }
      await new Promise((x) => setTimeout(x, 400));
    }
    throw new Error(`pool ${address}: no read at slot >= ${minSlot}`);
  }

  fees(w: Watched, r: { base: bigint; quote: bigint }): Fees {
    if (!this.feeCfg) return { lp: 20, protocol: 5, creator: 95 }; // never used for a fill: refresh() reads the config first
    return feesFor(this.feeCfg, mcapLamports(r, w.supply), true);
  }

  /** Holders and LP: the reads only a decision needs (2 + 1 calls). */
  async deep(w: Watched): Promise<Deep> {
    const [hold, lp] = await Promise.all([this.sol.largest(w.pool.baseMint), this.sol.largest(w.pool.lpMint).catch(() => null)]);
    const lpList = lp?.list ?? [];
    const accts = [...hold.list.map((x) => x.address), ...lpList.map((x) => x.address)];
    const info = accts.length ? await this.sol.accounts(accts) : { slot: 0, values: [] as (Acc | null)[] };
    const ownerOf = (i: number) => (info.values[i] ? tokenOwner(info.values[i]!.data) : "");
    const poolOwned = new Set([w.pool.address, w.pool.creator]);
    const holders = hold.list.map((x, i) => ({ address: x.address, owner: ownerOf(i), amount: x.amount.toString(), excluded: poolOwned.has(ownerOf(i)) || x.address === w.pool.baseVault }));
    const lps = lpList.map((x, i) => {
      const owner = ownerOf(hold.list.length + i);
      return { address: x.address, owner, amount: x.amount.toString(), wallet: owner !== "" && isOnCurve(b58decode(owner)) };
    });
    return { slot: Math.max(hold.slot, lp?.slot ?? 0, info.slot), holders, lp: lps, lpRead: lp !== null };
  }
}

export interface Deep {
  slot: number;
  holders: { address: string; owner: string; amount: string; excluded: boolean }[];
  lp: { address: string; owner: string; amount: string; wallet: boolean }[];
  /** false when the LP holder read failed; an empty list with lpRead = LP fully burned (pump migrations burn it). */
  lpRead: boolean;
}

const big = (s: string) => BigInt(s);
const at = (obs: Obs[], ms: number, now: number): Obs | null => {
  // the newest observation at least `ms` old
  for (let i = obs.length - 1; i >= 0; i--) if (obs[i].t <= now - ms) return obs[i];
  return null;
};
const px = (o: Obs) => Number(priceE12({ base: big(o.base), quote: big(o.quote) }));

/** The available set at the newest observation (plus `deep` when the holder reads were made). */
export function signals(w: Watched, fees: Fees, deep: Deep | null, now = w.obs.at(-1)?.t ?? Date.now()): Values {
  const last = w.obs.at(-1);
  const v: Values = {};
  if (!last) return v;
  const r = { base: big(last.base), quote: big(last.quote) };
  v.pool_sol = Number(r.quote) / 1e9;
  v.mcap_sol = Number(mcapLamports(r, w.supply)) / 1e9;
  v.fee_bps = totalBps(fees);
  // a window counts only when the series reaches back that far (allowing one tick of jitter)
  const ret = (ms: number) => {
    const o = at(w.obs, ms - 25_000, now);
    return o && o.t <= now - ms + 25_000 && px(o) > 0 ? px(last) / px(o) - 1 : null;
  };
  v.ret_5m = ret(5 * 60_000);
  v.ret_15m = ret(15 * 60_000);
  v.ret_60m = ret(60 * 60_000);
  const win = w.obs.filter((o) => o.t >= now - 15 * 60_000);
  if (win.length >= 6) {
    const lr: number[] = [];
    for (let i = 1; i < win.length; i++) if (px(win[i - 1]) > 0 && px(win[i]) > 0) lr.push(Math.log(px(win[i]) / px(win[i - 1])));
    const m = lr.reduce((a, b) => a + b, 0) / lr.length;
    v.vol_15m = lr.length > 1 ? Math.sqrt(lr.reduce((a, b) => a + (b - m) ** 2, 0) / (lr.length - 1)) : null;
    const q0 = Number(win[0].quote);
    v.flow_15m = q0 > 0 ? Number(last.quote) / q0 - 1 : null;
    const devs = win.map((o) => (o.dev === null ? null : big(o.dev))).filter((x): x is bigint => x !== null);
    v.dev_sold_15m = devs.length >= 2 ? (devs.some((d, i) => i > 0 && d < devs[i - 1]) ? 1 : 0) : null;
  } else {
    v.vol_15m = null;
    v.flow_15m = null;
    v.dev_sold_15m = null;
  }
  v.dev_share = last.dev !== null && w.supply > 0n ? Number(big(last.dev)) / Number(w.supply) : null;
  if (deep) {
    const h = deep.holders.filter((x) => !x.excluded).map((x) => Number(big(x.amount)));
    const s = Number(w.supply);
    v.top10_share = s > 0 ? h.slice(0, 10).reduce((a, b) => a + b, 0) / s : null;
    v.top1_share = s > 0 && h.length ? h[0] / s : null;
    const lpTotal = deep.lp.reduce((a, x) => a + Number(big(x.amount)), 0);
    v.lp_wallet_share = !deep.lpRead ? null : lpTotal > 0 ? deep.lp.filter((x) => x.wallet).reduce((a, x) => a + Number(big(x.amount)), 0) / lpTotal : 0;
  } else {
    v.top10_share = null;
    v.top1_share = null;
    v.lp_wallet_share = null;
  }
  return v;
}

export const SIGNALS_NEED_DEEP: Signal[] = ["top10_share", "top1_share", "lp_wallet_share"];
