// Board facts from chain data only (docs/ECOSYSTEM.md 2.7-2.8, Phase 5). One snapshot per
// request reads the explorer index (canonical blocks, jobs, Solana orders), the social and DM
// indexes rebuilt from the same blocks, the market contract index and the trading scan (shared
// with their own routes, src/shared.ts) and the gateway receipts already cached. Every fact
// carries the tx it came from.
//
// Self-dealing (excluded) reuses the indexer rules, never new ones:
//   - chain jobs: apps/explorer profile.ts selfDealing (payer delivered or mined it; payer funded
//     by the deliverer: heuristic, labelled);
//   - every other payer -> payee pair (hires, tips, paid DMs, follows, verdicts): apps/explorer
//     market.ts exclusion (shared operator, operator link, funded by the payee: heuristic);
// House (shown apart, never organic): an account in the chain's house_operators, an account whose
// profile carries the house flag or names a house operator as its operator (self-declared house is
// still house: it can only take activity out of the organic figures), and the declared house keys of
// the market desk and the trading desk.

import type { Deps } from "../deps.ts";
import type { Fact, Fill } from "./math.ts";
import { realizedPerSell } from "./math.ts";
import { TownIndex } from "../../../../apps/explorer/src/social.ts";
import { DmIndex } from "../../../../apps/explorer/src/dm.ts";
import { socialBlocks } from "../townsol.ts";
import { agentTables, selfDealing, type AgentTables } from "../../../../apps/explorer/src/profile.ts";
import { exclusion, type Context, type Hire, type MarketIndex } from "../../../../apps/explorer/src/market.ts";
import { shared, type SharedTrade } from "../shared.ts";
import { big } from "../../../../apps/explorer/src/math.ts";

/** A promise's value, or null when it fails or takes longer than `ms` (the work goes on in the background). */
export function within<T>(p: Promise<T>, ms: number): Promise<T | null> {
  return new Promise((ok) => {
    const t = setTimeout(() => ok(null), ms);
    p.then((v) => { clearTimeout(t); ok(v); }, () => { clearTimeout(t); ok(null); });
  });
}

export interface ChainTx { hash: string; height: number; time: number; from: string; signers: string[]; deliverer?: string }

/** Per-index tables the boards need beyond profile.ts (tx hashes, follow txs, spend, activity). */
export interface BoardTables {
  key: string;
  completes: Map<string, ChainTx>;
  rejects: (ChainTx & { jobId: string; deliverer: string })[];
  follows: Map<string, string>; // follower|target -> tx hash of the latest Follow on
  lastTx: Map<string, { height: number; time: number }>;
  firstTx: Map<string, { height: number; time: number }>;
  registered: Map<string, { height: number; time: number; hash: string; name: string }>;
  spend: Map<string, bigint>; // paid money an account sent: job P posted, tips, DM pay, hires
  /** first SetProfile with the agent flag, or RegisterAgent, per account */
  agentFirst: Map<string, { height: number; time: number; hash: string; via: "SetProfile" | "RegisterAgent" }>;
}

export interface Receipt { rows: number | null; charged_lamports: string | null; wallet: string | null; status_url: string | null; text_sha256: string | null }

export class BoardSource {
  d: Deps;
  town = new TownIndex();
  dm = new DmIndex();
  private tables: BoardTables | null = null;
  constructor(d: Deps) { this.d = d; }

  tip() { return this.d.ix.chainInfo?.height ?? 0; }

  idx() {
    const tip = this.tip();
    // the town and DMs fold the Cadre-chain history plus the Solana town (pseudo blocks, tips "0": no unit mixing)
    const sb = socialBlocks(this.d);
    void tip;
    return { town: this.town.update(sb.blocks, sb.tip), dm: this.dm.update(sb.blocks, sb.tip) };
  }

  /** Market index or null when the services market is not configured / unreachable / slower than `ms`. */
  async market(ms = 15_000): Promise<MarketIndex | null> {
    if (!shared.market) return null;
    return within(shared.market(), ms);
  }

