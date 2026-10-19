// A small fake agent chain for agent-profile route tests and headless screenshots:
// a fake node JSON-RPC (real tx bytes, so txFacts decodes them) plus an empty Solana RPC.
// Fixture data only; never shown as real figures.
import { createServer } from "node:http";
import { b58encode } from "../src/codec.ts";

export const key = (i: number) => b58encode(Uint8Array.from({ length: 32 }, (_, k) => (k === 0 ? i : (i * 31 + k) & 0xff)));
export const A = { seller: key(11), buyer1: key(12), buyer2: key(13), buyer3: key(14), evaluator: key(15), griefer: key(16), miner: key(17), sender: key(18) };
const raw = (b58: string) => {
  const ALPH = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
  let n = 0n;
  for (const c of b58) n = n * 58n + BigInt(ALPH.indexOf(c));
  const out = new Uint8Array(32);
  for (let i = 31; i >= 0; i--) { out[i] = Number(n & 0xffn); n >>= 8n; }
  return out;
};
const jid = (c: string) => c.repeat(64).slice(0, 64);
export const J = { a: jid("a1"), b: jid("b2"), c: jid("c3"), d: jid("d4"), e: jid("e5"), f: jid("f6") };

function txB64(kind: number, from: string, body: Buffer) {
  const h = Buffer.alloc(54);
  h[0] = 1;
  Buffer.from(raw(from)).copy(h, 5);
  h[53] = kind;
  return Buffer.concat([h, body, Buffer.alloc(64)]).toString("base64");
}
const sigList = (keys: string[]) => Buffer.concat([Buffer.from([keys.length]), ...keys.flatMap((k) => [Buffer.from(raw(k)), Buffer.alloc(64)])]);
const hx = (h: string) => Buffer.from(h, "hex");
const tx = (kind: string, k: number, from: string, body: Buffer) => ({ tx_hash: jid(String(k)), kind, from, fee: "1000", tx: txB64(k, from, body) });

export function fixtureChain() {
  const jobs = [
    { job_id: J.a, poster: A.buyer1, status: "completed", protocol_fee: "4000000000", worker_pay: "20000000000", posted_height: 3, closed_height: 9 },
    { job_id: J.b, poster: A.buyer2, status: "completed", protocol_fee: "2500000000", worker_pay: "12000000000", posted_height: 4, closed_height: 14 },
    { job_id: J.c, poster: A.buyer3, status: "completed", protocol_fee: "1000000000", worker_pay: "5000000000", posted_height: 5, closed_height: 12 },
    { job_id: J.d, poster: A.seller, status: "completed", protocol_fee: "3000000000", worker_pay: "9000000000", posted_height: 6, closed_height: 13 },
    { job_id: J.e, poster: A.buyer2, status: "expired", protocol_fee: "800000000", worker_pay: "3000000000", posted_height: 6, closed_height: 18, deliverer: A.seller },
    { job_id: J.f, poster: A.buyer1, status: "open", protocol_fee: "1200000000", worker_pay: "6000000000", posted_height: 7, closed_height: 0, deliverer: null },
  ].map((j, i) => ({ kind: "job", deliverer: A.seller, subsidy_paid: "0", cap_remaining: "0", blocks_mined: 0, max_blocks: 5, deadline_height: 999, index: i, hash_a: jid("0"), data_uri: "", evaluators: [A.evaluator], threshold: 1, ...j }));
  const name = Buffer.from("summarizer-7");
  const uri = Buffer.from(process.env.FIXTURE_CARD_URI ?? "");
  const reg = Buffer.concat([Buffer.from([name.length]), name, Buffer.from([uri.length & 0xff, uri.length >> 8]), uri]);
  const txs: Record<number, any[]> = {
    2: [tx("RegisterAgent", 1, A.seller, reg)],
    8: [tx("SubmitResult", 3, A.seller, Buffer.concat([hx(J.a), hx(jid("9")), Buffer.alloc(2)])), tx("SubmitResult", 3, A.griefer, Buffer.concat([hx(J.f), hx(jid("8")), Buffer.alloc(2)]))],
    9: [tx("CompleteJob", 4, A.sender, Buffer.concat([hx(J.a), sigList([A.evaluator])]))],
    10: [tx("RejectResult", 13, A.sender, Buffer.concat([hx(J.f), Buffer.from(raw(A.griefer)), hx(jid("8")), sigList([A.evaluator])]))],
    11: [tx("SubmitResult", 3, A.seller, Buffer.concat([hx(J.c), hx(jid("7")), Buffer.alloc(2)]))],
    12: [tx("CompleteJob", 4, A.sender, Buffer.concat([hx(J.c), sigList([A.evaluator])]))],
    13: [tx("CompleteJob", 4, A.sender, Buffer.concat([hx(J.d), sigList([A.evaluator])]))],
    14: [tx("CompleteJob", 4, A.sender, Buffer.concat([hx(J.b), sigList([A.buyer2])]))],
  };
  const tip = 20;
  const block = (h: number) => h < 1 || h > tip ? null : {
    hash: jid(`h${h}`), fees: "1000", subsidy: h === 5 || h === 6 ? "640000000" : "0", reward: "1000", proof_len: 100,
    header_fields: { height: h, prev_hash: jid(`h${h - 1}`), timestamp_ms: Date.now() - (tip - h) * 60_000, miner: h % 3 === 0 ? A.seller : A.miner, job_id: h === 5 || h === 6 ? J.a : "0".repeat(64), proof_hash: jid("p"), target_bits: 1 },
    txs: txs[h] ?? [],
  };
  return { jobs, block, tip };
}

export function fakeNode() {
  const c = fixtureChain();
  const handle = (m: string, p: any) => {
    switch (m) {
      case "getChainInfo": return { genesis_hash: jid("9e"), height: c.tip, finalized_height: c.tip - 2, finality_k: 2 };
      case "getParams": return { alpha_ppm: 800000, emission_ppm: 10000, min_fee: "1000" };
      case "getBlock": return c.block(p.height);
      case "listJobs": return { total: c.jobs.length, jobs: c.jobs };
      case "getBridgeEvents": return { withdrawals: [] };
      case "getAgent": return p.account === A.seller ? { account: A.seller, name: "summarizer-7", uri: process.env.FIXTURE_CARD_URI ?? "", registered_height: 2, updated_height: 2 } : null;
      case "getRejectStats": return p.account === A.griefer ? { account: p.account, rejected: 1, rejected_fee: "1200000000", first_height: 10, last_height: 10 } : { account: p.account, rejected: 0, rejected_fee: "0", first_height: 0, last_height: 0 };
      default: throw new Error("no " + m);
    }
  };
  return createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const q = JSON.parse(body);
      const one = (x: any) => { try { return { jsonrpc: "2.0", id: x.id, result: handle(x.method, x.params) }; } catch (e) { return { jsonrpc: "2.0", id: x.id, error: { message: (e as Error).message } }; } };
      res.end(JSON.stringify(Array.isArray(q) ? q.map(one) : one(q)));
    });
  });
}

export function fakeSolana() {
  return createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const q = JSON.parse(body);
      const r = (x: any) => ({ jsonrpc: "2.0", id: x.id, result: x.method === "getMultipleAccounts" ? { value: x.params[0].map(() => null) } : x.method === "getProgramAccounts" || x.method === "getSignaturesForAddress" ? [] : null });
      res.end(JSON.stringify(Array.isArray(q) ? q.map(r) : r(q)));
    });
  });
}
