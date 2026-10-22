// Agent services market (contracts/market, docs/MARKET.md): the call ABI, record and event
// decoders, and an index built only from the contract's own state (queryContract) and events
// (getContractEvents). Reputation follows the existing indexer rules (SPEC 4.2, profile.ts):
// money from distinct payers, never counts; self-dealing excluded (strict: same operator or an
// operator link; heuristic, labelled: the buyer was funded by the provider before hiring);
// house activity (a house buyer, or a house-labelled account) reported apart, never as organic.
// Pure TS on Buffer, no other deps, so cadre-api, the swarm and the CLI tests can share it.
import { b58decode, b58encode } from "./codec.ts";
import { blake3 } from "./blake3.ts";

export const STATUS = ["open", "delivered", "completed", "rejected", "expired", "declined"] as const;
export type HireStatus = (typeof STATUS)[number];
export const POLICY = ["buyer_names", "offer_list"] as const;
export const MARKET_EVENTS = ["OfferListed", "OfferUpdated", "Hired", "Declined", "Delivered", "Verdict", "Completed", "Rejected", "Expired", "ConfigSet"] as const;
export const MAX = { title: 96, category: 32, uri: 256, ref: 128, reason: 256, evaluators: 4, fee_bps: 5_000, turnaround: 100_800, deadline: 201_600 } as const;

export const eventTopicHex = (name: string) => Buffer.from(blake3(Buffer.from("pearl-sol:event:v1"), Buffer.from(name))).toString("hex");
const TOPIC_NAME = new Map<string, string>(MARKET_EVENTS.map((n) => [eventTopicHex(n), n]));

/** The protocol fee on a price: ceil(price x bps / 10000) (contract `fee_of`). */
export const feeOf = (price: bigint, bps: number) => (price * BigInt(bps) + 9_999n) / 10_000n;

// ---------------------------------------------------------------- ABI

const key = (a: string | Uint8Array) => {
  const k = typeof a === "string" ? b58decode(a) : a;
  if (k.length !== 32) throw new Error("address must be 32 bytes");
  return k;
};

export class W {
  p: number[] = [];
  static method(name: string) { const w = new W(); const b = Buffer.from(name); w.p.push(b.length, ...b); return w; }
  u8(v: number) { if (!Number.isInteger(v) || v < 0 || v > 255) throw new Error("u8 out of range"); this.p.push(v); return this; }
  u16(v: number) { const b = Buffer.alloc(2); b.writeUInt16LE(v); this.p.push(...b); return this; }
  u64(v: bigint | number | string) { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(v)); this.p.push(...b); return this; }
  bool(v: boolean) { return this.u8(v ? 1 : 0); }
  addr(a: string | Uint8Array) { this.p.push(...key(a)); return this; }
  hash(h: string | Uint8Array) { const b = typeof h === "string" ? Buffer.from(h, "hex") : Buffer.from(h); if (b.length !== 32) throw new Error("hash must be 32 bytes (64 hex)"); this.p.push(...b); return this; }
  bytes(b: Uint8Array) { const l = Buffer.alloc(4); l.writeUInt32LE(b.length); this.p.push(...l, ...b); return this; }
  str(s: string, max?: number, what = "text") { const b = Buffer.from(s ?? "", "utf8"); if (max !== undefined && b.length > max) throw new Error(`${what} over ${max} bytes`); return this.bytes(b); }
  evals(v: string[]) { if (v.length > MAX.evaluators) throw new Error("at most 4 evaluators"); this.u8(v.length); for (const a of v) this.addr(a); return this; }
  out() { return Uint8Array.from(this.p); }
  hex() { return Buffer.from(this.p).toString("hex"); }
}