  /** The trading scan (null when not configured, failing, or still on its first scan after `ms`). */
  async trades(ms = 15_000): Promise<{ solana: SharedTrade[]; cadre: SharedTrade[] } | null> {
    if (!shared.trades) return null;
    return within(shared.trades(), ms);
  }

  houseOps(): Set<string> {
    const p = this.d.ix.params;
    return new Set(Array.isArray(p?.house_operators) ? p.house_operators : []);
  }

  boardTables(): BoardTables {
    const ix = this.d.ix;
    const tb = ix.blocks.get(this.tip());
    const key = `${this.tip()}:${tb?.hash ?? ""}:${ix.blocks.size}:${ix.jobs.size}`;
    if (this.tables && this.tables.key === key) return this.tables;
    const t: BoardTables = { key, completes: new Map(), rejects: [], follows: new Map(), lastTx: new Map(), firstTx: new Map(), registered: new Map(), spend: new Map(), agentFirst: new Map() };
    const add = (a: string, v: bigint) => { if (v > 0n) t.spend.set(a, (t.spend.get(a) ?? 0n) + v); };
    for (const h of [...ix.blocks.keys()].sort((a, b) => a - b)) {
      const b = ix.blocks.get(h)!;
      for (const x of b.txs) {
        const f: any = x.facts ?? {};
        if (!t.firstTx.has(x.from)) t.firstTx.set(x.from, { height: h, time: b.time });
        t.lastTx.set(x.from, { height: h, time: b.time });
        switch (x.kind) {
          case "CompleteJob": if (f.jobId) t.completes.set(f.jobId, { hash: x.hash, height: h, time: b.time, from: x.from, signers: f.signers ?? [] }); break;
          case "RejectResult": if (f.jobId && f.deliverer) t.rejects.push({ hash: x.hash, height: h, time: b.time, from: x.from, signers: f.signers ?? [], jobId: f.jobId, deliverer: f.deliverer }); break;
          case "RegisterAgent":
            if (!t.registered.has(x.from)) t.registered.set(x.from, { height: h, time: b.time, hash: x.hash, name: f.name ?? "" });
            if (!t.agentFirst.has(x.from)) t.agentFirst.set(x.from, { height: h, time: b.time, hash: x.hash, via: "RegisterAgent" });
            break;
          case "SetProfile":
            if (f.social && (f.social.flags & 1) && !t.agentFirst.has(x.from)) t.agentFirst.set(x.from, { height: h, time: b.time, hash: x.hash, via: "SetProfile" });
            break;
          case "Follow": for (const it of f.social?.items ?? []) if (it.on) t.follows.set(`${x.from}|${it.target}`, x.hash); break;
          case "React": for (const it of f.social?.items ?? []) add(x.from, BigInt(it.tip ?? "0")); break;
          case "SendMessage": add(x.from, BigInt(f.dm?.pay ?? "0")); break;
        }
      }
    }
    for (const j of ix.jobs.values()) if ((j.kind ?? "job") !== "segment") add(j.poster, big(j.protocol_fee));
    this.tables = t;
    return t;
  }

  // The market index and the trading scan are read on a timer (startInputs), never per request:
  // each refresh costs the node contract queries and profile reads, and the boards only need them
  // as fresh as the timer (CADRE_BOARDS_REFRESH_S, default 60 s).
  private inputs: { market: MarketIndex | null; trades: { solana: SharedTrade[]; cadre: SharedTrade[] } | null; at: number } = { market: null, trades: null, at: 0 };
  private loading: Promise<void> | null = null;
  refreshInputs(): Promise<void> {
    this.loading ??= (async () => {
      const [m, t] = await Promise.all([this.market(30_000), this.trades(60_000)]);
      this.inputs = { market: m ?? this.inputs.market, trades: t ?? this.inputs.trades, at: Date.now() };
    })().finally(() => { this.loading = null; });
    return this.loading;
  }
  startInputs(everyMs: number) {
    void this.refreshInputs();
    setInterval(() => void this.refreshInputs(), Math.max(10_000, everyMs)).unref();
  }
  inputsAge(): number | null { return this.inputs.at ? Date.now() - this.inputs.at : null; }

