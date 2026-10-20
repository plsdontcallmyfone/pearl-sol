// /revenue: revenue after wash screens + supply (models in revenue.ts and supply.ts).
// Same primitives and shell as views.ts; Geist only, no monospace.

import { BRAND } from "./brand.ts";
import { big, fmtInt, pct } from "./math.ts";
import type { Summary } from "./models.ts";
import { S, T, badge, card, esc, id, kv, page, sol, spl, tok } from "./views.ts";

const share = (ppm: number) => esc(pct(ppm, 1_000_000));

function stat(label: string, value: string, sub: string) {
  return `<div class="stat"><div class="eyebrow">${esc(label)}</div><div class="stat-v">${value}</div><div class="stat-s">${sub}</div></div>`;
}

function supplyCard(sp: any) {
  if (!sp?.available) return card("Supply", `<p class="note">Not available: ${esc(sp?.reason ?? "unknown")}.</p>`);
  const nc = sp.not_circulating;
  const r = sp.reserve;
  const body =
    kv([
      ["Total supply", `<span class="num">${spl(sp.total_supply)}</span> ${T()} <span class="faint">(Solana mint supply)</span>`],
      ["Circulating", `<span class="num">${spl(sp.circulating_supply)}</span> ${T()}`],
      ["Burned", `<span class="num">${spl(sp.burned)}</span> ${T()} <span class="faint">of the 1B minted at launch</span>`],
      ["Mining reserve", `<span class="num">${spl(nc.mining_reserve.amount)}</span> ${T()} ${id(nc.mining_reserve.account)}`],
      ["Mining pool (agent chain)", `<span class="num">${spl(nc.mining_pool.amount)}</span> ${T()} ${id(nc.mining_pool.account)}`],
      ["Storefront transit", `<span class="num">${spl(nc.transit.amount)}</span> ${T()}`],
      ["Bridge vault", `<span class="num">${spl(sp.bridge.vault_amount)}</span> ${T()} <span class="faint">backs the chain balances outside the escrow (in flight <span class="num">${spl(sp.bridge.in_flight)}</span>)</span>`],
    ]) +
    `<p class="note">${esc(sp.method)} Machine-readable: <a href="/api/supply">/api/supply</a>, <a href="/api/supply/circulating">/api/supply/circulating</a>, <a href="/api/supply/total">/api/supply/total</a>.</p>`;
  const exit = !r
    ? `<p class="note">Reserve config not found on Solana.</p>`
    : !r.permissionless
      ? `<p class="note">This reserve program predates the permissionless fund_mining (config layout ${esc(r.layout)}): the admin or funder key triggers funding.</p>`
      : kv([
          ["Who can fund the pool", `${badge("ok", "anyone")} <span class="faint">fund_mining is permissionless; the destination is fixed</span>`],
          ["Destination", `${id(r.mining_pool)} <span class="faint">frozen since the first funding</span>`],
          ["Status", r.paused ? badge("warn", "paused by the admin") : badge("ok", "open")],
          ["Per call", big(r.max_fund_per_call) === 0n ? "no cap" : `<span class="num">${spl(r.max_fund_per_call)}</span> ${T()}`],
          ["Interval", `<span class="num">${fmtInt(r.min_interval_secs)}</span> s between calls`],
          ["Window", r.window_secs > 0 && big(r.max_per_window) > 0n ? `<span class="num">${spl(r.max_per_window)}</span> ${T()} per <span class="num">${fmtInt(r.window_secs)}</span> s (used <span class="num">${spl(r.window_used)}</span>)` : "no window cap"],
          ["Funded so far", `<span class="num">${spl(r.total_funded)}</span> ${T()} in <span class="num">${fmtInt(r.fund_calls)}</span> call${r.fund_calls === 1 ? "" : "s"}`],
        ]) + `<p class="note">Pool tokens can leave only as block subsidy on paid jobs, at most alpha x P per job; reserve tokens can leave only into the pool.</p>`;
  return card("Supply", body, { aside: `<a class="more-s" href="/api/supply">JSON</a>` }) + card("Reserve exit", exit);
}

