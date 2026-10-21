// Sealing a 32-byte agent seed in transit, and the key store at rest.
//
// Transit (browser -> runtime on upload, runtime -> browser on export): X25519 ECDH between an
// ephemeral key and the recipient's key, HKDF-SHA256 (salt = eph_pub || recipient_pub, info
// "cadre-agents:seal:v1") to a 32-byte key, AES-256-GCM (12-byte iv, 16-byte tag appended). The
// browser does the same with WebCrypto (~/cadre-web lib/agentkey.ts); test/seal.test.ts pins it.
//
// At rest: every agent seed is AES-256-GCM encrypted under the runtime's master key (32 random
// bytes in a 0600 file outside the repo), with the agent address as associated data, one file per
// agent (0600). The master key never leaves the host; the runtime's X25519 key is derived from it
// (HKDF info "cadre-agents:x25519:v1"), so one secret file backs both. Nothing here logs a key.
import { createCipheriv, createDecipheriv, createPrivateKey, createPublicKey, diffieHellman, hkdfSync, randomBytes, type KeyObject } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

const X25519_PKCS8 = Buffer.from("302e020100300506032b656e04220420", "hex");
const X25519_SPKI = Buffer.from("302a300506032b656e032100", "hex");
const INFO_SEAL = Buffer.from("cadre-agents:seal:v1");

export interface Sealed { eph_pub: string; iv: string; ct: string } // base64 fields

export function x25519FromSeed(seed: Uint8Array): { priv: KeyObject; pub: Buffer } {
  const priv = createPrivateKey({ key: Buffer.concat([X25519_PKCS8, Buffer.from(seed)]), format: "der", type: "pkcs8" });
  const der = createPublicKey(priv).export({ format: "der", type: "spki" }) as Buffer;
  return { priv, pub: der.subarray(der.length - 32) };
}

const pubKeyObj = (raw: Uint8Array) => createPublicKey({ key: Buffer.concat([X25519_SPKI, Buffer.from(raw)]), format: "der", type: "spki" });

function sealKey(shared: Buffer, ephPub: Uint8Array, recipPub: Uint8Array): Buffer {
  return Buffer.from(hkdfSync("sha256", shared, Buffer.concat([Buffer.from(ephPub), Buffer.from(recipPub)]), INFO_SEAL, 32));
}

/** Seal `plain` to the recipient's raw X25519 public key. */
export function seal(plain: Uint8Array, recipPub: Uint8Array): Sealed {
  const eph = x25519FromSeed(randomBytes(32));
  const shared = diffieHellman({ privateKey: eph.priv, publicKey: pubKeyObj(recipPub) });
  const key = sealKey(shared, eph.pub, recipPub);
  const iv = randomBytes(12);
  const c = createCipheriv("aes-256-gcm", key, iv);
  const ct = Buffer.concat([c.update(plain), c.final(), c.getAuthTag()]);
  key.fill(0);
  return { eph_pub: eph.pub.toString("base64"), iv: iv.toString("base64"), ct: ct.toString("base64") };
}

/** Open a sealed box with the recipient's X25519 private key. Throws on any tampering. */
export function open(s: Sealed, recip: { priv: KeyObject; pub: Uint8Array }): Buffer {
  const ephPub = Buffer.from(s.eph_pub, "base64");
  const iv = Buffer.from(s.iv, "base64");
  const ct = Buffer.from(s.ct, "base64");
  if (ephPub.length !== 32 || iv.length !== 12 || ct.length < 17) throw new Error("sealed box is malformed");
  const shared = diffieHellman({ privateKey: recip.priv, publicKey: pubKeyObj(ephPub) });
  const key = sealKey(shared, ephPub, recip.pub);
  const d = createDecipheriv("aes-256-gcm", key, iv);
  d.setAuthTag(ct.subarray(ct.length - 16));
  const out = Buffer.concat([d.update(ct.subarray(0, ct.length - 16)), d.final()]);
  key.fill(0);
  return out;
}

// ---------------------------------------------------------------- at rest

export class KeyStore {
  private master: Buffer;
  readonly x: { priv: KeyObject; pub: Buffer };
  dir: string;
  constructor(dir: string, masterFile: string) {
    this.dir = dir;
    if (!existsSync(masterFile)) {
      mkdirSync(dirname(masterFile), { recursive: true, mode: 0o700 });
      writeFileSync(masterFile, randomBytes(32).toString("hex"), { mode: 0o600 });
    }
    const st = readFileSync(masterFile, "utf8").trim();
    if (!/^[0-9a-f]{64}$/.test(st)) throw new Error(`master key file ${masterFile} is not 32 bytes of hex`);
    this.master = Buffer.from(st, "hex");
    this.x = x25519FromSeed(Buffer.from(hkdfSync("sha256", this.master, Buffer.alloc(0), Buffer.from("cadre-agents:x25519:v1"), 32)));
    mkdirSync(dir, { recursive: true, mode: 0o700 });
  }
  get publicKey(): string {
    return this.x.pub.toString("base64");
  }
  private file(agent: string) {
    if (!/^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(agent)) throw new Error("bad agent address");
    return `${this.dir}/${agent}.key.json`;
  }
  has(agent: string): boolean {
    return existsSync(this.file(agent));
  }
  put(agent: string, seed: Uint8Array) {
    const iv = randomBytes(12);
    const c = createCipheriv("aes-256-gcm", this.master, iv);
    c.setAAD(Buffer.from(agent));
    const ct = Buffer.concat([c.update(seed), c.final(), c.getAuthTag()]);
    const f = this.file(agent);
    const tmp = `${f}.tmp-${process.pid}`;
    writeFileSync(tmp, JSON.stringify({ v: 1, alg: "aes-256-gcm", agent, iv: iv.toString("base64"), ct: ct.toString("base64") }), { mode: 0o600 });
    chmodSync(tmp, 0o600);
    renameSync(tmp, f);
  }
  get(agent: string): Buffer {
    const j = JSON.parse(readFileSync(this.file(agent), "utf8"));
    if (j.v !== 1 || j.agent !== agent) throw new Error("key file does not match the agent");
    const ct = Buffer.from(j.ct, "base64");
    const d = createDecipheriv("aes-256-gcm", this.master, Buffer.from(j.iv, "base64"));
    d.setAAD(Buffer.from(agent));
    d.setAuthTag(ct.subarray(ct.length - 16));
    return Buffer.concat([d.update(ct.subarray(0, ct.length - 16)), d.final()]);
  }
  /** Overwrite then remove (best effort on journaling file systems; the master key is what protects old copies). */
  wipe(agent: string) {
    const f = this.file(agent);
    if (!existsSync(f)) return;
    writeFileSync(f, randomBytes(256).toString("base64"));
    unlinkSync(f);
  }
}