export class R {
  o = 0;
  b: Buffer;
  constructor(b: Uint8Array | string) { this.b = typeof b === "string" ? Buffer.from(b, "hex") : Buffer.from(b); }
  u8() { return this.b[this.o++]; }
  u16() { const v = this.b.readUInt16LE(this.o); this.o += 2; return v; }
  u64() { const v = this.b.readBigUInt64LE(this.o); this.o += 8; return v; }
  bool() { return this.u8() === 1; }
  addr() { const v = b58encode(this.b.subarray(this.o, this.o + 32)); this.o += 32; return v; }
  hash() { const v = this.b.subarray(this.o, this.o + 32).toString("hex"); this.o += 32; return v; }
  str() { const n = this.b.readUInt32LE(this.o); this.o += 4; const v = this.b.subarray(this.o, this.o + n).toString("utf8"); this.o += n; return v; }
  evals() { const n = this.u8(); const v: string[] = []; for (let i = 0; i < n; i++) v.push(this.addr()); return v; }
}

export interface OfferInput {
  title: string; category: string; spec_uri: string; spec_hash: string; price: bigint | string; turnaround: number | bigint;
  policy: number; evaluators: string[]; active: boolean;
}
function offerFields(w: W, o: OfferInput) {
  return w.str(o.title, MAX.title, "title").str(o.category ?? "", MAX.category, "category").str(o.spec_uri ?? "", MAX.uri, "spec_uri")
    .hash(o.spec_hash || "00".repeat(32)).u64(o.price).u64(o.turnaround).u8(o.policy).evals(o.evaluators ?? []).bool(o.active);
}

/** Call input (`method | fields`, CONTRACTS 4) for every market method. */
export const marketArgs = {
  offer: (o: OfferInput) => offerFields(W.method("offer"), o).out(),
  updateOffer: (id: bigint | number, o: OfferInput) => offerFields(W.method("update_offer").u64(id), o).out(),
  hire: (h: { offer: bigint | number; price: bigint | string; request_uri: string; request_hash: string; deadline_blocks: number | bigint; judge_fee: bigint | string; evaluators: string[]; threshold: number }) =>
    W.method("hire").u64(h.offer).u64(h.price).str(h.request_uri, MAX.uri, "request_uri").hash(h.request_hash).u64(h.deadline_blocks).u64(h.judge_fee).evals(h.evaluators).u8(h.threshold).out(),
  decline: (id: bigint | number, reason: string) => W.method("decline").u64(id).str(reason, MAX.reason, "reason").out(),
  deliver: (id: bigint | number, result_hash: string, result_uri: string, thought_ref: string) =>
    W.method("deliver").u64(id).hash(result_hash).str(result_uri, MAX.uri, "result_uri").str(thought_ref ?? "", MAX.ref, "thought_ref").out(),
  verdict: (id: bigint | number, accept: boolean, reason_uri: string, reason_hash: string, thought_ref: string) =>
    W.method("verdict").u64(id).bool(accept).str(reason_uri ?? "", MAX.uri, "reason_uri").hash(reason_hash || "00".repeat(32)).str(thought_ref ?? "", MAX.ref, "thought_ref").out(),
  expire: (id: bigint | number) => W.method("expire").u64(id).out(),
  setConfig: (admin: string, fee_bps: number, fee_to: string, hires_paused: boolean) => W.method("set_config").addr(admin).u16(fee_bps).addr(fee_to).bool(hires_paused).out(),
  instantiate: (admin: string, fee_bps: number, fee_to: string) => new W().addr(admin).u16(fee_bps).addr(fee_to).out(),
};
export const marketQuery = {
  config: () => W.method("config").out(),
  counts: () => W.method("counts").out(),
  offer: (id: bigint | number) => W.method("offer").u64(id).out(),
  hire: (id: bigint | number) => W.method("hire").u64(id).out(),
  quote: (id: bigint | number, judge_fee: bigint | string) => W.method("quote").u64(id).u64(judge_fee).out(),
};

// ---------------------------------------------------------------- records

export interface Offer {
  id: number; provider: string; title: string; category: string; spec_uri: string; spec_hash: string; price: string;
  turnaround: number; policy: (typeof POLICY)[number]; evaluators: string[]; active: boolean; created: number; updated: number; hires: number; completed: number;
}
export function decodeOffer(id: number, hex: string): Offer {
  const r = new R(hex);
  return {
    id, provider: r.addr(), title: r.str(), category: r.str(), spec_uri: r.str(), spec_hash: r.hash(), price: r.u64().toString(), turnaround: Number(r.u64()),
    policy: POLICY[r.u8()] ?? "buyer_names", evaluators: r.evals(), active: r.bool(), created: Number(r.u64()), updated: Number(r.u64()), hires: Number(r.u64()), completed: Number(r.u64()),
  };
}

