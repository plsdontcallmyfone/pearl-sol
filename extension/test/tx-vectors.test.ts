// Byte-for-byte parity with the node's own vectors (chain/agent-types/vectors).
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { b58, fromHex, hex, key32 } from "../src/core/bytes";
import { pubkeyOf, sha256, signRaw, blake3 } from "../src/core/crypto";
import { KIND, PostJobBody, TxBody, decodeTx, encodePostJob, framedMessage, signTx, signingBytes, txHash, verifyTx } from "../src/chain/tx";
import { RESERVED, decodeCoinbase, depositMessage, depositOrderMessage, jobIdForPost, orderJobId, rejectMessage, resultMessage, routeClearMessage } from "../src/chain/messages";

const dir = resolve(__dirname, "../../chain/agent-types/vectors");
// tx.json plus RejectResult (kind 13), published in its own file like standing.json
const rejv = JSON.parse(readFileSync(resolve(dir, "reject.json"), "utf8"));
const txv = [...JSON.parse(readFileSync(resolve(dir, "tx.json"), "utf8")), ...rejv.txs];
const msgv = JSON.parse(readFileSync(resolve(dir, "messages.json"), "utf8"));

function postJob(j: any): PostJobBody {
  return {
    m: j.m, n: j.n, k: j.k, config: fromHex(j.config), hashA: fromHex(j.hash_a), hashB: fromHex(j.hash_b),
    dataUri: j.data_uri, workerPay: BigInt(j.worker_pay), protocolFee: BigInt(j.protocol_fee),
    deadlineHeight: BigInt(j.deadline_height), maxBlocks: j.max_blocks, evaluators: j.evaluators.map(key32), threshold: j.threshold,
  };
}
const sigs = (s: any[]) => s.map((x) => ({ pubkey: key32(x.pubkey), sig: fromHex(x.sig) }));

function body(v: any): TxBody {
  const b = v.body;
  switch (v.kind_name) {
    case "Transfer": return { kind: "Transfer", to: key32(b.to), amount: BigInt(b.amount) };
    case "RegisterAgent": return { kind: "RegisterAgent", name: b.name, uri: b.uri };
    case "PostJob": return { kind: "PostJob", ...postJob(b) };
    case "SubmitResult": return { kind: "SubmitResult", jobId: fromHex(b.job_id), resultHash: fromHex(b.result_hash), uri: b.uri };
    case "CompleteJob": return { kind: "CompleteJob", jobId: fromHex(b.job_id), sigs: sigs(b.sigs) };
    case "ExpireJob": return { kind: "ExpireJob", jobId: fromHex(b.job_id) };
    case "BridgeUnlock": return { kind: "BridgeUnlock", solanaNonce: BigInt(b.solana_nonce), recipient: key32(b.recipient), amountSpl: BigInt(b.amount_spl), jobOrder: b.job_order ? postJob(b.job_order) : null, sigs: sigs(b.sigs) };
    case "BridgeLock": return { kind: "BridgeLock", solanaRecipient: key32(b.solana_recipient), amount: BigInt(b.amount) };
    case "AdminConfig": return { kind: "AdminConfig", key: b.key, value: fromHex(b.value) };
    case "SetRoute": return { kind: "SetRoute", solanaRecipient: key32(b.solana_recipient), routeBps: b.route_bps, untilHeight: BigInt(b.until_height), cap: BigInt(b.cap), releaseKey: key32(b.release_key) };
    case "ClearRoute": return { kind: "ClearRoute", account: key32(b.account), routeNonce: BigInt(b.route_nonce), sig: fromHex(b.sig) };
    case "RejectResult": return { kind: "RejectResult", jobId: fromHex(b.job_id), deliverer: key32(b.deliverer), resultHash: fromHex(b.result_hash), sigs: sigs(b.sigs) };
  }
  throw new Error(`unhandled ${v.kind_name}`);
}

describe("agent-chain tx vectors", () => {
  it("covers every tx kind", () => {
    expect(new Set(txv.map((v: any) => v.kind))).toEqual(new Set(Object.values(KIND)));
  });

  for (const v of txv) {
    it(`${v.name}: signing bytes, framing, signature, tx and hash`, () => {
      const seed = fromHex(v.seed);
      expect(b58(pubkeyOf(seed))).toBe(v.from);
      const t = { chainId: v.chain_id, from: key32(v.from), nonce: BigInt(v.nonce), fee: BigInt(v.fee), body: body(v) };
      expect(hex(signingBytes(t))).toBe(v.signing_bytes);
      expect(hex(framedMessage(t))).toBe(v.framed_message);
      const { bytes, hash, tx } = signTx(seed, t);
      expect(hex(tx.sig)).toBe(v.sig);
      expect(hex(bytes)).toBe(v.tx);
      expect(hex(hash)).toBe(v.tx_hash);
      expect(hex(txHash(fromHex(v.tx)))).toBe(v.tx_hash);
      // decode round trip
      const d = decodeTx(fromHex(v.tx));
      expect(d.body.kind).toBe(v.kind_name);
      expect(verifyTx(d)).toBe(true);
      expect(hex(signingBytes(d))).toBe(v.signing_bytes);
    });
  }

  it("a tampered tx fails verification", () => {
    const d = decodeTx(fromHex(txv[0].tx));
    expect(verifyTx({ ...d, fee: d.fee + 1n })).toBe(false);
  });
});

