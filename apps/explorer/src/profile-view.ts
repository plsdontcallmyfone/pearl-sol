// /agents and /agent/{address}: agent profiles (models in profile.ts / profile-route.ts).
// Same primitives and shell as views.ts; Geist only, no monospace.

import { BRAND } from "./brand.ts";
import { fmtInt, pct } from "./math.ts";
import type { Summary } from "./models.ts";
import { S, T, badge, card, esc, id, kv, page, sol, tok } from "./views.ts";

const share = (ppm: number | null | undefined) => (ppm === null || ppm === undefined ? `<span class="faint">none</span>` : `<span class="num">${esc(pct(ppm, 1_000_000))}</span>`);
const n = (v: number) => `<span class="num">${fmtInt(v)}</span>`;
const P = (v: string | null | undefined) => (v === null || v === undefined ? `<span class="faint">none</span>` : `<span class="num">${tok(v, 2)}</span> ${T()}`);
const L = (v: string) => `<span class="num">${sol(v)}</span> ${S()}`;
const jobs = (k: number) => `${n(k)} job${k === 1 ? "" : "s"}`;
const blocks = (v: number | null) => (v === null ? `<span class="faint">none</span>` : `<span class="num">${fmtInt(v)}</span> blocks`);

const STYLE = `<style>
.t-agent .tr { grid-template-columns: minmax(0, 1.3fr) minmax(0, 1fr) 1fr 1fr minmax(0, 1.4fr); }
.t-agents .tr { grid-template-columns: minmax(0, 1.5fr) 1fr 1fr 80px 90px 90px; }
.stats + .cards { margin-top: 16px; }
.intro .idwrap.full { max-width: 100%; display: inline-flex; min-width: 0; }
.intro .idwrap.full .hash { overflow-wrap: anywhere; word-break: break-all; min-width: 0; }
.signers { display: flex; flex-wrap: wrap; gap: 4px 10px; align-items: center; }
.card-h .health { margin-left: auto; }
.skills { margin: 6px 16px 14px; padding: 0; list-style: none; display: grid; gap: 6px; }
.skills li { color: var(--dim); font-size: 13px; }
.skills strong { color: var(--text); font-weight: 600; }
@media (max-width: 760px) { .t-agent .tr, .t-agents .tr { grid-template-columns: minmax(0, 1fr) auto; } }
</style>`;

function stat(label: string, value: string, sub: string) {
  return `<div class="stat"><div class="eyebrow">${esc(label)}</div><div class="stat-v">${value}</div><div class="stat-s">${sub}</div></div>`;
}

const HEALTH: Record<string, [any, string]> = {
  ok: ["ok", "card live"],
  failing: ["warn", "card failing"],
  stale: ["warn", "card stale"],
  none: ["neutral", "no card"],
};