export interface Hire {
  id: number; offer: number; buyer: string; provider: string; price: string; fee: string; judge_fee: string; total: string; created: number; deadline: number;
  request_uri: string; request_hash: string; evaluators: string[]; threshold: number; status: HireStatus; accepts: number; rejects: number;
  voted: string[]; accepted_by: string[]; result_hash: string | null; result_uri: string | null; delivered: number | null; closed: number | null;
}
export function decodeHire(id: number, hex: string): Hire {
  const r = new R(hex);
  const offer = Number(r.u64()), buyer = r.addr(), provider = r.addr();
  const price = r.u64(), fee = r.u64(), judge_fee = r.u64();
  const created = Number(r.u64()), deadline = Number(r.u64());
  const request_uri = r.str(), request_hash = r.hash(), evaluators = r.evals();
  const threshold = r.u8(), status = STATUS[r.u8()] ?? "open", accepts = r.u8(), rejects = r.u8(), voted = r.u8(), acc = r.u8();
  const result_hash = r.hash(), result_uri = r.str(), delivered = Number(r.u64()), closed = Number(r.u64());
  const bits = (m: number) => evaluators.filter((_, i) => m & (1 << i));
  return {
    id, offer, buyer, provider, price: price.toString(), fee: fee.toString(), judge_fee: judge_fee.toString(), total: (price + fee + judge_fee).toString(), created, deadline,
    request_uri, request_hash, evaluators, threshold, status, accepts, rejects, voted: bits(voted), accepted_by: bits(acc),
    result_hash: /^0+$/.test(result_hash) ? null : result_hash, result_uri: result_uri || null, delivered: delivered || null, closed: closed || null,
  };
}
export interface MarketConfig { admin: string; fee_bps: number; fee_to: string; hires_paused: boolean }
export function decodeConfig(hex: string): MarketConfig {
  const r = new R(hex);
  return { admin: r.addr(), fee_bps: r.u16(), fee_to: r.addr(), hires_paused: r.bool() };
}
export function decodeQuote(hex: string) {
  const r = new R(hex);
  return { price: r.u64().toString(), fee: r.u64().toString(), judge_fee: r.u64().toString(), total: r.u64().toString() };
}

// ---------------------------------------------------------------- events

export interface MarketEvent { name: string; height: number; tx_hash: string | null; id: number | null; who: string | null; party: string | null; data: Record<string, unknown> }
const idOf = (t: string | undefined) => (t ? Number(Buffer.from(t, "hex").readBigUInt64LE(0)) : null);
const addrOf = (t: string | undefined) => (t ? b58encode(Buffer.from(t, "hex")) : null);

export function decodeEvent(ev: { topics: string[]; data: string; height: number; tx_hash?: string }): MarketEvent | null {
  const name = TOPIC_NAME.get(ev.topics?.[0]?.toLowerCase());
  if (!name) return null;
  const r = new R(ev.data ?? "");
  const out: MarketEvent = { name, height: ev.height, tx_hash: ev.tx_hash ?? null, id: name === "ConfigSet" ? null : idOf(ev.topics[1]), who: addrOf(name === "ConfigSet" ? ev.topics[1] : ev.topics[2]), party: addrOf(ev.topics[3]), data: {} };
  try {
    switch (name) {
      case "OfferListed": case "OfferUpdated": out.data = { price: r.u64().toString(), turnaround: Number(r.u64()), policy: POLICY[r.u8()], title: r.str(), category: r.str(), active: r.bool() }; break;
      case "Hired": out.data = { offer: Number(r.u64()), price: r.u64().toString(), fee: r.u64().toString(), judge_fee: r.u64().toString(), deadline: Number(r.u64()), threshold: r.u8(), evaluators: r.evals(), request_hash: r.hash(), request_uri: r.str() }; break;
      case "Declined": out.data = { refund: r.u64().toString(), reason: r.str() }; break;
      case "Delivered": out.data = { result_hash: r.hash(), result_uri: r.str(), thought_ref: r.str() || null }; break;
      case "Verdict": out.data = { accept: r.bool(), reason_hash: r.hash(), reason_uri: r.str() || null, result_hash: r.hash(), thought_ref: r.str() || null }; break;
      case "Completed": out.data = { buyer: r.addr(), price: r.u64().toString(), fee: r.u64().toString(), judge_fee: r.u64().toString(), fee_to: r.addr() }; break;
      case "Rejected": out.data = { refund: r.u64().toString(), judge_fee: r.u64().toString() }; break;
      case "Expired": out.data = { delivered: r.bool(), refund: r.u64().toString() }; break;
      case "ConfigSet": out.data = { fee_bps: r.u16(), fee_to: r.addr(), hires_paused: r.bool() }; break;
    }
  } catch {
    out.data = { undecoded: ev.data };
  }
  return out;
}

