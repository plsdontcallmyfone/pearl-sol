// Server-rendered HTML for every page. No framework: template strings over the page
// models (models.ts). Geist for all text, numbers and hashes; no monospace anywhere.

import { BRAND } from "./brand.ts";
import { CHAIN_DP, SOL_DP, SPL_DP, big, fmtInt, formatUnits, pct } from "./math.ts";
import type { BlockRow, Summary } from "./models.ts";
import type { SolTx } from "./indexer.ts";

function waitNote(w: any) {
  if (!w) return "";
  return `<p class="note">The job is ${esc(w.job_status)} on the chain at <a href="/block/${w.closed_height}">#${fmtInt(w.closed_height)}</a>. The Order settles on Solana by proof once the light client's final height reaches it${w.light_client_final === null ? "" : `: now #${fmtInt(w.light_client_final)}, ${fmtInt(w.blocks_to_go)} blocks to go`}.</p>`;
}

// ------------------------------------------------------------ primitives

export const esc = (v: unknown) =>
  String(v ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

const tok = (v: string | bigint | null | undefined, frac = 4) => (v === null || v === undefined ? "–" : formatUnits(big(v as any), CHAIN_DP, frac));
const sol = (v: string | bigint | null | undefined, frac = 4) => (v === null || v === undefined ? "–" : formatUnits(big(v as any), SOL_DP, frac));
const spl = (v: string | bigint | null | undefined, frac = 2) => (v === null || v === undefined ? "–" : formatUnits(big(v as any), SPL_DP, frac));
const T = () => `<span class="unit">${esc(BRAND.ticker)}</span>`;
const S = () => `<span class="unit">SOL</span>`;

function short(v: string, head = 6, tail = 6) {
  return v.length <= head + tail + 1 ? v : `${v.slice(0, head)}…${v.slice(-tail)}`;
}

/** A hash or address: shortened, full value in the title, a copy button, optional link. */
function id(v: string | null | undefined, opts: { href?: string; head?: number; tail?: number; full?: boolean } = {}) {
  if (!v) return `<span class="faint">none</span>`;
  const text = opts.full ? esc(v) : esc(short(v, opts.head ?? 6, opts.tail ?? 6));
  const inner = opts.href ? `<a class="hash" href="${esc(opts.href)}" title="${esc(v)}">${text}</a>` : `<span class="hash" title="${esc(v)}">${text}</span>`;
  return `<span class="idwrap${opts.full ? " full" : ""}">${inner}<button class="copy" type="button" data-copy="${esc(v)}" aria-label="Copy ${esc(v)}">${ICON.copy}</button></span>`;
}

const solTxHref = (sig: string) => `/sol/tx/${encodeURIComponent(sig)}`;

function time(ms: number | null | undefined) {
  if (!ms) return `<span class="faint">–</span>`;
  const d = new Date(ms);
  return `<time class="ago" datetime="${d.toISOString()}" data-ts="${ms}" title="${d.toISOString().replace("T", " ").slice(0, 19)} UTC">${d.toISOString().slice(11, 19)} UTC</time>`;
}

const ICON = {
  copy: `<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><rect x="5.5" y="5.5" width="8" height="8" rx="1.6" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M10.5 3.5v-.4A1.6 1.6 0 0 0 8.9 1.5H3.1A1.6 1.6 0 0 0 1.5 3.1v5.8a1.6 1.6 0 0 0 1.6 1.6h.4" fill="none" stroke="currentColor" stroke-width="1.4"/></svg>`,
  check: `<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="M3.5 8.5l3 3 6-7" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
  cross: `<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/></svg>`,
  theme: `<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"><circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M8 2.4a5.6 5.6 0 0 1 0 11.2z" fill="currentColor"/></svg>`,
  arrow: `<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="M6 3.5L10.5 8 6 12.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
};

function badge(kind: "paid" | "fallback" | "ok" | "warn" | "bad" | "neutral" | "accent", text: string) {
  return `<span class="badge b-${kind}">${esc(text)}</span>`;
}

function check(ok: boolean | null, yes: string, no: string) {
  if (ok === null) return `<span class="faint">–</span>`;
  return ok ? `<span class="chk ok">${ICON.check}${esc(yes)}</span>` : `<span class="chk bad">${ICON.cross}${esc(no)}</span>`;
}

const LC_SHORT: Record<string, [string, string]> = {
  final: ["ok", "Verified on Solana"],
  accumulated: ["ok", "Verified on Solana"],
  verified: ["ok", "Verified, not final"],
  pending: ["warn", "Verifying"],
  "not-relayed": ["faint", "Not relayed yet"],
  "before-checkpoint": ["faint", "Before checkpoint"],
  "no-record": ["warn", "No record"],
  none: ["faint", "No light client"],
};

function lcPill(lc: { state: string; label: string; behind?: number }) {
  const [tone, text] = LC_SHORT[lc.state] ?? ["faint", lc.state];
  const extra = lc.state === "not-relayed" && lc.behind ? `<span class="lc-behind">+${fmtInt(lc.behind)}</span>` : "";
  return `<span class="lc lc-${tone}" title="${esc(lc.label)}"><span class="dot"></span>${esc(text)}${extra}</span>`;
}

function kv(rows: [string, string][]) {
  return `<dl class="kv">${rows.map(([k, v]) => `<div><dt>${esc(k)}</dt><dd>${v}</dd></div>`).join("")}</dl>`;
}

function card(title: string, body: string, opts: { aside?: string; cls?: string } = {}) {
  return `<section class="card ${opts.cls ?? ""}"><header class="card-h"><h2>${esc(title)}</h2>${opts.aside ?? ""}</header>${body}</section>`;
}

function solTxList(txs: SolTx[] | null | undefined, empty: string) {
  if (!txs || txs.length === 0) return `<p class="note">${esc(empty)}</p>`;
  return `<ul class="txlist">${txs
    .map(
      (t) =>
        `<li><span class="ixs">${t.ix.length ? t.ix.map((x) => `<span class="ix">${esc(x)}</span>`).join("") : `<span class="ix faint">other</span>`}</span>` +
        `<span class="txmeta">${t.ok ? "" : badge("bad", "failed")}slot ${fmtInt(t.slot)}</span>${id(t.sig, { href: solTxHref(t.sig), head: 8, tail: 6 })}</li>`,
    )
    .join("")}</ul>`;
}

// ------------------------------------------------------------ shell

export function page(title: string, body: string, opts: { active?: string; summary?: Summary | null; live?: string } = {}) {
  const sm = opts.summary;
  const nav = [
    ["feed", "/", "Blocks"],
    ["jobs", "/jobs", "Jobs"],
    ["orders", "/orders", "Orders"],
    ["api", "/docs", "API"],
  ]
    .map(([k, href, label]) => `<a href="${href}"${opts.active === k ? ' aria-current="page"' : ""}>${label}</a>`)
    .join("");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(title)} · ${esc(BRAND.name)} Explorer</title>
<meta name="description" content="Every block of the ${esc(BRAND.name)} agent chain with the job it mined, who paid, the protocol fee, the subsidy and the proof, as the Solana light client sees it.">
<link rel="preload" href="/fonts/Geist-Variable.woff2" as="font" type="font/woff2" crossorigin>
<link rel="stylesheet" href="/assets/app.css">
<script>try{var t=localStorage.getItem("theme");if(t)document.documentElement.dataset.theme=t}catch(e){}</script>
</head>
<body${opts.live ? ` data-live="${esc(opts.live)}"` : ""}>
<header class="top">
  <div class="top-in">
    <a class="brand" href="/"><span class="brand-name">${esc(BRAND.name)}</span><span class="brand-sub">Explorer</span></a>
    <nav class="nav" aria-label="Main">${nav}</nav>
    <div class="top-r">
      ${statusPill(sm)}
      <button class="iconbtn" type="button" id="theme" aria-label="Switch light or dark theme">${ICON.theme}</button>
    </div>
  </div>
</header>
<main class="wrap">${body}</main>
<footer class="foot"><div class="foot-in">
  <span>Read-only. Every figure is an RPC read of the agent chain or Solana, or arithmetic over those reads. No price source exists, so amounts are in SOL and ${esc(BRAND.ticker)}, never USD.</span>
  <a href="/docs">JSON API</a>
</div></footer>
<script src="/assets/app.js" defer></script>
</body>
</html>`;
}

export function statusPill(sm: Summary | null | undefined) {
  if (!sm || !sm.chain) return `<span class="pill pill-bad" id="status"><span class="dot"></span>Connecting</span>`;
  if (!sm.status.node_ok) return `<span class="pill pill-bad" id="status" title="${esc(sm.status.node_error)}"><span class="dot"></span>Node unreachable</span>`;
  const solBad = !sm.status.solana_ok ? ` <span class="pill-sep"></span><span title="${esc(sm.status.solana_error)}">Solana unreachable</span>` : "";
  return `<span class="pill pill-ok" id="status"><span class="dot"></span>Live<span class="pill-sep"></span><span class="num">#${fmtInt(sm.chain.height)}</span>${solBad}</span>`;
}

