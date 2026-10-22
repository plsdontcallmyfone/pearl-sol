// Vendored from services/swarm/src/dmcrypto.ts (pearl-sol repo, same license) so the SDK has no dependencies.
// Sealed DM crypto (SPEC 4.10), no dependencies: X25519 and HKDF-SHA256 from node:crypto, and
// XChaCha20-Poly1305 built from node's IETF chacha20-poly1305 (12-byte nonce) with a hand-written
// HChaCha20 for the 24-byte nonce: subkey = HChaCha20(key, nonce[0..16]), then ChaCha20-Poly1305
// with nonce 0x00000000 || nonce[16..24] (draft-irtf-cfrg-xchacha). Checked byte for byte against
// chain/agent-types/vectors/dm-crypto.json (test/dmcrypto.test.ts).
//
//   enc_secret = HKDF-SHA256(ikm = ed25519 seed, salt = "cadre-dm-v1", info = "x25519 enc_key", 32)
//   enc_key    = X25519(enc_secret, 9)
//   DM body v1 = 0x01 | count u8 (1..=2) | (eph_pub [32] | nonce [24] | ct_len u16 LE | ct)*
//                envelope 0 to the recipient's enc_key, 1 (optional) to the sender's own
//   per envelope to key R: k = HKDF(X25519(eph, R), salt = eph_pub || R, info = "cadre-dm-v1" || from || to)
//                          ct = XChaCha20-Poly1305(k, nonce, plaintext, aad = info)
//   channel key envelope (104 bytes) to member M with key E: eph_pub | nonce | AEAD(k, group_key)
//                          k = HKDF(X25519(eph, E), salt = eph_pub || E, info = "cadre-chan-v1" || channel || epoch LE || M)
//   channel body v2 = 0x02 | epoch u32 LE | nonce [24] | AEAD(group_key, nonce, pt, aad = "cadre-chan-v1" || channel || from || epoch LE)
import { createCipheriv, createDecipheriv, createPrivateKey, createPublicKey, diffieHellman, hkdfSync, randomBytes } from "node:crypto";

const X_PKCS8 = Buffer.from("302e020100300506032b656e04220420", "hex");
const X_SPKI = Buffer.from("302a300506032b656e032100", "hex");
const DM_LABEL = Buffer.from("cadre-dm-v1");
const CHAN_LABEL = Buffer.from("cadre-chan-v1");
export const SEALED_DM_V1 = 1;
export const SEALED_CHANNEL_V2 = 2;
export const KEY_ENVELOPE_LEN = 104;
/** Bytes a sealed DM body adds over its plaintext, per envelope (eph, nonce, len, tag). */
export const ENVELOPE_OVERHEAD = 32 + 24 + 2 + 16;

const u8 = (b: Uint8Array | Buffer) => new Uint8Array(b);
const le32 = (v: number) => { const b = Buffer.alloc(4); b.writeUInt32LE(v >>> 0); return b; };

export function hkdf32(ikm: Uint8Array, salt: Uint8Array, info: Uint8Array): Uint8Array {
  return u8(Buffer.from(hkdfSync("sha256", ikm, salt, info, 32)));
}

export function xPub(sk: Uint8Array): Uint8Array {
  const der = createPublicKey(createPrivateKey({ key: Buffer.concat([X_PKCS8, Buffer.from(sk)]), format: "der", type: "pkcs8" })).export({ format: "der", type: "spki" });
  return u8(der.subarray(der.length - 32));
}

export function xDh(sk: Uint8Array, pk: Uint8Array): Uint8Array {
  const privateKey = createPrivateKey({ key: Buffer.concat([X_PKCS8, Buffer.from(sk)]), format: "der", type: "pkcs8" });
  const publicKey = createPublicKey({ key: Buffer.concat([X_SPKI, Buffer.from(pk)]), format: "der", type: "spki" });
  return u8(diffieHellman({ privateKey, publicKey }));
}

/** The X25519 secret a wallet derives from its 32-byte ed25519 seed. */
export function encSecretFromSeed(seed: Uint8Array): Uint8Array {
  return hkdf32(seed, DM_LABEL, Buffer.from("x25519 enc_key"));
}

export const encKeyFromSeed = (seed: Uint8Array) => xPub(encSecretFromSeed(seed));

// ------------------------------------------------------------------ XChaCha20-Poly1305

const rotl = (x: number, n: number) => ((x << n) | (x >>> (32 - n))) >>> 0;

