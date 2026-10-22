// The trader board (docs/ECOSYSTEM.md 2.9, Phase 5): the revenue and burn board traders read.
// Every series comes from chain state the indexer recorded (storefront Orders and their Solana
// txs, the storefront and reserve program counters, the chain's jobs and blocks); every item links
// its tx. House spend is its own line, never inside organic. Days are UTC calendar days.

import type { Deps } from "../deps.ts";
import { big } from "../../../../apps/explorer/src/math.ts";
import { readBuyback } from "../../../../apps/explorer/src/buyback.ts";
import { reserveModel, supplyModel } from "../routes/stats.ts";
import type { OrderRec } from "../../../../apps/explorer/src/indexer.ts";
import { storeConfigPda } from "../../../../apps/explorer/src/decode.ts";

const configPda = () => storeConfigPda();

export const METRICS = ["sol-spent-on-ai", "cadre-bought", "buyback", "burned", "reserve", "paid-jobs", "paid-block-share", "house-spend"] as const;
export type Metric = (typeof METRICS)[number];

const day = (ms: number) => new Date(ms).toISOString().slice(0, 10);
const s = (v: bigint) => v.toString();
const ms = (ts: number | null | undefined) => (!ts ? null : ts < 1e12 ? ts * 1000 : ts);

export interface Linker {
  sol: (sig: string) => Record<string, string>;
  solAccount: (a: string) => Record<string, string>;
  chain: (hash: string) => Record<string, string>;
  job: (id: string) => Record<string, string>;
  block: (h: number) => Record<string, string>;
}

function houseSet(d: Deps): Set<string> {
  return new Set(Array.isArray(d.ix.params?.house_operators) ? d.ix.params.house_operators : []);
}

/** The order's buy tx (the one whose events carry its SOL in) and its SOL in. */
function orderBuy(o: OrderRec): { sig: string | null; solIn: bigint; time: number | null } {
  for (const t of o.txs ?? []) for (const p of t.paid ?? []) if (p.nonce === o.nonce) return { sig: t.sig, solIn: big(p.solSpent) + big(p.share), time: ms(t.time) };
  return { sig: o.txs?.[0]?.sig ?? null, solIn: big(o.solSpent), time: ms(o.createdTs) };
}

function payerOf(d: Deps, o: OrderRec): string {
  return d.ix.jobs.get(o.jobId)?.poster ?? o.recipient;
}

interface Series { day: string; organic: bigint; house: bigint; n: number }
function bucket(m: Map<string, Series>, at: number | null, v: bigint, house: boolean) {
  if (at === null) return;
  const k = day(at);
  const e = m.get(k) ?? { day: k, organic: 0n, house: 0n, n: 0 };
  if (house) e.house += v;
  else e.organic += v;
  e.n++;
  m.set(k, e);
}
const seriesOut = (m: Map<string, Series>, days: number) =>
  [...m.values()].sort((a, b) => (a.day < b.day ? -1 : 1)).slice(-days).map((e) => ({ day: e.day, organic: s(e.organic), house: s(e.house), items: e.n }));

function todayAll(m: Map<string, Series>, now: number) {
  const t = m.get(day(now));
  let org = 0n, house = 0n;
  for (const e of m.values()) { org += e.organic; house += e.house; }
  return { today: { organic: s(t?.organic ?? 0n), house: s(t?.house ?? 0n) }, all_time: { organic: s(org), house: s(house) } };
}