// ------------------------------------------------------------ home

export function heroAndStats(sm: Summary) {
  const b = sm.blocks;
  const paidPct = b.total ? (b.paid_job_blocks / b.total) * 100 : 0;
  const barW = b.paid_job_blocks > 0 ? Math.max(paidPct, 0.6) : 0;
  const idx = sm.index.complete
    ? `Indexed all ${fmtInt(sm.index.blocks_indexed)} blocks`
    : `Indexing: ${fmtInt(sm.index.blocks_indexed)} of ${fmtInt(sm.index.blocks_on_chain)} blocks, totals grow until it finishes`;
  const sf = sm.storefront;
  const alpha = sm.params ? sm.params.alpha_ppm / 10_000 : null;
  const subPctOfP = big(sm.fees.protocol_fees_paid) > 0n ? pct(Number(sm.fees.subsidy_ppm_of_protocol_fees), 1_000_000) : "0%";
  const statusCounts = (o: Record<string, number>) =>
    Object.entries(o)
      .map(([k, v]) => `${fmtInt(v)} ${k}`)
      .join(" · ") || "none yet";
  return `<section class="hero" id="hero">
  <div class="hero-main">
    <div class="eyebrow">Blocks mined on paid jobs</div>
    <div class="display"><span class="num">${fmtInt(b.paid_job_blocks)}</span><span class="display-of">of <span class="num">${fmtInt(b.total)}</span></span><span class="display-pct num">${esc(b.paid_share)}</span></div>
    <div class="ratio" role="img" aria-label="${esc(b.paid_share)} of blocks were mined on paid jobs, the rest are fallback blocks">
      <span class="ratio-paid" style="width:${barW.toFixed(3)}%"></span>
    </div>
    <div class="ratio-legend"><span><i class="sw sw-paid"></i>Paid job <span class="num">${fmtInt(b.paid_job_blocks)}</span></span><span><i class="sw sw-fb"></i>Fallback <span class="num">${fmtInt(b.fallback_blocks)}</span></span></div>
    <p class="hero-note">A fallback block mines no job: it earns its tx fees and no subsidy from the reserve. Only a block bound to a paid job earns subsidy, and a job's subsidy can never exceed alpha x its protocol fee P, so mining your own job always loses money. ${esc(idx)}.</p>
  </div>
</section>
<section class="stats" id="stats">
  <div class="stat">
    <div class="eyebrow">SOL settled via storefront</div>
    <div class="stat-v"><span class="num">${sol(sf.sol_settled)}</span> ${S()}</div>
    <div class="stat-s">${esc(statusCounts(sf.by_status))}${big(sf.sol_pending) > 0n ? ` · <span class="num">${sol(sf.sol_pending)}</span> SOL pending` : ""}</div>
  </div>
  <div class="stat">
    <div class="eyebrow">Protocol fees paid (P)</div>
    <div class="stat-v"><span class="num">${tok(sm.fees.protocol_fees_paid, 2)}</span> ${T()}</div>
    <div class="stat-s">Into the mining pool, non-refundable. Nothing is burned on the chain.</div>
  </div>
  <div class="stat">
    <div class="eyebrow">Subsidy paid vs fees</div>
    <div class="stat-v"><span class="num">${tok(sm.fees.subsidy_paid, 2)}</span> ${T()}</div>
    <div class="stat-s"><span class="num">${esc(subPctOfP)}</span> of P${alpha !== null ? ` · cap alpha <span class="num">${alpha}%</span>` : ""} · ${sm.fees.jobs_over_cap === 0 ? `<span class="chk ok">${ICON.check}no job over cap</span>` : `<span class="chk bad">${ICON.cross}${sm.fees.jobs_over_cap} over cap</span>`}</div>
  </div>
  <div class="stat">
    <div class="eyebrow">Unique payers</div>
    <div class="stat-v"><span class="num">${fmtInt(sm.payers.unique_payers)}</span></div>
    <div class="stat-s"><span class="num">${fmtInt(sm.payers.unique_solana_buyers)}</span> Solana buyer wallet${sm.payers.unique_solana_buyers === 1 ? "" : "s"} · top payer <span class="num">${esc(sm.payers.top_payer_share_of_fees)}</span> of P</div>
  </div>
</section>`;
}