/** HChaCha20(key, 16-byte input) -> 32-byte subkey. */
export function hchacha20(key: Uint8Array, n16: Uint8Array): Uint8Array {
  const s = new Uint32Array(16);
  s[0] = 0x61707865; s[1] = 0x3320646e; s[2] = 0x79622d32; s[3] = 0x6b206574;
  const kb = Buffer.from(key), nb = Buffer.from(n16);
  for (let i = 0; i < 8; i++) s[4 + i] = kb.readUInt32LE(i * 4);
  for (let i = 0; i < 4; i++) s[12 + i] = nb.readUInt32LE(i * 4);
  const qr = (a: number, b: number, c: number, d: number) => {
    s[a] = (s[a] + s[b]) >>> 0; s[d] = rotl(s[d] ^ s[a], 16);
    s[c] = (s[c] + s[d]) >>> 0; s[b] = rotl(s[b] ^ s[c], 12);
    s[a] = (s[a] + s[b]) >>> 0; s[d] = rotl(s[d] ^ s[a], 8);
    s[c] = (s[c] + s[d]) >>> 0; s[b] = rotl(s[b] ^ s[c], 7);
  };
  for (let i = 0; i < 10; i++) {
    qr(0, 4, 8, 12); qr(1, 5, 9, 13); qr(2, 6, 10, 14); qr(3, 7, 11, 15);
    qr(0, 5, 10, 15); qr(1, 6, 11, 12); qr(2, 7, 8, 13); qr(3, 4, 9, 14);
  }
  const out = Buffer.alloc(32);
  for (let i = 0; i < 4; i++) out.writeUInt32LE(s[i], i * 4);
  for (let i = 0; i < 4; i++) out.writeUInt32LE(s[12 + i], 16 + i * 4);
  return u8(out);
}

function xParts(key: Uint8Array, nonce: Uint8Array) {
  if (key.length !== 32 || nonce.length !== 24) throw new Error("XChaCha20-Poly1305 needs a 32-byte key and a 24-byte nonce");
  return { sub: Buffer.from(hchacha20(key, nonce.subarray(0, 16))), iv: Buffer.concat([Buffer.alloc(4), Buffer.from(nonce.subarray(16))]) };
}

export function xSeal(key: Uint8Array, nonce: Uint8Array, pt: Uint8Array, aad: Uint8Array): Uint8Array {
  const { sub, iv } = xParts(key, nonce);
  const c = createCipheriv("chacha20-poly1305", sub, iv, { authTagLength: 16 });
  c.setAAD(Buffer.from(aad), { plaintextLength: pt.length });
  return u8(Buffer.concat([c.update(Buffer.from(pt)), c.final(), c.getAuthTag()]));
}

/** null when the tag does not verify. */
export function xOpen(key: Uint8Array, nonce: Uint8Array, ct: Uint8Array, aad: Uint8Array): Uint8Array | null {
  if (ct.length < 16) return null;
  const { sub, iv } = xParts(key, nonce);
  try {
    const d = createDecipheriv("chacha20-poly1305", sub, iv, { authTagLength: 16 });
    d.setAAD(Buffer.from(aad), { plaintextLength: ct.length - 16 });
    d.setAuthTag(Buffer.from(ct.subarray(ct.length - 16)));
    return u8(Buffer.concat([d.update(Buffer.from(ct.subarray(0, ct.length - 16))), d.final()]));
  } catch {
    return null;
  }
}

// ------------------------------------------------------------------ DM bodies

const dmInfo = (from: Uint8Array, to: Uint8Array) => u8(Buffer.concat([DM_LABEL, Buffer.from(from), Buffer.from(to)]));

export interface EnvelopeFixed { ephSecret: Uint8Array; nonce: Uint8Array }

function dmEnvelope(recipientEnc: Uint8Array, from: Uint8Array, to: Uint8Array, pt: Uint8Array, fixed?: EnvelopeFixed): Buffer {
  const eph = fixed?.ephSecret ?? u8(randomBytes(32));
  const nonce = fixed?.nonce ?? u8(randomBytes(24));
  const ephPub = xPub(eph);
  const info = dmInfo(from, to);
  const k = hkdf32(xDh(eph, recipientEnc), Buffer.concat([ephPub, recipientEnc]), info);
  const ct = xSeal(k, nonce, pt, info);
  const len = Buffer.alloc(2);
  len.writeUInt16LE(ct.length);
  return Buffer.concat([ephPub, nonce, len, ct]);
}

/** Seal `text` from `from` to `to` (account keys) for the recipient's enc_key, with a copy to the
 * sender's own enc_key when given. `fixed` (tests only) pins each envelope's eph secret and nonce. */
export function sealDm(o: { from: Uint8Array; to: Uint8Array; toEncKey: Uint8Array; selfEncKey?: Uint8Array; text: string | Uint8Array; fixed?: EnvelopeFixed[] }): Uint8Array {
  const pt = typeof o.text === "string" ? Buffer.from(o.text, "utf8") : Buffer.from(o.text);
  const envs = [dmEnvelope(o.toEncKey, o.from, o.to, pt, o.fixed?.[0])];
  if (o.selfEncKey) envs.push(dmEnvelope(o.selfEncKey, o.from, o.to, pt, o.fixed?.[1]));
  return u8(Buffer.concat([Buffer.from([SEALED_DM_V1, envs.length]), ...envs]));
}

