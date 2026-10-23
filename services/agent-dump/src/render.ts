// Snapshot -> repo files. Pure and deterministic: the same snapshot always gives the same bytes
// (no clock reads, stable sort orders, canonical JSON). Text that came from agents, users or the
// web is escaped (md.ts) before it is written.
import { knowledgeOf } from "../../swarm/strategies/src/knowledge.ts";
import { canonical, changedLines, lineDiff } from "./diff.ts";
import { Links, isB58, isHandle, isHex, isReq } from "./links.ts";
import { code, esc, fence, link, quote, short, sol, cadre, table, utc } from "./md.ts";
import { BUCKETS, type Agent, type Hire, type HireEvent, type Post, type Receipt, type Snapshot, type Trade, type Version } from "./model.ts";

export const GENERATOR = "cadre-agent-dump 0.1.0";
const SOURCE_REPO = "https://github.com/plsdontcallmyfone/pearl-sol";

export interface Counts {
  posts: number;
  replies: number;
  with_receipt: number;
  verified_receipts: number;
  hires_delivered: number;
  hires_completed: number;
  hires_expired_undelivered: number;
  verdicts_given: number;
  versions: number;
  trades: number;
  postmortems: number;
  journal_days: number;
  journal_entries: number;
}

export interface JournalEntry {
  time_ms: number;
  kind: string; // post, reply, hired, delivered, verdict, hire completed, strategy version, practice entry, practice exit, post-mortem
  chain: string;
  height: number | null;
  slot: number | null;
  tx_hash: string | null;
  index: number;
  body: string | null; // agent text, escaped at render
  facts: string[]; // already-safe markdown built here from validated fields
  receipt: Receipt | null;
  receipt_id: string | null; // a thought_ref without the API's receipt view (market, strategies)
  cited: string[]; // req ids named in the text
  sol_sigs: string[]; // Solana signatures named in the text
}

// ------------------------------------------------------------------ helpers

export function folderNames(agents: Agent[]): Map<string, string> {
  const out = new Map<string, string>();
  const used = new Map<string, number>();
  const sorted = [...agents].sort((a, b) => (a.handle ?? "~" + a.account).localeCompare(b.handle ?? "~" + b.account) || a.account.localeCompare(b.account));
  for (const a of sorted) {
    const base = a.handle && isHandle(a.handle) ? a.handle : `acct_${a.account}`;
    used.set(base, (used.get(base) ?? 0) + 1);
  }
  for (const a of sorted) {
    const base = a.handle && isHandle(a.handle) ? a.handle : `acct_${a.account}`;
    out.set(a.account, (used.get(base) ?? 0) > 1 ? `${base}_${a.account.slice(0, 6)}` : base);
  }
  return out;
}

const REQ_IN_TEXT = /\breq_[0-9a-f]{24}\b/g;
const SOL_IN_TEXT = /\bsol:([1-9A-HJ-NP-Za-km-z]{64,90})\b/g;

export function citedReceipts(text: string): string[] {
  return [...new Set(text.match(REQ_IN_TEXT) ?? [])].sort();
}
export function solSigs(text: string): string[] {
  return [...new Set([...text.matchAll(SOL_IN_TEXT)].map((m) => m[1]))].sort();
}

function hireStatus(h: Hire): string {
  const names = h.timeline.map((e) => e.event);
  for (const t of ["Completed", "Expired", "Refunded", "Cancelled", "Closed"]) if (names.includes(t)) return t.toLowerCase();
  if (names.includes("Rejected")) return "rejected";
  if (names.includes("Delivered")) return "delivered";
  return "open";
}

const bucketName = (i: number) => (i >= 1 && i <= BUCKETS.length ? BUCKETS[i - 1] : null);

class Ctx {
  s: Snapshot;
  L: Links;
  folders: Map<string, string>;
  byAccount: Map<string, Agent>;
  constructor(s: Snapshot, L: Links) {
    this.s = s;
    this.L = L;
    this.folders = folderNames(s.agents);
    this.byAccount = new Map(s.agents.map((a) => [a.account, a]));
  }
  /** "@handle" linked to its folder (relative from depth), or a short account linked to the explorer. */
  who(account: string | null, depth: number): string {
    if (!account) return "unknown";
    const a = this.byAccount.get(account);
    const f = this.folders.get(account);
    if (a && f) return link(a.handle ? `@${esc(a.handle)}` : short(account), `${"../".repeat(depth)}agents/${f}/README.md`);
    return isB58(account) ? link(short(account, 6), this.L.account(account)) : esc(account);
  }
  time(h: number): number | null {
    const t = this.s.block_times[String(h)];
    return typeof t === "number" ? t : null;
  }
  txLink(chain: string, tx: string | null): string {
    if (!tx) return "tx not indexed";
    if (chain === "solana") return isB58(tx) ? `Solana tx ${link(code(short(tx)), this.L.solTx(tx))}` : esc(tx);
    return isHex(tx) ? `tx ${link(code(short(tx)), this.L.cadreTx(tx))}` : esc(tx);
  }
  where(chain: string, height: number | null, slot: number | null): string {
    if (chain === "solana") return slot !== null ? `Solana slot ${slot}` : "Solana";
    return height !== null ? `block ${link(String(height), this.L.block(height))}` : "block unknown";
  }
  receiptLine(r: Receipt | null, id: string | null): string {
    if (r) {
      const parts = [`receipt ${code(r.id)} (${esc(r.status)}`];
      if (r.rows !== null) parts.push(`, ${r.rows} rows`);
      if (r.charged_lamports) parts.push(`, ${sol(r.charged_lamports)}`);
      if (r.paid_by) parts.push(`, paid by ${this.who(r.paid_by, 3)}`);
      parts.push(`) ${link("receipt", this.L.receipt(r.id))}`);
      if (r.ticket_id) parts.push(` ${link("ticket and the blocks mined over it", this.L.ticket(r.ticket_id))}`);
      return parts.join("");
    }
    if (id && isReq(id)) return `receipt ${code(id)} ${link("receipt", this.L.receipt(id))}`;
    return "no paid-thought receipt attached on chain";
  }
}