describe("domain messages", () => {
  const by = (n: string) => msgv.find((m: any) => m.name === n);
  it("deposit (raw, unframed)", () => {
    const v = by("deposit");
    const m = depositMessage(42n, key32("6DDSqnFUxWMu66T4pgQJpaUxEmfYdrJLJdcJmUtL6HW7"), 1_000_000n);
    expect(hex(m)).toBe(v.message);
    expect(hex(signRaw(fromHex(v.seed), m))).toBe(v.sig);
  });
  it("deposit order + order job id", () => {
    const v = by("deposit_order");
    const bodyBytes = fromHex(v.post_job_body);
    expect(hex(sha256(bodyBytes))).toBe(v.post_job_body_sha256);
    const m = depositOrderMessage(BigInt(v.solana_nonce), key32("6DDSqnFUxWMu66T4pgQJpaUxEmfYdrJLJdcJmUtL6HW7"), 6_000_000n, bodyBytes);
    expect(hex(m)).toBe(v.message);
    expect(hex(orderJobId(BigInt(v.solana_nonce)))).toBe(v.job_id);
    // the vector body is the bridge_unlock_order job_order body
    const order = txv.find((t: any) => t.name === "bridge_unlock_order");
    expect(hex(encodePostJob(postJob(order.body.job_order)))).toBe(v.post_job_body);
  });
  it("result message", () => {
    const v = by("result");
    const sub = txv.find((t: any) => t.name === "submit_result");
    const m = resultMessage(fromHex(sub.body.job_id), key32(sub.from), fromHex(sub.body.result_hash));
    expect(hex(m)).toBe(v.message);
    expect(hex(signRaw(fromHex(v.seed), m))).toBe(v.sig);
  });
  it("job id and reserved addresses", () => {
    const v = by("job_id");
    expect(hex(jobIdForPost(key32(v.poster), BigInt(v.poster_nonce)))).toBe(v.job_id);
    const r = by("reserved_addresses");
    expect(b58(RESERVED.escrow)).toBe(r.escrow);
    expect(b58(RESERVED.pool)).toBe(r.pool);
    expect(b58(blake3(new TextEncoder().encode("pearl-sol:escrow:v1")))).toBe(r.escrow);
  });
  it("reject message (RejectResult) and both evaluator signatures", () => {
    const v = rejv.messages.find((m: any) => m.name === "reject");
    const rj = rejv.txs.find((t: any) => t.name === "reject_result");
    const m = rejectMessage(fromHex(rj.body.job_id), key32(rj.body.deliverer), fromHex(rj.body.result_hash));
    expect(hex(m)).toBe(v.message);
    expect(hex(signRaw(fromHex(v.seed), m))).toBe(v.sig);
    expect(rj.body.sigs[0].sig).toBe(v.sig);
    // same fields as the result message, another domain
    expect(hex(m)).not.toBe(hex(resultMessage(fromHex(rj.body.job_id), key32(rj.body.deliverer), fromHex(rj.body.result_hash))));
  });
  it("route clear (R2 payout lien release)", () => {
    const v = by("route_clear");
    const clr = txv.find((t: any) => t.name === "clear_route");
    const m = routeClearMessage(clr.chain_id, key32(clr.body.account), BigInt(clr.body.route_nonce));
    expect(hex(m)).toBe(v.message);
    expect(hex(signRaw(fromHex(v.seed), m))).toBe(clr.body.sig);
  });
  it("coinbase receipt (R1) and its tx_root leaf", () => {
    const h = JSON.parse(readFileSync(resolve(dir, "header.json"), "utf8"));
    const rec = fromHex(h.coinbase.record);
    const c = decodeCoinbase(rec);
    expect(c.height).toBe(BigInt(h.coinbase.fields.height));
    expect(c.fees.toString()).toBe(h.coinbase.fields.fees);
    expect(c.subsidy.toString()).toBe(h.coinbase.fields.subsidy);
    expect(b58(c.miner)).toBe(h.coinbase.fields.miner);
    expect(c.matureHeight).toBe(BigInt(h.coinbase.fields.mature_height));
    expect(hex(blake3(rec))).toBe(h.coinbase.receipt_hash);
    expect(() => decodeCoinbase(fromHex(txv[0].tx))).toThrow();
  });
});