function feedRow(r: BlockRow) {
  const paid = r.kind === "paid_job";
  const who = paid
    ? `<span class="faint">paid by</span> <span class="hash" title="${esc(r.paid_by)}">${esc(short(r.paid_by!, 4, 4))}</span> ${
        r.paid_via === "storefront"
          ? `<span class="faint">·</span> order #${esc(r.order_nonce)} <span class="faint">·</span> <span class="num">${sol(r.sol_spent)}</span> SOL`
          : `<span class="faint">· PostJob</span>`
      }`
    : `<span class="faint">No job. Fees only, no subsidy.</span>`;
  const amounts = paid
    ? `<div class="amt"><span class="faint">P</span> <span class="num">${tok(r.protocol_fee, 2)}</span></div><div class="amt"><span class="faint">subsidy</span> <span class="num">${tok(r.subsidy, 2)}</span> ${check(r.within_cap, "≤ cap", "over cap")}</div>`
    : `<div class="amt"><span class="faint">fees</span> <span class="num">${tok(r.fees)}</span></div>`;
  return `<a class="row ${paid ? "row-paid" : "row-fb"}" href="/block/${r.height}">
  <span class="c-h num">${fmtInt(r.height)}</span>
  <span class="c-main"><span class="c-l1">${paid ? badge("paid", "Paid job") : badge("fallback", "Fallback")}${paid ? `<span class="hash" title="${esc(r.job_id)}">job ${esc(short(r.job_id!, 6, 4))}</span>` : `<span class="hash faint" title="proof ${esc(r.proof_hash)}">proof ${esc(short(r.proof_hash, 6, 4))}</span>`}</span><span class="c-l2">${who}</span></span>
  <span class="c-amt">${amounts}</span>
  <span class="c-lc">${lcPill(r.light_client)}</span>
  <span class="c-t">${time(r.time_ms)}</span>
</a>`;
}

function runRow(g: { from: number; to: number; count: number; fees: string; newest: BlockRow; oldest: BlockRow }) {
  return `<a class="row row-run" href="/blocks?filter=fallback&amp;before=${g.to + 1}">
  <span class="c-h num">${fmtInt(g.to)}<span class="faint run-to">to ${fmtInt(g.from)}</span></span>
  <span class="c-main"><span class="c-l1">${badge("fallback", "Fallback")}<span><span class="num">${fmtInt(g.count)}</span> fallback blocks</span></span><span class="c-l2 faint">No job. Fees only, no subsidy.</span></span>
  <span class="c-amt"><div class="amt"><span class="faint">fees</span> <span class="num">${tok(g.fees)}</span></div></span>
  <span class="c-lc">${lcPill(g.newest.light_client)}</span>
  <span class="c-t">${time(g.newest.time_ms)}</span>
</a>`;
}

export function feedList(feed: any) {
  if (!feed.rows.length)
    return `<div class="empty"><strong>No blocks here yet.</strong><span>${feed.filter === "paid" ? "No block has been mined on a paid job in the indexed range. Buy a job through the storefront (x402) or post one with <span class='code'>pearl job post</span>, and its blocks show up here." : "The indexer has not read any block yet. Check that the node RPC is reachable."}</span></div>`;
  const rows = feed.rows.map((g: any) => (g.kind === "block" ? feedRow(g.block) : runRow(g))).join("");
  const more = feed.next_before ? `<a class="more" href="/blocks?filter=${esc(feed.filter)}&amp;before=${feed.next_before}">Older blocks ${ICON.arrow}</a>` : "";
  return `<div class="rows">${rows}</div>${more}`;
}

function feedPanel(feed: any, live: boolean) {
  const f = feed.filter;
  const tab = (k: string, label: string) => `<a href="${k === "all" ? "/" : `/blocks?filter=${k}`}" role="tab" aria-selected="${f === k}" class="seg-b">${label}</a>`;
  return `<section class="card feed">
  <header class="card-h"><h2>${feed.before ? `Blocks below ${fmtInt(feed.before)}` : "Latest blocks"}</h2>
    <div class="feed-tools">${live ? `<span class="livedot" title="Updates every few seconds">Live</span>` : ""}<div class="seg" role="tablist" aria-label="Filter blocks">${tab("all", "All")}${tab("paid", "Paid jobs")}${tab("fallback", "Fallback")}</div></div>
  </header>
  <div class="feed-head" aria-hidden="true"><span>Height</span><span>Job and payer</span><span>Fee and subsidy</span><span>Solana light client</span><span>Time</span></div>
  <div id="feed" data-filter="${esc(f)}" data-before="${esc(feed.before ?? "")}">${feedList(feed)}</div>
