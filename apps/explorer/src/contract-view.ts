// HTML for the contract pages (models in contract-route.ts). Same primitives and shell as
// views.ts; Geist only, no monospace. Missing data shows "Unavailable" with the reason.

import { BRAND } from "./brand.ts";
import { fmtInt } from "./math.ts";
import type { Summary } from "./models.ts";
import type { Read } from "./contract-route.ts";
import { T, badge, card, esc, id, kv, page, tok } from "./views.ts";
import { txStatusCell } from "./txstatus-view.ts";

const STYLE = `<style>
.t-contracts .tr { grid-template-columns: minmax(0, 1.3fr) minmax(0, 1.2fr) minmax(0, 1fr) 90px 70px 70px 110px; }
.t-codes .tr { grid-template-columns: minmax(0, 1.4fr) minmax(0, 1.2fr) 90px 90px 80px; }
.t-events .tr { grid-template-columns: 60px 90px minmax(0, 1fr) minmax(0, 1.2fr) minmax(0, 1.2fr); }
.t-cev .tr { grid-template-columns: 50px minmax(0, 1fr) minmax(0, 1.3fr) minmax(0, 1.3fr); }
.intro .idwrap.full { max-width: 100%; display: inline-flex; min-width: 0; }
.intro .idwrap.full .hash { overflow-wrap: anywhere; word-break: break-all; min-width: 0; }
.topics { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.filters { display: flex; flex-wrap: wrap; gap: 6px; margin: 4px 16px 10px; }
.filters a { padding: 3px 9px; border: 1px solid var(--line); border-radius: 999px; font-size: 12.5px; color: var(--dim); }
.filters a[aria-current] { color: var(--text); border-color: var(--text); }
.stats + .cards { margin-top: 16px; }
.unavail { margin: 4px 16px 14px; }
.bad-t { color: var(--bad); }
@media (max-width: 760px) { .t-contracts .tr, .t-codes .tr, .t-events .tr, .t-cev .tr { grid-template-columns: minmax(0, 1fr) auto; } }
</style>`;

/** Thousands separators on a decimal integer string (gas, bytes: no decimals). */
const grp = (v: string | number | null | undefined) => (v === null || v === undefined ? `<span class="faint">none</span>` : `<span class="num">${esc(String(v).replace(/\B(?=(\d{3})+(?!\d))/g, ","))}</span>`);
const amt = (v: string | null | undefined, frac = 4) => (v === null || v === undefined ? `<span class="faint">none</span>` : `<span class="num">${tok(v, frac)}</span> ${T()}`);
const blk = (h: number | null | undefined) => (h ? `<a href="/block/${h}">#${fmtInt(h)}</a>` : `<span class="faint">none</span>`);
const unavailable = (reason: string) => `<p class="note unavail"><strong>Unavailable:</strong> ${esc(reason)}.</p>`;
const stat = (label: string, value: string, sub: string) => `<div class="stat"><div class="eyebrow">${esc(label)}</div><div class="stat-v">${value}</div><div class="stat-s">${sub}</div></div>`;

const STATUS: Record<string, [any, string]> = {
  ok: ["ok", "ok"],
  reverted: ["bad", "reverted"],
  out_of_gas: ["bad", "out of gas"],
  trapped: ["bad", "trapped"],
  refused: ["warn", "refused"],
};
export const statusBadge = (s: string) => {
  const [tone, label] = STATUS[s] ?? ["neutral", s];
  return badge(tone, label);
};

function topicCell(e: any) {
  const t = e.topics ?? [];
  if (!t.length) return `<span class="faint">no topics</span>`;
  return `<span class="topics">${e.name ? `<strong>${esc(e.name)}</strong>` : id(t[0], { head: 8, tail: 6 })}${t.slice(1).map((x: string) => id(x, { head: 8, tail: 6 })).join("")}</span>`;
}
const dataCell = (d: string) => (d ? `${id(d, { head: 10, tail: 6 })} <span class="faint num">${fmtInt(d.length / 2)} B</span>` : `<span class="faint">empty</span>`);

