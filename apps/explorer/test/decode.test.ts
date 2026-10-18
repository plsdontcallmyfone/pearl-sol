import { test } from "node:test";
import assert from "node:assert/strict";
import { b58decode, b58encode, findPda, isOnCurve, sha256, u64le, utf8 } from "../src/codec.ts";
import * as D from "../src/decode.ts";
import { systemTransfer, toBlockRec } from "../src/indexer.ts";

test("base58 round trip incl. leading zeros", () => {
  for (const s of ["11111111111111111111111111111111", D.STOREFRONT_ID, "3yZe7d"]) assert.equal(b58encode(b58decode(s)), s);
  assert.deepEqual([...b58decode("1112")], [0, 0, 0, 1]);
});

test("ed25519 on-curve check: real pubkeys on, a known PDA off", () => {
  assert.equal(isOnCurve(b58decode("ETGT761HTQQpYVnmwX6eQtdSLfXjmNZi7hJ9n514azXH")), true);
  assert.equal(isOnCurve(b58decode("8y78YcmL6ggaJXK37VWyehWT5mH7eJvbivBnmpNHjChc")), false);
});

test("PDAs match the ones the programs created on the test VPS", () => {
  assert.equal(D.orderPda(2), "8y78YcmL6ggaJXK37VWyehWT5mH7eJvbivBnmpNHjChc");
  assert.equal(D.orderPda(3), "C5MySFzQjzRQUsVyyrZTbSdugk3Tchbn6dpdnp86vxg4");
  assert.equal(D.lcConfigPda(), "7Y6K2XThvb2gLGo9JUj8EyjS98Hjvyhy6GoDEyo4xc51");
  assert.equal(D.headerPda("41eb0129bf5f427d9c5c13f6097afc39d48c3cc7f02ec495e2744baaced78a2e"), "DjWv9M45SZ3nuQBAr458991VKDNnuiqJhcBywwDDxvYF");
  // the PDA rule itself
  const [a, bump] = findPda([utf8("order"), u64le(2n)], D.STOREFRONT_ID);
  assert.equal(a, D.orderPda(2));
  assert.ok(bump >= 0 && bump <= 255);
});

test("order job id = sha256(domain || nonce LE), as the chain and storefront use", () => {
  assert.equal(D.orderJobId(2), "c6fb083d20cb8100977ed28b4d08cc7f3b245a4d84275078745aa6032dfc7268");
});

function orderBytes() {
  const d = new Uint8Array(1070);
  d.set(D.accountDisc("Order"), 0);
  const dv = Buffer.from(d.buffer);
  d[8] = 1; // version
  d[9] = 2; // finalized
  d[10] = 0; // curve
  dv.writeBigUInt64LE(7n, 12);
  d.set(sha256(utf8("job")), 20);
  d.set(b58decode("ETGT761HTQQpYVnmwX6eQtdSLfXjmNZi7hJ9n514azXH"), 52);
  d.set(b58decode("2gkQQkfwRy5LYVmPQNYUfxpgWsTrQZF1vhqUAR4Phveq"), 84);
  dv.writeBigUInt64LE(10_008_389n, 148);
  dv.writeBigUInt64LE(284_879_935_361n, 156);
  dv.writeBigUInt64LE(227_903_948_289_000n, 164);
  dv.writeBigUInt64LE(56_975_987_072_000n, 172);
  dv.writeBigUInt64LE(462n, 180);
  dv.writeBigInt64LE(1_792_292_358n, 188);
  dv.writeUInt16LE(4, 268);
  d.set(utf8("blob"), 270);
  dv.writeUInt32LE(64, 470);
  dv.writeUInt32LE(64, 474);
  dv.writeUInt32LE(2048, 478);
  dv.writeBigUInt64LE(400n, 470 + 128);
  dv.writeUInt32LE(5, 470 + 136);
  return d;
}

