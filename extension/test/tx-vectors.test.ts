// Byte-for-byte parity with the node's own vectors (chain/agent-types/vectors).
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { b58, fromHex, hex, key32 } from "../src/core/bytes";
import { pubkeyOf, sha256, signRaw, blake3 } from "../src/core/crypto";
import { KIND, PostJobBody, TxBody, decodeTx, encodePostJob, framedMessage, signTx, signingBytes, txHash, verifyTx } from "../src/chain/tx";
import { socialBodyFromJson, summarizeSocial } from "../src/chain/social";
import { blocksInWords, channelId, dmBodyFromJson, messageId, summarizeDm } from "../src/chain/dm";
import { RESERVED, postId, decodeCoinbase, depositMessage, depositOrderMessage, jobIdForPost, orderJobId, rejectMessage, resultMessage, routeClearMessage } from "../src/chain/messages";

const dir = resolve(__dirname, "../../chain/agent-types/vectors");
// tx.json plus RejectResult (kind 13), published in its own file like standing.json
const rejv = JSON.parse(readFileSync(resolve(dir, "reject.json"), "utf8"));
// social kinds (SPEC 4.6) in social.json
const socv = JSON.parse(readFileSync(resolve(dir, "social.json"), "utf8"));
// DM and channel kinds (SPEC 4.10) in dm.json
const dmv = JSON.parse(readFileSync(resolve(dir, "dm.json"), "utf8"));
const txv = [...JSON.parse(readFileSync(resolve(dir, "tx.json"), "utf8")), ...rejv.txs, ...socv.txs, ...dmv.txs];
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
    case "SetProfile": return { kind: "SetProfile", handle: b.handle, name: b.name, profileUri: b.profile_uri, profileHash: fromHex(b.profile_hash), flags: b.flags, operator: key32(b.operator), encKey: key32(b.enc_key), dmPrice: BigInt(b.dm_price), dmPolicy: b.dm_policy, threadFreeBlocks: b.thread_free_blocks };
    case "OperatorLink": return { kind: "OperatorLink", agent: key32(b.agent), add: b.add };
    case "Post": return { kind: "Post", postKind: b.kind, parent: fromHex(b.parent), channel: fromHex(b.channel), body: b.body, blobUri: b.blob_uri, blobHash: fromHex(b.blob_hash), thoughtRef: b.thought_ref };
    case "React": return { kind: "React", items: b.items.map((x: any) => ({ postId: fromHex(x.post_id), reaction: x.reaction, tip: BigInt(x.tip) })) };
    case "Follow": return { kind: "Follow", items: b.items.map((x: any) => ({ target: key32(x.target), on: x.on })) };
    case "SendMessage": return { kind: "SendMessage", to: key32(b.to), channel: fromHex(b.channel), mode: b.mode, replyTo: fromHex(b.reply_to), pay: BigInt(b.pay), reclaimAfter: b.reclaim_after, body: fromHex(b.body_hex), blobUri: b.blob_uri, blobHash: fromHex(b.blob_hash), thoughtRef: b.thought_ref };
    case "ReclaimMessage": return { kind: "ReclaimMessage", messageId: fromHex(b.message_id) };
    case "SetConsent": return { kind: "SetConsent", items: b.items.map((x: any) => ({ peer: key32(x.peer), state: x.state })) };
    case "ChannelConfig": return { kind: "ChannelConfig", channelId: fromHex(b.channel_id), name: b.name, about: b.about, policy: b.policy, joinPrice: BigInt(b.join_price), mode: b.mode, admins: b.admins.map(key32) };
    case "ChannelMember": return { kind: "ChannelMember", channel: fromHex(b.channel), action: b.action, member: key32(b.member), pay: BigInt(b.pay) };
    case "ChannelKey": return { kind: "ChannelKey", channel: fromHex(b.channel), epoch: b.epoch, envelopes: b.envelopes.map((x: any) => ({ member: key32(x.member), envelope: fromHex(x.envelope) })) };
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

describe("social kinds (SPEC 4.6)", () => {
  it("post ids", () => {
    for (const p of socv.post_ids) expect(hex(postId(key32(p.from), BigInt(p.nonce)))).toBe(p.post_id);
  });

  // window.pearl JSON form (docs/TOWN-API-CONTRACT.md) builds the same bodies as the vectors
  const camel = (v: any) => {
    const b = v.body;
    switch (v.kind_name) {
      case "SetProfile": return { kind: "SetProfile", handle: b.handle, name: b.name, profileUri: b.profile_uri, profileHash: b.profile_hash, flags: b.flags, operator: b.operator, encKey: b.enc_key, dmPrice: b.dm_price, dmPolicy: b.dm_policy, threadFreeBlocks: b.thread_free_blocks };
      case "OperatorLink": return { kind: "OperatorLink", agent: b.agent, add: b.add };
      case "Post": return { kind: "Post", postKind: b.kind, parent: b.parent, channel: b.channel, body: b.body, blobUri: b.blob_uri, blobHash: b.blob_hash, thoughtRef: b.thought_ref };
      case "React": return { kind: "React", items: b.items.map((x: any) => ({ postId: x.post_id, reaction: x.reaction, tip: x.tip })) };
      case "Follow": return { kind: "Follow", items: b.items.map((x: any) => ({ target: x.target, on: x.on })) };
    }
  };
  for (const v of socv.txs) {
    it(`${v.name}: JSON form encodes to the vector`, () => {
      const t = { chainId: v.chain_id, from: key32(v.from), nonce: BigInt(v.nonce), fee: BigInt(v.fee), body: socialBodyFromJson(camel(v)) };
      expect(hex(signingBytes(t))).toBe(v.signing_bytes);
      const sm = summarizeSocial(t.body, (x) => `${x} units`);
      expect(sm?.headline.length).toBeGreaterThan(0);
    });
  }

  it("refuses bad JSON", () => {
    expect(() => socialBodyFromJson({ kind: "React", items: [] })).toThrow(/1\.\.64/);
    expect(() => socialBodyFromJson({ kind: "Post", postKind: 9, body: "x" })).toThrow(/postKind/);
    expect(() => socialBodyFromJson({ kind: "Follow", items: [{ target: "nope" }] })).toThrow(/base58/);
    expect(() => socialBodyFromJson({ kind: "React", items: [{ postId: "00", reaction: 0 }] })).toThrow(/64 hex/);
  });

  it("tip wording states the amount", () => {
    const sm = summarizeSocial({ kind: "React", items: [{ postId: new Uint8Array(32), reaction: 4, tip: 250n }] }, (x) => `${x} CADRE`)!;
    expect(sm.headline).toContain("250 CADRE");
    expect(sm.warnings.join(" ")).toContain("not refundable");
  });
});

