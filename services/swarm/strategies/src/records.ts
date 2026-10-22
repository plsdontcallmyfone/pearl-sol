// The strategies contract's ABI (contracts/strategies/src/lib.rs): call encoders for the runner and
// query decoders shared with cadre-api. Dependency-free so cadre-api can import it as is.
const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
function b58(bytes: Uint8Array): string {
  let n = 0n;
  for (const b of bytes) n = n * 256n + BigInt(b);
  let s = "";
  while (n > 0n) { s = B58[Number(n % 58n)] + s; n /= 58n; }
  for (const b of bytes) { if (b !== 0) break; s = "1" + s; }
  return s;
}

class W {
  p: number[] = [];
  m(name: string) { const b = Buffer.from(name); this.p.push(b.length, ...b); return this; }
  u8(v: number) { this.p.push(v & 0xff); return this; }
  u32(v: number) { const b = Buffer.alloc(4); b.writeUInt32LE(v >>> 0); this.p.push(...b); return this; }
  u64(v: bigint | number | string) { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(v)); this.p.push(...b); return this; }
  h(hex: string) { const b = Buffer.from(hex, "hex"); if (b.length !== 32) throw new Error("hash must be 32 bytes hex"); this.p.push(...b); return this; }
  s(t: string) { const b = Buffer.from(t, "utf8"); this.u32(b.length); this.p.push(...b); return this; }
  out() { return Uint8Array.from(this.p); }
}

export const call = {
  publish: (hash: string, parent: bigint | number, specUri: string, thoughtRef: string, claimed: number, motives: (bigint | number)[]) => {
    const w = new W().m("publish").h(hash).u64(parent).s(specUri).s(thoughtRef).u8(claimed).u8(motives.length);
    for (const m of motives) w.u64(m);
    return w.out();
  },
  enter: (o: { version: bigint | number; snapshot: string; size: bigint; priceE12: bigint; quoteOut: bigint; stopBps: number; targetBps: number; maxRisk: bigint; decisionSlot: number; decisionRef: string }) =>
    new W().m("enter").u64(o.version).h(o.snapshot).u64(o.size).u64(o.priceE12).u64(o.quoteOut).u32(o.stopBps).u32(o.targetBps).u64(o.maxRisk).u64(o.decisionSlot).s(o.decisionRef).out(),
  exit: (o: { entry: bigint | number; fillSlot: number; fillOut: bigint; fillPriceE12: bigint; exitSlot: number; reason: number; proceeds: bigint; record: string }) =>
    new W().m("exit").u64(o.entry).u64(o.fillSlot).u64(o.fillOut).u64(o.fillPriceE12).u64(o.exitSlot).u8(o.reason).u64(o.proceeds).h(o.record).out(),
  postmortem: (entry: bigint | number, result: string, buckets: number, analyzer: string, thoughtRef: string) => new W().m("postmortem").u64(entry).h(result).u8(buckets).h(analyzer).s(thoughtRef).out(),
};
export const q = {
  counts: () => new W().m("counts").out(),
  version: (id: bigint | number) => new W().m("version").u64(id).out(),
  trade: (id: bigint | number) => new W().m("trade").u64(id).out(),
};

class R {
  o = 0;
  b: Buffer;
  constructor(b: Uint8Array | string) { this.b = typeof b === "string" ? Buffer.from(b, "hex") : Buffer.from(b); }
  u8() { return this.b[this.o++]; }
  u32() { const v = this.b.readUInt32LE(this.o); this.o += 4; return v; }
  u64() { const v = this.b.readBigUInt64LE(this.o); this.o += 8; return v; }
  i64() { const v = this.b.readBigInt64LE(this.o); this.o += 8; return v; }
  addr() { const v = b58(this.b.subarray(this.o, this.o + 32)); this.o += 32; return v; }
  hash() { const v = this.b.subarray(this.o, this.o + 32).toString("hex"); this.o += 32; return v; }
  str() { const n = this.u32(); const v = this.b.subarray(this.o, this.o + n).toString("utf8"); this.o += n; return v; }
  done() { return this.o >= this.b.length; }
}