export async function metric(d: Deps, m: Metric, L: Linker, now: number, days = 30) {
  const house = houseSet(d);
  const orders = [...d.ix.orders.values()];
  switch (m) {
    case "sol-spent-on-ai": {
      const ser = new Map<string, Series>();
      const items: any[] = [];
      let pending = 0n, refunded = 0n;
      for (const o of orders) {
        const b = orderBuy(o);
        const payer = payerOf(d, o);
        const h = house.has(payer);
        if (o.status === "refunded" || o.status === "expired") { refunded += b.solIn; continue; }
        if (o.status !== "finalized") { pending += b.solIn; continue; }
        bucket(ser, b.time, b.solIn, h);
        items.push({ order: o.nonce, status: o.status, lamports: s(b.solIn), payer, house: h, at: b.time, job: o.jobId, links: { ...(b.sig ? L.sol(b.sig) : {}), ...L.job(o.jobId) } });
      }
      items.sort((a, b) => (b.at ?? 0) - (a.at ?? 0));
      return {
        metric: m, unit: "lamports", ...todayAll(ser, now), by_day: seriesOut(ser, days), pending_lamports: s(pending), refunded_or_expired_lamports: s(refunded), items: items.slice(0, 100),
        source: "storefront Orders (finalized: the job was delivered): OrderPlaced sol_spent + BuybackShareHeld lamports of the buy_job tx; pending orders shown apart, refunded and expired orders returned the SOL and never count; house = the job's payer is a house operator",
      };
    }
    case "cadre-bought": {
      const ser = new Map<string, Series>();
      const items: any[] = [];
      for (const o of orders) {
        if (o.status === "refunded") continue;
        const b = orderBuy(o);
        const h = house.has(payerOf(d, o));
        bucket(ser, b.time, big(o.tokens), h);
        items.push({ order: o.nonce, tokens_spl: o.tokens, house: h, at: b.time, links: b.sig ? L.sol(b.sig) : L.solAccount(o.address) });
      }
      items.sort((a, b) => (b.at ?? 0) - (a.at ?? 0));
      const bb = await readBuyback(d.ix).catch(() => null);
      const res = await reserveModel(d).catch(() => null);
      return {
        metric: m, unit: "CADRE SPL base units (6 dp)", ...todayAll(ser, now), by_day: seriesOut(ser, days), items: items.slice(0, 100),
        buyback_bought_spl: bb?.tokens_bought_spl ?? null, buyback_counter: bb ? { ...L.solAccount(configPda()), source: "storefront Config bb_tokens" } : null,
        reserve_crank_bought_spl: res?.totals.bought_spl ?? null, reserve_crank_spent_lamports: res?.totals.spent_lamports ?? null,
        reserve_config: res ? L.solAccount(res.config) : null,
        source: "orders: the CADRE each storefront order bought on pump.fun or PumpSwap (Order.tokens, by its buy_job tx); buyback: the storefront program's bb_tokens counter; reserve crank: the reserve program's total_bought counter (creator-fee buybacks)",
      };
    }
    case "buyback": {
      const bb = await readBuyback(d.ix).catch(() => null);
      if (!bb) return { metric: m, unit: "lamports", active: false, note: "the deployed storefront has no buyback fields or no buyback activity yet" };
      return {
        metric: m, unit: "lamports", active: true, share_bps: bb.share_bps, destination: bb.destination,
        accrued_lamports: bb.accrued_lamports, spent_lamports: bb.spent_lamports, tokens_bought_spl: bb.tokens_bought_spl, to_reserve_spl: bb.to_reserve_spl, burned_spl: bb.burned_spl, buys: bb.buys, last_buy_at: ms(bb.last_buy_ts),
        buys_recorded: bb.recent_buys.map((x: any) => ({ buy: x.buy, sol_in: x.solIn, tokens_out: x.tokensOut, destination: x.destination, split: x.split, at: ms(x.time), links: L.sol(x.sig) })),
        shares_released: bb.order_shares.slice(0, 100).map((x: any) => ({ order: x.nonce, lamports: x.lamports, links: L.sol(x.sig) })),
        counters: { ...L.solAccount(configPda()), note: "totals are the storefront program's Config counters; buys_recorded lists the BuybackExecuted txs the indexer saw (older ones were pruned by the RPC before it saw them)" },
      };
    }
    case "burned": {
      const sup = await supplyModel(d).catch(() => null);
      const bb = await readBuyback(d.ix).catch(() => null);
      const burns = (d.ix.buybackTxs ?? []).flatMap((t) => (t.parts ?? []).filter((p) => p.kind === "burn").map((p) => ({ tokens_spl: p.tokens, reason: p.reason, supply_after: p.supplyAfter, at: ms(t.time), links: L.sol(t.sig) })));
      return {
        metric: m, unit: "CADRE SPL base units (6 dp)",
        burned_spl: sup?.burned_spl ?? null, initial_supply_spl: sup?.initial_supply_spl ?? null, mint_supply_spl: sup?.mint_supply_spl ?? null,
        buyback_burned_spl: bb?.burned_spl ?? null, burns_recorded: burns,
        mint: sup ? L.solAccount(sup.mint) : null,
        source: "burned = initial supply (pump bonding curve token_total_supply) - live mint supply; buyback burns = BuybackBurned events (storefront) and its bb_burned counter",
      };
    }
    case "reserve": {
      const r = await reserveModel(d).catch(() => null);
      if (!r) return { metric: m, unit: "CADRE SPL base units (6 dp)", available: false, note: "the reserve Config account is not on this Solana cluster" };
      return {
        metric: m, unit: "CADRE SPL base units (6 dp)", available: true, tokens_spl: r.tokens_spl, cap_note: "the buyback sends CADRE here up to the storefront's bb_reserve_cap and burns above it",
        totals: r.totals, last_crank_at: ms(r.last_crank_ts), token_account: r.token_account ? L.solAccount(r.token_account) : null, config: L.solAccount(r.config),
        source: "the reserve creator PDA's token account balance (live) and the reserve program's Config counters; no history is stored on chain, so the board shows the live level",
      };
    }
    case "paid-jobs": {
      const ser = new Map<string, Series>();
      const items: any[] = [];
      for (const j of d.ix.jobs.values()) {
        if ((j.kind ?? "job") === "segment") continue;
        const at = d.ix.blocks.get(j.posted_height)?.time ?? null;
        const h = house.has(j.poster);
        bucket(ser, at, big(j.protocol_fee), h);
        items.push({ job: j.job_id, kind: j.kind ?? "job", status: j.status, protocol_fee: j.protocol_fee, worker_pay: j.worker_pay, poster: j.poster, house: h, posted_height: j.posted_height, at, links: L.job(j.job_id) });
      }
      items.sort((a, b) => b.posted_height - a.posted_height);
      return { metric: m, unit: "chain units (9 dp) of P", ...todayAll(ser, now), by_day: seriesOut(ser, days), items: items.slice(0, 100), source: "listJobs (jobs and inference tickets, not segments): P paid into the mining pool at post, by the day of the posting block; house = the poster is a house operator" };
    }
    case "paid-block-share": {
      const ser = new Map<string, { day: string; paid: number; total: number }>();
      for (const b of d.ix.blocks.values()) {
        const k = day(b.time);
        const e = ser.get(k) ?? { day: k, paid: 0, total: 0 };
        e.total++;
        if (b.jobId) e.paid++;
        ser.set(k, e);
      }
      const rows = [...ser.values()].sort((a, b) => (a.day < b.day ? -1 : 1)).slice(-days).map((e) => ({ day: e.day, paid_blocks: e.paid, blocks: e.total, paid_share_ppm: e.total ? Math.round((e.paid * 1e6) / e.total) : null }));
      let paid = 0, total = 0;
      for (const e of ser.values()) { paid += e.paid; total += e.total; }
      const t = ser.get(day(now));
      const tip = d.ix.chainInfo?.height ?? 0;
      return {
        metric: m, unit: "ppm of blocks", today: t ? { paid_blocks: t.paid, blocks: t.total, paid_share_ppm: Math.round((t.paid * 1e6) / t.total) } : null,
        all_time: { paid_blocks: paid, blocks: total, paid_share_ppm: total ? Math.round((paid * 1e6) / total) : null }, by_day: rows,
        latest_block: tip ? L.block(tip) : null,
        source: "indexed canonical blocks: a paid block carries a job id in its header (mined on a paid job), a fallback block does not",
      };
    }
    case "house-spend": {
      const ser = new Map<string, Series>();
      const items: any[] = [];
      for (const o of orders) {
        if (o.status !== "finalized") continue;
        const payer = payerOf(d, o);
        if (!house.has(payer)) continue;
        const b = orderBuy(o);
        bucket(ser, b.time, b.solIn, true);
        items.push({ kind: "order", order: o.nonce, lamports: s(b.solIn), payer, at: b.time, links: { ...(b.sig ? L.sol(b.sig) : {}), ...L.job(o.jobId) } });
      }
      let p = 0n;
      for (const j of d.ix.jobs.values()) if ((j.kind ?? "job") !== "segment" && house.has(j.poster)) p += big(j.protocol_fee);
      items.sort((a, b) => (b.at ?? 0) - (a.at ?? 0));
      const ta = todayAll(ser, now);
      return {
        metric: m, unit: "lamports", today: ta.today.house, all_time: ta.all_time.house, house_protocol_fees: s(p), house_operators: [...house], by_day: seriesOut(ser, days).map((x) => ({ day: x.day, lamports: x.house, items: x.items })), items: items.slice(0, 100),
        source: "finalized storefront orders whose job payer is a house operator (chain config house_operators): the house paying for its own agents' thoughts. Never counted in sol-spent-on-ai organic",
      };
    }
  }
}


export async function summary(d: Deps, L: Linker, now: number) {
  const out: Record<string, unknown> = {};
  for (const m of METRICS) {
    try {
      const r: any = await metric(d, m, L, now, 14);
      out[m] = r;
    } catch (e) {
      out[m] = { metric: m, error: (e as Error).message.slice(0, 200) };
    }
  }
  return out;
}
