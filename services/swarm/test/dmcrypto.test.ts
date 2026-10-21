// Sealed DM crypto against the chain's published vectors (chain/agent-types/vectors/dm-crypto.json,
// written by chain/agent-types/tests/dm_crypto.rs): every key derivation, every envelope opened,
// every sealing reproduced byte for byte from the vector's fixed eph secrets and nonces, and the
// failure cases (wrong key, flipped tag, wrong AAD).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { b58decode, b58encode, key32 } from "../src/b58.ts";
import { blake3 } from "../src/blake3.ts";
import {
  encKeyFromSeed, encSecretFromSeed, hchacha20, hkdf32, openChannelBody, openDm, openKeyEnvelope, sealChannelBody, sealDm, sealKeyEnvelope, xOpen, xSeal,
} from "../src/dmcrypto.ts";
import { walletFromSeed } from "../src/wallet.ts";

const V = JSON.parse(readFileSync(new URL("../../../chain/agent-types/vectors/dm-crypto.json", import.meta.url), "utf8"));
const DMV = JSON.parse(readFileSync(new URL("../../../chain/agent-types/vectors/dm.json", import.meta.url), "utf8"));
const h = (s: string) => Uint8Array.from(Buffer.from(s, "hex"));
const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");
const acct = (name: string) => V.accounts.find((a: any) => a.name === name);
const byAccount = (addr: string) => V.accounts.find((a: any) => a.account === addr);

test("hkdf check vector", () => {
  assert.equal(hex(hkdf32(h(V.hkdf_check.ikm), h(V.hkdf_check.salt), h(V.hkdf_check.info))), V.hkdf_check.okm);
});

test("enc_secret and enc_key from each seed; the wallet exposes the same enc_key", () => {
  for (const a of V.accounts) {
    assert.equal(hex(encSecretFromSeed(h(a.seed))), a.enc_secret, a.name);
    assert.equal(hex(encKeyFromSeed(h(a.seed))), a.enc_key_hex, a.name);
    assert.equal(b58encode(encKeyFromSeed(h(a.seed))), a.enc_key, a.name);
    const w = walletFromSeed(h(a.seed));
    assert.equal(w.address, a.account);
    assert.equal(hex(w.encKey), a.enc_key_hex);
    assert.notEqual(hex(w.encKey), hex(w.publicKey), "enc_key never equals the signing key");
  }
});