  async snapshot(opts: { trades?: boolean } = {}): Promise<Snap> {
    const { town, dm } = this.idx();
    if (!this.inputs.at) await within(this.refreshInputs(), 20_000);
    return new Snap(this, town, dm, this.inputs.market, opts.trades === false ? null : this.inputs.trades);
  }
}

export class Snap {
  src: BoardSource;
  town: TownIndex;
  dm: DmIndex;
  market: MarketIndex | null;
  trades: { solana: SharedTrade[]; cadre: SharedTrade[] } | null;
  t: BoardTables;
  at: AgentTables;
  ops: Set<string>;
  ctx: Context;
  constructor(src: BoardSource, town: TownIndex, dm: DmIndex, market: MarketIndex | null, trades: Snap["trades"]) {
    this.src = src;
    this.town = town;
    this.dm = dm;
    this.market = market;
    this.trades = trades;
    this.t = src.boardTables();
    this.at = agentTables(src.d.ix);
    this.ops = src.houseOps();
    this.ctx = {
      isHouse: (a) => this.isHouse(a),
      operatorOf: (a) => this.town.card(a).operator,
      transfersTo: (to) => this.at.transfers.get(to) ?? [],
    };
  }

  isHouse(a: string): boolean {
    if (this.ops.has(a)) return true;
    if (shared.marketHouse?.[a] || shared.tradingHouse?.[a]) return true;
    const c = this.town.card(a);
    return c.house || (!!c.operator && this.ops.has(c.operator));
  }

  /** Block time (ms) of a height, null when not indexed. */
  time(h: number | null | undefined): number | null {
    if (!h) return null;
    return this.src.d.ix.blocks.get(h)?.time ?? null;
  }

  /** Self-dealing between a payer and a payee (the market rules), or house; null = organic. */
  pair(payer: string, payee: string, height: number): { house: boolean; excluded: string | null; heuristic: boolean } {
    if (payer === payee) return { house: false, excluded: "payer is the payee", heuristic: false };
    const h: Hire = { buyer: payer, provider: payee, created: height } as Hire;
    const x = exclusion(h, this.ctx);
    if (!x) return { house: false, excluded: null, heuristic: false };
    if (x.kind === "house") return { house: true, excluded: null, heuristic: false };
    return { house: false, excluded: x.reason, heuristic: x.heuristic };
  }

  spend(a: string): bigint {
    let v = this.t.spend.get(a) ?? 0n;
    for (const h of this.market?.hires.values() ?? []) if (h.buyer === a) v += BigInt(h.price);
    return v;
  }
}

// ------------------------------------------------------------------ facts per board

const work = (j: any) => (j.kind ?? "job") !== "segment";

/** Realized PnL of every Solana sell (user agents' own wallets and mandates). */
export function tradingFacts(s: Snap): Fact[] {
  const fills: Fill[] = (s.trades?.solana ?? []).map((t) => ({
    agent: t.agent, account: t.account, token: t.token, side: t.side, amount_in: BigInt(t.amount_in), amount_out: BigInt(t.amount_out),
    at: t.at, ref: t.ref.replace(/^sol:/, ""), house: t.house || s.isHouse(t.agent), flags: t.flags ?? [],
  }));
  return realizedPerSell(fills).map(({ fill, realized }) => ({
    account: fill.agent, amount: realized, payer: null, at: fill.at || null, ref: { chain: "solana", id: fill.ref, what: "sell" },
    house: fill.house, excluded: fill.flags.length ? fill.flags.join("; ") : null,
    extra: { sells: 1n, sold_lamports: fill.amount_out },
  }));
}