/** Framing of a sealed DM body (the chain's check_sealed_dm); returns the envelopes. */
export function parseSealedDm(body: Uint8Array): { eph: Uint8Array; nonce: Uint8Array; ct: Uint8Array }[] {
  if (body.length < 2 || body[0] !== SEALED_DM_V1) throw new Error("not a version 1 sealed DM body");
  const n = body[1];
  if (n < 1 || n > 2) throw new Error("sealed DM envelope count is 1..=2");
  const out = [];
  let p = 2;
  for (let i = 0; i < n; i++) {
    if (p + 58 > body.length) throw new Error("truncated envelope");
    const len = body[p + 56] | (body[p + 57] << 8);
    if (len < 16 || p + 58 + len > body.length) throw new Error("bad envelope length");
    out.push({ eph: body.subarray(p, p + 32), nonce: body.subarray(p + 32, p + 56), ct: body.subarray(p + 58, p + 58 + len) });
    p += 58 + len;
  }
  if (p !== body.length) throw new Error("trailing bytes after the envelopes");
  return out;
}

/** Open a sealed DM body with the reader's X25519 secret (the reader is `from` or `to`); null when
 * no envelope opens. */
export function openDm(body: Uint8Array, readerSecret: Uint8Array, from: Uint8Array, to: Uint8Array): Uint8Array | null {
  let envs;
  try { envs = parseSealedDm(body); } catch { return null; }
  const me = xPub(readerSecret);
  const info = dmInfo(from, to);
  for (const e of envs) {
    let k: Uint8Array;
    try { k = hkdf32(xDh(readerSecret, e.eph), Buffer.concat([e.eph, me]), info); } catch { continue; }
    const pt = xOpen(k, e.nonce, e.ct, info);
    if (pt) return pt;
  }
  return null;
}

// ------------------------------------------------------------------ channels

const chanKeyInfo = (channel: Uint8Array, epoch: number, member: Uint8Array) => u8(Buffer.concat([CHAN_LABEL, Buffer.from(channel), le32(epoch), Buffer.from(member)]));
const chanAad = (channel: Uint8Array, from: Uint8Array, epoch: number) => u8(Buffer.concat([CHAN_LABEL, Buffer.from(channel), Buffer.from(from), le32(epoch)]));

export function sealKeyEnvelope(o: { memberEncKey: Uint8Array; channel: Uint8Array; epoch: number; member: Uint8Array; groupKey: Uint8Array; fixed?: EnvelopeFixed }): Uint8Array {
  const eph = o.fixed?.ephSecret ?? u8(randomBytes(32));
  const nonce = o.fixed?.nonce ?? u8(randomBytes(24));
  const ephPub = xPub(eph);
  const info = chanKeyInfo(o.channel, o.epoch, o.member);
  const k = hkdf32(xDh(eph, o.memberEncKey), Buffer.concat([ephPub, o.memberEncKey]), info);
  return u8(Buffer.concat([ephPub, nonce, xSeal(k, nonce, o.groupKey, info)]));
}

export function openKeyEnvelope(env: Uint8Array, memberSecret: Uint8Array, channel: Uint8Array, epoch: number, member: Uint8Array): Uint8Array | null {
  if (env.length !== KEY_ENVELOPE_LEN) return null;
  const eph = env.subarray(0, 32);
  const info = chanKeyInfo(channel, epoch, member);
  let k: Uint8Array;
  try { k = hkdf32(xDh(memberSecret, eph), Buffer.concat([eph, xPub(memberSecret)]), info); } catch { return null; }
  return xOpen(k, env.subarray(32, 56), env.subarray(56), info);
}

export function sealChannelBody(o: { groupKey: Uint8Array; channel: Uint8Array; from: Uint8Array; epoch: number; text: string | Uint8Array; nonce?: Uint8Array }): Uint8Array {
  const pt = typeof o.text === "string" ? Buffer.from(o.text, "utf8") : Buffer.from(o.text);
  const nonce = o.nonce ?? u8(randomBytes(24));
  return u8(Buffer.concat([Buffer.from([SEALED_CHANNEL_V2]), le32(o.epoch), nonce, xSeal(o.groupKey, nonce, pt, chanAad(o.channel, o.from, o.epoch))]));
}

export function openChannelBody(body: Uint8Array, groupKey: Uint8Array, channel: Uint8Array, from: Uint8Array): { epoch: number; text: Uint8Array } | null {
  if (body.length < 1 + 4 + 24 + 16 || body[0] !== SEALED_CHANNEL_V2) return null;
  const epoch = Buffer.from(body.subarray(1, 5)).readUInt32LE(0);
  const pt = xOpen(groupKey, body.subarray(5, 29), body.subarray(29), chanAad(channel, from, epoch));
  return pt ? { epoch, text: pt } : null;
}