/** Contract row for lists. */
function contractRow(c: any) {
  return `<a class="tr" href="/contract/${esc(c.address)}"><span>${c.label ? `<strong>${esc(c.label)}</strong> ` : ""}<span class="faint">${esc(c.address.slice(0, 6))}…${esc(c.address.slice(-4))}</span></span><span class="faint">code ${esc(c.code_hash.slice(0, 8))}…</span><span>${c.upgradable ? badge("warn", "upgradable") : badge("ok", "immutable")}</span><span class="num">#${fmtInt(c.created_height)}</span><span class="num">${fmtInt(c.calls)}</span><span class="num">${fmtInt(c.events)}</span><span class="num">${grp(c.storage_bytes)} B</span></a>`;
}

function pager(base: string, key: string, offset: number, limit: number, total: number, extra = "") {
  const prev = offset > 0 ? `<a href="${base}?${key}=${Math.max(0, offset - limit)}${extra}">Newer</a>` : "";
  const next = offset + limit < total ? `<a href="${base}?${key}=${offset + limit}${extra}">Older</a>` : "";
  return prev || next ? `<span class="pager">${prev}${next}</span>` : "";
}

export function contractsPage(sm: Summary, m: any) {
  const c: Read = m.contracts;
  const k: Read = m.codes;
  const list = !c.ok
    ? unavailable(c.reason)
    : c.v.items.length
      ? `<div class="table t-contracts"><div class="tr th"><span>Contract</span><span>Code</span><span>Admin</span><span>Created</span><span>Calls</span><span>Events</span><span>Storage</span></div>${c.v.items.map(contractRow).join("")}</div>`
      : `<p class="note">No contract has been instantiated on this chain yet.</p>`;
  const codes = !k.ok
    ? unavailable(k.reason)
    : k.v.items.length
      ? `<div class="table t-codes"><div class="tr th"><span>Code hash</span><span>Deployer</span><span>Height</span><span>Size</span><span>Instances</span></div>${k.v.items
          .map((x: any) => `<a class="tr" href="/code/${esc(x.code_hash)}"><span class="hash" title="${esc(x.code_hash)}">${esc(x.code_hash.slice(0, 10))}…${esc(x.code_hash.slice(-6))}</span><span class="faint">${esc(x.deployer.slice(0, 6))}…${esc(x.deployer.slice(-4))}</span><span class="num">#${fmtInt(x.height)}</span><span class="num">${grp(x.size)} B</span><span class="num">${fmtInt(x.instances)}</span></a>`)
          .join("")}</div>`
      : `<p class="note">No code has been deployed on this chain yet.</p>`;
  const p = m.params;
  const params = p.gas_price === null
    ? unavailable("getParams has no contract fields: the node predates the contracts upgrade")
    : kv([
        ["Gas price", `${grp(p.gas_price)} chain units per ${grp(p.gas_price_unit)} gas`],
        ["Gas limits", `${grp(p.tx_gas_limit)} per tx, ${grp(p.block_gas_limit)} per block`],
        ["Contract tx fee floor", `max(min_fee, ${tok(p.contract_base_fee)} + ${tok(p.contract_byte_fee)} x tx bytes + gas_limit x gas price) ${T()}`],
        ["Storage deposit", `${amt(p.storage_byte_deposit, 9)} per stored byte`],
        ["Code size limit", `${grp(p.max_code_size)} bytes, call depth ${grp(p.max_call_depth)}`],
      ]);
  return page(
    "Contracts",
    `${STYLE}<nav class="crumbs"><a href="/">Overview</a><span>/</span><span>Contracts</span></nav>
<div class="intro"><h1>Contracts</h1><p>WASM contracts on the ${esc(BRAND.name)} agent chain, newest first, read from the node's contract index. Admin "immutable" means no key can migrate the contract.</p></div>
<div class="cards">
  ${card(`Contracts${c.ok ? ` (${fmtInt(c.v.total)})` : ""}`, list, { cls: "wide", aside: `${c.ok ? pager("/contracts", "offset", m.offset, m.limit, c.v.total) : ""}<a class="more-s" href="/api/contracts">JSON</a>` })}
  ${card(`Code${k.ok ? ` (${fmtInt(k.v.total)})` : ""}`, codes, { cls: "wide", aside: k.ok ? pager("/contracts", "codes_offset", m.codes_offset, m.limit, k.v.total) : "" })}
  ${card("Gas and fees", params, { cls: "wide" })}
</div>`,
    { active: "contracts", summary: sm },
  );
}

export function contractPage(sm: Summary, m: any) {
  const r: Read = m.contract;
  if (!r.ok) {
    return page("Contract", `${STYLE}<nav class="crumbs"><a href="/">Overview</a><span>/</span><a href="/contracts">Contracts</a></nav>