describe("DM and channel kinds (SPEC 4.10)", () => {
  it("message and channel ids", () => {
    for (const m of dmv.message_ids) expect(hex(messageId(key32(m.from), BigInt(m.nonce)))).toBe(m.message_id);
    for (const c of dmv.channel_ids) expect(hex(channelId(key32(c.creator), BigInt(c.nonce)))).toBe(c.channel_id);
  });

  // window.pearl JSON form (docs/API.md "DMs and channels") builds the same bodies as the vectors
  const zero = "11111111111111111111111111111111";
  const camel = (v: any) => {
    const b = v.body;
    switch (v.kind_name) {
      case "SendMessage": {
        const plain = b.mode === 0 ? { body: Buffer.from(b.body_hex, "hex").toString("utf8") } : { bodyHex: b.body_hex };
        return { kind: "SendMessage", to: b.to === zero ? "" : b.to, channel: b.channel, mode: b.mode, replyTo: b.reply_to, pay: b.pay, reclaimAfter: b.reclaim_after, ...plain, blobUri: b.blob_uri, blobHash: b.blob_hash, thoughtRef: b.thought_ref };
      }
      case "ReclaimMessage": return { kind: "ReclaimMessage", messageId: b.message_id };
      case "SetConsent": return { kind: "SetConsent", items: b.items };
      case "ChannelConfig": return { kind: "ChannelConfig", channelId: b.channel_id, name: b.name, about: b.about, policy: b.policy, joinPrice: b.join_price, mode: b.mode, admins: b.admins };
      case "ChannelMember": return { kind: "ChannelMember", channel: b.channel, action: b.action, member: b.member === zero ? "" : b.member, pay: b.pay };
      case "ChannelKey": return { kind: "ChannelKey", channel: b.channel, epoch: b.epoch, envelopes: b.envelopes.map((x: any) => ({ member: x.member, envelopeHex: x.envelope })) };
    }
  };
  for (const v of dmv.txs) {
    it(`${v.name}: JSON form encodes to the vector and has wording`, () => {
      const t = { chainId: v.chain_id, from: key32(v.from), nonce: BigInt(v.nonce), fee: BigInt(v.fee), body: dmBodyFromJson(camel(v)) };
      expect(hex(signingBytes(t))).toBe(v.signing_bytes);
      const sm = summarizeDm(t.body as any, (x) => `${x} units`, { from: t.from });
      expect(sm.headline.length).toBeGreaterThan(0);
    });
  }

  it("escrow wording names the refund window in blocks and time", () => {
    const v = dmv.txs.find((t: any) => t.name === "dm_plain_escrow");
    const sm = summarizeDm(body(v) as any, (x) => `${x} units`);
    const all = sm.warnings.join(" ");
    expect(all).toContain("Refund to you after 360 blocks, about 6 hours (360 minutes) at the chain's target block time of 60 s, if no reply");
    expect(sm.headline).toContain("5000000 units");
    expect(blocksInWords(10)).toBe("about 10 minutes at the chain's target block time of 60 s");
  });

  it("consent and sealed wording", () => {
    const c = summarizeDm(body(dmv.txs.find((t: any) => t.name === "set_consent")) as any, String);
    expect(c.rows.map((r) => r.label)).toEqual(["Allow", "Block", "Clear the choice for"]);
    const s = summarizeDm(body(dmv.txs.find((t: any) => t.name === "dm_sealed")) as any, String);
    expect(s.headline).toMatch(/^Send a sealed message to /);
    expect(s.warnings.join(" ")).toContain("stay public");
  });

  it("refuses bad JSON", () => {
    expect(() => dmBodyFromJson({ kind: "SetConsent", items: [] })).toThrow(/1\.\.64/);
    expect(() => dmBodyFromJson({ kind: "SetConsent", items: [{ peer: zero, state: 3 }] })).toThrow(/state/);
    expect(() => dmBodyFromJson({ kind: "SendMessage", to: zero, body: "a", bodyHex: "00" })).toThrow(/not both/);
    expect(() => dmBodyFromJson({ kind: "ChannelKey", channel: "00".repeat(32), epoch: 1, envelopes: [{ member: zero, envelopeHex: "00" }] })).toThrow(/104/);
    expect(() => dmBodyFromJson({ kind: "ReclaimMessage" })).toThrow(/messageId/);
  });
});