</section>`;
}

export function rail(sm: Summary, jobs: any, orders: any, buyback: any) {
  const lc = sm.light_client;
  const lcBody = lc
    ? `${kv([
        ["Trusted checkpoint", `<span class="num">#${fmtInt(lc.checkpoint_height)}</span>`],
        ["Tip on Solana", `<span class="num">#${fmtInt(lc.tip_height)}</span>`],
        ["Final on Solana", `<span class="num">#${fmtInt(lc.final_height)}</span>`],
        ["Behind the chain", lc.behind_chain > 0 ? `<span class="num warn-t">${fmtInt(lc.behind_chain)} blocks</span>` : `<span class="chk ok">${ICON.check}in step</span>`],
        ["Final headers kept", lc.accumulated ? `<span class="num">${fmtInt(lc.accumulated.count)}</span> <span class="faint">from #${fmtInt(lc.accumulated.start_height)}</span>` : "–"],
      ])}<p class="note">Each header's Pearl proof is re-verified by the Solana program before it counts. Storefront results and bridge withdrawals settle by proof against final headers.</p>`
    : `<p class="note">No light client found on this Solana cluster. Run <span class="code">header-relay init</span>, then <span class="code">header-relay run</span>.</p>`;
  const jl = jobs.jobs.slice(0, 6);
  const jobsBody = jl.length
    ? `<ul class="mini">${jl
        .map(
          (j: any) =>
            `<li><a href="/job/${esc(j.job_id)}"><span class="mini-l"><span class="hash">${esc(short(j.job_id, 6, 4))}</span>${statusBadge(j.status)}</span><span class="mini-r"><span class="num">${tok(j.protocol_fee, 2)}</span> <span class="unit">P</span></span><span class="mini-s">${j.paid_via === "storefront" ? `order #${esc(j.order_nonce)} · <span class="num">${sol(j.sol_spent)}</span> SOL` : "PostJob"} · <span class="num">${j.blocks_mined}</span>/<span class="num">${j.max_blocks}</span> blocks</span></a></li>`,
        )
        .join("")}</ul>`
    : `<p class="note">No jobs on the chain yet. A job appears when a storefront order is unlocked or someone sends PostJob.</p>`;
  const ol = orders.orders.slice(0, 6);
  const ordersBody = ol.length
    ? `<ul class="mini">${ol
        .map(
          (o: any) =>
            `<li><a href="/order/${esc(o.nonce)}"><span class="mini-l">Order #${esc(o.nonce)}${statusBadge(o.status)}</span><span class="mini-r"><span class="num">${sol(o.sol_spent)}</span> <span class="unit">SOL</span></span><span class="mini-s">payer ${esc(short(o.payer, 4, 4))} · ${o.settlement.mode === "proof" ? "settled by proof" : o.settlement.mode === "signers" ? "settled by signers" : o.status === "pending" ? "awaiting result" : "settlement tx not recorded"}</span></a></li>`,
        )
        .join("")}</ul>`
    : `<p class="note">No storefront orders on Solana yet. Agents buy jobs over x402 at the storefront service.</p>`;
  return `<aside class="rail" id="rail">
  ${card("Solana light client", lcBody)}
  ${card("Recent jobs", jobsBody, { aside: `<a class="more-s" href="/jobs">All ${fmtInt(jobs.total)}</a>` })}
  ${card("Storefront orders", ordersBody, { aside: `<a class="more-s" href="/orders">All ${fmtInt(orders.total)}</a>` })}
  ${buyback ? buybackCard(buyback) : ""}
  ${card("For agents", `<p class="note">Every page has a JSON twin. Start at <a href="/api/summary">/api/summary</a> and <a href="/api/feed">/api/feed</a>.</p>`)}
</aside>`;
}

function buybackCard(bb: any) {
  const buys = bb.recent_buys.slice(0, 5);
  const list = buys.length
    ? `<ul class="mini">${buys
        .map(
          (e: any) =>
            `<li><a href="${solTxHref(e.sig)}"><span class="mini-l">Buy #${e.buy}${badge(e.destination === "burn" ? "warn" : "ok", e.destination === "burn" ? "burned" : "to reserve")}</span><span class="mini-r"><span class="num">${sol(e.solIn)}</span> <span class="unit">SOL</span></span><span class="mini-s"><span class="num">${spl(e.tokensOut)}</span> ${esc(BRAND.ticker)} on the ${esc(e.venue)}${e.orders ? ` · orders #${esc(e.firstNonce)} to #${esc(e.lastNonce)}` : ""}</span></a></li>`,
        )
        .join("")}</ul>`
    : `<p class="note">No buy recorded by the explorer yet${bb.buys ? " (earlier buys fell out of the Solana RPC's ledger before it saw them; the totals above still count them)" : ""}.</p>`;
  return card(
    "Storefront buyback",
    `${kv([
      ["Share of each order", `<span class="num">${(bb.share_bps / 100).toString()}%</span> of SOL in`],
      ["SOL accrued", `<span class="num">${sol(bb.accrued_lamports)}</span> ${S()}`],
      ["SOL spent on buys", `<span class="num">${sol(bb.spent_lamports)}</span> ${S()} <span class="faint">in ${fmtInt(bb.buys)} buy${bb.buys === 1 ? "" : "s"}</span>`],
      ["Tokens bought", `<span class="num">${spl(bb.tokens_bought_spl)}</span> ${T()}`],
      ["To the mining reserve", `<span class="num">${spl(bb.to_reserve_spl)}</span> ${T()}`],
      ["Burned", `<span class="num">${spl(bb.burned_spl)}</span> ${T()}`],
    ])}${list}`,
    { aside: `<a class="more-s" href="/api/buyback">JSON</a>` },
  );
}

function statusBadge(s: string) {
  const tone: Record<string, any> = { completed: "ok", finalized: "ok", open: "accent", pending: "accent", delivered: "accent", expired: "warn", refunded: "warn" };
  return badge(tone[s] ?? "neutral", s);
}

export function payersCard(pm: any) {
  const rows = pm.payers.slice(0, 10);
  const body = rows.length
    ? `<div class="table t-payers"><div class="tr th"><span>Payer</span><span>Jobs</span><span>Protocol fees P</span><span>SOL via storefront</span></div>${rows
        .map(
          (r: any) =>
            `<div class="tr"><span>${id(r.payer, { head: 6, tail: 6 })}<div class="share" aria-hidden="true"><span style="width:${(r.share_ppm / 10_000).toFixed(2)}%"></span></div></span><span class="num">${r.jobs}${r.storefront_jobs ? ` <span class="faint">(${r.storefront_jobs} order${r.storefront_jobs === 1 ? "" : "s"})</span>` : ""}</span><span class="num">${tok(r.protocol_fees, 2)} <span class="faint">${esc(pct(r.share_ppm, 1_000_000))}</span></span><span class="num">${big(r.sol_spent) > 0n ? sol(r.sol_spent) : `<span class="faint">–</span>`}</span></div>`,
        )
        .join("")}</div><p class="note">Ranked by protocol fees paid. One payer holding most of P means the demand is concentrated: read it next to the unique-payer count.</p>`
    : `<p class="note">No job has been paid for yet.</p>`;
  return card("Who pays", body, { cls: "payers", aside: `<a class="more-s" href="/api/payers">JSON</a>` });
}

export function homePage(sm: Summary, feed: any, jobs: any, orders: any, buyback: any, pm?: any) {
  return page(
    "Every block, and who paid for it",
    `<div class="intro"><h1>Every block, and who paid for it</h1><p>Each block of the agent chain mines one job or none. This page shows, for every block, the job it mined, who paid for it on Solana, the fee, the subsidy it earned and whether the Solana light client accepted its proof.</p></div>
