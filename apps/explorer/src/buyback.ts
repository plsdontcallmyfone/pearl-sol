// Storefront buyback panel. Shown only when the deployed storefront program has the
// buyback fields (Config tail) and they record activity; totals are the program's own
// all-time counters, buys are the BuybackExecuted events from the txs the explorer saw.
// Before that it returns null and the panel stays hidden (nothing is invented).

import type { Indexer } from "./indexer.ts";

export async function readBuyback(ix: Indexer) {
  const b = ix.buyback;
  if (!b) return null;
  const buys = ix.buybackTxs
    .filter((t) => t.buyback?.executed.length)
    .flatMap((t) => t.buyback!.executed.map((e) => ({ ...e, sig: t.sig, slot: t.slot, time: t.time })))
    .sort((a, b2) => b2.buy - a.buy);
  const accruals = [...ix.orders.values()].flatMap((o) =>
    o.txs.flatMap((t) => (t.buyback?.accrued ?? []).map((a) => ({ ...a, sig: t.sig, slot: t.slot }))),
  );
  if (b.buys === 0 && BigInt(b.accrued) === 0n && b.bps === 0) return null;
  return {
    share_bps: b.bps,
    destination: b.dest,
    accrued_lamports: b.accrued,
    spent_lamports: b.spent,
    tokens_bought_spl: b.tokens,
    to_reserve_spl: b.toReserve,
    burned_spl: b.burned,
    buys: b.buys,
    orders_waiting: b.pendingOrders,
    last_buy_ts: b.lastTs || null,
    recent_buys: buys.slice(0, 20),
    order_shares: accruals,
    note: "Totals are the storefront program's own counters (Config). Buys list the BuybackExecuted events the explorer recorded.",
  };
}
