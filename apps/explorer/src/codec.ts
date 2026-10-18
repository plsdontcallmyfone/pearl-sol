// Byte helpers with no dependencies: base58, hex, little-endian readers, sha256 and
// Solana program-derived addresses (with the ed25519 on-curve check done in BigInt).

import { createHash } from "node:crypto";

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const B58_MAP = new Map([...B58].map((c, i) => [c, i]));

export function b58encode(bytes: Uint8Array): string {
  let zeros = 0;
  while (zeros < bytes.length && bytes[zeros] === 0) zeros++;
  let n = 0n;
  for (const b of bytes) n = (n << 8n) | BigInt(b);
  let out = "";
  while (n > 0n) {
    out = B58[Number(n % 58n)] + out;
    n /= 58n;
  }
  return "1".repeat(zeros) + out;
}

export function b58decode(s: string): Uint8Array {
  let zeros = 0;
  while (zeros < s.length && s[zeros] === "1") zeros++;
  let n = 0n;
  for (const c of s) {
    const v = B58_MAP.get(c);
    if (v === undefined) throw new Error(`bad base58 character ${JSON.stringify(c)}`);
    n = n * 58n + BigInt(v);
  }
  const body: number[] = [];
  while (n > 0n) {
    body.unshift(Number(n & 0xffn));
    n >>= 8n;
  }
  return Uint8Array.from([...new Array(zeros).fill(0), ...body]);
}

export const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");
export const unhex = (s: string) => Uint8Array.from(Buffer.from(s, "hex"));
export const isZeroHex = (s: string) => /^0+$/.test(s);

export const sha256 = (...parts: Uint8Array[]) => {
  const h = createHash("sha256");
  for (const p of parts) h.update(p);
  return Uint8Array.from(h.digest());
};

export const u64le = (n: bigint | number) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(n));
  return Uint8Array.from(b);
};
export const rd8 = (d: Uint8Array, o: number) => d[o];
export const rd16 = (d: Uint8Array, o: number) => Buffer.from(d.buffer, d.byteOffset, d.byteLength).readUInt16LE(o);
export const rd32 = (d: Uint8Array, o: number) => Buffer.from(d.buffer, d.byteOffset, d.byteLength).readUInt32LE(o);
export const rd64 = (d: Uint8Array, o: number) => Buffer.from(d.buffer, d.byteOffset, d.byteLength).readBigUInt64LE(o);
export const rdi64 = (d: Uint8Array, o: number) => Buffer.from(d.buffer, d.byteOffset, d.byteLength).readBigInt64LE(o);
export const slice = (d: Uint8Array, o: number, n: number) => d.subarray(o, o + n);
export const pk = (d: Uint8Array, o: number) => b58encode(slice(d, o, 32));

// ------------------------------------------------------------ ed25519 on-curve check

const P = 2n ** 255n - 19n;
const D = (((-121665n * modpow(121666n, P - 2n, P)) % P) + P) % P;

function modpow(b: bigint, e: bigint, m: bigint): bigint {
  let r = 1n;
  b %= m;
  while (e > 0n) {
    if (e & 1n) r = (r * b) % m;
    b = (b * b) % m;
    e >>= 1n;
  }
  return r;
}

/** True when the 32 bytes decompress to a point on ed25519 (as curve25519-dalek does). */
export function isOnCurve(bytes: Uint8Array): boolean {
  if (bytes.length !== 32) return false;
  const yb = Uint8Array.from(bytes);
  yb[31] &= 0x7f;
  let y = 0n;
  for (let i = 31; i >= 0; i--) y = (y << 8n) | BigInt(yb[i]);
  if (y >= P) return false;
  const y2 = (y * y) % P;
  const u = (y2 - 1n + P) % P;
  const v = (D * y2 + 1n) % P;
  const x2 = (u * modpow(v, P - 2n, P)) % P;
  if (x2 === 0n) return true;
  return modpow(x2, (P - 1n) / 2n, P) === 1n;
}

const PDA_MARKER = new TextEncoder().encode("ProgramDerivedAddress");

/** Solana findProgramAddress: [address base58, bump]. */
export function findPda(seeds: Uint8Array[], programId: string): [string, number] {
  const prog = b58decode(programId);
  for (let bump = 255; bump >= 0; bump--) {
    const h = sha256(...seeds, Uint8Array.of(bump), prog, PDA_MARKER);
    if (!isOnCurve(h)) return [b58encode(h), bump];
  }
  throw new Error("no viable bump");
}

export const utf8 = (s: string) => new TextEncoder().encode(s);