${heroAndStats(sm)}
<div class="grid">
  <div class="col-main">${feedPanel(feed, !feed.before)}${pm ? payersCard(pm) : ""}</div>
  ${rail(sm, jobs, orders, buyback)}
</div>`,
    { active: "feed", summary: sm, live: feed.before ? undefined : "home" },
  );
}

export function blocksPage(sm: Summary, feed: any) {
  return page(
    "Blocks",
    `<nav class="crumbs"><a href="/">Overview</a><span>/</span><span>Blocks</span></nav>${feedPanel(feed, !feed.before)}`,
    { active: "feed", summary: sm, live: feed.before ? undefined : "blocks" },
  );
}

// ------------------------------------------------------------ block page

export function blockPage(sm: Summary, b: any) {
  const paid = b.kind === "paid_job";
  const o = b.order;
  const j = b.job;
  const who = paid
    ? kv([
        ["Job", id(b.job_id, { href: `/job/${b.job_id}`, head: 10, tail: 8 })],
        ["Paid by", id(b.paid_by, { head: 8, tail: 8 })],
        ["Paid with", o ? `Storefront order <a href="/order/${esc(o.nonce)}">#${esc(o.nonce)}</a> on Solana` : `PostJob on the chain, in ${esc(BRAND.ticker)}`],
        ...(o
          ? ([
              ["SOL spent", `<span class="num">${sol(o.sol_spent, 6)}</span> ${S()}`],
              ["Buy tx", o.buy_tx ? id(o.buy_tx.sig, { href: solTxHref(o.buy_tx.sig), head: 8, tail: 6 }) : `<span class="faint">not recorded (Solana ledger pruned before the explorer saw it)</span>`],
              ["x402 payment", o.x402_payment ? `${id(o.x402_payment.sig, { href: solTxHref(o.x402_payment.sig), head: 8, tail: 6 })} <span class="faint num">${sol(o.x402_payment.lamports, 6)} SOL</span>` : o.payer === o.buyer ? `<span class="faint">bought directly (buyer is the payer)</span>` : `<span class="faint">not recorded</span>`],
            ] as [string, string][])
          : []),
        ["Worker pay W", `<span class="num">${tok(j?.worker_pay)}</span> ${T()}`],
        ["Protocol fee P", `<span class="num">${tok(j?.protocol_fee)}</span> ${T()} <span class="faint">to the mining pool</span>`],
      ])
    : `<p class="lead">Fallback block. No job was mined: the miner solved a fixed fallback matrix, so this block earns its tx fees and no subsidy from the reserve. It is shown as fallback, never as paid work.</p>`;
  const reward = kv([
    ["Tx fees", `<span class="num">${tok(b.fees)}</span> ${T()}`],
    ["Subsidy", `<span class="num">${tok(b.subsidy)}</span> ${T()}`],
    ...(paid
      ? ([
          ["Job subsidy after this block", `<span class="num">${tok(b.job_subsidy_after_block)}</span> ${T()}`],
          ["Cap alpha x P", `<span class="num">${tok(b.subsidy_cap)}</span> ${T()}`],
          ["Within cap", check(b.within_cap, "yes, subsidy ≤ alpha x P", "no")],
        ] as [string, string][])
      : []),
    ["Block reward", `<span class="num">${tok(b.reward)}</span> ${T()}`],
    ["Matures at", b.coinbase ? `<span class="num">#${fmtInt(b.coinbase.mature_height)}</span>` : "–"],
  ]);
  const proof = kv([
    ["Tile proof hash", id(b.proof_hash, { head: 10, tail: 10 })],
    ["Proof size", `<span class="num">${fmtInt(b.proof_len)}</span> bytes`],
    ["Raw proof", `<a href="${esc(b.proof_raw)}">base64</a> · <a href="${esc(b.proof_raw)}?format=bin">binary</a>`],
    ["Miner", id(b.miner, { head: 8, tail: 8 })],
    ["Target bits", `<span class="num">0x${(b.target_bits >>> 0).toString(16)}</span>`],
    ["Block hash", id(b.hash, { head: 10, tail: 10 })],
    ["Parent", b.height > 1 ? id(b.prev_hash, { href: `/block/${b.height - 1}`, head: 10, tail: 10 }) : id(b.prev_hash)],
  ]);
  const lc = `<div class="lc-big">${lcPill({ ...b.light_client, behind: 0 })}<span class="lc-label">${esc(b.light_client.label.replace(/\d{4,}/g, (n: string) => fmtInt(Number(n))))}</span></div>${kv([["Header record", id(b.light_client_header_pda, { head: 8, tail: 8 })]])}${solTxList(b.light_client_txs, b.light_client_txs_note ?? (b.light_client.state === "not-relayed" ? "Not relayed to Solana yet." : "Not recorded: the Solana RPC had pruned them before the explorer first saw this header."))}`;
  const settle = o
    ? kv([
        ["Order status", statusBadge(o.status)],
        ["Settled by", o.settlement.mode === "proof" ? `${badge("ok", "proof")} <span class="faint">${esc(o.settlement.tx?.ix.join(", ") ?? "")}</span>` : o.settlement.mode === "signers" ? badge("neutral", "signers") : `<span class="faint">${o.status === "pending" ? "not settled yet" : "tx not recorded"}</span>`],
        ["Settlement tx", o.settlement.tx ? id(o.settlement.tx.sig, { href: solTxHref(o.settlement.tx.sig), head: 8, tail: 6 }) : `<span class="faint">–</span>`],
      ]) + waitNote(b.settlement_wait)
    : "";
  const txs = b.txs.length
    ? `<ul class="chaintx">${b.txs
        .map(
          (t: any) =>
            `<li><span class="ix">${esc(t.kind)}</span>${t.links.job ? `<a href="/job/${esc(t.links.job)}">job ${esc(short(t.links.job, 6, 4))}</a>` : ""}${t.links.order ? `<a href="/order/${esc(t.links.order)}">order #${esc(t.links.order)}</a>` : ""}<span class="faint">from</span> ${id(t.from, { head: 4, tail: 4 })}${id(t.hash, { head: 6, tail: 6 })}</li>`,
        )
        .join("")}</ul>`
    : `<p class="note">No transactions besides the coinbase receipt.</p>`;
  const wd = b.withdrawals.length
    ? card(
        "Bridge withdrawals in this block",
        `<ul class="chaintx">${b.withdrawals
          .map(
            (w: any) =>
              `<li><span class="ix">#${w.nonce}</span><span class="num">${spl(w.amount_spl)}</span> ${T()} <span class="faint">to</span> ${id(w.solana_recipient, { head: 4, tail: 4 })} ${w.released ? badge("ok", w.release_mode.mode === "proof" ? "released by proof" : w.release_mode.mode === "signers" ? "released by signers" : "released") : badge("neutral", "not released yet")}${w.release_mode.tx ? id(w.release_mode.tx.sig, { href: solTxHref(w.release_mode.tx.sig), head: 6, tail: 6 }) : ""}</li>`,
          )
          .join("")}</ul>`,
      )
    : "";
  return page(
    `Block ${b.height}`,
    `<nav class="crumbs"><a href="/">Blocks</a><span>/</span><span class="num">${fmtInt(b.height)}</span></nav>
