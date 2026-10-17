// ed25519 + hashes. One 32-byte seed is the account on both chains: Solana
// keypairs are seed || pubkey, and Commonware ed25519 keys are the same curve and
// the same seed expansion (SPEC 2).

import { ed25519 } from "@noble/curves/ed25519";
import { blake3 as nobleBlake3 } from "@noble/hashes/blake3";
import { sha256 as nobleSha256 } from "@noble/hashes/sha256";
import { concat, utf8, varint } from "./bytes";

export const TX_NAMESPACE = utf8("pearl-sol:tx:v1");

export const pubkeyOf = (seed: Uint8Array) => ed25519.getPublicKey(seed);
export const signRaw = (seed: Uint8Array, msg: Uint8Array) => ed25519.sign(msg, seed);
export const verifyRaw = (sig: Uint8Array, msg: Uint8Array, pub: Uint8Array) => {
  try {
    return ed25519.verify(sig, msg, pub);
  } catch {
    return false;
  }
};

/** Commonware `sign(ns, msg)` framing: varint(len ns) || ns || msg. */
export const frame = (ns: Uint8Array, msg: Uint8Array) => concat(varint(ns.length), ns, msg);
export const signFramed = (seed: Uint8Array, ns: Uint8Array, msg: Uint8Array) => signRaw(seed, frame(ns, msg));

export const blake3 = (...parts: Uint8Array[]) => nobleBlake3(concat(...parts));
export const blake3Keyed = (key: Uint8Array, data: Uint8Array) => nobleBlake3(data, { key });
export const sha256 = (...parts: Uint8Array[]) => nobleSha256(concat(...parts));

/** Solana: a valid PDA must not be on the curve. */
export function isOnCurve(b: Uint8Array): boolean {
  try {
    ed25519.ExtendedPoint.fromHex(b);
    return true;
  } catch {
    return false;
  }
}

/** Accepts a 32-byte seed or a 64-byte Solana secret key (seed || pubkey, checked). */
export function seedFromSecret(sk: Uint8Array): Uint8Array {
  if (sk.length === 32) return sk.slice();
  if (sk.length === 64) {
    const seed = sk.slice(0, 32);
    const pub = pubkeyOf(seed);
    for (let i = 0; i < 32; i++) if (pub[i] !== sk[32 + i]) throw new Error("secret key halves do not match");
    return seed;
  }
  throw new Error(`a secret key is 32 or 64 bytes, got ${sk.length}`);
}
