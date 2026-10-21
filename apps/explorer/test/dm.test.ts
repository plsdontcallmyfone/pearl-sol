// DMs and channels (SPEC 4.10): the TS codec of the six DM kinds checked byte for byte against
// the chain's own vectors (chain/agent-types/vectors/dm.json), ids, sealed framing, and the DM
// index over canonical blocks (threads, escrow release / reclaim / decline, consent, channels
// with invites, joins, removals and key epochs, rebuild after a reorg).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { blake3 } from "../src/blake3.ts";
import { b58encode, hex } from "../src/codec.ts";
import { txFacts } from "../src/decode.ts";
import type { BlockRec } from "../src/indexer.ts";
import {
  DM_KIND, DmIndex, channelId, checkSealedChannel, checkSealedDm, decodeDmBody, decodeDmTx, dmFee, dmSigningBytes, dmTxLen, encodeDmBody, messageId,
  type DmBody,
} from "../src/dm.ts";
import { socialSigningBytes, type SocialBody } from "../src/social.ts";

const here = dirname(fileURLToPath(import.meta.url));
const VECTORS = join(here, "..", "..", "..", "chain", "agent-types", "vectors", "dm.json");
const Z = "0".repeat(64);
const ZK = b58encode(new Uint8Array(32));
const key = (i: number) => b58encode(Uint8Array.from({ length: 32 }, (_, j) => (j === 0 ? i : 7)));

function fromVector(name: string, b: any): DmBody {
  switch (name) {
    case "SendMessage":
      return {
        kind: "SendMessage", to: b.to, channel: b.channel, mode: b.mode, replyTo: b.reply_to, pay: b.pay, reclaimAfter: b.reclaim_after,
        ...(b.mode === 0 ? { body: Buffer.from(b.body_hex, "hex").toString("utf8") } : {}), bodyHex: b.body_hex, blobUri: b.blob_uri, blobHash: b.blob_hash, thoughtRef: b.thought_ref,
      };
    case "ReclaimMessage":
      return { kind: "ReclaimMessage", messageId: b.message_id };
    case "SetConsent":
      return { kind: "SetConsent", items: b.items.map((i: any) => ({ peer: i.peer, state: i.state })) };
    case "ChannelConfig":
      return { kind: "ChannelConfig", channelId: b.channel_id, name: b.name, about: b.about, policy: b.policy, joinPrice: b.join_price, mode: b.mode, admins: b.admins };
    case "ChannelMember":
      return { kind: "ChannelMember", channel: b.channel, action: b.action, member: b.member, pay: b.pay };
    default:
      return { kind: "ChannelKey", channel: b.channel, epoch: b.epoch, envelopes: b.envelopes.map((e: any) => ({ member: e.member, envelopeHex: e.envelope })) };
  }
}

test("vectors: the TS codec matches chain/agent-types/vectors/dm.json byte for byte (every DM kind)", () => {
  const v = JSON.parse(readFileSync(VECTORS, "utf8"));
  for (const m of v.message_ids) assert.equal(messageId(m.from, m.nonce), m.message_id);
  for (const c of v.channel_ids) assert.equal(channelId(c.creator, c.nonce), c.channel_id);
  const kinds = new Set<string>();
  for (const x of v.txs) {
    kinds.add(x.kind_name);
    const tx = Uint8Array.from(Buffer.from(x.tx, "hex"));
    const d = decodeDmTx(tx);
    assert.equal(d.kind, x.kind, x.name);
    assert.equal(d.from, x.from);
    assert.equal(d.nonce, String(x.nonce));
    assert.equal(d.fee, x.fee);
    assert.equal(d.chainId, x.chain_id);
    const want = fromVector(x.kind_name, x.body);
    assert.deepEqual(d.body, want, x.name);
    // the encoder reproduces the exact signing bytes and, with the vector's signature, the tx and its hash
    const sb = dmSigningBytes(x.chain_id, x.from, x.nonce, x.fee, want);
    assert.equal(hex(sb), x.signing_bytes, `${x.name} signing bytes`);
    const full = Uint8Array.from([...sb, ...Buffer.from(x.sig, "hex")]);
    assert.equal(hex(full), x.tx, `${x.name} tx`);
    assert.equal(hex(blake3(full)), x.tx_hash, `${x.name} tx_hash`);
    assert.equal(dmTxLen(want), full.length);
    // the window.pearl JSON form without bodyHex (plain text only) encodes the same bytes
    if (want.kind === "SendMessage" && want.mode === 0) {
      const { bodyHex: _drop, ...plain } = want;
      assert.equal(hex(dmSigningBytes(x.chain_id, x.from, x.nonce, x.fee, plain as DmBody)), x.signing_bytes, `${x.name} plain form`);
    }
    if (want.kind === "SendMessage" && want.mode === 1) {
      if (want.channel === Z) assert.ok(checkSealedDm(Buffer.from(want.bodyHex!, "hex")) >= 1);
      else assert.equal(checkSealedChannel(Buffer.from(want.bodyHex!, "hex")), 1);
    }
  }
  assert.deepEqual([...kinds].sort(), Object.keys(DM_KIND).sort(), "the vectors cover every DM kind");
});