<div class="intro"><h1>Contract</h1><p>${id(m.address, { full: true })}</p></div><div class="cards">${card("Contract", unavailable(r.reason), { cls: "wide" })}</div>`, { active: "contracts", summary: sm });
  }
  const c = r.v;
  const name = c.label ? esc(c.label) : "Unlabelled contract";
  const stats = `<section class="stats">
  ${stat("Balance", amt(c.balance, 4), "held by the contract account")}
  ${stat("Storage", `${grp(c.storage_bytes)} B`, `${fmtInt(c.storage_items)} item${c.storage_items === 1 ? "" : "s"}, deposit ${tok(c.deposit, 9)} ${esc(BRAND.ticker)}`)}
  ${stat("Calls", grp(c.calls), "Call txs executed")}
  ${stat("Events", grp(c.events), "emitted over its life")}
</section>`;
  const rec = kv([
    ["Address", id(c.address, { head: 10, tail: 8 })],
    ["Label", c.label ? esc(c.label) : `<span class="faint">none</span>`],
    ["Code", `${id(c.code_hash, { href: `/code/${c.code_hash}`, head: 10, tail: 8 })}${c.code_size !== undefined ? ` <span class="faint num">${grp(c.code_size)} B</span>` : ""}`],
    ["Admin", c.admin ? `${id(c.admin, { head: 8, tail: 6 })} ${badge("warn", "upgradable")}` : `${badge("ok", "immutable")} <span class="faint">no admin: it can never be migrated</span>`],
    ["Creator", id(c.creator, { head: 8, tail: 6 })],
    ["Created", blk(c.created_height)],
    ["Migrated", c.migrated_height ? blk(c.migrated_height) : `<span class="faint">never</span>`],
    ["Entries", Array.isArray(c.entries) && c.entries.length ? esc(c.entries.join(", ")) : `<span class="faint">not reported by the node</span>`],
    ...(Array.isArray(c.imports) && c.imports.length ? ([["Host imports", esc(c.imports.join(", "))]] as [string, string][]) : []),
  ]);
  const ev: Read = m.events;
  const base = `/contract/${encodeURIComponent(c.address)}`;
  const tq = m.topic ? `&topic=${m.topic}` : "";
  const filters = `<nav class="filters" aria-label="Filter by event"><a href="${base}"${m.topic ? "" : ' aria-current="true"'}>All</a>${m.known_events.map((n: string) => `<a href="${base}?topic=${encodeURIComponent(n)}"${m.topic_name === n ? ' aria-current="true"' : ""}>${esc(n)}</a>`).join("")}</nav>`;
  const evBody = !ev.ok
    ? unavailable(ev.reason)
    : ev.v.events.length
      ? `<div class="table t-events"><div class="tr th"><span>#</span><span>Block</span><span>Tx</span><span>Event</span><span>Data</span></div>${ev.v.events
          .map((e: any) => `<div class="tr"><span class="num">${fmtInt(e.n)}</span><span>${blk(e.height)}</span><span>${id(e.tx_hash, { href: `/tx/${e.tx_hash}`, head: 6, tail: 4 })}</span><span>${topicCell(e)}</span><span>${dataCell(e.data)}</span></div>`)
          .join("")}</div>`
      : `<p class="note">${m.topic ? `No ${m.topic_name ? esc(m.topic_name) : "matching"} events among the ones scanned.` : "This contract has emitted no events yet."}</p>`;
  const evPager = ev.ok ? pager(base, "offset", m.offset, m.limit, ev.v.total, tq) : "";
  return page(
    `Contract ${c.label || c.address.slice(0, 8)}`,
    `${STYLE}<nav class="crumbs"><a href="/">Overview</a><span>/</span><a href="/contracts">Contracts</a><span>/</span><span>${name}</span></nav>
<div class="intro"><h1>${name}</h1><p>${id(c.address, { full: true })}</p><p>A WASM contract on the agent chain. Everything here is the node's contract record at the tip; events are read from the node's per-contract event index.</p></div>
${stats}
<div class="cards">
  ${card("Record", rec, { cls: "wide", aside: `<a class="more-s" href="/api/contract/${esc(c.address)}">JSON</a>` })}
  ${card(`Events${ev.ok ? ` (${fmtInt(ev.v.total)})` : ""}`, filters + evBody + `<p class="note">${esc(m.topic_rule)}.</p>`, { cls: "wide", aside: evPager })}