test("decodeOrder reads the storefront layout", () => {
  const o = D.decodeOrder("X", orderBytes())!;
  assert.equal(o.status, "finalized");
  assert.equal(o.nonce, "7");
  assert.equal(o.buyer, "ETGT761HTQQpYVnmwX6eQtdSLfXjmNZi7hJ9n514azXH");
  assert.equal(o.recipient, "2gkQQkfwRy5LYVmPQNYUfxpgWsTrQZF1vhqUAR4Phveq");
  assert.equal(o.solSpent, "10008389");
  assert.equal(o.protocolFee, "56975987072000");
  assert.equal(o.deliverer, null);
  assert.equal(o.resultUri, "blob");
  assert.deepEqual([o.m, o.n, o.k, o.maxBlocks, o.deadlineBlocks], [64, 64, 2048, 5, 400]);
  const bad = orderBytes();
  bad[0] ^= 1;
  assert.equal(D.decodeOrder("X", bad), null);
});

function txBytes(kind: number, body: Uint8Array) {
  const h = new Uint8Array(54);
  h[0] = 1;
  h[53] = kind;
  return Buffer.concat([h, body, new Uint8Array(64)]).toString("base64");
}

test("txFacts: job ids from SubmitResult / CompleteJob, order unlocks, PostJob", () => {
  const jid = sha256(utf8("j"));
  const rh = sha256(utf8("r"));
  const sub = D.txFacts("SubmitResult", txBytes(3, Buffer.concat([jid, rh, Buffer.from([0, 0])])));
  assert.equal(sub.jobId, Buffer.from(jid).toString("hex"));
  assert.equal(sub.resultHash, Buffer.from(rh).toString("hex"));
  assert.equal(D.txFacts("CompleteJob", txBytes(4, Buffer.concat([jid, Buffer.from([0])]))).jobId, Buffer.from(jid).toString("hex"));

  const unlock = Buffer.alloc(49);
  unlock.writeBigUInt64LE(2n, 0);
  Buffer.from(b58decode("2gkQQkfwRy5LYVmPQNYUfxpgWsTrQZF1vhqUAR4Phveq")).copy(unlock, 8);
  unlock.writeBigUInt64LE(284_879_935_361n, 40);
  unlock[48] = 1;
  const u = D.txFacts("BridgeUnlock", txBytes(6, unlock));
  assert.equal(u.jobOrder, true);
  assert.equal(u.solanaNonce, "2");
  assert.equal(u.orderJobId, D.orderJobId(2));
  assert.equal(u.recipient, "2gkQQkfwRy5LYVmPQNYUfxpgWsTrQZF1vhqUAR4Phveq");

  const post = Buffer.alloc(12 + 52 + 64 + 2 + 3 + 16);
  const ha = sha256(utf8("a"));
  Buffer.from(ha).copy(post, 64);
  post.writeUInt16LE(3, 128);
  post.write("x:y", 130);
  post.writeBigUInt64LE(500n, 133);
  post.writeBigUInt64LE(100n, 141);
  const p = D.txFacts("PostJob", txBytes(2, post));
  assert.equal(p.postHashA, Buffer.from(ha).toString("hex"));
  assert.equal(p.workerPay, "500");
  assert.equal(p.protocolFee, "100");
  assert.deepEqual(D.txFacts("Transfer", txBytes(0, Buffer.alloc(40))), {});
});

test("anchorInstructions keeps only the named program's top-level and CPI frames", () => {
  const logs = [
    `Program ${D.STOREFRONT_ID} invoke [1]`,
    "Program log: Instruction: BuyJob",
    "Program 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P invoke [2]",
    "Program log: Instruction: Buy",
    "Program 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P success",
    `Program ${D.STOREFRONT_ID} success`,
    `Program ${D.STOREFRONT_ID} invoke [1]`,
    "Program log: Instruction: FinalizeOrderProof",
    `Program ${D.STOREFRONT_ID} success`,
  ];
  assert.deepEqual(D.anchorInstructions(logs, D.STOREFRONT_ID), ["BuyJob", "FinalizeOrderProof"]);
});