test("codec: round trips, refusals, fee rule", () => {
  const bodies: DmBody[] = [
    { kind: "SendMessage", to: key(2), channel: Z, mode: 0, replyTo: Z, pay: "0", reclaimAfter: 0, body: "café", bodyHex: Buffer.from("café").toString("hex"), blobUri: "", blobHash: Z, thoughtRef: "" },
    { kind: "ReclaimMessage", messageId: "ab".repeat(32) },
    { kind: "SetConsent", items: [{ peer: key(3), state: 2 }] },
    { kind: "ChannelConfig", channelId: Z, name: "desk", about: "", policy: 0, joinPrice: "0", mode: 0, admins: [] },
    { kind: "ChannelMember", channel: "cd".repeat(32), action: 3, member: key(4), pay: "0" },
    { kind: "ChannelKey", channel: "cd".repeat(32), epoch: 2, envelopes: [{ member: key(4), envelopeHex: "ee".repeat(104) }] },
  ];
  for (const b of bodies) assert.deepEqual(decodeDmBody(DM_KIND[b.kind], encodeDmBody(b)), b, b.kind);
  assert.throws(() => decodeDmBody(18, Uint8Array.of(0)), /count/);
  assert.throws(() => decodeDmBody(20, Uint8Array.from([...new Uint8Array(32), 4])), /action/);
  assert.throws(() => decodeDmBody(17, new Uint8Array(33)), /trailing/);
  assert.throws(() => encodeDmBody({ kind: "ChannelKey", channel: Z, epoch: 1, envelopes: [{ member: key(1), envelopeHex: "00" }] }), /104/);
  assert.throws(() => encodeDmBody({ kind: "SetConsent", items: [] }), /1\.\.=64/);
  const len = BigInt(dmTxLen(bodies[0]));
  assert.equal(dmFee({ min_fee: "1000" }, bodies[0]), 1000n);
  assert.equal(dmFee({ min_fee: "1000", social_min_fee: "900", social_byte_fee: "5" }, bodies[0]), 900n + 5n * len);
  assert.throws(() => checkSealedDm(Uint8Array.of(1, 1, 2, 3)), /short/);
  assert.throws(() => checkSealedChannel(Uint8Array.from([1, ...new Array(60).fill(0)])), /version 2/);
});

// ------------------------------------------------------------ index

let nonces = new Map<string, number>();
function tx(from: string, body: DmBody | SocialBody) {
  const n = nonces.get(from) ?? 0;
  nonces.set(from, n + 1);
  const sb = body.kind in DM_KIND ? dmSigningBytes(7, from, n, "1000", body as DmBody) : socialSigningBytes(7, from, n, "1000", body as SocialBody);
  const bytes = Uint8Array.from([...sb, ...new Uint8Array(64)]);
  return { hash: hex(blake3(bytes)), kind: body.kind, from, fee: "1000", facts: txFacts(body.kind, Buffer.from(bytes).toString("base64")), nonce: n };
}
const block = (height: number, txs: any[], tag = "h"): BlockRec => ({ height, hash: `${tag}${height}`, prev: "", time: 1_760_000_000_000 + height * 5000, miner: "M", jobId: null, fees: "0", subsidy: "0", reward: "0", proofHash: Z, proofLen: 0, targetBits: 0, txs });
const dm = (to: string, text: string, o: Partial<Extract<DmBody, { kind: "SendMessage" }>> = {}): DmBody => ({ kind: "SendMessage", to, channel: Z, mode: 0, replyTo: Z, pay: "0", reclaimAfter: 0, body: text, blobUri: "", blobHash: Z, thoughtRef: "", ...o });