</div>`,
    { active: "contracts", summary: sm },
  );
}

export function codePage(sm: Summary, m: any) {
  const r: Read = m.code;
  if (!r.ok) {
    return page("Code", `${STYLE}<nav class="crumbs"><a href="/">Overview</a><span>/</span><a href="/contracts">Contracts</a></nav>
<div class="intro"><h1>Code</h1><p>${id(m.code_hash, { full: true })}</p></div><div class="cards">${card("Code", unavailable(r.reason), { cls: "wide" })}</div>`, { active: "contracts", summary: sm });
  }
  const c = r.v;
  const inst: Read = m.instances;
  const list = !inst.ok
    ? unavailable(inst.reason)
    : inst.v.items.length
      ? `<div class="table t-contracts"><div class="tr th"><span>Contract</span><span>Code</span><span>Admin</span><span>Created</span><span>Calls</span><span>Events</span><span>Storage</span></div>${inst.v.items.map(contractRow).join("")}</div>`
      : `<p class="note">No contract among the newest ${fmtInt(inst.v.scanned)} runs this code.</p>`;
  return page(
    `Code ${c.code_hash.slice(0, 8)}`,
    `${STYLE}<nav class="crumbs"><a href="/">Overview</a><span>/</span><a href="/contracts">Contracts</a><span>/</span><span>Code</span></nav>
<div class="intro"><h1>Code</h1><p>${id(c.code_hash, { full: true })}</p><p>code_hash = blake3(WASM bytes). Instances share the code; each has its own storage and balance.</p></div>
<div class="cards">
  ${card("Code", kv([
    ["Code hash", id(c.code_hash, { head: 10, tail: 8 })],
    ["Deployer", id(c.deployer, { head: 8, tail: 6 })],
    ["Deployed", blk(c.height)],
    ["Size", `${grp(c.size)} bytes`],
    ["Deposit", amt(c.deposit)],
    ["Instances", grp(c.instances)],
  ]), { cls: "wide", aside: `<a class="more-s" href="/api/code/${esc(c.code_hash)}">JSON</a>` })}
  ${card("Contracts running this code", list + `<p class="note">${esc(m.instances_rule)}.</p>`, { cls: "wide" })}
</div>`,
    { active: "contracts", summary: sm },
  );
}

export function receiptBody(rc: any, height?: number | null) {
  const rows: [string, string][] = [
    ["Status", `${statusBadge(rc.status)}${rc.final === true ? ` ${badge("ok", "final")}` : rc.final === false ? ` ${badge("neutral", "not final")}` : ""}`],
    ["Gas used", `${grp(rc.gas_used)} of ${grp(rc.gas_limit)}`],
    ["Fee charged", `${amt(rc.fee_charged, 9)} <span class="faint">(unused gas refunded)</span>`],
    ...(rc.contract ? ([["Contract", id(rc.contract, { href: `/contract/${rc.contract}`, head: 8, tail: 6 })]] as [string, string][]) : []),
    ...(rc.code_hash && !/^0+$/.test(rc.code_hash) ? ([["Code", id(rc.code_hash, { href: `/code/${rc.code_hash}`, head: 8, tail: 6 })]] as [string, string][]) : []),
    ...(rc.method ? ([["Method", esc(rc.method)]] as [string, string][]) : []),
    ["Return data", rc.return ? dataCell(rc.return) : `<span class="faint">empty</span>`],
    ...(rc.error ? ([["Error", `<span class="bad-t">${esc(rc.error)}</span>`]] as [string, string][]) : []),
    ["Events", grp(rc.event_count)],
    ["Block", blk(rc.height ?? height)],
    ["Receipt hash", id(rc.hash, { head: 8, tail: 6 })],
  ];
  const evs = rc.events ?? [];
  const evTable = evs.length
    ? `<div class="table t-cev"><div class="tr th"><span>#</span><span>Contract</span><span>Event</span><span>Data</span></div>${evs
        .map((e: any) => `<div class="tr"><span class="num">${fmtInt(e.index)}</span><span>${id(e.contract, { href: `/contract/${e.contract}`, head: 6, tail: 4 })}</span><span>${topicCell(e)}</span><span>${dataCell(e.data)}</span></div>`)
        .join("")}</div>`
    : "";
  return kv(rows) + evTable;
}

export function txPage(sm: Summary, m: any) {
  const r: Read = m.tx;
  if (!r.ok) {
    return page("Transaction", `${STYLE}<nav class="crumbs"><a href="/">Overview</a><span>/</span><span>Transaction</span></nav>
