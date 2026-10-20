// ed25519 keys with node:crypto. A key file is a Solana-style JSON array of 64 bytes
// (seed || public key), mode 0600 in a 0700 directory. The secret never leaves this module:
// callers get `publicKey`, `address` and `sign(msg)`. Keys are never printed or logged.
import { createPrivateKey, createPublicKey, randomBytes, sign as edSign, verify as edVerify } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, statSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { b58encode } from "./b58.ts";

const PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");
const SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");

export interface Wallet {
  publicKey: Uint8Array;
  address: string;
  sign: (msg: Uint8Array) => Uint8Array;
  /** Only for the leak test: does `s` contain this key's secret in any common encoding? */
  leaksIn: (s: string) => boolean;
}

function keyFromSeed(seed: Uint8Array) {
  return createPrivateKey({ key: Buffer.concat([PKCS8_PREFIX, Buffer.from(seed)]), format: "der", type: "pkcs8" });
}

function pubOf(priv: ReturnType<typeof createPrivateKey>): Uint8Array {
  const der = createPublicKey(priv).export({ format: "der", type: "spki" });
  return new Uint8Array(der.subarray(der.length - 32));
}

export function walletFromSeed(seed: Uint8Array): Wallet {
  const priv = keyFromSeed(seed);
  const publicKey = pubOf(priv);
  const full = Uint8Array.from([...seed, ...publicKey]);
  const forms = [Buffer.from(seed).toString("hex"), b58encode(full), JSON.stringify([...full]), JSON.stringify([...seed]), Buffer.from(seed).toString("base64")];
  return {
    publicKey,
    address: b58encode(publicKey),
    sign: (msg) => new Uint8Array(edSign(null, Buffer.from(msg), priv)),
    leaksIn: (s) => forms.some((f) => s.includes(f)) || s.includes(JSON.stringify([...seed]).slice(1, 40)),
  };
}

export function verifySig(pubkey: Uint8Array, msg: Uint8Array, sig: Uint8Array): boolean {
  const pk = createPublicKey({ key: Buffer.concat([SPKI_PREFIX, Buffer.from(pubkey)]), format: "der", type: "spki" });
  return edVerify(null, Buffer.from(msg), pk, Buffer.from(sig));
}

/** Create a key file (0600) if missing. Returns the public address only. */
export function createKeyFile(path: string): { address: string; created: boolean } {
  if (existsSync(path)) return { address: loadWallet(path).address, created: false };
  mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const seed = randomBytes(32);
  const w = walletFromSeed(seed);
  const tmp = `${path}.tmp-${process.pid}`;
  writeFileSync(tmp, JSON.stringify([...seed, ...w.publicKey]), { mode: 0o600 });
  chmodSync(tmp, 0o600);
  renameSync(tmp, path);
  seed.fill(0);
  return { address: w.address, created: true };
}

export function loadWallet(path: string): Wallet {
  const mode = statSync(path).mode & 0o777;
  if (mode & 0o077) throw new Error(`key file ${path} is readable by others (mode ${mode.toString(8)}); chmod 600 it`);
  const arr = JSON.parse(readFileSync(path, "utf8"));
  if (!Array.isArray(arr) || (arr.length !== 64 && arr.length !== 32)) throw new Error(`key file ${path} is not a 64-byte key array`);
  const seed = Uint8Array.from(arr.slice(0, 32));
  arr.fill(0);
  return walletFromSeed(seed);
}