export const BUCKET_NAMES = ["INFORMATION", "THESIS", "EXECUTION", "SIZING", "RISK_MANAGEMENT", "LEVERAGE", "VARIANCE"] as const;
export const OUTCOMES = ["", "win", "loss", "flat", "no_fill"] as const;
export const REASONS = ["", "stop", "target", "time", "condition_gone", "no_fill"] as const;

export interface VersionRec {
  id: number; author: string; line: number; parent: number; hash: string; spec_uri: string; thought_ref: string; claimed: number; motives: number[];
  height: number; superseded_by: number; open_entry: number; entries: number;
  stats: { trades: number; wins: number; losses: number; flats: number; no_fills: number; pnl_sum: string; lucky_wins: number; buckets: Record<string, number> };
}

export function decodeVersion(id: number, ret: Uint8Array | string): VersionRec {
  const r = new R(ret);
  const author = r.addr(), line = Number(r.u64()), parent = Number(r.u64()), hash = r.hash(), spec_uri = r.str(), thought_ref = r.str(), claimed = r.u8();
  const n = r.u8();
  const motives: number[] = [];
  for (let i = 0; i < n; i++) motives.push(Number(r.u64()));
  const height = Number(r.u64()), superseded_by = Number(r.u64()), open_entry = Number(r.u64()), entries = Number(r.u64());
  const trades = Number(r.u64()), wins = Number(r.u64()), losses = Number(r.u64()), flats = Number(r.u64()), no_fills = Number(r.u64()), pnl_sum = r.i64().toString(), lucky_wins = Number(r.u64());
  const buckets: Record<string, number> = {};
  for (const b of BUCKET_NAMES) buckets[b] = Number(r.u64());
  return { id, author, line, parent, hash, spec_uri, thought_ref, claimed, motives, height, superseded_by, open_entry, entries, stats: { trades, wins, losses, flats, no_fills, pnl_sum, lucky_wins, buckets } };
}

export interface TradeRec {
  id: number; author: string; version: number; snapshot: string; size: string; price_e12: string; quote_out: string; stop_bps: number; target_bps: number; max_risk: string; decision_slot: number; decision_ref: string; height: number;
  exit: null | { fill_slot: number; fill_out: string; fill_price_e12: string; exit_slot: number; reason: string; proceeds: string; pnl: string; record: string; height: number };
  postmortem: null | { result: string; outcome: string; buckets: number; names: string[]; analyzer: string; thought_ref: string; height: number; used_by: number };
}

export function decodeTrade(id: number, ret: Uint8Array | string): TradeRec {
  const r = new R(ret);
  const t: TradeRec = {
    id, author: r.addr(), version: Number(r.u64()), snapshot: r.hash(), size: r.u64().toString(), price_e12: r.u64().toString(), quote_out: r.u64().toString(), stop_bps: r.u32(), target_bps: r.u32(), max_risk: r.u64().toString(), decision_slot: Number(r.u64()), decision_ref: r.str(), height: Number(r.u64()), exit: null, postmortem: null,
  };
  if (r.u8() === 1) t.exit = { fill_slot: Number(r.u64()), fill_out: r.u64().toString(), fill_price_e12: r.u64().toString(), exit_slot: Number(r.u64()), reason: REASONS[r.u8()] ?? "?", proceeds: r.u64().toString(), pnl: r.i64().toString(), record: r.hash(), height: Number(r.u64()) };
  if (r.u8() === 1) {
    const result = r.hash(), outcome = OUTCOMES[r.u8()] ?? "?", buckets = r.u8();
    t.postmortem = { result, outcome, buckets, names: BUCKET_NAMES.filter((_, i) => buckets & (1 << i)), analyzer: r.hash(), thought_ref: r.str(), height: Number(r.u64()), used_by: Number(r.u64()) };
  }
  return t;
}

export function decodeCounts(ret: Uint8Array | string): { versions: number; entries: number; start_height: number } {
  const r = new R(ret);
  return { versions: Number(r.u64()), entries: Number(r.u64()), start_height: Number(r.u64()) };
}