<div class="intro"><h1>Transaction</h1><p>${id(m.tx_hash, { full: true })}</p></div><div class="cards">${card("Transaction", unavailable(r.reason), { cls: "wide" })}</div>`, { summary: sm });
  }
  const t = r.v;
  const pending = t.status === "pending";
  const head = kv([
    ["Kind", `<span class="ix">${esc(t.kind)}</span>`],
    ["Status", m.tx_status ? txStatusCell(m.tx_status) : pending ? badge("neutral", "pending in the mempool") : `${badge("ok", "included")} ${blk(t.height)}${t.confirmations !== undefined ? ` <span class="faint">${fmtInt(t.confirmations)} confirmations</span>` : ""}`],
    ["From", id(t.from, { href: `/agent/${t.from}`, head: 8, tail: 6 })],
    ["Nonce", grp(t.nonce)],
    ["Fee paid (bid)", amt(t.fee, 9)],
    ...(t.block_hash ? ([["Block hash", id(t.block_hash, { href: `/block/${t.height}`, head: 8, tail: 6 })]] as [string, string][]) : []),
  ]);
  const c = m.contract;
  let body = "";
  if (c) {
    const b = c.body;
    const rows: [string, string][] = [];
    if (b.contract) rows.push(["Contract", id(b.contract, { href: `/contract/${b.contract}`, head: 8, tail: 6 })]);
    if (b.code_hash) rows.push(["Code", id(b.code_hash, { href: `/code/${b.code_hash}`, head: 8, tail: 6 })]);
    if (b.code_size !== undefined) rows.push(["Code size", `${grp(b.code_size)} bytes`]);
    if (b.label !== undefined) rows.push(["Label", b.label ? esc(b.label) : `<span class="faint">none</span>`]);
    if ("admin" in b) rows.push([c.kind === "SetContractAdmin" ? "New admin" : "Admin", b.admin ? id(b.admin, { head: 8, tail: 6 }) : `${badge("ok", "immutable")} <span class="faint">no admin</span>`]);
    if (b.value !== undefined) rows.push(["Value sent", amt(b.value, 9)]);
    if (b.gas_limit !== undefined) rows.push(["Gas limit", grp(b.gas_limit)]);
    if (b.method) rows.push(["Method", esc(b.method)]);
    if (b.args !== undefined) rows.push(["Args", b.args ? dataCell(b.args) : `<span class="faint">empty</span>`]);
    body = card(`${c.kind} body`, kv(rows));
  }
  const rc: Read | null = m.receipt;
  const receipt = rc === null ? "" : card("Receipt", rc.ok ? receiptBody(rc.v, t.height) : unavailable(rc.reason), { cls: "wide", aside: rc.ok ? statusBadge(rc.v.status) : "" });
  return page(
    `Tx ${m.tx_hash.slice(0, 8)}`,
    `${STYLE}<nav class="crumbs"><a href="/">Overview</a><span>/</span>${t.height ? `<a href="/block/${t.height}">Block ${fmtInt(t.height)}</a><span>/</span>` : ""}<span>Transaction</span></nav>
<div class="intro"><h1>Transaction</h1><p>${id(m.tx_hash, { full: true })}</p></div>
<div class="cards">
  ${card("Transaction", head, { cls: "wide", aside: `<a class="more-s" href="/api/tx/${esc(m.tx_hash)}">JSON</a>` })}
  ${receipt}
  ${body}
</div>`,
    { summary: sm },
  );
}

/** The block's contracts record (getBlock.contracts) for the block page; "" when the block has none. */
export function blockContractsCard(rec: any) {
  if (!rec) return "";
  return card("Contracts record", kv([
    ["Receipts", grp(rec.receipt_count)],
    ["Events", grp(rec.event_count)],
    ["Gas used", grp(rec.gas_used)],
    ["Receipts root", id(rec.receipts_root, { head: 8, tail: 6 })],
    ["Events root", id(rec.events_root, { head: 8, tail: 6 })],
    ["Record hash", id(rec.record_hash, { head: 8, tail: 6 })],
    ["tx_root leaf", `<span class="num">${fmtInt(rec.leaf_index)}</span> <span class="faint">(proved on Solana by this path, then the event under events_root)</span>`],
  ]));
}