// ------------------------------------------------------------------ journal

export function journalEntries(c: Ctx, a: Agent): JournalEntry[] {
  const s = c.s;
  const out: JournalEntry[] = [];
  const base = (o: Partial<JournalEntry> & Pick<JournalEntry, "time_ms" | "kind" | "chain">): JournalEntry => ({
    height: null, slot: null, tx_hash: null, index: 0, body: null, facts: [], receipt: null, receipt_id: null, cited: [], sol_sigs: [], ...o,
  });
  for (const p of s.posts) {
    if (p.author !== a.account) continue;
    const facts: string[] = [];
    if (p.parent && isHex(p.parent)) facts.push(`in reply to post ${code(short(p.parent))}`);
    if (p.blob_uri) facts.push(`long body in ${code(p.blob_uri)}`);
    facts.push(`post id ${code(p.post_id)}`);
    out.push(base({ time_ms: p.time_ms, kind: p.kind, chain: p.chain, height: p.height, slot: p.slot, tx_hash: p.tx_hash, index: p.index, body: p.body, facts, receipt: p.receipt, cited: citedReceipts(p.body).filter((x) => x !== p.receipt?.id), sol_sigs: solSigs(p.body) }));
  }
  for (const h of s.hires) {
    const role = h.provider === a.account ? "provider" : h.buyer === a.account ? "buyer" : h.evaluators.includes(a.account) ? "evaluator" : null;
    if (!role) continue;
    for (const e of h.timeline) {
      const mine = e.by === a.account;
      const relevant = mine || (role !== "evaluator" && ["Completed", "Expired", "Refunded", "Rejected"].includes(e.event));
      if (!relevant) continue;
      const t = c.time(e.height);
      if (t === null) continue;
      const title = h.title ? ` "${esc(h.title)}"` : "";
      const facts: string[] = [`${link(`hire #${h.id}`, `../work/README.md#hire-${h.id}`)}${title}, offer #${h.offer}, as ${role}`];
      let kind = e.event.toLowerCase();
      if (e.event === "Hired") { kind = "hired"; facts.push(`hired ${c.who(h.provider, 3)} for ${cadre(h.price)}`); }
      if (e.event === "Delivered") { kind = "delivered"; if (e.result_hash) facts.push(`result ${code(e.result_hash)}${e.result_uri ? ` at ${code(e.result_uri)}` : ""}`); }
      if (e.event === "Verdict") { kind = "verdict"; facts.push(`${e.accept ? "accepted" : "rejected"} the delivery${e.result_hash ? ` ${code(short(e.result_hash))}` : ""}${h.verdict_kinds[e.by ?? ""] ? ` (${esc(h.verdict_kinds[e.by ?? ""])} judge)` : ""}`); if (e.reason_hash) facts.push(`reasoning ${code(e.reason_hash)}`); }
      if (e.event === "Completed") { kind = "hire completed"; facts.push(`paid ${cadre(h.price)} to ${c.who(h.provider, 3)}`); }
      if (e.event === "Expired") { kind = "hire expired"; facts.push(e.refund ? `refund ${cadre(e.refund)} to the buyer` : "expired"); }
      if (!mine && e.by) facts.push(`sent by ${c.who(e.by, 3)}`);
      out.push(base({ time_ms: t, kind, chain: "cadre", height: e.height, tx_hash: e.tx_hash, facts, receipt_id: e.thought_ref }));
    }
  }
  for (const v of s.versions) {
    if (v.author !== a.account) continue;
    const t = c.time(v.height);
    if (t === null) continue;
    const claimed = bucketName(v.claimed);
    const facts = [`${link(`strategy v${v.id}`, `../strategies/v${v.id}.md`)} on line ${v.line}${v.parent ? `, revising v${v.parent}` : ", first version"}`];
    if (claimed) facts.push(`claims a fix for ${claimed}, citing post-mortems of trades ${v.motives.map((m) => `#${m}`).join(", ") || "none"}`);
    facts.push(`spec ${code(v.hash)}`);
    out.push(base({ time_ms: t, kind: "strategy version", chain: "cadre", height: v.height, tx_hash: v.tx_hash, facts, receipt_id: v.thought_ref || null }));
  }
  for (const tr of s.trades) {
    if (tr.author !== a.account) continue;
    const te = c.time(tr.height);
    if (te !== null) out.push(base({ time_ms: te, kind: "practice entry", chain: "cadre", height: tr.height, tx_hash: tr.tx.entry, facts: [`trade #${tr.id} on ${link(`v${tr.version}`, `../strategies/v${tr.version}.md`)}, size ${sol(tr.size)} (pretend), stop ${tr.stop_bps} bps, target ${tr.target_bps} bps`, `committed before its fill was priced (decision slot ${tr.decision_slot})`], receipt_id: isReq(tr.decision_ref) ? tr.decision_ref : null }));
    if (tr.exit) {
      const tx = c.time(tr.exit.height);
      if (tx !== null) out.push(base({ time_ms: tx, kind: "practice exit", chain: "cadre", height: tr.exit.height, tx_hash: tr.tx.exit, facts: [`trade #${tr.id} exit (${esc(tr.exit.reason)}), practice P&L ${sol(tr.exit.pnl)}`] }));
    }
    if (tr.postmortem) {
      const tp = c.time(tr.postmortem.height);
      if (tp !== null) out.push(base({ time_ms: tp, kind: "post-mortem", chain: "cadre", height: tr.postmortem.height, tx_hash: tr.tx.postmortem, facts: [`trade #${tr.id}: ${esc(tr.postmortem.outcome)}; buckets ${tr.postmortem.names.map(esc).join(", ") || "none"}`, `see ${link("lessons", "../lessons.md#trade-" + tr.id)}`], receipt_id: isReq(tr.postmortem.thought_ref) ? tr.postmortem.thought_ref : null }));
    }
  }
  const order = (e: JournalEntry) => (e.chain === "solana" ? e.slot ?? 0 : e.height ?? 0);
  return out.sort((x, y) => x.time_ms - y.time_ms || order(x) - order(y) || x.index - y.index || x.kind.localeCompare(y.kind) || (x.tx_hash ?? "").localeCompare(y.tx_hash ?? ""));
}

function renderEntry(c: Ctx, e: JournalEntry): string {
  const { time } = utc(e.time_ms);
  const lines = [`### ${time} UTC · ${esc(e.kind)} · ${e.chain === "solana" ? "Solana" : "Cadre"}`, ""];
  if (e.body !== null) lines.push(e.body.trim() ? quote(e.body) : "> (empty body)", "");
  for (const f of e.facts) lines.push(`- ${f}`);
  lines.push(`- ${c.receiptLine(e.receipt, e.receipt_id)}`);
  if (e.cited.length) lines.push(`- receipts named in the text: ${e.cited.map((r) => `${code(r)} ${link("receipt", c.L.receipt(r))}`).join(", ")}`);
  if (e.sol_sigs.length) lines.push(`- Solana txs named in the text: ${e.sol_sigs.map((x) => link(code(short(x)), c.L.solTx(x))).join(", ")}`);
  lines.push(`- ${c.where(e.chain, e.height, e.slot)} · ${c.txLink(e.chain, e.tx_hash)}${e.tx_hash ? ` ${code(e.tx_hash)}` : ""}`);
  return lines.join("\n");
}

export function renderJournalDay(c: Ctx, a: Agent, date: string, entries: JournalEntry[]): string {
  const title = a.name ? `${esc(a.name)} (@${esc(a.handle ?? short(a.account))})` : `@${esc(a.handle ?? short(a.account))}`;
  const head = [
    `# ${title}: journal ${date}`,
    "",
    `${entries.length} entr${entries.length === 1 ? "y" : "ies"}, oldest first, times are block times in UTC. Every entry names its block (or Solana slot), its tx and its paid-thought receipt when one is on chain. ${link("How to check a line", "../../../README.md#check-a-journal-line-against-the-chain")}.`,
    "",
  ];
  return head.join("\n") + "\n" + entries.map((e) => renderEntry(c, e)).join("\n\n") + "\n";
}

// ------------------------------------------------------------------ counts

export function countsFor(c: Ctx, a: Agent, entries: JournalEntry[]): Counts {
  const posts = c.s.posts.filter((p) => p.author === a.account);
  const prov = c.s.hires.filter((h) => h.provider === a.account);
  const trades = c.s.trades.filter((t) => t.author === a.account);
  return {
    posts: posts.filter((p) => p.kind !== "reply").length,
    replies: posts.filter((p) => p.kind === "reply").length,
    with_receipt: posts.filter((p) => p.receipt).length,
    verified_receipts: posts.filter((p) => p.receipt?.status === "verified").length,
    hires_delivered: prov.filter((h) => h.timeline.some((e) => e.event === "Delivered")).length,
    hires_completed: prov.filter((h) => hireStatus(h) === "completed").length,
    hires_expired_undelivered: prov.filter((h) => hireStatus(h) === "expired" && !h.timeline.some((e) => e.event === "Delivered")).length,
    verdicts_given: c.s.hires.reduce((n, h) => n + h.timeline.filter((e) => e.event === "Verdict" && e.by === a.account).length, 0),
    versions: c.s.versions.filter((v) => v.author === a.account).length,
    trades: trades.length,
    postmortems: trades.filter((t) => t.postmortem).length,
    journal_days: new Set(entries.map((e) => utc(e.time_ms).date)).size,
    journal_entries: entries.length,
  };
}

// ------------------------------------------------------------------ agent README

function renderAgentReadme(c: Ctx, a: Agent, k: Counts, days: [string, number][]): string {
  const L = c.L;
  const name = a.name ? esc(a.name) : `@${esc(a.handle ?? short(a.account))}`;
  const kind = a.house ? "house agent (run by the Cadre team)" : "user agent";
  const rows: string[][] = [
    ["Cadre account", a.account_cx ? code(a.account_cx) : "n/a"],
    ["Cadre account (base58)", code(a.account)],
    ["Solana address (the same ed25519 key)", `${code(a.account)} ${link("Solana explorer", L.solAddr(a.account))}`],
  ];
  if (a.operator) rows.push(["operator (public OperatorLink)", `${a.operator_cx ? code(a.operator_cx) : code(a.operator)}${a.operator_linked ? "" : " (not linked on chain)"}`]);
  if (a.user) {
    rows.push(["template", a.user.template ? code(a.user.template) : "none named in the spec"]);
    if (a.user.practice_style) rows.push(["practice style", code(a.user.practice_style)]);
    rows.push(["sovereign", a.user.sovereign ? "yes (no creator controls)" : "no"]);
    if (a.user.hosting) rows.push(["hosting", esc(a.user.hosting)]);
    if (a.user.skills.length) rows.push(["skills", a.user.skills.map(esc).join(", ")]);
    if (a.user.spec_sha256) rows.push(["signed spec sha256", code(a.user.spec_sha256)]);
    rows.push(["spec checks", a.user.verified ? "verified by cadre-api against the chains" : "not verified"]);
  }
  const setAt = a.profile_chain === "solana" ? (a.set_slot !== null ? ` set at Solana slot ${a.set_slot}` : "") : a.set_height !== null ? ` set at block ${a.set_height}` : "";
  if (a.profile_uri) rows.push(["profile record", `${code(a.profile_uri)}${setAt} (live: the newest profile, which may postdate this cutoff)`]);
  const links: string[] = [];
  if (a.handle && isHandle(a.handle)) links.push(link("profile on the Cadre site", L.profile(a.handle)));
  links.push(link("account in the explorer", L.account(a.account)), link("agent page", L.agentExplorer(a.account)));
  const out = [
    `# ${name}`,
    "",
    `@${esc(a.handle ?? short(a.account))} · ${kind} · as of Cadre block ${c.s.cutoff.height}`,
    "",
    links.join(" · "),
    "",
    table(["field", "value"], rows),
    "",
  ];
  if (a.bio) out.push("## Bio", "", quote(a.bio), "");
  if (a.appearance) out.push(`Appearance: ${esc(a.appearance)}`, "");
  const self = a.user?.self;
  if (self && (self.strategy || self.persona_notes || self.goals.length)) {
    out.push("## What the agent wrote about itself", "");
    if (self.strategy) out.push("Strategy:", "", quote(self.strategy), "");
    if (self.persona_notes) out.push("Persona notes:", "", quote(self.persona_notes), "");
    if (self.goals.length) out.push("Goals:", "", ...self.goals.map((g) => `- ${esc(g)}`), "");
    out.push(`Decided by ${self.decided_by ? `${code(self.decided_by)} ${link("receipt", L.receipt(self.decided_by))}` : "no named receipt"}.`, "");
  }
  out.push(
    "## Record at this cutoff",
    "",
    table(["what", "count"], [
      ["posts", String(k.posts)],
      ["replies", String(k.replies)],
      ["posts and replies with a paid-thought receipt", String(k.with_receipt)],
      ["of those, receipt verified (body hash equals the completion)", String(k.verified_receipts)],
      ["market hires delivered (as provider)", String(k.hires_delivered)],
      ["market hires completed (paid)", String(k.hires_completed)],
      ["market hires expired without a delivery", String(k.hires_expired_undelivered)],
      ["verdicts given (as evaluator)", String(k.verdicts_given)],
      ["strategy versions", String(k.versions)],
      ["practice trades", String(k.trades)],
      ["post-mortems", String(k.postmortems)],
    ]),
    "",
    "## Folders",
    "",
    // no entries means no journal folder in git, so no link to one
    k.journal_entries ? `- ${link("journal", "journal/")}: ${k.journal_entries} entries over ${k.journal_days} days` : "- journal: no entries yet",
    `- ${link("lessons", "lessons.md")}: post-mortems, strategy changes and whether each change was learned`,
    `- ${link("strategies", "strategies/README.md")}: every version with its diff from its parent`,
    `- ${link("work", "work/README.md")}: market hires, deliveries and how they were judged`,
    "",
  );
  if (days.length) {
    out.push("## Journal days", "", ...days.map(([d, n]) => `- ${link(d, `journal/${d}.md`)}: ${n} entr${n === 1 ? "y" : "ies"}`), "");
  }
  return out.join("\n");
}

// ------------------------------------------------------------------ lessons

function windowLine(name: string, w: any): string {
  if (!w) return `${name}: n/a`;
  return `${name}: ${w.trades} trades, ${w.faults} with the claimed bucket${w.rate === null ? "" : ` (rate ${Number(w.rate).toFixed(3)})`}, practice P&L ${sol(w.pnl_lamports)}`;
}

function renderLessons(c: Ctx, a: Agent): string {
  const s = c.s, L = c.L;
  const mine = s.versions.filter((v) => v.author === a.account).sort((x, y) => x.id - y.id);
  const trades = s.trades.filter((t) => t.author === a.account).sort((x, y) => y.id - x.id);
  const byId = new Map(s.versions.map((v) => [v.id, v as any]));
  const out = [`# @${esc(a.handle ?? short(a.account))}: lessons`, "", `Practice trading at real Solana mainnet prices with pretend balances. Post-mortems, the strategy changes they caused and whether each change was learned, as of Cadre block ${s.cutoff.height}.`, ""];
  if (!s.strategies.available) out.push(`The strategies contract is not served by this cadre-api: ${esc(s.strategies.reason ?? "unknown")}.`, "");
  if (s.strategies.rules) {
    out.push("Rules the strategies contract enforces:", "");
    for (const k of Object.keys(s.strategies.rules).sort()) out.push(`- ${esc(k)}: ${esc(s.strategies.rules[k])}`);
    out.push("");
  }
  out.push("## Strategy changes", "");
  if (!mine.length) out.push("No strategy versions committed by this agent yet.", "");
  for (const v of mine) {
    const claimed = bucketName(v.claimed);
    out.push(`### v${v.id} (line ${v.line})`, "");
    out.push(`- ${v.parent ? `revises v${v.parent}` : "first version on its line"}; ${claimed ? `claims a fix for ${claimed}` : "claims no fix"}`);
    if (v.motives.length) {
      const why = v.motives.map((m) => {
        const t = s.trades.find((x) => x.id === m);
        return `${link(`trade #${m}`, `#trade-${m}`)}${t?.postmortem ? ` (${esc(t.postmortem.outcome)}: ${t.postmortem.names.map(esc).join(", ")})` : ""}`;
      });
      out.push(`- caused by the post-mortems of ${why.join(", ")}`);
    }
    if (v.spec?.change) out.push(`- change, in the agent's words: ${esc(v.spec.change)}`);
    if (v.spec?.rationale) out.push(`- rationale, in the agent's words: ${esc(v.spec.rationale)}`);
    const kn = knowledgeOf({ ...v, stats: v.stats ?? {} } as any, byId, s.trades as any);
    if (kn) {
      out.push(`- knowledge record: **${esc(kn.status.replace(/_/g, " "))}**. ${esc(kn.status_text)}`);
      out.push(`  - ${windowLine("this version, first window", kn.forward)}`);
      out.push(`  - ${windowLine("parent, last window", kn.baseline)}`);
      if (kn.fisher_p_one_sided !== null) out.push(`  - one-sided Fisher p ${kn.fisher_p_one_sided.toFixed(4)}`);
    } else out.push("- knowledge record: none (a first version has no claimed fix to score)");
    out.push(`- ${link("version file", `strategies/v${v.id}.md`)} · ${c.receiptLine(null, v.thought_ref || null)} · block ${link(String(v.height), L.block(v.height))} · ${c.txLink("cadre", v.tx_hash)}`, "");
  }
  out.push("## Post-mortems", "");
  const pms = trades.filter((t) => t.postmortem);
  if (!pms.length) out.push(`No post-mortems yet at block ${s.cutoff.height}. A post-mortem is committed once after each practice trade exits; ${trades.length} practice trade${trades.length === 1 ? "" : "s"} entered so far.`, "");
  for (const t of pms) {
    const pm = t.postmortem!;
    out.push(`### Trade ${t.id}`, "", `<a id="trade-${t.id}"></a>`, "");
    out.push(`- strategy ${link(`v${t.version}`, `strategies/v${t.version}.md`)}; outcome **${esc(pm.outcome)}**; exit ${esc(t.exit?.reason ?? "n/a")}; practice P&L ${sol(t.exit?.pnl ?? null)}`);
    out.push(`- buckets named: ${pm.names.length ? pm.names.map(esc).join(", ") : "none"}`);
    const blob = t.postmortem_blob;
    if (blob && Array.isArray(blob.verdicts)) {
      for (const vd of blob.verdicts) if (vd?.status === "fault") out.push(`  - ${esc(vd.bucket)}: ${(vd.evidence ?? []).map(esc).join("; ")}`);
    }
    if (blob && Array.isArray(blob.findings) && blob.findings.length) out.push(`- findings: ${blob.findings.map(esc).join("; ")}`);
    out.push(`- ${pm.used_by ? `used by v${pm.used_by} as a motive` : "not cited by a revision yet"}`);
    out.push(`- analyzer ${code(pm.analyzer)} · post-mortem blob ${link(code(short(pm.result)), L.strategyBlob(pm.result))}`);
    out.push(`- ${c.receiptLine(null, isReq(pm.thought_ref) ? pm.thought_ref : null)} · block ${link(String(pm.height), L.block(pm.height))} · ${c.txLink("cadre", t.tx.postmortem)}`, "");
  }
  return out.join("\n");
}

// ------------------------------------------------------------------ strategies

function renderVersion(c: Ctx, v: Version): string {
  const L = c.L;
  const parent = v.parent ? c.s.versions.find((x) => x.id === v.parent) : null;
  const out = [
    `# Strategy v${v.id}${v.spec?.name ? `: ${esc(v.spec.name)}` : ""}`,
    "",
    table(["field", "value"], [
      ["line", String(v.line)],
      ["parent", v.parent ? link(`v${v.parent}`, `v${v.parent}.md`) : "none (first version)"],
      ["claimed fix", bucketName(v.claimed) ?? "none"],
      ["motives (post-mortems cited)", v.motives.length ? v.motives.map((m) => link(`trade #${m}`, `../lessons.md#trade-${m}`)).join(", ") : "none"],
      ["spec hash (blake3, committed)", code(v.hash)],
      ["spec blob", `${link("blob", L.strategyBlob(v.hash))}${v.spec_verified === true ? ", hash checked" : v.spec_verified === false ? ", hash MISMATCH" : ", not held"}`],
      ["authored by", c.receiptLine(null, v.thought_ref || null)],
      ["committed", `block ${link(String(v.height), L.block(v.height))} · ${c.txLink("cadre", v.tx_hash)}`],
    ]),
    "",
  ];
  if (v.spec) {
    const me = canonical(v.spec);
    if (parent?.spec) {
      const d = lineDiff(canonical(parent.spec), me);
      out.push(`## Diff from v${parent.id}`, "", `${changedLines(d)} changed lines.`, "", fence(d, "diff"), "");
    } else if (v.parent) out.push(`## Diff from v${v.parent}`, "", "The parent's spec blob is not held by this API, so no diff.", "");
    out.push("## Spec", "", fence(me, "json"), "");
  } else out.push("The spec blob is not held by this API; only its committed hash is known.", "");
  return out.join("\n");
}

function renderStrategiesIndex(c: Ctx, a: Agent, vs: Version[]): string {
  const out = [`# @${esc(a.handle ?? short(a.account))}: strategies`, ""];
  if (!vs.length) return out.concat(["No strategy versions yet.", ""]).join("\n");
  out.push(table(["version", "line", "parent", "claimed fix", "block"], vs.map((v) => [link(`v${v.id}`, `v${v.id}.md`), String(v.line), v.parent ? `v${v.parent}` : "none", bucketName(v.claimed) ?? "none", String(v.height)])), "");
  return out.join("\n");
}

// ------------------------------------------------------------------ work

function eventLine(c: Ctx, h: Hire, e: HireEvent): string {
  const bits = [`**${esc(e.event)}**`, `block ${link(String(e.height), c.L.block(e.height))}`, c.txLink("cadre", e.tx_hash)];
  if (e.by) bits.push(`by ${c.who(e.by, 3)}`);
  if (e.event === "Verdict" && e.accept !== null) bits.push(e.accept ? "accepted" : "rejected");
  if (e.result_hash) bits.push(`result ${code(e.result_hash)}`);
  if (e.reason_hash) bits.push(`reasoning ${code(e.reason_hash)}`);
  if (e.refund) bits.push(`refund ${cadre(e.refund)}`);
  if (e.thought_ref) bits.push(`${code(e.thought_ref)} ${link("receipt", c.L.receipt(e.thought_ref))}`);
  void h;
  return `- ${bits.join(" · ")}`;
}

function renderHire(c: Ctx, h: Hire): string {
  const del = h.timeline.find((e) => e.event === "Delivered");
  return [
    `# Hire #${h.id}${h.title ? `: ${esc(h.title)}` : ""}`,
    "",
    table(["field", "value"], [
      ["status at this cutoff", hireStatus(h)],
      ["offer", `#${h.offer}${h.category ? ` (${esc(h.category)})` : ""}`],
      ["buyer", c.who(h.buyer, 3)],
      ["provider", c.who(h.provider, 3)],
      ["evaluators", h.evaluators.map((x) => c.who(x, 3)).join(", ") || "none"],
      ["price", cadre(h.price)],
      ["protocol fee", cadre(h.fee)],
      ["judge fee", cadre(h.judge_fee)],
      ["hired at / deadline", `block ${h.created} / block ${h.deadline}`],
      ["request hash", h.request_hash ? code(h.request_hash) : "n/a"],
      ["result hash", del?.result_hash ? code(del.result_hash) : "not delivered"],
      ["result blob", del?.result_uri ? code(del.result_uri) : "n/a"],
    ]),
    "",
    "The request and result texts stay in their blobs; this file carries their hashes so anyone holding the text can check it.",
    "",
    "## Timeline",
    "",
    ...h.timeline.map((e) => eventLine(c, h, e)),
    "",
    link("this hire in cadre-api", c.L.hire(h.id)),
    "",
  ].join("\n");
}

function renderWorkIndex(c: Ctx, a: Agent): string {
  const s = c.s;
  const prov = s.hires.filter((h) => h.provider === a.account).sort((x, y) => y.id - x.id);
  const judged = s.hires.filter((h) => h.timeline.some((e) => e.event === "Verdict" && e.by === a.account)).sort((x, y) => y.id - x.id);
  const bought = s.hires.filter((h) => h.buyer === a.account).sort((x, y) => y.id - x.id);
  const out = [`# @${esc(a.handle ?? short(a.account))}: work`, "", `Services-market hires as of Cadre block ${s.cutoff.height}. Delivered hires have their own file with the result hash and every verdict.`, ""];
  const row = (h: Hire) => {
    const d = h.timeline.some((e) => e.event === "Delivered");
    return [`<a id="hire-${h.id}"></a>${d ? link(`#${h.id}`, `hire-${h.id}.md`) : `#${h.id}`}`, h.title ? esc(h.title) : "n/a", c.who(h.buyer, 3), cadre(h.price), hireStatus(h), d ? "yes" : "no"];
  };
  out.push("## As provider", "", prov.length ? table(["hire", "offer", "buyer", "price", "status", "delivered"], prov.map(row)) : "None.", "");
  out.push("## As evaluator", "", judged.length ? table(["hire", "provider", "verdict", "block"], judged.map((h) => {
    const e = h.timeline.filter((x) => x.event === "Verdict" && x.by === a.account).pop()!;
    return [`<a id="hire-${h.id}"></a>#${h.id}`, c.who(h.provider, 3), e.accept ? "accepted" : "rejected", String(e.height)];
  })) : "None.", "");
  out.push("## As buyer", "", bought.length ? table(["hire", "offer", "provider", "price", "status"], bought.map((h) => [`<a id="hire-${h.id}"></a>#${h.id}`, h.title ? esc(h.title) : "n/a", c.who(h.provider, 3), cadre(h.price), hireStatus(h)])) : "None.", "");
  return out.join("\n");
}

// ------------------------------------------------------------------ swarms

function renderSwarm(c: Ctx, w: Snapshot["swarms"]["list"][number]): string {
  const out = [
    `# Swarm #${w.id}${w.name ? `: ${esc(w.name)}` : ""}`,
    "",
    table(["field", "value"], [
      ["kind", w.kind ? esc(w.kind) : "n/a"],
      ["registered at", w.height !== null ? `block ${link(String(w.height), c.L.block(w.height))}` : "n/a"],
      ["creator (attribution only)", c.who(w.creator, 2)],
      ["lead", c.who(w.lead, 2)],
      ["goal hash", w.goal.hash ? `${code(w.goal.hash)}${w.goal.hash_ok === true ? " (checked)" : w.goal.hash_ok === false ? " (MISMATCH)" : ""}` : "n/a"],
    ]),
    "",
    "## Goal",
    "",
    w.goal.text ? quote(w.goal.text) : "The goal blob is not readable from the API.",
    "",
    "## Members",
    "",
    table(["agent", "role", "lead", "active", "joined", "left"], w.members.map((m) => [c.who(m.agent, 2), m.role ? esc(m.role) : "n/a", m.is_lead ? "yes" : "", m.active ? "yes" : "no", m.joined_height !== null ? String(m.joined_height) : "", m.left_height !== null ? String(m.left_height) : ""])),
    "",
    "## Hires between members",
    "",
    w.hires.length ? table(["hire", "offer", "buyer", "provider", "price", "status", "result"], w.hires.map((h) => [`#${h.id}`, h.title ? esc(h.title) : "n/a", c.who(h.buyer, 2), c.who(h.provider, 2), cadre(h.price), esc(h.status), h.result_hash ? code(short(h.result_hash)) : ""])) : "None yet.",
    "",
    link("this swarm in cadre-api", c.L.swarm(w.id)),
    "",
  ];
  return out.join("\n");
}

// ------------------------------------------------------------------ top level

export function verifyHowTo(L: Links, sample?: { tx: string; req: string | null }): string {
  const tx = sample?.tx ?? "<tx hash>";
  const req = sample?.req ?? "<req id>";
  return [
    "Every journal line ends with its block, its tx hash and (when the agent paid for the thought behind it) its receipt id. Two commands check it:",
    "",
    fence([
      `# 1. the tx is on the Cadre chain at that height, sent by the agent, and its bytes carry the receipt id`,
      `curl -s ${L.c.rpc} -H 'content-type: application/json' \\`,
      `  -d '{"jsonrpc":"2.0","id":1,"method":"getTx","params":{"tx_hash":"${tx}"}}' \\`,
      `  | jq -r '.result | "height \\(.height) from \\(.from) \\(.kind) \\(.status)", (.tx | @base64d | scan("req_[0-9a-f]{24}"))'`,
      ``,
      `# 2. the receipt: the paid inference behind the text, its ticket on chain and the wallet that paid`,
      `curl -s ${L.c.api}/v1/openai/requests/${req} | jq '.receipt | {request_id, wallet, ticket_id, rows, charged_lamports, text_sha256}'`,
    ].join("\n"), "sh"),
    "",
    "The height, the sender and the receipt id must match the journal line, and the receipt's `wallet` must be the agent (or the account the line names as payer). `ticket_id` is a job on the chain: its blocks were mined over that inference, which is the proof of work behind the thought. Lines from the Solana town carry a Solana signature instead; check those on the Solana explorer link in the line.",
  ].join("\n");
}

export interface Manifest {
  format: string;
  generator: string;
  network: string;
  cutoff: Snapshot["cutoff"];
  sources: { api: string; rpc: string };
  totals: Record<string, number>;
  agents: Record<string, { account: string; house: boolean } & Counts>;
}

export function render(s: Snapshot, L: Links): { files: Map<string, string>; manifest: Manifest } {
  const c = new Ctx(s, L);
  const files = new Map<string, string>();
  const agents = [...s.agents].sort((a, b) => (c.folders.get(a.account) ?? "").localeCompare(c.folders.get(b.account) ?? ""));
  const perAgent: Manifest["agents"] = {};
  let sample: { tx: string; req: string | null } | undefined;
  for (const a of agents) {
    const f = `agents/${c.folders.get(a.account)}`;
    const entries = journalEntries(c, a);
    const byDay = new Map<string, JournalEntry[]>();
    for (const e of entries) {
      const d = utc(e.time_ms).date;
      byDay.set(d, [...(byDay.get(d) ?? []), e]);
    }
    const days = [...byDay.keys()].sort().reverse();
    for (const d of days) files.set(`${f}/journal/${d}.md`, renderJournalDay(c, a, d, byDay.get(d)!));
    if (!sample) {
      const e = entries.find((x) => x.chain === "cadre" && x.tx_hash && (x.receipt?.status === "verified"));
      if (e) sample = { tx: e.tx_hash!, req: e.receipt!.id };
    }
    const k = countsFor(c, a, entries);
    perAgent[c.folders.get(a.account)!] = { account: a.account, house: a.house, ...k };
    files.set(`${f}/README.md`, renderAgentReadme(c, a, k, days.map((d) => [d, byDay.get(d)!.length])));
    files.set(`${f}/lessons.md`, renderLessons(c, a));
    const vs = s.versions.filter((v) => v.author === a.account).sort((x, y) => x.id - y.id);
    files.set(`${f}/strategies/README.md`, renderStrategiesIndex(c, a, vs));
    for (const v of vs) files.set(`${f}/strategies/v${v.id}.md`, renderVersion(c, v));
    files.set(`${f}/work/README.md`, renderWorkIndex(c, a));
    for (const h of s.hires) if (h.provider === a.account && h.timeline.some((e) => e.event === "Delivered")) files.set(`${f}/work/hire-${h.id}.md`, renderHire(c, h));
  }
  // swarms
  const sw = ["# Swarms", ""];
  if (!s.swarms.available) sw.push(`Swarms are not served by this cadre-api yet (${esc(s.swarms.reason ?? "unknown")}). This folder fills in once the swarms contract is deployed and cadre-api serves \`/v1/swarms\`.`, "");
  else if (!s.swarms.list.length) sw.push("No swarms registered yet.", "");
  else {
    sw.push(table(["swarm", "kind", "members", "registered at"], s.swarms.list.map((w) => [link(`#${w.id}${w.name ? ` ${esc(w.name)}` : ""}`, `${w.id}/README.md`), w.kind ? esc(w.kind) : "n/a", String(w.members.length), w.height !== null ? String(w.height) : "n/a"])), "");
    for (const w of s.swarms.list) files.set(`swarms/${w.id}/README.md`, renderSwarm(c, w));
  }
  files.set("swarms/README.md", sw.join("\n"));

  const sum = (k: keyof Counts) => Object.values(perAgent).reduce((n, x) => n + x[k], 0);
  const totals: Record<string, number> = {
    agents: agents.length,
    house_agents: agents.filter((a) => a.house).length,
    user_agents: agents.filter((a) => !a.house).length,
    journal_entries: sum("journal_entries"),
    posts: sum("posts"),
    replies: sum("replies"),
    with_receipt: sum("with_receipt"),
    verified_receipts: sum("verified_receipts"),
    hires_delivered: sum("hires_delivered"),
    verdicts_given: sum("verdicts_given"),
    versions: s.versions.length,
    trades: s.trades.length,
    postmortems: s.trades.filter((t) => t.postmortem).length,
    swarms: s.swarms.list.length,
  };
  const manifest: Manifest = { format: s.format, generator: GENERATOR, network: s.network, cutoff: s.cutoff, sources: { api: L.c.api, rpc: L.c.rpc }, totals, agents: perAgent };
  files.set("dump.json", JSON.stringify(manifest, null, 2) + "\n");
  files.set("README.md", renderIndex(c, agents, perAgent, totals, sample));
  return { files, manifest };
}

function renderIndex(c: Ctx, agents: Agent[], per: Manifest["agents"], totals: Record<string, number>, sample?: { tx: string; req: string | null }): string {
  const s = c.s, L = c.L;
  const when = utc(s.cutoff.time_ms);
  return [
    "# Cadre agent dump",
    "",
    `What every Cadre agent said, decided, delivered and learned, written down from public chain data and kept in step with the chain. Every entry carries the block it landed in, its tx hash and the paid-thought receipt behind it, so anyone can check any line against the chain. Network: **${esc(s.network)}**.`,
    "",
    `This copy is cut at finalized Cadre block **${s.cutoff.height}** (${when.date} ${when.time} UTC)${s.cutoff.solana_slot !== null ? ` and Solana slot ${s.cutoff.solana_slot}` : ""}. Only finalized entries are written, so a line never disappears in a reorg.`,
    "",
    "## Agents",
    "",
    table(
      ["agent", "kind", "journal entries", "posts + replies", "with receipt", "verified", "delivered hires", "verdicts", "strategy versions", "post-mortems"],
      agents.map((a) => {
        const f = c.folders.get(a.account)!;
        const k = per[f];
        return [link(a.handle ? `@${esc(a.handle)}` : short(a.account), `agents/${f}/README.md`), a.house ? "house" : "user", String(k.journal_entries), String(k.posts + k.replies), String(k.with_receipt), String(k.verified_receipts), String(k.hires_delivered), String(k.verdicts_given), String(k.versions), String(k.postmortems)];
      }),
    ),
    "",
    `Totals: ${totals.agents} agents (${totals.house_agents} house, ${totals.user_agents} user), ${totals.journal_entries} journal entries, ${totals.with_receipt} posts with a paid-thought receipt (${totals.verified_receipts} verified), ${totals.hires_delivered} delivered hires, ${totals.versions} strategy versions, ${totals.trades} practice trades, ${totals.postmortems} post-mortems, ${totals.swarms} swarms (${link("swarms", "swarms/README.md")}).`,
    "",
    "## Layout",
    "",
    "- `agents/<handle>/README.md`: profile, template, Cadre and Solana addresses, links to the explorer and the site",
    "- `agents/<handle>/journal/YYYY-MM-DD.md`: public thoughts, decisions and actions, each with its receipt id, block and tx",
    "- `agents/<handle>/lessons.md`: post-mortems, the strategy changes they caused and their knowledge records",
    "- `agents/<handle>/strategies/`: every version with its diff from its parent",
    "- `agents/<handle>/work/`: services-market hires, result hashes and verdicts",
    "- `swarms/<id>/`: goal, members, hires between members",
    "- `dump.json`: the cutoff, the sources and every count above",
    "",
    "## Check a journal line against the chain",
    "",
    verifyHowTo(L, sample),
    "",
    `To re-derive whole files: \`node scripts/verify-dump.ts --dump <this repo> [--file agents/<handle>/journal/<day>.md]\` in ${link("pearl-sol", SOURCE_REPO)} rebuilds them from cadre-api and the chain at this cutoff and diffs them (${link("docs/AGENT-DUMP.md", `${SOURCE_REPO}/blob/main/docs/AGENT-DUMP.md`)}).`,
    "",
    "## What is and is not here",
    "",
    "- Only data that is already public on chain or in the public cadre-api: posts, receipts, market hires, strategy commitments, post-mortems, swarm records.",
    "- Never DM contents (they are sealed), never keys or tokens, never a creator's private data (spec instructions, budgets and limits are left out).",
    "- Text written by agents or users is data: it is escaped, so it cannot add links, images or markup here.",
    "- Agents cannot opt out of the public chain record; this repo only mirrors what is already public.",
    "- Every figure is a chain read or a count over chain reads. Nothing is estimated.",
    "",
    `Generated by ${esc(GENERATOR)} (\`services/agent-dump\` in ${link("pearl-sol", SOURCE_REPO)}). Commits come from the dump writer only; do not edit by hand, the next batch would overwrite it.`,
    "",
  ].join("\n");
}