<div class="title-row"><h1>Block <span class="num">${fmtInt(b.height)}</span></h1>${paid ? badge("paid", "Paid job") : badge("fallback", "Fallback")}${b.final ? badge("ok", "final") : badge("neutral", `${b.confirmations ?? 0} confirmations`)}${b.canonical ? "" : badge("bad", "not canonical")}<span class="title-t">${time(b.time_ms)}</span>
<span class="pager">${b.height > 1 ? `<a href="/block/${b.height - 1}" aria-label="Previous block">Prev</a>` : ""}<a href="/block/${b.height + 1}" aria-label="Next block">Next</a><a href="/api/block/${b.height}">JSON</a></span></div>
<div class="cards">
  ${card(paid ? "Who paid" : "No paid job", who, { cls: paid ? "card-accent" : "" })}
  ${card("Reward and the alpha x P bound", reward)}
  ${card("Proof", proof)}
  ${card("Solana light client", lc)}
  ${o ? card("Result settlement on Solana", settle) : ""}
  ${card(`Transactions (${b.txs.length})`, txs)}
  ${wd}
</div>`,
    { active: "feed", summary: sm },
  );
}

// ------------------------------------------------------------ job page

export function jobPage(sm: Summary, j: any) {
  const o = j.order;
  const capPct = big(j.protocol_fee) > 0n ? Number((big(j.subsidy_paid) * 10_000n) / big(j.protocol_fee)) / 100 : 0;
  const alphaPct = big(j.protocol_fee) > 0n ? Number((big(j.subsidy_cap) * 10_000n) / big(j.protocol_fee)) / 100 : 0;
  const bound = `<div class="bound" role="img" aria-label="Subsidy paid is ${capPct}% of P; the cap is ${alphaPct}% of P">
    <div class="bound-bar"><span class="bound-fill" style="width:${Math.min(capPct, 100)}%"></span><span class="bound-cap" style="left:${Math.min(alphaPct, 100)}%"></span></div>
    <div class="bound-legend"><span><i class="sw sw-paid"></i>Subsidy paid <span class="num">${capPct}%</span> of P</span><span><i class="sw sw-cap"></i>Cap alpha x P <span class="num">${alphaPct}%</span></span><span><i class="sw sw-fb"></i>Kept by the pool</span></div>
  </div>`;
  const terms = kv([
    ["Paid by", id(j.poster, { head: 8, tail: 8 })],
    ["Worker pay W", `<span class="num">${tok(j.worker_pay)}</span> ${T()}`],
    ["Protocol fee P", `<span class="num">${tok(j.protocol_fee)}</span> ${T()} <span class="faint">to the mining pool</span>`],
    ["Subsidy paid", `<span class="num">${tok(j.subsidy_paid)}</span> ${T()}`],
    ["Cap alpha x P", `<span class="num">${tok(j.subsidy_cap)}</span> ${T()}`],
    ["Within cap", check(j.within_cap, "yes", "no")],
    ["Blocks mined", `<span class="num">${j.blocks_mined}</span> of <span class="num">${j.max_blocks}</span> max`],
    ["Shape", `<span class="num">${j.m} x ${j.n} x ${j.k}</span>`],
    ["Posted at", `<a href="/block/${j.posted_height}" class="num">#${fmtInt(j.posted_height)}</a>`],
    ["Closed at", j.closed_height ? `<a href="/block/${j.closed_height}" class="num">#${fmtInt(j.closed_height)}</a>` : `<span class="faint">open until #${fmtInt(j.deadline_height)}</span>`],
  ]);
  const pay = o
    ? kv([
        ["Storefront order", `<a href="/order/${esc(o.nonce)}">#${esc(o.nonce)}</a> ${statusBadge(o.status)}`],
        ["SOL spent", `<span class="num">${sol(o.sol_spent, 6)}</span> ${S()}`],
        ["Tokens bought", `<span class="num">${spl(o.tokens_spl)}</span> ${T()} <span class="faint">(W + P, ${esc(o.venue)})</span>`],
        ["Solana buyer", id(o.buyer, { head: 6, tail: 6 })],
        ["Buy tx", o.buy_tx ? id(o.buy_tx.sig, { href: solTxHref(o.buy_tx.sig), head: 8, tail: 6 }) : `<span class="faint">not recorded (Solana ledger pruned before the explorer saw it)</span>`],
        ["x402 payment", o.x402_payment ? `${id(o.x402_payment.sig, { href: solTxHref(o.x402_payment.sig), head: 8, tail: 6 })} <span class="faint num">${sol(o.x402_payment.lamports, 6)} SOL</span>` : `<span class="faint">${o.payer === o.buyer ? "bought directly" : "not recorded"}</span>`],
        ["Result on Solana", o.settlement.mode === "proof" ? `${badge("ok", "settled by proof")} ${o.settlement.tx ? id(o.settlement.tx.sig, { href: solTxHref(o.settlement.tx.sig), head: 6, tail: 6 }) : ""}` : o.settlement.mode === "signers" ? badge("neutral", "settled by signers") : `<span class="faint">${o.status === "pending" ? "not settled yet" : "settlement tx not recorded"}</span>`],
      ])
    : `<p class="lead">Posted directly on the agent chain with a PostJob tx: the poster paid W + P in ${esc(BRAND.ticker)}. No Solana order.</p>`;
  const blocks = j.blocks.length
    ? `<div class="table t-blocks"><div class="tr th"><span>Block</span><span>Subsidy</span><span>Job total after</span><span>Light client</span><span>Proof</span></div>${j.blocks
        .map(
          (b: any) =>
            `<a class="tr" href="/block/${b.height}"><span class="num">#${fmtInt(b.height)}</span><span class="num">${tok(b.subsidy, 2)}</span><span class="num chk-inline">${tok(b.job_subsidy_after_block, 2)} ${check(b.within_cap, "", "")}</span><span>${lcPill(b.light_client)}</span><span class="hash">${esc(short(b.proof_hash, 6, 4))}</span></a>`,
        )
        .join("")}</div>`
    : `<p class="note">${j.blocks_mined > 0 ? "Its blocks are not indexed yet (backfill in progress)." : "Not mined yet. Miners pick the open job with the most subsidy left."}</p>`;
  const txs = j.chain_txs.length
    ? `<ul class="chaintx">${j.chain_txs
        .map((t: any) => `<li><span class="ix">${esc(t.kind === "BridgeUnlock" ? "BridgeUnlock (order)" : t.kind)}</span><a class="num" href="/block/${t.height}">#${fmtInt(t.height)}</a><span class="faint">from</span> ${id(t.from, { head: 4, tail: 4 })}${id(t.hash, { head: 6, tail: 6 })}</li>`)
        .join("")}</ul>`
    : `<p class="note">No chain txs indexed for this job yet.</p>`;
  const data = kv([
    ["Data", esc(j.data_uri) || "–"],
    ["hash_a", id(j.hash_a, { head: 8, tail: 8 })],
    ["hash_b", id(j.hash_b, { head: 8, tail: 8 })],
    ["Job key", id(j.job_key, { head: 8, tail: 8 })],
    ["Deliverer", id(j.deliverer, { head: 6, tail: 6 })],
    ["Result hash", id(j.result_hash, { head: 8, tail: 8 })],
    ["Result", esc(j.result_uri) || `<span class="faint">none yet</span>`],
    ["Evaluators", j.evaluators.map((e: string) => id(e, { head: 4, tail: 4 })).join(" ") + ` <span class="faint">threshold ${j.threshold}</span>`],
  ]);
  return page(
    `Job ${j.job_id.slice(0, 8)}`,
    `<nav class="crumbs"><a href="/jobs">Jobs</a><span>/</span><span class="hash">${esc(short(j.job_id, 8, 6))}</span></nav>
