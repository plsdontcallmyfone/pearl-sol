// The notable-event feed (docs/ECOSYSTEM.md 2.9, decision D9): typed events a Telegram or X bot
// can consume, built from the same chain data as the boards. Posting it anywhere is the owner's
// call per account; this module only builds the list (JSON, SSE and RSS in routes/boards.ts).
//
// Thresholds for "big" are proposed defaults (design choices, admin-editable by env), not
// measurements: FEED_BIG_TRADE_LAMPORTS 100000000 (0.1 SOL), FEED_BIG_BUYBACK_LAMPORTS 0 (every
// recorded buyback), FEED_MILESTONE_BLOCKS 1000, FEED_MILESTONE_PAID_BLOCKS 100.

import type { Snap } from "./collect.ts";
import { readBuyback } from "../../../../apps/explorer/src/buyback.ts";
import { shortCx } from "../../../../packages/cx-address/src/index.ts";

export const EVENT_TYPES = ["buyback", "burn", "big_trade", "hire_completed", "paid_dm_answered", "new_agent", "milestone"] as const;
export type EventType = (typeof EVENT_TYPES)[number];

export interface FeedEvent {
  id: string;
  type: EventType;
  at: number | null;
  height: number | null;
  title: string;
  account: string | null;
  amount: string | null;
  unit: string | null;
  house: boolean;
  ref: { chain: "cadre" | "solana" | "market"; id: string };
}

export interface FeedCfg { bigTrade: bigint; bigBuyback: bigint; milestoneBlocks: number; milestonePaidBlocks: number }

export function feedCfg(env: NodeJS.ProcessEnv = process.env): FeedCfg {
  const n = (k: string, d: string) => (env[k] && /^\d+$/.test(env[k]!) ? env[k]! : d);
  return {
    bigTrade: BigInt(n("FEED_BIG_TRADE_LAMPORTS", "100000000")),
    bigBuyback: BigInt(n("FEED_BIG_BUYBACK_LAMPORTS", "0")),
    milestoneBlocks: Math.max(1, Number(n("FEED_MILESTONE_BLOCKS", "1000"))),
    milestonePaidBlocks: Math.max(1, Number(n("FEED_MILESTONE_PAID_BLOCKS", "100"))),
  };
}

const ms = (ts: number | null | undefined) => (!ts ? null : ts < 1e12 ? ts * 1000 : ts);
const sol = (l: bigint) => {
  const w = l / 1_000_000_000n, f = (l % 1_000_000_000n).toString().padStart(9, "0").slice(0, 4).replace(/0+$/, "");
  return f ? `${w}.${f}` : `${w}`;
};
const cad9 = (v: bigint) => sol(v);
const cad6 = (v: bigint) => {
  const w = v / 1_000_000n;
  return w.toLocaleString("en-US");
};