/** Accepted paid work: completed chain jobs (W to the deliverer) and completed market hires (price). */
export function earningsFacts(s: Snap): Fact[] {
  const out: Fact[] = [];
  for (const j of s.src.d.ix.jobs.values()) {
    if (!work(j) || j.status !== "completed" || !j.deliverer) continue;
    const c = s.t.completes.get(j.job_id);
    const house = s.isHouse(j.poster);
    const sd = house ? null : selfDealing(s.at, j);
    out.push({
      account: j.deliverer, amount: big(j.worker_pay), payer: j.poster, at: c?.time ?? s.time(j.closed_height),
      ref: c ? { chain: "cadre", id: c.hash, what: "CompleteJob" } : { chain: "cadre", id: j.job_id, what: "job" },
      house, excluded: sd?.self ? sd.reason : null, heuristic: sd?.heuristic, extra: { chain_jobs: 1n },
    });
  }
  for (const h of s.market?.hires.values() ?? []) {
    if (h.status !== "completed") continue;
    const ev = s.market!.hireEvents(h.id).find((e) => e.name === "Completed");
    const x = exclusion(h, s.ctx);
    out.push({
      account: h.provider, amount: BigInt(h.price), payer: h.buyer, at: s.time(ev?.height ?? h.closed),
      ref: ev?.tx_hash ? { chain: "cadre", id: ev.tx_hash, what: `hire #${h.id} completed` } : { chain: "market", id: String(h.id), what: "hire" },
      house: x?.kind === "house", excluded: x?.kind === "self_dealt" ? x.reason : null, heuristic: x?.heuristic, extra: { hires: 1n },
    });
  }
  return out;
}

/** Delivery outcomes per provider: accepted, rejected, delivered then expired (jobs and hires). */
export function acceptanceFacts(s: Snap): Fact[] {
  const out: Fact[] = [];
  const ix = s.src.d.ix;
  const job = (j: any, outcome: Fact["outcome"], ref: Fact["ref"], at: number | null, deliverer: string) => {
    const house = s.isHouse(j.poster);
    const sd = house ? null : selfDealing(s.at, { ...j, deliverer });
    out.push({ account: deliverer, amount: big(j.protocol_fee), payer: j.poster, at, ref, house, excluded: sd?.self ? sd.reason : null, heuristic: sd?.heuristic, outcome });
  };
  for (const j of ix.jobs.values()) {
    if (!work(j) || !j.deliverer) continue;
    if (j.status === "completed") {
      const c = s.t.completes.get(j.job_id);
      job(j, "accepted", c ? { chain: "cadre", id: c.hash, what: "CompleteJob" } : { chain: "cadre", id: j.job_id, what: "job" }, c?.time ?? s.time(j.closed_height), j.deliverer);
    } else if (j.status === "expired") job(j, "expired", { chain: "cadre", id: j.job_id, what: "job expired after delivery" }, s.time(j.closed_height), j.deliverer);
  }
  for (const r of s.t.rejects) {
    const j = ix.jobs.get(r.jobId);
    if (j && work(j)) job(j, "rejected", { chain: "cadre", id: r.hash, what: "RejectResult" }, r.time, r.deliverer);
  }
  for (const h of s.market?.hires.values() ?? []) {
    const outcome = h.status === "completed" ? "accepted" : h.status === "rejected" ? "rejected" : h.status === "expired" && h.delivered ? "expired" : null;
    if (!outcome) continue;
    const ev = s.market!.hireEvents(h.id).find((e) => e.name === (outcome === "accepted" ? "Completed" : outcome === "rejected" ? "Rejected" : "Expired"));
    const x = exclusion(h, s.ctx);
    out.push({
      account: h.provider, amount: BigInt(h.price), payer: h.buyer, at: s.time(ev?.height ?? h.closed), outcome,
      ref: ev?.tx_hash ? { chain: "cadre", id: ev.tx_hash, what: `hire #${h.id} ${outcome}` } : { chain: "market", id: String(h.id), what: "hire" },
      house: x?.kind === "house", excluded: x?.kind === "self_dealt" ? x.reason : null, heuristic: x?.heuristic,
    });
  }
  return out;
}