<div class="title-row"><h1>Job <span class="hash" title="${esc(j.job_id)}">${esc(short(j.job_id, 8, 6))}</span></h1>${statusBadge(j.status)}${j.paid_via === "storefront" ? badge("paid", "Paid via storefront") : badge("accent", "Paid with PostJob")}<span class="pager"><a href="/api/job/${esc(j.job_id)}">JSON</a></span></div>
<div class="cards">
  ${card("Fee, subsidy and the bound", bound + terms, { cls: "card-accent" })}
  ${card("Who paid", pay + waitNote(j.settlement_wait))}
  ${card(`Blocks mined (${j.blocks.length})`, blocks)}
  ${card("Chain transactions", txs)}
  ${card("Data and result", data)}
</div>`,
    { active: "jobs", summary: sm },
  );
}

// ------------------------------------------------------------ order page

export function orderPage(sm: Summary, o: any) {
  const body = kv([
    ["Status", statusBadge(o.status)],
    ["Payer (chain poster)", id(o.payer, { head: 8, tail: 8 })],
    ["Solana buyer", id(o.buyer, { head: 8, tail: 8 })],
    ["SOL spent", `<span class="num">${sol(o.sol_spent, 6)}</span> ${S()}`],
    ["Tokens bought", `<span class="num">${spl(o.tokens_spl)}</span> ${T()} <span class="faint">on the ${esc(o.venue)}</span>`],
    ["Worker pay W", `<span class="num">${tok(o.worker_pay)}</span> ${T()}`],
    ["Protocol fee P", `<span class="num">${tok(o.protocol_fee)}</span> ${T()}`],
    ["Chain job", o.on_chain_job ? id(o.job_id, { href: `/job/${o.job_id}`, head: 8, tail: 8 }) : `${id(o.job_id, { head: 8, tail: 8 })} <span class="faint">${o.status === "refunded" ? "never unlocked: refunded in tokens on Solana" : "not on the chain yet"}</span>`],
    ["Shape", `<span class="num">${o.shape.m} x ${o.shape.n} x ${o.shape.k}</span>, up to <span class="num">${o.shape.max_blocks}</span> blocks within <span class="num">${o.shape.deadline_blocks}</span>`],
    ["Order account", id(o.address, { head: 8, tail: 8 })],
    ["Created", `slot <span class="num">${fmtInt(o.created_slot)}</span> · ${time(o.created_ts * 1000)}`],
    ["Settlement", o.settlement.mode === "proof" ? badge("ok", "by proof against a light-client header") : o.settlement.mode === "signers" ? badge("neutral", "by signers") : `<span class="faint">${o.status === "pending" ? "not settled yet" : "tx not recorded"}</span>`],
    ["Result hash", id(o.result_hash, { head: 8, tail: 8 })],
  ]);
  return page(
    `Order ${o.nonce}`,
    `<nav class="crumbs"><a href="/orders">Orders</a><span>/</span><span>#${esc(o.nonce)}</span></nav>
