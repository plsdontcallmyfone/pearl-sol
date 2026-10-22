// The agent's own ed25519 key, held in this process only. A Solana keypair is the same key as its
// Cadre account (base58 address = Solana address; Cx is the display form, docs/SPEC.md 2.1).
// Accepted inputs: a Solana CLI key file (JSON array of 64 bytes: seed || public key, or 32 bytes
// of seed), a base58 secret key string (64 bytes, the Phantom export form), or raw seed bytes.
// The secret never leaves this module: callers get `address`, `publicKey`, `sign(msg)` and the
// DM helpers. toJSON / inspect are overridden so an accidental log prints only the address.
import { createPrivateKey, createPublicKey, randomBytes, sign as edSign, verify as edVerify } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, statSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { b58decode, b58encode } from "./vendor/b58.ts";
import { toCx } from "./vendor/cx.ts";
import { encSecretFromSeed, openDm, sealDm, xPub } from "./vendor/dmcrypto.ts";

const PKCS8 = Buffer.from("302e020100300506032b657004220420", "hex");
const SPKI = Buffer.from("302a300506032b6570032100", "hex");

export interface AgentKey {
  address: string; // base58 (the Solana address and the Cadre account)
  cx: string; // Cx display form
  publicKey: Uint8Array;
  sign(msg: Uint8Array): Uint8Array;
  /** X25519 DM key (SPEC 4.10), derived from the seed. */
  encKey: Uint8Array;
  sealDm(to: Uint8Array, toEncKey: Uint8Array, text: string): Uint8Array;
  openDm(body: Uint8Array, from: Uint8Array, to: Uint8Array): string | null;
}

export function keyFromSeed(seed0: Uint8Array): AgentKey {
  if (seed0.length !== 32) throw new Error("an ed25519 seed is 32 bytes");
  const seed = Buffer.from(seed0);
  const priv = createPrivateKey({ key: Buffer.concat([PKCS8, seed]), format: "der", type: "pkcs8" });
  const der = createPublicKey(priv).export({ format: "der", type: "spki" });
  const publicKey = new Uint8Array(der.subarray(der.length - 32));
  const encSecret = encSecretFromSeed(seed);
  seed.fill(0);
  const address = b58encode(publicKey);
  const k: AgentKey = {
    address,
    cx: toCx(publicKey),
    publicKey,
    sign: (msg) => new Uint8Array(edSign(null, Buffer.from(msg), priv)),
    encKey: xPub(encSecret),
    sealDm: (to, toEncKey, text) => sealDm({ from: publicKey, to, toEncKey, selfEncKey: xPub(encSecret), text }),
    openDm: (body, from, to) => {
      const pt = openDm(body, encSecret, from, to);
      if (!pt) return null;
      const t = Buffer.from(pt).toString("utf8");
      return Buffer.from(t, "utf8").equals(Buffer.from(pt)) ? t : null;
    },
  };
  Object.defineProperty(k, "toJSON", { value: () => ({ address }), enumerable: false });
  Object.defineProperty(k, Symbol.for("nodejs.util.inspect.custom"), { value: () => `AgentKey(${address})`, enumerable: false });
  return k;
}

/** Parse key material (Solana JSON array, base58 secret, or a 32/64-byte array) into a key. */
export function keyFrom(material: string | Uint8Array | number[]): AgentKey {
  let bytes: Uint8Array;
  if (typeof material === "string") {
    const s = material.trim();
    if (s.startsWith("[")) bytes = Uint8Array.from(JSON.parse(s));
    else bytes = b58decode(s);
  } else bytes = Uint8Array.from(material as ArrayLike<number>);
  if (bytes.length !== 64 && bytes.length !== 32) throw new Error("key material is a 64-byte secret key (seed || public key) or a 32-byte seed");
  const k = keyFromSeed(bytes.subarray(0, 32));
  if (bytes.length === 64 && b58encode(bytes.subarray(32)) !== k.address) throw new Error("the key's public half does not match its seed");
  bytes.fill(0);
  return k;
}

/** Load a key file. Refuses files readable by group or others (chmod 600). */
export function loadKeyFile(path: string): AgentKey {
  const mode = statSync(path).mode & 0o777;
  if (mode & 0o077) throw new Error(`key file ${path} is readable by others (mode ${mode.toString(8)}); chmod 600 it`);
  return keyFrom(readFileSync(path, "utf8"));
}

/** Create a new Solana-format key file (0600) if missing; returns the address only. */
export function createKeyFile(path: string): { address: string; created: boolean } {
  if (existsSync(path)) return { address: loadKeyFile(path).address, created: false };
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const seed = randomBytes(32);
  const k = keyFromSeed(seed);
  const tmp = `${path}.tmp-${process.pid}`;
  writeFileSync(tmp, JSON.stringify([...seed, ...k.publicKey]), { mode: 0o600 });
  chmodSync(tmp, 0o600);
  renameSync(tmp, path);
  seed.fill(0);
  return { address: k.address, created: true };
}

export function verify(publicKey: Uint8Array, msg: Uint8Array, sig: Uint8Array): boolean {
  const pk = createPublicKey({ key: Buffer.concat([SPKI, Buffer.from(publicKey)]), format: "der", type: "spki" });
  return edVerify(null, Buffer.from(msg), pk, Buffer.from(sig));
}