test("systemTransfer finds the x402 payment transfer", () => {
  const data = Buffer.alloc(12);
  data.writeUInt32LE(2, 0);
  data.writeBigUInt64LE(10_008_389n, 4);
  const tx = {
    transaction: { message: { accountKeys: ["svc", "payer", "11111111111111111111111111111111"], instructions: [{ programIdIndex: 2, accounts: [1, 0], data: b58encode(data) }] } },
    meta: {},
  };
  assert.equal(systemTransfer(tx, "payer", "svc"), "10008389");
  assert.equal(systemTransfer(tx, "svc", "payer"), null);
});

test("toBlockRec: zero job id is a fallback block", () => {
  const r = toBlockRec({
    hash: "ab",
    fees: "1000",
    subsidy: "0",
    reward: "1000",
    proof_len: 10,
    header_fields: { height: 5, prev_hash: "aa", timestamp_ms: 1, miner: "m", job_id: "0".repeat(64), proof_hash: "pp", target_bits: 1 },
    txs: [],
  });
  assert.equal(r.jobId, null);
  assert.equal(r.height, 5);
});

test("buybackEvents decodes BuybackExecuted / BuybackAccrued from Program data logs", () => {
  const ex = Buffer.alloc(8 + 8 + 1 + 24 + 1 + 32 + 8 * 11);
  Buffer.from(D.eventDisc("BuybackExecuted")).copy(ex, 0);
  let o = 8;
  ex.writeBigUInt64LE(3n, o); o += 8;
  ex[o] = 0; o += 1;
  ex.writeBigUInt64LE(50_000_000n, o); o += 8;
  ex.writeBigUInt64LE(1_234_567n, o); o += 8;
  ex.writeBigUInt64LE(1_234_567n, o); o += 8;
  ex[o] = 1; o += 1;
  Buffer.from(b58decode(D.STOREFRONT_ID)).copy(ex, o); o += 32;
  o += 8;
  ex.writeBigUInt64LE(4n, o); o += 8;
  ex.writeBigUInt64LE(6n, o); o += 8;
  ex.writeBigUInt64LE(3n, o); o += 8;
  o += 8;
  ex.writeBigUInt64LE(7n, o);
  const ac = Buffer.alloc(8 + 32 + 24);
  Buffer.from(D.eventDisc("BuybackAccrued")).copy(ac, 0);
  ac.writeBigUInt64LE(9n, 40);
  ac.writeBigUInt64LE(1_000_000n, 48);
  const ev = D.buybackEvents(["Program log: x", `Program data: ${ex.toString("base64")}`, `Program data: ${ac.toString("base64")}`, "Program data: AAAA"]);
  assert.equal(ev.executed.length, 1);
  assert.deepEqual(
    [ev.executed[0].buy, ev.executed[0].venue, ev.executed[0].solIn, ev.executed[0].tokensOut, ev.executed[0].destination, ev.executed[0].destinationAccount, ev.executed[0].firstNonce, ev.executed[0].lastNonce, ev.executed[0].orders, ev.executed[0].carry],
    [3, "curve", "50000000", "1234567", "burn", D.STOREFRONT_ID, "4", "6", 3, "7"],
  );
  assert.deepEqual(ev.accrued, [{ order: "11111111111111111111111111111111", nonce: "9", lamports: "1000000" }]);
});

test("decodeStoreBuyback: null without the appended tail, fields with it", () => {
  const n = 1;
  const at = 146 + 32 * n;
  const short = new Uint8Array(at + 59);
  short.set(D.accountDisc("Config"), 0);
  Buffer.from(short.buffer).writeUInt32LE(n, 142);
  assert.equal(D.decodeStoreBuyback(short), null);
  const full = new Uint8Array(at + 230);
  full.set(short, 0);
  const dv = Buffer.from(full.buffer);
  const b = at + 59;
  dv.writeUInt16LE(1000, b);
  full[b + 2] = 0;
  dv.writeBigUInt64LE(5_000_000n, b + 99);
  dv.writeBigUInt64LE(4_000_000n, b + 107);
  dv.writeBigUInt64LE(2n, b + 139);
  const bb = D.decodeStoreBuyback(full)!;
  assert.deepEqual([bb.bps, bb.dest, bb.accrued, bb.spent, bb.buys], [1000, "reserve", "5000000", "4000000", 2]);
});
