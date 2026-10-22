// Vectors for src/market.rs, encoded and decoded by the TS codec (apps/explorer/src/market.ts).
// Regenerate: node crates/pearl-cli/tests/fixtures/market_vectors.ts > crates/pearl-cli/tests/fixtures/market_vectors.json
import { W, decodeOffer, decodeHire, decodeConfig, decodeQuote, decodeEvent, eventTopicHex, marketArgs } from "../../../../apps/explorer/src/market.ts";
import { b58encode, b58decode } from "../../../../apps/explorer/src/codec.ts";
const A = (n: number) => b58encode(Uint8Array.from({ length: 32 }, (_, i) => (i * 7 + n) & 255));
const P = A(1), B = A(2), J = A(3), J2 = A(4), F = A(5);
const kh = (a: string) => Buffer.from(b58decode(a)).toString("hex");
const idt = (id: number) => { const b = Buffer.alloc(32); b.writeBigUInt64LE(BigInt(id)); return b.toString("hex"); };
const out: any = { addrs: { P, B, J, J2, F } };
const offer = new W().addr(P).str("Summarize a block range").str("research").str("blob:" + "ab".repeat(32)).hash("cd".repeat(32)).u64(2_500_000_000n).u64(30).u8(1).evals([J, J2]).bool(true).u64(4850).u64(4851).u64(3).u64(2).hex();
out.offer = { hex: offer, json: decodeOffer(0, offer) };
const hire = new W().u64(0).addr(B).addr(P).u64(2_500_000_000n).u64(500_000_000n).u64(100_000_000n).u64(4900).u64(4990).str("blob:" + "11".repeat(32)).hash("11".repeat(32)).evals([J, J2]).u8(1).u8(2).u8(1).u8(0).u8(1).u8(1).hash("22".repeat(32)).str("blob:" + "22".repeat(32)).u64(4910).u64(4912).hex();
out.hire = { hex: hire, json: decodeHire(7, hire) };
const open = new W().u64(1).addr(B).addr(P).u64(5).u64(1).u64(0).u64(10).u64(40).str("").hash("00".repeat(32)).evals([J]).u8(1).u8(0).u8(0).u8(0).u8(0).u8(0).hash("00".repeat(32)).str("").u64(0).u64(0).hex();
out.hire_open = { hex: open, json: decodeHire(8, open) };
const cfg = new W().addr(J).u16(2000).addr(F).bool(false).hex();
out.config = { hex: cfg, json: decodeConfig(cfg) };
const q = new W().u64(2_500_000_000n).u64(500_000_000n).u64(100_000_000n).u64(3_100_000_000n).hex();
out.quote = { hex: q, json: decodeQuote(q) };
const ev = (name: string, topics: string[], data: string, height: number) => ({ topics: [eventTopicHex(name), ...topics], data, height, tx_hash: "ee".repeat(32) });
const evs = [
  ev("OfferListed", [idt(0), kh(P)], new W().u64(2_500_000_000n).u64(30).u8(1).str("Summarize a block range").str("research").bool(true).hex(), 4850),
  ev("Hired", [idt(7), kh(B), kh(P)], new W().u64(0).u64(2_500_000_000n).u64(500_000_000n).u64(100_000_000n).u64(4990).u8(1).evals([J, J2]).hash("11".repeat(32)).str("blob:" + "11".repeat(32)).hex(), 4900),
  ev("Delivered", [idt(7), kh(P), kh(B)], new W().hash("22".repeat(32)).str("blob:" + "22".repeat(32)).str("req_abc123").hex(), 4910),
  ev("Verdict", [idt(7), kh(J), kh(P)], new W().bool(true).hash("33".repeat(32)).str("blob:" + "33".repeat(32)).hash("22".repeat(32)).str("").hex(), 4912),
  ev("Completed", [idt(7), kh(P), kh(B)], new W().addr(B).u64(2_500_000_000n).u64(500_000_000n).u64(100_000_000n).addr(F).hex(), 4912),
  ev("Rejected", [idt(9), kh(P), kh(B)], new W().u64(3_000_000_000n).u64(100_000_000n).hex(), 4920),
  ev("Declined", [idt(10), kh(P), kh(B)], new W().u64(3_100_000_000n).str("busy").hex(), 4921),
  ev("Expired", [idt(11), kh(P), kh(B)], new W().bool(false).u64(3_100_000_000n).hex(), 4922),
  ev("ConfigSet", [kh(J)], new W().u16(2500).addr(F).bool(true).hex(), 4930),
];
out.events = evs.map((e) => ({ raw: e, json: decodeEvent(e) }));
out.hire_args = Buffer.from(marketArgs.hire({ offer: 0, price: 2_500_000_000n, request_uri: "blob:" + "11".repeat(32), request_hash: "11".repeat(32), deadline_blocks: 60, judge_fee: 100_000_000n, evaluators: [J], threshold: 1 })).toString("hex");
out.offer_args = Buffer.from(marketArgs.offer({ title: "T", category: "c", spec_uri: "blob:" + "ab".repeat(32), spec_hash: "ab".repeat(32), price: 1_000_000_000n, turnaround: 30, policy: 0, evaluators: [], active: true })).toString("hex");
console.log(JSON.stringify(out, null, 1));