function cardCard(c: any) {
  const [tone, label] = HEALTH[c?.health ?? "none"];
  const aside = `<span class="health">${badge(tone, label)}</span>`;
  if (!c?.uri) return card("Agent card", `<p class="note">The agent registered no URI (RegisterAgent.uri), so there is no card to show. Its record below comes from chain state either way.</p>`, { aside });
  const k = c.card;
  const rows: [string, string][] = [
    ["URI", `<a href="${esc(c.uri)}" rel="nofollow noopener">${esc(c.uri)}</a>`],
    ["Last probe", c.checked_at ? `${esc(new Date(c.checked_at).toISOString().replace("T", " ").slice(0, 19))} UTC${c.error ? ` <span class="faint">(${esc(c.error)})</span>` : ""}` : `<span class="faint">not probed</span>`],
    ["Last good answer", c.last_ok_at ? `${esc(new Date(c.last_ok_at).toISOString().replace("T", " ").slice(0, 19))} UTC` : `<span class="faint">none</span>`],
    ["In lists", c.listed ? badge("ok", "listed") : badge("warn", "hidden as stale")],
  ];
  if (k) {
    rows.push(["Name on the card", esc(k.name)]);
    if (k.description) rows.push(["Description", esc(k.description)]);
    if (k.url) rows.push(["Endpoint", `<a href="${esc(k.url)}" rel="nofollow noopener">${esc(k.url)}</a>`]);
    for (const i of k.interfaces ?? []) rows.push([`Interface${i.transport ? ` (${i.transport})` : ""}`, `<a href="${esc(i.url)}" rel="nofollow noopener">${esc(i.url)}</a>`]);
    if (k.version) rows.push(["Version", esc(k.version)]);
    if (k.provider) rows.push(["Provider", esc(k.provider)]);
    if (k.capabilities?.length) rows.push(["Capabilities", esc(k.capabilities.join(", "))]);
    rows.push(["Signed card", k.signed ? "yes (signatures present, not verified here)" : "no"]);
  }
  const skills = k?.skills?.length
    ? `<ul class="skills">${k.skills.map((s: any) => `<li><strong>${esc(s.name ?? s.id ?? "skill")}</strong>${s.description ? `: ${esc(s.description)}` : ""}</li>`).join("")}</ul>`
    : "";
  return card("Agent card", kv(rows) + skills + `<p class="note">${esc(c.listed_reason.charAt(0).toUpperCase() + c.listed_reason.slice(1))}. The card is what the agent says about itself: display only, never part of the record.</p>`, { aside });
}

function signers(list: string[] | null, poster: string, deliverer: string) {
  if (list === null) return `<span class="faint">CompleteJob not indexed</span>`;
  if (!list.length) return `<span class="faint">none</span>`;
  return `<span class="signers">${list.map((k) => `${id(k, { href: `/agent/${k}`, head: 4, tail: 4 })}${k === poster ? badge("warn", "buyer") : k === deliverer ? badge("warn", "seller") : ""}`).join("")}</span>`;
}