// ---------------------------------------------------------------- index

export type Rpc = <T = any>(method: string, params: unknown) => Promise<T>;

/** Everything the contract holds, refreshed on demand (closed hires are cached for good). */
export class MarketIndex {
  address: string;
  rpc: Rpc;
  config: MarketConfig | null = null;
  offers: Offer[] = [];
  hires = new Map<number, Hire>();
  events: MarketEvent[] = []; // oldest first
  private eventTotal = 0;
  at = 0;
  height = 0;
  constructor(address: string, rpc: Rpc) { this.address = address; this.rpc = rpc; }

  private async query(args: Uint8Array): Promise<string> {
    const r = await this.rpc<any>("queryContract", { address: this.address, args: Buffer.from(args).toString("hex") });
    if (!r?.ok) throw new Error(`queryContract: ${r?.error ? Buffer.from(r.error, "hex").toString("utf8") : "failed"}`);
    return r.return as string;
  }
  async quote(offer: number, judgeFee: bigint) { return decodeQuote(await this.query(marketQuery.quote(offer, judgeFee))); }

  async refresh(maxAgeMs = 0): Promise<this> {
    if (maxAgeMs && Date.now() - this.at < maxAgeMs) return this;
    const [cfg, counts, info] = await Promise.all([this.query(marketQuery.config()), this.query(marketQuery.counts()), this.rpc<any>("getChainInfo", {})]);
    this.config = decodeConfig(cfg);
    this.height = info?.height ?? 0;
    const c = new R(counts);
    const nOffers = Number(c.u64()), nHires = Number(c.u64());
    const offs: Offer[] = [];
    for (let i = 0; i < nOffers; i += 16) {
      const ids = [...Array(Math.min(16, nOffers - i)).keys()].map((k) => i + k);
      offs.push(...(await Promise.all(ids.map(async (id) => decodeOffer(id, await this.query(marketQuery.offer(id)))))));
    }
    this.offers = offs;
    const open = [...Array(nHires).keys()].filter((id) => { const h = this.hires.get(id); return !h || h.status === "open" || h.status === "delivered"; });
    for (let i = 0; i < open.length; i += 16) {
      await Promise.all(open.slice(i, i + 16).map(async (id) => this.hires.set(id, decodeHire(id, await this.query(marketQuery.hire(id))))));
    }
    await this.pullEvents();
    this.at = Date.now();
    return this;
  }

  /** New events since the last pull (the RPC lists newest first). */
  private async pullEvents() {
    const first = await this.rpc<any>("getContractEvents", { address: this.address, limit: 1, offset: 0 });
    const total = Number(first?.total ?? 0);
    const want = total - this.eventTotal;
    if (want <= 0) return;
    const fresh: any[] = [];
    for (let off = 0; off < want; off += 200) {
      const p = await this.rpc<any>("getContractEvents", { address: this.address, limit: Math.min(200, want - off), offset: off });
      fresh.push(...(p?.events ?? []));
    }
    fresh.reverse();
    for (const e of fresh) {
      const d = decodeEvent(e);
      if (d) this.events.push(d);
    }
    this.eventTotal = total;
  }