test("index: threads, escrow released by a reply in time, reclaim after the window, decline, consent", () => {
  nonces = new Map();
  const A = key(1), B = key(2), C = key(3);
  const m1 = tx(A, dm(B, "are you taking questions?", { pay: "5000", reclaimAfter: 10 }));
  const id1 = messageId(A, m1.nonce);
  assert.equal(m1.facts.messageId, id1);
  const m2 = tx(A, dm(C, "second question", { pay: "7000", reclaimAfter: 10 }));
  const id2 = messageId(A, m2.nonce);
  const m3 = tx(A, dm(B, "late one", { pay: "9000", reclaimAfter: 10 }));
  const id3 = messageId(A, m3.nonce);
  const r1 = tx(B, dm(A, "yes", { replyTo: id1 })); // in time: releases id1
  const late = tx(B, dm(A, "too late", { replyTo: id3 })); // after id3's deadline: no release
  const rc = tx(A, { kind: "ReclaimMessage", messageId: id3 });
  const dc = tx(C, { kind: "ReclaimMessage", messageId: id2 }); // the recipient declines
  const cs = tx(B, { kind: "SetConsent", items: [{ peer: C, state: 2 }, { peer: A, state: 1 }] });
  const cs2 = tx(B, { kind: "SetConsent", items: [{ peer: A, state: 0 }] });
  const blocks = new Map<number, BlockRec>([
    [1, block(1, [m1, m2])], [2, block(2, [m3])], [5, block(5, [r1])], [13, block(13, [late])], [14, block(14, [rc, dc, cs])], [15, block(15, [cs2])],
  ]);
  const ix = new DmIndex().update(blocks, 15);
  const th = ix.thread(A, B);
  assert.deepEqual(th.map((m) => m.body), ["are you taking questions?", "late one", "yes", "too late"]);
  assert.deepEqual(ix.thread(B, A), th, "pair key is symmetric");
  const e1 = ix.escrows.get(id1)!;
  assert.equal(e1.deadline, 11);
  assert.equal(DmIndex.escrowState(e1, 15), "released");
  assert.equal(e1.settled!.txHash, r1.hash);
  const e3 = ix.escrows.get(id3)!;
  assert.equal(DmIndex.escrowState(e3, 15), "refunded");
  assert.equal(e3.settled!.declined, false);
  const e2 = ix.escrows.get(id2)!;
  assert.equal(e2.settled!.declined, true, "the recipient sent the reclaim");
  assert.equal(ix.consentOf(B, C), 2);
  assert.equal(ix.consentOf(B, A), null, "cleared");
  assert.deepEqual([...ix.peers.get(A)!].sort(), [B, C].sort());
  // open vs expired by height (state view before settlement)
  const ix2 = new DmIndex().update(new Map([[1, block(1, [m1, m2])], [2, block(2, [m3])]]), 2);
  assert.equal(DmIndex.escrowState(ix2.escrows.get(id1)!, 10), "open", "tip 10: block 11 can still release it");
  assert.equal(DmIndex.escrowState(ix2.escrows.get(id1)!, 11), "expired");
});

test("index: channels (create, invite, join, paid join, key epochs, removal sets rekey, sealed messages) and a reorg rebuild", () => {
  nonces = new Map();
  const A = key(1), B = key(2), C = key(3), D = key(4);
  const cc = tx(A, { kind: "ChannelConfig", channelId: Z, name: "desk", about: "bids", policy: 2, joinPrice: "100", mode: 1, admins: [B] });
  const ch = channelId(A, cc.nonce);
  assert.equal(cc.facts.channelId, ch);
  const inv = tx(A, { kind: "ChannelMember", channel: ch, action: 2, member: C, pay: "0" });
  const jc = tx(C, { kind: "ChannelMember", channel: ch, action: 0, member: ZK, pay: "0" });
  const jd = tx(D, { kind: "ChannelMember", channel: ch, action: 0, member: ZK, pay: "100" });
  const k1 = tx(A, { kind: "ChannelKey", channel: ch, epoch: 1, envelopes: [A, B, C, D].map((m, i) => ({ member: m, envelopeHex: String(i + 1).repeat(208) })) });
  const body = "02" + "01000000" + "44".repeat(24) + "55".repeat(16);
  const msg = tx(B, { kind: "SendMessage", to: ZK, channel: ch, mode: 1, replyTo: Z, pay: "0", reclaimAfter: 0, bodyHex: body, blobUri: "", blobHash: Z, thoughtRef: "" });
  const rm = tx(A, { kind: "ChannelMember", channel: ch, action: 3, member: D, pay: "0" });
  const blocks = new Map<number, BlockRec>([[1, block(1, [cc])], [2, block(2, [inv])], [3, block(3, [jc, jd])], [4, block(4, [k1, msg])], [5, block(5, [rm])]]);
  const ix = new DmIndex().update(blocks, 5);
  const c = ix.channels.get(ch)!;
  assert.deepEqual([...c.members.keys()].sort(), [A, B, C].sort());
  assert.equal(c.invites.size, 0, "the invite was used up");
  assert.equal(c.epoch, 1);
  assert.equal(c.rekeyDue, true, "a removal after a key sets rekey_due");
  assert.equal(c.epochs.get(1)!.envelopes.get(D), "4".repeat(208));
  assert.equal(c.messages.length, 1);
  assert.equal(c.messages[0].epoch, 1);
  assert.equal(c.messages[0].body, null, "sealed: no text");
  assert.equal(ix.memberOf.get(D)?.has(ch), false);
  // a new key epoch clears rekey_due
  const k2 = tx(A, { kind: "ChannelKey", channel: ch, epoch: 2, envelopes: [A, B, C].map((m) => ({ member: m, envelopeHex: "ab".repeat(104) })) });
  blocks.set(6, block(6, [k2]));
  ix.update(blocks, 6);
  assert.equal(ix.channels.get(ch)!.epoch, 2);
  assert.equal(ix.channels.get(ch)!.rekeyDue, false);
  // reorg: block 5 and 6 replaced by an empty branch; the removal is gone
  blocks.delete(6);
  blocks.set(5, block(5, [], "x"));
  ix.update(blocks, 5);
  assert.ok(ix.channels.get(ch)!.members.has(D), "the removal rolled back with its branch");
  assert.equal(ix.channels.get(ch)!.rekeyDue, false);
});