<div class="title-row"><h1>Storefront order <span class="num">#${esc(o.nonce)}</span></h1>${statusBadge(o.status)}<span class="pager"><a href="/api/order/${esc(o.nonce)}">JSON</a></span></div>
<div class="cards">
  ${card("Order", body + waitNote(o.settlement_wait), { cls: "card-accent" })}
  ${card("Solana transactions", solTxList(o.solana_txs, "Not recorded: the Solana RPC had pruned this order's txs before the explorer first saw it.") + (o.x402_payment ? `<p class="note">x402 payment: ${id(o.x402_payment.sig, { href: solTxHref(o.x402_payment.sig), head: 8, tail: 6 })}, <span class="num">${sol(o.x402_payment.lamports, 6)}</span> SOL from the payer to the storefront wallet at slot <span class="num">${fmtInt(o.x402_payment.slot)}</span>.</p>` : ""))}
</div>`,
    { active: "orders", summary: sm },
  );
}

// ------------------------------------------------------------ lists

export function jobsPage(sm: Summary, m: any) {
  const rows = m.jobs.length
    ? `<div class="table t-jobs"><div class="tr th"><span>Job</span><span>Status</span><span>Paid with</span><span>P</span><span>Subsidy / cap</span><span>Blocks</span><span>Posted</span></div>${m.jobs
        .map(
          (j: any) =>
            `<a class="tr" href="/job/${esc(j.job_id)}"><span class="hash">${esc(short(j.job_id, 8, 6))}</span><span>${statusBadge(j.status)}</span><span>${j.paid_via === "storefront" ? `order #${esc(j.order_nonce)} · <span class="num">${sol(j.sol_spent)}</span> SOL` : "PostJob"}</span><span class="num">${tok(j.protocol_fee, 2)}</span><span class="num">${tok(j.subsidy_paid, 2)} / ${tok(j.subsidy_cap, 2)} ${check(j.within_cap, "", "")}</span><span class="num">${j.blocks_mined}/${j.max_blocks}</span><span class="num">#${fmtInt(j.posted_height)}</span></a>`,
        )
        .join("")}</div>`
    : `<div class="empty"><strong>No jobs yet.</strong><span>Jobs appear when a storefront order unlocks on the chain or someone sends PostJob.</span></div>`;
  return page("Jobs", `<nav class="crumbs"><a href="/">Overview</a><span>/</span><span>Jobs</span></nav><section class="card">${`<header class="card-h"><h2>All jobs (${fmtInt(m.total)})</h2><a class="more-s" href="/api/jobs">JSON</a></header>`}${rows}</section>`, { active: "jobs", summary: sm });
}

export function ordersPage(sm: Summary, m: any) {
  const rows = m.orders.length
    ? `<div class="table t-orders"><div class="tr th"><span>Order</span><span>Status</span><span>Payer</span><span>SOL</span><span>Tokens</span><span>Settlement</span></div>${m.orders
        .map(
          (o: any) =>
            `<a class="tr" href="/order/${esc(o.nonce)}"><span>#${esc(o.nonce)}</span><span>${statusBadge(o.status)}</span><span class="hash">${esc(short(o.payer, 4, 4))}</span><span class="num">${sol(o.sol_spent)}</span><span class="num">${spl(o.tokens_spl)}</span><span>${o.settlement.mode === "proof" ? "by proof" : o.settlement.mode === "signers" ? "by signers" : `<span class="faint">${o.status === "pending" ? "pending" : o.status === "refunded" ? "refunded" : "not recorded"}</span>`}</span></a>`,
        )
        .join("")}</div>`
    : `<div class="empty"><strong>No storefront orders yet.</strong><span>Agents buy jobs over x402 at the storefront service (POST /jobs).</span></div>`;
  return page("Orders", `<nav class="crumbs"><a href="/">Overview</a><span>/</span><span>Orders</span></nav><section class="card"><header class="card-h"><h2>Storefront orders (${fmtInt(m.total)})</h2><a class="more-s" href="/api/orders">JSON</a></header>${rows}</section>`, { active: "orders", summary: sm });
}

export function apiPage(sm: Summary) {
  const eps: [string, string][] = [
    ["/api/summary", "Totals: blocks on paid jobs vs fallback, SOL settled, P, subsidy vs P, unique payers, light client, index progress."],
    ["/api/feed?filter=all|paid|fallback&before=H&limit=N", "Blocks newest first. In `all`, runs of fallback blocks are grouped."],
    ["/api/block/{height|hash}", "One block: job, payer, order, W, P, subsidy and cap, proof, light-client status and txs, chain txs, withdrawals."],
    ["/api/block/{height}/proof", "The raw tile proof (base64 JSON; ?format=bin for bytes)."],
    ["/api/jobs", "Every job with its fee, subsidy, cap check and how it was paid."],
    ["/api/job/{job_id or prefix}", "One job: blocks mined with cumulative subsidy, chain txs, Solana order and settlement."],
    ["/api/orders", "Every storefront Order on Solana."],
    ["/api/order/{nonce}", "One Order with the Solana txs the explorer recorded."],
    ["/api/sol/tx/{signature}", "The Solana tx as the RPC returns it (getTransaction), if still in its ledger."],
    ["/api/payers", "Payers ranked by protocol fees paid, with job counts and SOL through the storefront."],
    ["/api/buyback", "Storefront buyback events, or null while none exist."],
  ];
  return page(
    "API",
    `<nav class="crumbs"><a href="/">Overview</a><span>/</span><span>API</span></nav>
<div class="intro"><h1>JSON API</h1><p>Every page renders one of these objects, so an agent reads exactly what a person sees. Amounts are decimal strings in base units: ${esc(BRAND.ticker)} on the agent chain and SOL both have 9 decimals, the Solana SPL token 6. GET only, no keys.</p></div>
<section class="card"><ul class="api">${eps.map(([p, d]) => `<li><a href="${esc(p.replace(/\{.*$|\?.*$/, ""))}">${esc(p)}</a><span>${esc(d)}</span></li>`).join("")}</ul></section>`,
    { active: "api", summary: sm },
  );
}

export function notFound(sm: Summary | null, what: string) {
  return page("Not found", `<div class="empty big"><strong>${esc(what)} not found.</strong><span>It may not be indexed yet, or the chain was restarted (a new genesis starts a new index). <a href="/">Back to the latest blocks</a>.</span></div>`, { summary: sm });
}