test("HChaCha20 test vector (draft-irtf-cfrg-xchacha 2.2.1)", () => {
  const key = h("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
  const n = h("000000090000004a0000000031415927");
  assert.equal(hex(hchacha20(key, n)), "82413b4227b27bfed30e42508a877d73a0f9e4d58a74a853c12ec41326d3ecdc");
});

test("XChaCha20-Poly1305 AEAD test vector (draft-irtf-cfrg-xchacha A.3.1)", () => {
  const pt = Buffer.from("Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.");
  const aad = h("50515253c0c1c2c3c4c5c6c7");
  const key = h("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
  const nonce = h("404142434445464748494a4b4c4d4e4f5051525354555657");
  const ct = xSeal(key, nonce, pt, aad);
  assert.equal(hex(ct.subarray(ct.length - 16)), "c0875924c1c7987947deafd8780acf49");
  assert.equal(hex(ct.subarray(0, 16)), "bd6d179d3e83d43b9576579493c0e939");
  assert.equal(Buffer.from(xOpen(key, nonce, ct, aad)!).toString(), pt.toString());
});

test("sealed DM: opens for the recipient and the sender (self copy), reseals byte for byte", () => {
  const d = V.sealed_dm;
  const from = byAccount(d.from), to = byAccount(d.to);
  const body = h(d.body_hex);
  for (const reader of [to, from]) {
    const pt = openDm(body, h(reader.enc_secret), key32(d.from), key32(d.to));
    assert.ok(pt, `${reader.name} opens`);
    assert.equal(Buffer.from(pt!).toString("utf8"), d.plaintext);
    // the wallet's own open path (secret stays inside the wallet)
    assert.equal(walletFromSeed(h(reader.seed)).openDm(body, key32(d.from), key32(d.to)), d.plaintext);
  }
  const again = sealDm({
    from: key32(d.from), to: key32(d.to), toEncKey: h(to.enc_key_hex), selfEncKey: h(from.enc_key_hex), text: d.plaintext,
    fixed: d.eph_secrets.map((e: string, i: number) => ({ ephSecret: h(e), nonce: h(d.nonces[i]) })),
  });
  assert.equal(hex(again), d.body_hex);
});

test("sealed DM failures: a third party, a flipped tag, swapped from/to", () => {
  const d = V.sealed_dm;
  const body = h(d.body_hex);
  const carol = acct("carol");
  assert.equal(openDm(body, h(carol.enc_secret), key32(d.from), key32(d.to)), null, "wrong key");
  const bad = Uint8Array.from(body);
  // envelope 0: 2 + 58 + ct_len; flip the last byte of its tag, and of envelope 1's
  const len0 = bad[58] | (bad[59] << 8);
  bad[2 + 58 + len0 - 1] ^= 1;
  bad[bad.length - 1] ^= 1;
  assert.equal(openDm(bad, h(byAccount(d.to).enc_secret), key32(d.from), key32(d.to)), null, "flipped tag");
  assert.equal(openDm(body, h(byAccount(d.to).enc_secret), key32(d.to), key32(d.from)), null, "aad binds from and to");
  assert.equal(openDm(h("02"), h(carol.enc_secret), key32(d.from), key32(d.to)), null, "not a DM body");
});

test("random sealing round trip with the wallet", () => {
  const a = walletFromSeed(h(acct("alice").seed)), b = walletFromSeed(h(acct("bob").seed));
  const body = sealDm({ from: a.publicKey, to: b.publicKey, toEncKey: b.encKey, selfEncKey: a.encKey, text: "the lantern is lit" });
  assert.equal(b.openDm(body, a.publicKey, b.publicKey), "the lantern is lit");
  assert.equal(a.openDm(body, a.publicKey, b.publicKey), "the lantern is lit");
  const noSelf = sealDm({ from: a.publicKey, to: b.publicKey, toEncKey: b.encKey, text: "x" });
  assert.equal(noSelf[1], 1);
  assert.equal(a.openDm(noSelf, a.publicKey, b.publicKey), null, "no self copy, the sender cannot open it");
});

test("channel key envelopes and channel body", () => {
  const c = V.sealed_channel;
  const ch = h(c.channel_id);
  for (const e of c.envelopes) {
    const m = byAccount(e.member);
    const gk = openKeyEnvelope(h(e.envelope_hex), h(m.enc_secret), ch, c.epoch, key32(e.member));
    assert.equal(hex(gk!), c.group_key, `${m.name} opens its envelope`);
    const again = sealKeyEnvelope({ memberEncKey: h(m.enc_key_hex), channel: ch, epoch: c.epoch, member: key32(e.member), groupKey: h(c.group_key), fixed: { ephSecret: h(e.eph_secret), nonce: h(e.nonce) } });
    assert.equal(hex(again), e.envelope_hex);
    assert.equal(openKeyEnvelope(h(e.envelope_hex), h(acct("bob").enc_secret), ch, c.epoch, key32(e.member)), null, "a non-member cannot open it");
    assert.equal(openKeyEnvelope(h(e.envelope_hex), h(m.enc_secret), ch, c.epoch + 1, key32(e.member)), null, "epoch is bound");
  }
  const msg = c.message;
  const opened = openChannelBody(h(msg.body_hex), h(c.group_key), ch, key32(msg.from));
  assert.equal(opened!.epoch, c.epoch);
  assert.equal(Buffer.from(opened!.text).toString(), msg.plaintext);
  const again = sealChannelBody({ groupKey: h(c.group_key), channel: ch, from: key32(msg.from), epoch: c.epoch, text: msg.plaintext, nonce: h(msg.nonce) });
  assert.equal(hex(again), msg.body_hex);
  const flipped = h(msg.body_hex);
  flipped[flipped.length - 1] ^= 0x80;
  assert.equal(openChannelBody(flipped, h(c.group_key), ch, key32(msg.from)), null, "flipped tag");
  assert.equal(openChannelBody(h(msg.body_hex), h(c.group_key), ch, key32(acct("alice").account)), null, "from is bound");
});

test("message_id and channel_id (blake3) match dm.json", () => {
  const le = (n: number) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
  for (const m of DMV.message_ids) assert.equal(hex(blake3(Buffer.from("pearl-sol:msg:v1"), b58decode(m.from), le(m.nonce))), m.message_id);
  for (const c of DMV.channel_ids) assert.equal(hex(blake3(Buffer.from("pearl-sol:channel:v1"), b58decode(c.creator), le(c.nonce))), c.channel_id);
});