export async function buildEvents(s: Snap, cfg: FeedCfg, cadreMint: string | null = null): Promise<FeedEvent[]> {
  const out: FeedEvent[] = [];
  const ix = s.src.d.ix;
  const name = (a: string) => {
    const c = s.town.card(a);
    return c.handle ? `@${c.handle}` : shortCx(a);
  };

  // buybacks and burns (storefront events the indexer recorded)
  const bb = await readBuyback(ix).catch(() => null);
  for (const b of bb?.recent_buys ?? []) {
    const lam = BigInt(b.solIn);
    if (lam < cfg.bigBuyback) continue;
    out.push({
      id: `buyback:${b.sig}:${b.buy}`, type: "buyback", at: ms(b.time), height: null,
      title: `Buyback #${b.buy}: ${sol(lam)} SOL bought ${cad6(BigInt(b.tokensOut))} CADRE (${b.destination === "burn" ? "burned" : "to the mining reserve"})`,
      account: null, amount: b.solIn, unit: "lamports", house: false, ref: { chain: "solana", id: b.sig },
    });
  }
  for (const t of ix.buybackTxs ?? []) {
    for (const p of t.parts ?? []) {
      if (p.kind !== "burn") continue;
      out.push({
        id: `burn:${t.sig}:${p.buy}`, type: "burn", at: ms(t.time), height: null,
        title: `Burned ${cad6(BigInt(p.tokens))} CADRE${p.reason === "above cap" ? " (the reserve is at its cap)" : ""}`,
        account: null, amount: p.tokens, unit: "CADRE SPL base units (6 dp)", house: false, ref: { chain: "solana", id: t.sig },
      });
    }
  }

  // big Solana trades by agents
  for (const t of s.trades?.solana ?? []) {
    const q = BigInt(t.side === "buy" ? t.amount_in : t.amount_out);
    if (q < cfg.bigTrade) continue;
    const sig = t.ref.replace(/^sol:/, "");
    const house = t.house || s.isHouse(t.agent);
    out.push({
      id: `trade:${sig}`, type: "big_trade", at: t.at || null, height: null,
      title: `${name(t.agent)}${house ? " (house)" : ""} ${t.side === "buy" ? "bought" : "sold"} ${sol(q)} SOL of ${t.token === cadreMint ? "CADRE" : `token ${t.token.slice(0, 4)}...${t.token.slice(-4)}`} on PumpSwap`,
      account: t.agent, amount: q.toString(), unit: "lamports", house, ref: { chain: "solana", id: sig },
    });
  }

  // completed market hires
  for (const h of s.market?.hires.values() ?? []) {
    if (h.status !== "completed") continue;
    const ev = s.market!.hireEvents(h.id).find((e) => e.name === "Completed");
    const o = s.market!.offer(h.offer);
    const house = s.isHouse(h.buyer);
    out.push({
      id: `hire:${h.id}`, type: "hire_completed", at: s.time(ev?.height ?? h.closed), height: ev?.height ?? h.closed,
      title: `${name(h.provider)} completed a hire${o?.title ? ` ("${o.title.slice(0, 60)}")` : ""} for ${name(h.buyer)}: ${cad9(BigInt(h.price))} CADRE${house ? " (house buyer)" : ""}`,
      account: h.provider, amount: h.price, unit: "CADRE base units (9 dp)", house, ref: ev?.tx_hash ? { chain: "cadre", id: ev.tx_hash } : { chain: "market", id: String(h.id) },
    });
  }

  // answered paid DMs (the reply released the pay)
  for (const e of s.dm.escrows.values()) {
    if (e.settled?.state !== "released") continue;
    const p = s.pair(e.from, e.to, e.height);
    if (p.excluded) continue;
    out.push({
      id: `dm:${e.messageId}`, type: "paid_dm_answered", at: s.time(e.settled.height), height: e.settled.height,
      title: `${name(e.to)} answered a paid DM from ${name(e.from)} and earned ${cad9(BigInt(e.pay))} CADRE`,
      account: e.to, amount: e.pay, unit: "CADRE base units (9 dp)", house: p.house, ref: { chain: "cadre", id: e.settled.txHash },
    });
  }

  // new agents
  for (const [a, f] of s.t.agentFirst) {
    out.push({
      id: `agent:${a}`, type: "new_agent", at: f.time, height: f.height,
      title: `New agent ${name(a)}${s.isHouse(a) ? " (house)" : ""} joined Cadre`,
      account: a, amount: null, unit: null, house: s.isHouse(a), ref: { chain: "cadre", id: f.hash },
    });
  }

  // milestones: every N blocks, every M paid job blocks
  let paid = 0;
  for (const h of [...ix.blocks.keys()].sort((x, y) => x - y)) {
    const b = ix.blocks.get(h)!;
    if (h % cfg.milestoneBlocks === 0) out.push({ id: `milestone:block:${h}`, type: "milestone", at: b.time, height: h, title: `Cadre reached block ${h.toLocaleString("en-US")}`, account: null, amount: String(h), unit: "blocks", house: false, ref: { chain: "cadre", id: b.hash } });
    if (b.jobId) {
      paid++;
      if (paid % cfg.milestonePaidBlocks === 0) out.push({ id: `milestone:paid:${paid}`, type: "milestone", at: b.time, height: h, title: `${paid.toLocaleString("en-US")} blocks mined on paid AI jobs`, account: null, amount: String(paid), unit: "paid blocks", house: false, ref: { chain: "cadre", id: b.hash } });
    }
  }

  out.sort((x, y) => (y.at ?? 0) - (x.at ?? 0) || (x.id < y.id ? -1 : 1));
  return out;
}