  offer(id: number) { return this.offers[id] ?? null; }
  hireEvents(id: number) { return this.events.filter((e) => e.id === id && e.name !== "OfferListed" && e.name !== "OfferUpdated"); }
}

// ---------------------------------------------------------------- reputation

export interface Context {
  /** Accounts labelled house (profile house flag, or a declared house list). */
  isHouse: (a: string) => boolean;
  /** Operator of an account from its profile (null = self). */
  operatorOf: (a: string) => string | null;
  /** Transfers into `to` (indexed blocks), for the funding heuristic. */
  transfersTo: (to: string) => { from: string; height: number }[];
}

/** Why a hire's payment does not count toward the provider's organic record, or null. */
export function exclusion(h: Hire, ctx: Context): { kind: "self_dealt" | "house"; reason: string; heuristic: boolean } | null {
  // house first: house agents share the declared house operator, so the operator rule below would
  // call every house-to-house hire self-dealt; it is house activity (excluded from organic either way)
  if (ctx.isHouse(h.buyer)) return { kind: "house", reason: "house buyer: house activity, not organic", heuristic: false };
  const ob = ctx.operatorOf(h.buyer), op = ctx.operatorOf(h.provider);
  if (ob && (ob === h.provider || ob === op)) return { kind: "self_dealt", reason: "buyer and provider share an operator", heuristic: false };
  if (op && op === h.buyer) return { kind: "self_dealt", reason: "the buyer operates the provider", heuristic: false };
  const fund = ctx.transfersTo(h.buyer).find((t) => t.from === h.provider && t.height <= h.created);
  if (fund) return { kind: "self_dealt", reason: `heuristic: the buyer was funded by the provider (Transfer at #${fund.height})`, heuristic: true };
  return null;
}

export function providerRecord(hires: Hire[], provider: string, ctx: Context) {
  const mine = hires.filter((h) => h.provider === provider);
  const done = mine.filter((h) => h.status === "completed");
  let organic = 0n, house = 0n, selfDealt = 0n;
  const payers = new Map<string, bigint>();
  for (const h of done) {
    const x = exclusion(h, ctx);
    const p = BigInt(h.price);
    if (x?.kind === "self_dealt") selfDealt += p;
    else if (x?.kind === "house") house += p;
    else { organic += p; payers.set(h.buyer, (payers.get(h.buyer) ?? 0n) + p); }
  }
  const top = [...payers.values()].reduce((a, b) => (b > a ? b : a), 0n);
  const independent = done.filter((h) => h.accepted_by.some((e) => e !== h.buyer)).length;
  const turn = done.filter((h) => h.delivered).map((h) => h.delivered! - h.created).sort((a, b) => a - b);
  return {
    provider,
    paid_volume_organic: organic.toString(),
    distinct_payers: payers.size,
    top_payer_share_bps: organic > 0n ? Number((top * 10_000n) / organic) : null,
    house_volume: house.toString(),
    self_dealt_excluded: selfDealt.toString(),
    hires: mine.length,
    completed: done.length,
    rejected: mine.filter((h) => h.status === "rejected").length,
    delivered_then_expired: mine.filter((h) => h.status === "expired" && h.delivered).length,
    expired_undelivered: mine.filter((h) => h.status === "expired" && !h.delivered).length,
    declined: mine.filter((h) => h.status === "declined").length,
    independent_evaluation_share_bps: done.length ? Math.round((independent * 10_000) / done.length) : null,
    median_turnaround_blocks: turn.length ? turn[Math.floor((turn.length - 1) / 2)] : null,
    rule: "paid volume = completed hire prices from distinct payers; self-dealing (shared operator, operator link, or funded by the provider: heuristic, labelled) and house buyers are excluded and shown apart; counts are shown, never ranked on",
  };
}

/** Whether an address string is a plausible base58 32-byte key. */
export function isKey(s: unknown): s is string {
  if (typeof s !== "string") return false;
  try { return b58decode(s).length === 32; } catch { return false; }
}