export function revenuePage(sm: Summary, rv: any, sp: any) {
  const sc = rv.screened;
  const top = sc.top_payer;
  const stats = `<section class="stats">
  ${stat("Protocol fees P, screened", `<span class="num">${tok(sc.protocol_fees, 2)}</span> ${T()}`, `of <span class="num">${tok(rv.gross.protocol_fees, 2)}</span> gross over <span class="num">${fmtInt(sc.jobs)}</span> jobs`)}
  ${stat("SOL via storefront, screened", `<span class="num">${sol(sc.storefront_sol_lamports)}</span> ${S()}`, `<span class="num">${fmtInt(sc.storefront_orders)}</span> finalized order${sc.storefront_orders === 1 ? "" : "s"}`)}
  ${stat("Unique payers, screened", `<span class="num">${fmtInt(sc.unique_payers)}</span>`, top ? `top payer <span class="num">${share(top.share_ppm)}</span> of P · top two <span class="num">${share(sc.top_two_share_ppm)}</span>` : "no payer yet")}
  ${stat("Released buyback shares", rv.buyback ? `<span class="num">${sol(rv.buyback.released_shares_lamports)}</span> ${S()}` : `<span class="faint">–</span>`, "holders revenue: SOL the buyback spends on the token")}
</section>`;
  const ex = rv.self_dealt_excluded;
  const exList = ex.jobs_list.length
    ? `<div class="table t-payers"><div class="tr th"><span>Job</span><span>Payer</span><span>Why excluded</span><span>P</span></div>${ex.jobs_list
        .map((j: any) => `<div class="tr"><span>${id(j.job_id, { href: `/job/${j.job_id}` })}</span><span>${id(j.payer)}</span><span>${esc(j.reason)}</span><span class="num">${tok(j.protocol_fee, 2)}</span></div>`)
        .join("")}</div>`
    : `<p class="note">No job was paid for by its own deliverer or miner.</p>`;
  const payers = rv.payers.length
    ? `<div class="table t-payers"><div class="tr th"><span>Payer</span><span>Protocol fees P</span><span>Share</span></div>${rv.payers
        .map((p: any) => `<div class="tr"><span>${id(p.payer)}</span><span class="num">${tok(p.protocol_fees, 2)}</span><span class="num">${share(p.share_ppm)}</span></div>`)
        .join("")}</div>`
    : `<p class="note">No screened payer yet.</p>`;
  const seller = sc.from_deliverers_under_10_payers;
  return page(
    "Revenue",
    `<nav class="crumbs"><a href="/">Overview</a><span>/</span><span>Revenue</span></nav>
<div class="intro"><h1>Revenue after wash screens</h1><p>What payers spent on work they did not do themselves. A job whose payer also delivered it or mined a block on it is excluded: that payer gets its own work pay and the subsidy back. Counts of transactions are never the measure; paid volume from distinct payers is. Amounts are ${esc(BRAND.ticker)} and SOL, no USD.</p></div>
${stats}
<div class="cards">
  ${card(
    "Screened, gross and excluded",
    kv([
      ["Gross", `<span class="num">${tok(rv.gross.protocol_fees, 2)}</span> ${T()} P · <span class="num">${sol(rv.gross.storefront_sol_lamports)}</span> ${S()} · <span class="num">${fmtInt(rv.gross.unique_payers)}</span> payers`],
      ["House swarm spend (kept apart)", rv.house_spend ? `<span class="num">${tok(rv.house_spend.protocol_fees, 2)}</span> ${T()} P over <span class="num">${fmtInt(rv.house_spend.jobs)}</span> job${rv.house_spend.jobs === 1 ? "" : "s"} · <span class="num">${sol(rv.house_spend.storefront_sol_lamports)}</span> ${S()} <span class="faint">(${rv.house_spend.house_operators.length ? `${fmtInt(rv.house_spend.house_operators.length)} declared house operator${rv.house_spend.house_operators.length === 1 ? "" : "s"}` : "no house operator declared on the chain"})</span>` : `<span class="faint">not reported</span>`],
      ["Excluded as self-dealing", `<span class="num">${tok(ex.protocol_fees, 2)}</span> ${T()} P over <span class="num">${fmtInt(ex.jobs)}</span> job${ex.jobs === 1 ? "" : "s"} · <span class="num">${sol(ex.storefront_sol_lamports)}</span> ${S()}`],
      ["Screened", `<span class="num">${tok(sc.protocol_fees, 2)}</span> ${T()} P · W delivered <span class="num">${tok(sc.worker_pay_delivered, 2)}</span> ${T()}`],
      ["From deliverers with under 10 payers", `<span class="num">${tok(seller.protocol_fees, 2)}</span> ${T()} P over <span class="num">${fmtInt(seller.jobs)}</span> job${seller.jobs === 1 ? "" : "s"} <span class="faint">(${fmtInt(sc.deliverers)} deliverer${sc.deliverers === 1 ? "" : "s"}; shown, not excluded)</span>`],
    ]) + `<p class="note">${esc(rv.method)}</p>${rv.notes.map((n: string) => `<p class="note">${esc(n)}</p>`).join("")}`,
    { aside: `<a class="more-s" href="/api/revenue">JSON</a>` },
  )}
  ${card("Screened payers", payers)}
  ${card("Excluded jobs", exList)}
  ${supplyCard(sp)}
</div>`,
    { active: "revenue", summary: sm },
  );
}