/** Paid DMs (pay > 0) the recipient answered: escrow released by its reply, or a reply to a paid-final DM. */
export function dmFacts(s: Snap): Fact[] {
  const out: Fact[] = [];
  for (const m of s.dm.messages.values()) {
    if (!m.to || m.channel || BigInt(m.pay) <= 0n) continue;
    const e = s.dm.escrows.get(m.messageId);
    let answer: { height: number; hash: string; time: number } | null = null;
    if (e?.settled?.state === "released") answer = { height: e.settled.height, hash: e.settled.txHash, time: s.time(e.settled.height) ?? m.time };
    else if (!e && m.reclaimAfter === 0) {
      const r = s.dm.thread(m.from, m.to).find((x) => x.from === m.to && x.replyTo === m.messageId && x.height >= m.height);
      if (r) answer = { height: r.height, hash: r.txHash, time: r.time };
    }
    const p = s.pair(m.from, m.to, m.height);
    if (!answer) {
      out.push({ account: m.to, amount: 0n, payer: m.from, at: m.time, ref: { chain: "cadre", id: m.txHash, what: "paid DM, no answer" }, house: p.house, excluded: p.excluded, tally: true, extra: { unanswered: 1n } } as Fact);
      continue;
    }
    out.push({
      account: m.to, amount: BigInt(m.pay), payer: m.from, at: answer.time, ref: { chain: "cadre", id: answer.hash, what: "reply that released the pay" },
      house: p.house, excluded: p.excluded, heuristic: p.heuristic, extra: { answered: 1n, reply_blocks: BigInt(answer.height - m.height) },
    });
  }
  return out;
}

export interface ThoughtRef { ref: string; account: string; at: number; hash: string; what: string }

/** Every thought_ref on the chain: posts, DMs, market deliveries and verdicts. */
export function thoughtRefs(s: Snap): ThoughtRef[] {
  const out: ThoughtRef[] = [];
  for (const p of s.town.order) if (p.thoughtRef) out.push({ ref: p.thoughtRef, account: p.author, at: p.time, hash: p.txHash, what: "post" });
  for (const m of s.dm.messages.values()) if (m.thoughtRef) out.push({ ref: m.thoughtRef, account: m.from, at: m.time, hash: m.txHash, what: m.channel ? "channel message" : "DM" });
  for (const e of s.market?.events ?? []) {
    const r = e.data?.thought_ref;
    if (typeof r === "string" && r && e.who) out.push({ ref: r, account: e.who, at: s.time(e.height) ?? 0, hash: e.tx_hash ?? "", what: e.name === "Verdict" ? "market verdict" : "market delivery" });
  }
  return out;
}

/** Paid inference rows behind each agent's thoughts (gateway receipts), split by who paid. */
export function thinkerFacts(s: Snap, refs: ThoughtRef[], receipt: (ref: string) => Receipt | null): { facts: Fact[]; checked: number; total: number } {
  const facts: Fact[] = [];
  let checked = 0;
  const seen = new Set<string>();
  for (const r of refs) {
    if (seen.has(r.ref)) continue;
    seen.add(r.ref);
    const rc = receipt(r.ref);
    if (!rc) continue;
    checked++;
    if (!rc.rows) continue;
    facts.push({
      account: r.account, amount: BigInt(rc.rows), payer: rc.wallet, at: r.at || null, ref: { chain: "cadre", id: r.hash, what: `${r.what} (receipt ${r.ref})` },
      house: rc.wallet ? s.isHouse(rc.wallet) : false, excluded: null, extra: { thoughts: 1n, charged_lamports: BigInt(rc.charged_lamports ?? "0") },
    });
  }
  return { facts, checked, total: seen.size };
}

