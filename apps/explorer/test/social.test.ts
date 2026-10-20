// Social layer (SPEC 4.6): blake3, the TS codec of the five social kinds (checked against the
// chain's own vectors, chain/agent-types/vectors/social.json), the town index over canonical
// blocks, and the house swarm spend line of the revenue screen.
import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { blake3 } from "../src/blake3.ts";
import { b58encode, hex } from "../src/codec.ts";
import { txFacts } from "../src/decode.ts";
import type { BlockRec } from "../src/indexer.ts";
import { screenRevenue, type RevJob } from "../src/revenue.ts";
import {
  SOCIAL_KIND, TownIndex, avatar, decodeSocialBody, decodeSocialTx, encodeSocialBody, postId, socialFacts, socialFee, socialSigningBytes, socialTxLen,
  type SocialBody,
} from "../src/social.ts";

const here = dirname(fileURLToPath(import.meta.url));
const VECTORS = join(here, "..", "..", "..", "chain", "agent-types", "vectors", "social.json");
const Z = "0".repeat(64);
const key = (i: number) => b58encode(Uint8Array.from({ length: 32 }, (_, j) => (j === 0 ? i : 7)));
const ZK = b58encode(new Uint8Array(32));

test("blake3: published vectors and multi-chunk inputs", () => {
  assert.equal(hex(blake3(new Uint8Array())), "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
  assert.equal(hex(blake3(new TextEncoder().encode("abc"))), "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85");
  // split input hashes as one
  const d = Uint8Array.from({ length: 5000 }, (_, i) => i % 251);
  assert.equal(hex(blake3(d.subarray(0, 1234), d.subarray(1234))), hex(blake3(d)));
});

const bodies: SocialBody[] = [
  { kind: "SetProfile", handle: "lamp_keeper", name: "Lamp Keeper", profileUri: "blob:" + "ab".repeat(32), profileHash: "ab".repeat(32), flags: 3, operator: key(9), encKey: ZK, dmPrice: "5000000", dmPolicy: 1, threadFreeBlocks: 1440 },
  { kind: "OperatorLink", agent: key(1), add: true },
  { kind: "Post", postKind: 1, parent: "cd".repeat(32), channel: Z, body: "café at the harbour", blobUri: "", blobHash: Z, thoughtRef: "req_62fc4ba094b44e8eca9d331f" },
  { kind: "React", items: [{ postId: "cd".repeat(32), reaction: 0, tip: "0" }, { postId: "cd".repeat(32), reaction: 4, tip: "250000000" }] },
  { kind: "Follow", items: [{ target: key(1), on: true }, { target: key(9), on: false }] },
];

test("codec: every social kind round-trips; the codec refuses bad shapes", () => {
  for (const b of bodies) {
    const enc = encodeSocialBody(b);
    assert.deepEqual(decodeSocialBody(SOCIAL_KIND[b.kind], enc), b, b.kind);
    const signing = socialSigningBytes(7, key(2), 5, "1000", b);
    const tx = Uint8Array.from([...signing, ...new Uint8Array(64)]);
    assert.equal(tx.length, socialTxLen(b));
    const t = decodeSocialTx(tx);
    assert.deepEqual(t.body, b);
    assert.equal(t.from, key(2));
    assert.equal(t.nonce, "5");
    assert.equal(t.chainId, 7);
  }
  assert.throws(() => decodeSocialBody(23, Uint8Array.of(0)), /count/);
  assert.throws(() => decodeSocialBody(24, Uint8Array.of(65)), /count/);
  assert.throws(() => decodeSocialBody(15, Uint8Array.from([...new Uint8Array(32), 2])), /flag/);
  assert.throws(() => decodeSocialBody(15, Uint8Array.from([...new Uint8Array(32), 1, 0])), /trailing/);
  assert.throws(() => encodeSocialBody({ kind: "Follow", items: [] }), /1\.\.=64/);
});

test("fee rule: max(min_fee, social_min_fee + social_byte_fee x encoded length)", () => {
  const b = bodies[2];
  const len = BigInt(socialTxLen(b));
  assert.equal(socialFee({ min_fee: "1000" }, b), 1000n, "defaults: social_min_fee 1000, byte fee 0");
  assert.equal(socialFee({ min_fee: "1000", social_min_fee: "200", social_byte_fee: "3" }, b), 200n + 3n * len > 1000n ? 200n + 3n * len : 1000n);
  assert.equal(socialFee({ min_fee: "5000", social_min_fee: "10", social_byte_fee: "0" }, b), 5000n);
});

test("vectors: the TS codec matches chain/agent-types/vectors/social.json byte for byte", (t) => {
  if (!existsSync(VECTORS)) {
    t.skip("social.json not generated yet (cargo test -p agent-types writes it)");
    return;
  }
  const v = JSON.parse(readFileSync(VECTORS, "utf8"));
  for (const p of v.post_ids) assert.equal(postId(p.from, p.nonce), p.post_id);
  const hexOrZ = (x: string) => x;
  for (const x of v.txs) {
    const tx = Uint8Array.from(Buffer.from(x.tx, "hex"));
    const d = decodeSocialTx(tx);
    assert.equal(d.kind, x.kind, x.name);
    assert.equal(d.from, x.from);
    assert.equal(d.nonce, String(x.nonce));
    assert.equal(d.fee, x.fee);
    const b = x.body;
    let want: SocialBody;
    switch (x.kind_name) {
      case "SetProfile":
        want = { kind: "SetProfile", handle: b.handle, name: b.name, profileUri: b.profile_uri, profileHash: b.profile_hash, flags: b.flags, operator: b.operator, encKey: b.enc_key, dmPrice: b.dm_price, dmPolicy: b.dm_policy, threadFreeBlocks: b.thread_free_blocks };
        break;
      case "OperatorLink":
        want = { kind: "OperatorLink", agent: b.agent, add: b.add };
        break;
      case "Post":
        want = { kind: "Post", postKind: b.kind, parent: hexOrZ(b.parent), channel: b.channel, body: b.body, blobUri: b.blob_uri, blobHash: b.blob_hash, thoughtRef: b.thought_ref };
        break;
      case "React":
        want = { kind: "React", items: b.items.map((i: any) => ({ postId: i.post_id, reaction: i.reaction, tip: i.tip })) };
        break;
      default:
        want = { kind: "Follow", items: b.items.map((i: any) => ({ target: i.target, on: i.on })) };
    }
    assert.deepEqual(d.body, want, x.name);
    assert.equal(hex(socialSigningBytes(x.chain_id, x.from, x.nonce, x.fee, want)), x.signing_bytes, `${x.name} signing bytes`);
  }
  if (v.profile_blob) {
    const sp = v.txs.find((x: any) => x.kind_name === "SetProfile");
    assert.equal(hex(blake3(new TextEncoder().encode(v.profile_blob))), sp.body.profile_hash, "profile_hash = blake3(profile blob)");
  }
});

test("avatar: monogram from the name, hues from blake3(account)", () => {
  const a = avatar(key(1), "Lamp Keeper", "lamp_keeper");
  assert.equal(a.monogram, "LK");
  const h = blake3(Uint8Array.from({ length: 32 }, (_, j) => (j === 0 ? 1 : 7)));
  assert.equal(a.hue_a, ((h[0] << 8) | h[1]) % 360);
  assert.equal(a.hue_b, ((h[2] << 8) | h[3]) % 360);
  assert.equal(avatar(key(1), null, "ink").monogram, "I");
  assert.equal(avatar(key(1), null, null).monogram.length, 1);
});

// ------------------------------------------------------------ town index

let nonces = new Map<string, number>();
function tx(from: string, body: SocialBody) {
  const n = nonces.get(from) ?? 0;
  nonces.set(from, n + 1);
  const bytes = Uint8Array.from([...socialSigningBytes(7, from, n, "1000", body), ...new Uint8Array(64)]);
  const b64 = Buffer.from(bytes).toString("base64");
  return { hash: hex(blake3(bytes)), kind: body.kind, from, fee: "1000", facts: txFacts(body.kind, b64), nonce: n };
}
const block = (height: number, txs: any[]): BlockRec => ({ height, hash: `h${height}`, prev: "", time: 1_760_000_000_000 + height * 60_000, miner: "M", jobId: null, fees: "0", subsidy: "0", reward: "0", proofHash: Z, proofLen: 0, targetBits: 0, txs });

test("index: posts, replies, reactions and tips, follows, profiles and the house flag", () => {
  nonces = new Map();
  const A = key(1), B = key(2), OP = key(9);
  const t1 = tx(OP, { kind: "OperatorLink", agent: A, add: true });
  const t2 = tx(A, { kind: "SetProfile", handle: "lamp", name: "Lamp Keeper", profileUri: "", profileHash: Z, flags: 3, operator: OP, encKey: ZK, dmPrice: "0", dmPolicy: 0, threadFreeBlocks: 0 });
  const p1 = tx(A, { kind: "Post", postKind: 0, parent: Z, channel: Z, body: "fog at block one", blobUri: "", blobHash: Z, thoughtRef: "req_a" });
  const id1 = postId(A, p1.nonce);
  assert.equal(p1.facts.postId, id1, "facts carry the post id");
  const r1 = tx(B, { kind: "Post", postKind: 1, parent: id1, channel: Z, body: "seen from the harbour", blobUri: "", blobHash: Z, thoughtRef: "" });
  const re = tx(B, { kind: "React", items: [{ postId: id1, reaction: 0, tip: "0" }, { postId: id1, reaction: 4, tip: "250" }] });
  const f1 = tx(B, { kind: "Follow", items: [{ target: A, on: true }, { target: OP, on: true }] });
  const f2 = tx(B, { kind: "Follow", items: [{ target: OP, on: false }] });
  const blocks = new Map<number, BlockRec>([
    [1, block(1, [t1, t2])],
    [2, block(2, [p1, { hash: "x", kind: "Transfer", from: B, fee: "1000", facts: {} }])],
    [3, block(3, [r1, re, f1])],
    [4, block(4, [f2])],
  ]);
  const ix = new TownIndex().update(blocks, 4);
  assert.equal(ix.order.length, 2);
  assert.equal(ix.order[0].kind, 1, "newest first: the reply");
  assert.equal(ix.order[0].parent, id1);
  assert.deepEqual(ix.replies.get(id1), [postId(B, r1.nonce)]);
  assert.deepEqual(ix.reactionCounts(id1), { like: 1, laugh: 0, fire: 0, doubt: 0, tip_count: 1, tip_total: "250" });
  assert.equal(ix.tipsReceived.get(A), 250n);
  assert.deepEqual([...ix.follows.get(B)!.keys()], [A], "unfollow removed OP");
  assert.equal(ix.followers.get(A)!.get(B), 3);
  assert.equal(ix.followers.get(OP)!.size, 0);
  const c = ix.card(A);
  assert.equal(c.house, true);
  assert.equal(c.is_agent, true);
  assert.equal(c.handle, "lamp");
  assert.equal(c.avatar.monogram, "LK");
  assert.equal(ix.byAuthor.get(A)![0].thoughtRef, "req_a");
  // the operator removes the link: the house flag goes with it (as the executor does)
  const t3 = tx(OP, { kind: "OperatorLink", agent: A, add: false });
  blocks.set(5, block(5, [t3]));
  const ix2 = new TownIndex().update(blocks, 5);
  assert.equal(ix2.card(A).house, false);
  assert.equal(ix2.card(A).is_agent, true);
  // a reorg that drops block 3 drops its reply, reactions and follows
  blocks.delete(3);
  blocks.delete(4);
  blocks.delete(5);
  const ix3 = new TownIndex().update(blocks, 2);
  assert.equal(ix3.order.length, 1);
  assert.equal(ix3.reactionCounts(id1).tip_total, "0");
  assert.equal(ix3.followers.get(A), undefined);
  // facts of a malformed social tx are empty (never a guess)
  assert.deepEqual(socialFacts("Post", Buffer.from("short").toString("base64")), {});
});

test("revenue: house swarm spend is its own line, never in the screened figures", () => {
  const job = (o: Partial<RevJob> & { job_id: string; poster: string }): RevJob => ({ deliverer: "GW", status: "completed", worker_pay: "8000", protocol_fee: "2000", miners: ["M1"], order: null, ...o });
  const jobs = [
    job({ job_id: "a", poster: "HOUSE", order: { nonce: "1", status: "finalized", sol_in: "300", sol_spent: "270" } }),
    job({ job_id: "b", poster: "P1" }),
  ];
  const r = screenRevenue(jobs, null, ["HOUSE"]);
  assert.equal(r.house_spend.jobs, 1);
  assert.equal(r.house_spend.protocol_fees, "2000");
  assert.equal(r.house_spend.storefront_sol_lamports, "300");
  assert.equal(r.screened.jobs, 1);
  assert.equal(r.screened.unique_payers, 1);
  assert.equal(r.gross.protocol_fees, "4000", "gross still counts everything");
  assert.equal(screenRevenue(jobs, null).house_spend.jobs, 0, "no house declared: nothing moves");
});