export function agentPage(sm: Summary, p: any) {
  const h = p.headline;
  const sv = p.server;
  const name = p.agent?.name ? esc(p.agent.name) : "Unregistered account";
  const top = sv.top_payer;
  const stats = `<section class="stats">
  ${stat("P behind completed work", P(h.p_behind_completed_work), `${jobs(h.completed_jobs)}, self-dealing excluded`)}
  ${stat("SOL via storefront", L(h.storefront_sol_behind_completed_work), `${n(h.storefront_orders)} finalized order${h.storefront_orders === 1 ? "" : "s"} behind that work`)}
  ${stat("Distinct payers", n(sv.distinct_payers), top ? `top payer ${share(top.share_ppm)} of that P` : "no payer yet")}
  ${stat("Accepted, by P", share(sv.acceptance_by_p_ppm), `independently signed ${share(sv.independent_evaluation.share_ppm)}`)}
</section>`;
  const sd = sv.self_dealt_excluded;
  const record = kv([
    ["Work pay W earned", P(h.w_earned)],
    ["Self-dealing excluded", `${P(sd.protocol_fees)} over ${jobs(sd.jobs)}${sd.share_ppm_of_completed_p !== null ? `, ${share(sd.share_ppm_of_completed_p)} of completed P` : ""}${sd.heuristic_protocol_fees !== "0" ? ` <span class="faint">(of which ${tok(sd.heuristic_protocol_fees, 2)} by the funding heuristic)</span>` : ""}`],
    ["Rejected by evaluators", `${P(sv.rejected.protocol_fees)} over ${jobs(sv.rejected.jobs)}${sv.rejected.chain_tally ? ` <span class="faint">(chain tally: ${fmtInt(sv.rejected.chain_tally.rejected)})</span>` : ""}`],
    ["Delivered, then expired", `${P(sv.delivered_then_expired.protocol_fees)} over ${jobs(sv.delivered_then_expired.jobs)}`],
    ["Awaiting evaluation", `${P(sv.awaiting_evaluation.protocol_fees)} over ${jobs(sv.awaiting_evaluation.jobs)}`],
    ["Signed by a third-party evaluator", `${P(sv.independent_evaluation.protocol_fees)}${sv.independent_evaluation.signers_not_indexed_protocol_fees !== "0" ? ` <span class="faint">(${tok(sv.independent_evaluation.signers_not_indexed_protocol_fees, 2)} with signers not indexed)</span>` : ""}`],
    ["Time to completion", `p50 ${blocks(sv.time_to_completion_blocks.p50)}, p90 ${blocks(sv.time_to_completion_blocks.p90)}`],
    ["Time to delivery", `p50 ${blocks(sv.time_to_delivery_blocks.p50)}, p90 ${blocks(sv.time_to_delivery_blocks.p90)}`],
    ["Last delivery", sv.last_delivery ? `<a href="/block/${sv.last_delivery.height}">#${fmtInt(sv.last_delivery.height)}</a>` : `<span class="faint">none</span>`],
  ]);
  const done = sv.completed.length
    ? `<div class="table t-agent"><div class="tr th"><span>Job</span><span>Payer</span><span>P</span><span>SOL</span><span>Signed by</span></div>${sv.completed
        .map(
          (j: any) =>
            `<div class="tr"><span>${id(j.job_id, { href: `/job/${j.job_id}` })}${j.self_dealt ? ` ${badge("warn", "excluded")}` : ""}</span><span>${id(j.poster, { href: `/agent/${j.poster}` })}</span><span class="num">${tok(j.protocol_fee, 2)}</span><span class="num">${j.storefront_sol_lamports === "0" ? `<span class="faint">none</span>` : sol(j.storefront_sol_lamports)}</span><span>${signers(j.signers, j.poster, p.account)}</span></div>`,
        )
        .join("")}</div>`
    : `<p class="note">No completed work yet.</p>`;
  const rej = sv.rejected.list.length
    ? `<div class="table t-agent"><div class="tr th"><span>Job</span><span>Payer</span><span>P</span><span>Block</span><span>Rejected by</span></div>${sv.rejected.list
        .map(
          (r: any) =>
            `<div class="tr"><span>${id(r.job_id, { href: `/job/${r.job_id}` })}</span><span>${id(r.poster)}</span><span class="num">${r.protocol_fee === null ? "none" : tok(r.protocol_fee, 2)}</span><span><a href="/block/${r.height}">#${fmtInt(r.height)}</a></span><span>${signers(r.signers, r.poster ?? "", p.account)}</span></div>`,
        )
        .join("")}</div>`
    : `<p class="note">No delivery of this account was rejected (RejectResult).</p>`;
  const m = p.miner;
  const b = p.buyer;
  const e = p.evaluator;
  return page(
    `Agent ${p.agent?.name ?? p.account.slice(0, 8)}`,
    `${STYLE}<nav class="crumbs"><a href="/">Overview</a><span>/</span><a href="/agents">Agents</a><span>/</span><span>${name}</span></nav>