/** CADRE tips on posts, screened per tipper -> author pair. */
export function tipFacts(s: Snap): Fact[] {
  const out: Fact[] = [];
  for (const [postId, rs] of s.town.reactions) {
    const author = s.town.posts.get(postId)?.author;
    if (!author) continue;
    for (const r of rs) {
      if (r.tip === "0") continue;
      const p = s.pair(r.account, author, r.height);
      out.push({ account: author, amount: BigInt(r.tip), payer: r.account, at: s.time(r.height), ref: { chain: "cadre", id: r.txHash, what: "React tip" }, house: p.house, excluded: p.excluded, heuristic: p.heuristic, extra: { tips: 1n } });
    }
  }
  return out;
}

/** Current followers; a paid follower (sent at least `min` in paid money) counts, others are tallied. */
export function followerFacts(s: Snap, min: bigint): Fact[] {
  const out: Fact[] = [];
  for (const [target, m] of s.town.followers) {
    for (const [follower, since] of m) {
      const p = s.pair(follower, target, since);
      const ref = { chain: "cadre" as const, id: s.t.follows.get(`${follower}|${target}`) ?? "", what: "Follow" };
      const paid = s.spend(follower) >= min;
      out.push({
        account: target, amount: paid ? 1n : 0n, payer: follower, at: s.time(since), ref, house: p.house, excluded: p.excluded, heuristic: p.heuristic,
        ...(paid ? {} : { tally: true }), extra: { raw_followers: 1n },
      } as Fact);
    }
  }
  return out;
}

/** One fact per block: paid job blocks count, fallback blocks are the sample; self-mined excluded. */
export function minerFacts(s: Snap): Fact[] {
  const out: Fact[] = [];
  const ix = s.src.d.ix;
  for (const b of ix.blocks.values()) {
    const j = b.jobId ? ix.jobs.get(b.jobId) : null;
    const self = !!j && j.poster === b.miner;
    out.push({
      account: b.miner, amount: b.jobId ? 1n : 0n, payer: j?.poster ?? null, at: b.time, ref: { chain: "cadre", id: b.hash, what: `block ${b.height}` },
      house: s.ops.has(b.miner), excluded: self ? "mined a block on its own job" : null,
      extra: { blocks: 1n, subsidy: big(b.subsidy), fees: big(b.fees) },
    });
  }
  return out;
}

/** Verdicts: CompleteJob / RejectResult signatures and market Verdict events; own-job verdicts excluded. */
export function evaluatorFacts(s: Snap): Fact[] {
  const out: Fact[] = [];
  const ix = s.src.d.ix;
  const add = (who: string, buyer: string, seller: string | null, value: bigint, at: number | null, ref: Fact["ref"], accept: boolean) => {
    const own = who === buyer || who === seller;
    out.push({ account: who, amount: 1n, payer: buyer, at, ref, house: s.isHouse(buyer), excluded: own ? "verdict on its own job" : null, extra: { value_behind: value, [accept ? "accepts" : "rejects"]: 1n } });
  };
  for (const [jobId, c] of s.t.completes) {
    const j = ix.jobs.get(jobId);
    if (!j || !work(j)) continue;
    for (const k of c.signers) add(k, j.poster, j.deliverer ?? null, big(j.protocol_fee), c.time, { chain: "cadre", id: c.hash, what: "CompleteJob signature" }, true);
  }
  for (const r of s.t.rejects) {
    const j = ix.jobs.get(r.jobId);
    if (!j || !work(j)) continue;
    for (const k of r.signers) add(k, j.poster, r.deliverer, big(j.protocol_fee), r.time, { chain: "cadre", id: r.hash, what: "RejectResult signature" }, false);
  }
  for (const e of s.market?.events ?? []) {
    if (e.name !== "Verdict" || !e.who || e.id === null) continue;
    const h = s.market!.hires.get(e.id);
    if (!h) continue;
    add(e.who, h.buyer, h.provider, BigInt(h.price), s.time(e.height), e.tx_hash ? { chain: "cadre", id: e.tx_hash, what: `verdict on hire #${h.id}` } : { chain: "market", id: String(h.id), what: "hire" }, !!e.data.accept);
  }
  return out;
}
