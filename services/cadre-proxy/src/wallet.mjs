// The proxy's own wallet: one ed25519 key, the same key on Solana (x402 payer) and on the
// Cadre chain (balance owner / ticket poster). Stored as a Solana-style JSON array of 64
// bytes (seed || public key) in a 0600 file under a 0700 directory. The secret never
// leaves this module: callers get `publicKey`, `address` and `sign(msg)` only.
import { createPrivateKey, createPublicKey, randomBytes, sign as edSign } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, statSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { b58encode } from "./b58.mjs";

const PKCS8_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");

function keyFromSeed(seed) {
  return createPrivateKey({ key: Buffer.concat([PKCS8_PREFIX, Buffer.from(seed)]), format: "der", type: "pkcs8" });
}

function pubOf(priv) {
  const der = createPublicKey(priv).export({ format: "der", type: "spki" });
  return new Uint8Array(der.subarray(der.length - 32));
}

/** Load the wallet at `path`, creating it (0600) when missing. */
export function loadWallet(path, { create = true } = {}) {
  let created = false;
  if (!existsSync(path)) {
    if (!create) throw new Error(`no wallet at ${path} (run: cadre-proxy wallet)`);
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
    const seed = randomBytes(32);
    const pub = pubOf(keyFromSeed(seed));
    const tmp = `${path}.tmp-${process.pid}`;
    writeFileSync(tmp, JSON.stringify([...seed, ...pub]), { mode: 0o600 });
    chmodSync(tmp, 0o600);
    renameSync(tmp, path);
    seed.fill(0);
    created = true;
  }
  const mode = statSync(path).mode & 0o777;
  if (mode & 0o077) throw new Error(`wallet file ${path} is readable by others (mode ${mode.toString(8)}); chmod 600 it`);
  const arr = JSON.parse(readFileSync(path, "utf8"));
  if (!Array.isArray(arr) || (arr.length !== 64 && arr.length !== 32)) throw new Error(`wallet file ${path} is not a 64-byte key array`);
  const priv = keyFromSeed(Uint8Array.from(arr.slice(0, 32)));
  arr.fill(0);
  const publicKey = pubOf(priv);
  return {
    created,
    path,
    publicKey,
    address: b58encode(publicKey),
    /** Raw ed25519 signature (64 bytes). */
    sign: (msg) => new Uint8Array(edSign(null, Buffer.from(msg), priv)),
    toJSON: () => ({ address: b58encode(publicKey) }),
  };
}
