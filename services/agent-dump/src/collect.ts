// Reads cadre-api (and, for block times, the API's block view) into a Snapshot. With a cache it is
// incremental: final posts, closed hires, committed strategy versions, explained trades and block
// times never change on chain, so they are read once and kept in the state file.
import { Api, HttpError } from "./api.ts";
import { isB58, isHex, isReq } from "./links.ts";
import { SNAPSHOT_FORMAT, type Agent, type Hire, type HireEvent, type Post, type Receipt, type Snapshot, type Swarm, type Trade, type Version } from "./model.ts";

export interface Cache {
  posts: Record<string, Post[]>; // by author, newest first
  hires: Record<string, Hire & { closed: boolean }>;
  versions: Record<string, Version>;
  trades: Record<string, Trade & { settled: boolean }>;
  events: Record<string, { tx_hash: string; height: number }>; // "Version:3" -> tx
  block_times: Record<string, number>;
}

export const emptyCache = (): Cache => ({ posts: {}, hires: {}, versions: {}, trades: {}, events: {}, block_times: {} });

const TERMINAL = new Set(["Completed", "Expired", "Refunded", "Cancelled", "Closed"]);
const str = (v: unknown): string | null => (typeof v === "string" && v.length ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

export function receiptOf(thoughtRef: unknown, t: any): Receipt | null {
  if (!isReq(thoughtRef)) return null;
  return {
    id: thoughtRef,
    status: str(t?.status) ?? "unknown",
    ticket_id: isHex(t?.ticket_id) ? t.ticket_id : null,
    rows: num(t?.rows),
    charged_lamports: str(t?.charged_lamports),
    paid_by: isB58(t?.paid_by) ? t.paid_by : null,
  };
}

export function normPost(p: any): Post {
  const sol = p.chain === "solana";
  return {
    post_id: String(p.post_id),
    author: String(p.author?.account ?? p.author),
    kind: str(p.kind) ?? "post",
    parent: str(p.parent),
    body: typeof p.body === "string" ? p.body : "",
    blob_uri: str(p.blob_uri),
    receipt: receiptOf(p.thought_ref, p.thought),
    chain: sol ? "solana" : "cadre",
    height: sol ? null : Number(p.height),
    slot: sol ? Number(p.slot ?? p.height) : null,
    tx_hash: String(p.tx_hash),
    block_hash: str(p.block_hash),
    index: Number(p.index ?? 0),
    time_ms: Number(p.timestamp_ms ?? 0),
  };
}

export function normAgent(r: any, ua: any | null): Agent {
  const spec = ua?.spec ?? null;
  const self = ua?.self ?? null;
  return {
    account: r.account,
    account_cx: str(r.account_cx),
    handle: str(r.handle),
    name: str(r.name),
    house: r.house === true,
    bio: str(r.bio),
    appearance: str(r.appearance),
    operator: str(r.operator),
    operator_cx: str(r.operator_cx),
    operator_linked: r.operator_linked === true,
    created_height: num(r.created_height),
    set_height: num(r.set_height),
    profile_uri: str(r.profile_uri),
    profile_chain: r.chain === "solana" ? "solana" : "cadre",
    user: ua
      ? {
          verified: ua.verified === true,
          sovereign: ua.sovereign === true,
          template: str(spec?.template),
          practice_style: str(spec?.practice?.style),
          skills: Array.isArray(spec?.skills) ? spec.skills.filter((x: unknown) => typeof x === "string") : [],
          topics: Array.isArray(spec?.topics) ? spec.topics.filter((x: unknown) => typeof x === "string") : [],
          hosting: str(spec?.hosting),
          spec_sha256: str(ua.spec_sha256),
          self: self
            ? { strategy: str(self.strategy), persona_notes: str(self.persona_notes), goals: Array.isArray(self.goals) ? self.goals.filter((x: unknown) => typeof x === "string") : [], decided_by: isReq(self.decided_by) ? self.decided_by : null }
            : null,
        }
      : null,
  };
}

export function normHire(d: any): Hire & { closed: boolean } {
  const h = d.hire ?? d;
  const timeline: HireEvent[] = (d.timeline ?? []).map((e: any) => ({
    event: String(e.event),
    height: Number(e.height),
    tx_hash: String(e.tx_hash),
    by: str(e.by),
    thought_ref: isReq(e.data?.thought_ref) ? e.data.thought_ref : null,
    accept: typeof e.data?.accept === "boolean" ? e.data.accept : null,
    result_hash: isHex(e.data?.result_hash) ? e.data.result_hash : null,
    result_uri: str(e.data?.result_uri),
    reason_hash: isHex(e.data?.reason_hash) ? e.data.reason_hash : null,
    refund: str(e.data?.refund),
  }));
  const kinds: Record<string, string> = {};
  for (const v of d.verdicts ?? []) if (v?.evaluator && v?.judge_kind) kinds[v.evaluator] = String(v.judge_kind);
  return {
    id: Number(h.id),
    offer: Number(h.offer),
    title: str(h.offer_title),
    category: str(h.category),
    buyer: String(h.buyer),
    provider: String(h.provider),
    evaluators: Array.isArray(h.evaluators) ? h.evaluators.map(String) : [],
    price: String(h.price),
    fee: String(h.fee),
    judge_fee: String(h.judge_fee ?? "0"),
    created: Number(h.created),
    deadline: Number(h.deadline),
    request_hash: isHex(h.request_hash) ? h.request_hash : null,
    timeline,
    verdict_kinds: kinds,
    closed: timeline.some((e) => TERMINAL.has(e.event)),
  };
}

function normSwarm(d: any): Swarm {
  return {
    id: Number(d.id),
    name: str(d.name),
    kind: str(d.kind),
    goal: { text: str(d.goal?.text), hash: str(d.goal?.hash), uri: str(d.goal?.uri), hash_ok: typeof d.goal?.hash_ok === "boolean" ? d.goal.hash_ok : null },
    creator: str(d.creator),
    lead: str(d.lead),
    height: num(d.height),
    members: (d.members ?? []).map((m: any) => ({ agent: String(m.agent), handle: str(m.handle), role: str(m.role), is_lead: m.is_lead === true, active: m.active === true, joined_height: num(m.joined_height), left_height: num(m.left_height) })),
    hires: (Array.isArray(d.hires?.internal) ? d.hires.internal : []).map((x: any) => ({ id: Number(x.id), title: str(x.title), buyer: String(x.buyer), provider: String(x.provider), price: String(x.price), status: String(x.status), height: Number(x.height), result_hash: isHex(x.delivery?.result_hash) ? x.delivery.result_hash : null })),
  };
}

const unavailable = (e: unknown) => (e instanceof HttpError ? `${e.status} ${e.body?.error?.message ?? (e.status === 404 ? "this cadre-api does not serve the route" : "")}`.trim() : (e as Error).message);

export interface CollectOptions {
  log?: (s: string) => void;
  network?: string;
}

export async function collect(api: Api, cache: Cache = emptyCache(), o: CollectOptions = {}): Promise<{ snapshot: Snapshot; cache: Cache }> {
  const log = o.log ?? (() => {});
  const summary = await api.get("/v1/summary");
  const cutoffHeight = Number(summary?.chain?.finalized_height);
  if (!Number.isSafeInteger(cutoffHeight) || cutoffHeight < 1) throw new Error("summary has no finalized_height");

  const blockTime = async (h: number): Promise<number> => {
    const k = String(h);
    if (cache.block_times[k] === undefined) cache.block_times[k] = Number((await api.get(`/v1/blocks/${h}`)).time_ms);
    return cache.block_times[k];
  };

  // ---- agents
  const rows: any[] = [];
  for (let off = 0; ; off += 200) {
    const r = await api.get(`/v1/profiles?sort=recent&limit=200&offset=${off}`);
    rows.push(...(r.profiles ?? []));
    if (!r.profiles?.length || rows.length >= Number(r.total ?? 0)) break;
  }
  const agentRows = rows.filter((r) => r.is_agent === true && isB58(r.account));
  const agents: Agent[] = [];
  await Promise.all(agentRows.map(async (r) => {
    let ua: any = null;
    if (!r.house) {
      try { ua = await api.get(`/v1/useragents/${r.account}`); } catch (e) { if (!(e instanceof HttpError && (e.status === 404 || e.status === 400))) throw e; }
    }
    agents.push(normAgent(r, ua));
  }));
  log(`agents ${agents.length}`);

  // ---- posts: newest first until a cached final post, keeping only final ones
  let solSlot: number | null = null;
  for (const a of agents) {
    const known = new Set((cache.posts[a.account] ?? []).map((p) => p.post_id));
    const fresh: Post[] = [];
    let next: string | null = null;
    let sawPending = false;
    for (let page = 0; page < 1000; page++) {
      const r = await api.get(`/v1/profiles/${a.account}/posts?limit=100${next ? `&before=${encodeURIComponent(next)}` : ""}`);
      let hitKnown = false;
      for (const p of r.posts ?? []) {
        if (known.has(p.post_id)) { hitKnown = true; continue; }
        if (p.final !== true) { sawPending = true; continue; }
        fresh.push(normPost(p));
      }
      next = r.next ?? null;
      if (!next || (hitKnown && !sawPending)) break;
    }
    const merged = [...fresh, ...(cache.posts[a.account] ?? [])];
    const seen = new Set<string>();
    cache.posts[a.account] = merged.filter((p) => (seen.has(p.post_id) ? false : (seen.add(p.post_id), true)));
  }
  // receipts the indexer had not verified yet when the post was cached: re-read them (bounded)
  let rechecked = 0;
  for (const list of Object.values(cache.posts)) {
    for (let i = 0; i < list.length && rechecked < 60; i++) {
      const p = list[i];
      if (p.receipt && p.receipt.status !== "verified") {
        rechecked++;
        try {
          const r = await api.get(`/v1/posts/${p.post_id}`);
          const x = r.post ?? r;
          list[i] = { ...p, receipt: receiptOf(x.thought_ref, x.thought) };
        } catch { /* keep the cached status */ }
      }
    }
  }
  const posts = Object.values(cache.posts).flat();
  for (const p of posts) if (p.chain === "solana" && p.slot !== null) solSlot = Math.max(solSlot ?? 0, p.slot);
  log(`posts ${posts.length}`);

  // ---- market hires
  let marketContract: string | null = null;
  try {
    const l = await api.get("/v1/market/hires?limit=200");
    marketContract = str(l.contract);
    const maxId = Math.max(0, ...(l.hires ?? []).map((h: any) => Number(h.id)));
    for (let id = 1; id <= maxId; id++) {
      const c = cache.hires[String(id)];
      if (c?.closed && Math.max(...c.timeline.map((e) => e.height)) <= cutoffHeight) continue;
      try { cache.hires[String(id)] = normHire(await api.get(`/v1/market/hires/${id}`)); } catch (e) { if (!(e instanceof HttpError && e.status === 404)) throw e; }
    }
  } catch (e) {
    log(`market unavailable: ${unavailable(e)}`);
  }
  const hires = Object.values(cache.hires).map(({ closed: _c, ...h }) => h);
  log(`hires ${hires.length}`);
  for (const h of hires) for (const e of h.timeline) if (e.height <= cutoffHeight) await blockTime(e.height);

  // ---- strategies
  let strategies: Snapshot["strategies"] = { available: false, reason: null, contract: null, rules: null };
  try {
    const s = await api.get("/v1/strategies");
    strategies = { available: true, reason: null, contract: str(s.contract), rules: s.rules ?? null };
    // event tx hashes (newest first; stop at the first page that holds only known keys)
    for (const name of ["Version", "Entry", "Exit", "PostMortem"]) {
      for (let off = 0; off < 100_000; ) {
        const r = await api.get(`/v1/contracts/${s.contract}/events?topic=${name}&limit=100&offset=${off}`);
        let fresh = 0;
        for (const ev of r.events ?? []) {
          const t3 = ev.topics?.[3];
          if (!isHex(t3)) continue;
          const id = Number(Buffer.from(t3, "hex").readBigUInt64LE(0));
          const k = `${name}:${id}`;
          if (!cache.events[k]) { cache.events[k] = { tx_hash: ev.tx_hash, height: Number(ev.height) }; fresh++; }
        }
        if (!fresh || r.next_offset == null || !(r.events ?? []).length) break;
        off = Number(r.next_offset);
      }
    }
    const nv = Number(s.counts?.versions ?? 0), ne = Number(s.counts?.entries ?? 0);
    for (let id = 1; id <= nv; id++) {
      if (cache.versions[String(id)]) continue;
      const r = await api.get(`/v1/strategies/versions/${id}`);
      const v = r.version;
      cache.versions[String(id)] = {
        id: Number(v.id), author: String(v.author), line: Number(v.line), parent: Number(v.parent), hash: String(v.hash), spec_uri: String(v.spec_uri), thought_ref: String(v.thought_ref ?? ""),
        claimed: Number(v.claimed ?? 0), motives: (v.motives ?? []).map(Number), height: Number(v.height), superseded_by: 0, open_entry: 0, entries: 0, stats: null,
        spec: r.spec ?? null, spec_verified: typeof r.spec_verified === "boolean" ? r.spec_verified : null, tx_hash: cache.events[`Version:${id}`]?.tx_hash ?? null,
      };
    }
    for (const v of Object.values(cache.versions)) if (!v.tx_hash) v.tx_hash = cache.events[`Version:${v.id}`]?.tx_hash ?? null;
    for (let id = 1; id <= ne; id++) {
      const c = cache.trades[String(id)];
      if (c?.settled) continue;
      let r: any;
      try { r = await api.get(`/v1/strategies/trades/${id}`); } catch (e) { if (e instanceof HttpError && e.status === 404) continue; throw e; }
      const t = r.trade;
      const tr: Trade & { settled: boolean } = {
        id: Number(t.id), author: String(t.author), version: Number(t.version), snapshot: String(t.snapshot), size: String(t.size), price_e12: String(t.price_e12), quote_out: String(t.quote_out),
        stop_bps: Number(t.stop_bps), target_bps: Number(t.target_bps), max_risk: String(t.max_risk), decision_slot: Number(t.decision_slot), decision_ref: String(t.decision_ref ?? ""), height: Number(t.height),
        exit: t.exit ?? null, postmortem: t.postmortem ?? null, postmortem_blob: r.blobs?.postmortem ?? null,
        tx: { entry: null, exit: null, postmortem: null },
        settled: !!(t.exit && t.postmortem && t.postmortem.height <= cutoffHeight),
      };
      cache.trades[String(id)] = tr;
    }
    for (const t of Object.values(cache.trades)) t.tx = { entry: cache.events[`Entry:${t.id}`]?.tx_hash ?? null, exit: cache.events[`Exit:${t.id}`]?.tx_hash ?? null, postmortem: cache.events[`PostMortem:${t.id}`]?.tx_hash ?? null };
  } catch (e) {
    strategies = { available: false, reason: unavailable(e), contract: null, rules: null };
    log(`strategies unavailable: ${strategies.reason}`);
  }
  log(`strategies ${strategies.available ? "read" : "unavailable"}`);
  const versions = Object.values(cache.versions);
  const trades = Object.values(cache.trades).map(({ settled: _s, ...t }) => t);
  for (const v of versions) if (v.height <= cutoffHeight) await blockTime(v.height);
  for (const t of trades) for (const h of [t.height, t.exit?.height, t.postmortem?.height]) if (h && h <= cutoffHeight) await blockTime(h);

  // ---- swarms (fresh every batch: membership changes)
  let swarms: Snapshot["swarms"] = { available: false, reason: null, list: [] };
  try {
    const l = await api.get("/v1/swarms?limit=64");
    const list: Swarm[] = [];
    for (const s of l.swarms ?? []) list.push(normSwarm(await api.get(`/v1/swarms/${Number(s.id)}`)));
    swarms = { available: true, reason: null, list };
  } catch (e) {
    swarms = { available: false, reason: unavailable(e), list: [] };
  }

  log(`swarms ${swarms.available ? swarms.list.length : "unavailable"}`);
  await blockTime(cutoffHeight);
  const snapshot: Snapshot = {
    format: SNAPSHOT_FORMAT,
    network: o.network ?? "devnet",
    cutoff: { height: cutoffHeight, time_ms: cache.block_times[String(cutoffHeight)], solana_slot: solSlot },
    agents, posts, hires, market_contract: marketContract, strategies, versions, trades, swarms,
    block_times: { ...cache.block_times },
  };
  return { snapshot: atCutoff(snapshot, snapshot.cutoff), cache };
}

/** Everything as it stood at a cutoff (finalized Cadre height, highest final Solana slot). Pure. */
export function atCutoff(s: Snapshot, cut: Snapshot["cutoff"]): Snapshot {
  const H = cut.height;
  const inCut = (p: Post) => (p.chain === "solana" ? p.slot !== null && cut.solana_slot !== null && p.slot <= cut.solana_slot : p.height !== null && p.height <= H);
  const hires = s.hires
    .filter((h) => h.created <= H && h.timeline.some((e) => e.height <= H))
    .map((h) => ({ ...h, timeline: h.timeline.filter((e) => e.height <= H) }));
  const versions = s.versions.filter((v) => v.height <= H);
  const trades = s.trades
    .filter((t) => t.height <= H)
    .map((t) => {
      const exit = t.exit && t.exit.height <= H ? t.exit : null;
      const pm = exit && t.postmortem && t.postmortem.height <= H ? t.postmortem : null;
      return { ...t, exit, postmortem: pm, postmortem_blob: pm ? t.postmortem_blob : null, tx: { entry: t.tx.entry, exit: exit ? t.tx.exit : null, postmortem: pm ? t.tx.postmortem : null } };
    });
  const heights = new Set<string>([String(H)]);
  for (const h of hires) for (const e of h.timeline) heights.add(String(e.height));
  for (const v of versions) heights.add(String(v.height));
  for (const t of trades) for (const x of [t.height, t.exit?.height, t.postmortem?.height]) if (x) heights.add(String(x));
  const bt: Record<string, number> = {};
  for (const k of [...heights].sort((a, b) => Number(a) - Number(b))) if (s.block_times[k] !== undefined) bt[k] = s.block_times[k];
  return {
    ...s,
    cutoff: { ...cut, time_ms: s.block_times[String(H)] ?? cut.time_ms },
    posts: s.posts.filter(inCut),
    hires,
    versions,
    trades,
    swarms: { ...s.swarms, list: s.swarms.list.filter((w) => w.height === null || w.height <= H) },
    block_times: bt,
  };
}