<div class="intro"><h1>${name}</h1><p>${id(p.account, { full: true })}</p><p>The record of this account in every role it played, read from chain state: money behind completed work first, with self-dealing excluded and failures shown next to it. Amounts are ${esc(BRAND.ticker)} and SOL, no USD. Counts appear only beside the amount they carry.</p></div>
${stats}
<div class="cards">
  ${card("As seller", record, { aside: `<a class="more-s" href="/api/agent/${esc(p.account)}">JSON</a>` })}
  ${card("Completed work and who signed it", done, { cls: "wide" })}
  ${card("Rejected deliveries", rej, { cls: "wide" })}
  ${card(
    "As miner",
    kv([
      ["Job blocks", `${n(m.job_blocks)}, subsidy ${P(m.subsidy)}, fees ${P(m.fees_job_blocks)}`],
      ["Fallback blocks", `${n(m.fallback_blocks)}, fees ${P(m.fees_fallback_blocks)}`],
      ["Distinct jobs mined", `${n(m.distinct_jobs)} for ${n(m.distinct_payers_mined_for)} payer${m.distinct_payers_mined_for === 1 ? "" : "s"}`],
      ...m.segment_blocks_by_model.map((x: any) => [`Segment blocks, model ${x.model_id.slice(0, 8)}`, n(x.blocks)] as [string, string]),
    ]),
  )}
  ${card(
    "As buyer",
    kv([
      ["Posted", `${P(b.protocol_fees)} P and ${P(b.worker_pay)} W over ${jobs(b.jobs)}`],
      ["Completed", P(b.completed_p)],
      ["Expired without a delivery", `${P(b.expired_without_delivery_p)}${b.expired_without_delivery_share_ppm !== null ? `, ${share(b.expired_without_delivery_share_ppm)} of posted P` : ""}`],
      ["Still open", P(b.open_p)],
    ]),
  )}
  ${card(
    "As evaluator",
    kv([
      ["Completions signed", `${P(e.protocol_fees)} over ${jobs(e.jobs_signed)}`],
      ["Distinct sellers and buyers", `${n(e.distinct_sellers)} sellers, ${n(e.distinct_buyers)} buyers`],
      ["Rejections signed", `${P(e.rejects_signed_p)} over ${jobs(e.rejects_signed)}`],
      ["On its own jobs", e.own_job_share_ppm === null ? `<span class="faint">none</span>` : `${share(e.own_job_share_ppm)} of the P it signed`],
    ]),
  )}
  ${cardCard(p.card)}
</div>
<p class="note">Sources: every figure above is listed with its chain source in the JSON (<a href="/api/agent/${esc(p.account)}">sources</a>). Index: ${fmtInt(p.indexed.blocks)} blocks.</p>`,
    { active: "agents", summary: sm },
  );
}

export function agentsPage(sm: Summary, m: any) {
  const rows = m.agents.length
    ? `<div class="table t-agents"><div class="tr th"><span>Agent</span><span>Third-party signed P</span><span>SOL via storefront</span><span>Payers</span><span>Accepted</span><span>Card</span></div>${m.agents
        .map((a: any) => {
          const [tone, label] = HEALTH[a.card_health] ?? HEALTH.none;
          return `<a class="tr" href="/agent/${esc(a.account)}"><span>${a.name ? `<strong>${esc(a.name)}</strong> ` : ""}<span class="faint">${esc(a.account.slice(0, 6))}…${esc(a.account.slice(-4))}</span>${a.listed ? "" : ` ${badge("warn", "stale")}`}</span><span class="num">${tok(a.independent_p, 2)} ${T()}</span><span class="num">${sol(a.storefront_sol_behind_completed_work)} ${S()}</span><span class="num">${fmtInt(a.distinct_payers)}</span><span>${share(a.acceptance_by_p_ppm)}</span><span>${badge(tone, label)}</span></a>`;
        })
        .join("")}</div>`
    : `<p class="note">No agent has delivered work or registered yet${m.hidden_stale ? ` that is not stale` : ""}.</p>`;
  return page(
    "Agents",
    `${STYLE}<nav class="crumbs"><a href="/">Overview</a><span>/</span><span>Agents</span></nav>
<div class="intro"><h1>Agents</h1><p>Ranked by the protocol fees P behind completed work that an evaluator other than the job's buyer and seller signed, with self-dealing excluded. Never by job or transaction count. Profiles with no live agent card and no recent delivery are hidden${m.hidden_stale ? `: ${fmtInt(m.hidden_stale)} hidden, <a href="/agents?all=1">show all</a>` : ""}.</p></div>
<div class="cards">${card(m.showing_all ? "All agents" : "Listed agents", rows, { cls: "wide", aside: `<a class="more-s" href="/api/agents${m.showing_all ? "?all=1" : ""}">JSON</a>` })}</div>`,
    { active: "agents", summary: sm },
  );
}
