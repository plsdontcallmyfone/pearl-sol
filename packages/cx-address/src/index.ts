// Cadre "Cx" address format (docs/SPEC.md section 2.1): display and parsing of Cadre-chain
// account keys. Nothing changes on chain: an address is still the raw 32-byte ed25519 public key.
//
//   Cx + 64 hex characters of the key, with EIP-55 style checksum casing over blake3:
//   h = blake3(ascii(lowercase 64-hex)); letter at hex position i is uppercased iff nibble i of h >= 8
//   (nibble 2j is the high nibble of h[j], nibble 2j+1 the low one).
//
// Parsing accepts the checksummed Cx form, all-lowercase Cx (no checksum), and the legacy base58
// form; any other casing is a checksum failure. Dependency free and browser safe (no Buffer, no
// node: imports), so services, the explorer, ~/cadre-web and the Cadre Wallet share this one file.
// Shared test vectors: vectors/cx-address.json (also checked by chain/agent-types).

export const CX_PREFIX = "Cx";
export const CX_LENGTH = 66;


// ---------- base58 (legacy form) ----------

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const B58_MAP = new Map([...B58].map((c, i) => [c, i]));

export function base58Encode(bytes: Uint8Array): string {
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

export function base58Decode(s: string): Uint8Array {
  let zeros = 0;
  while (zeros < s.length && s[zeros] === "1") zeros++;
  let n = 0n;
  for (const c of s) {
    const v = B58_MAP.get(c);
    if (v === undefined) throw new CxError("bad_base58", `${JSON.stringify(c)} is not a base58 character`);
    n = n * 58n + BigInt(v);
  }
  const body: number[] = [];
  while (n > 0n) {
    body.unshift(Number(n & 0xffn));
    n >>= 8n;
  }
  return Uint8Array.from([...new Array(zeros).fill(0), ...body]);
}

// ---------- Cx ----------

export type CxErrorCode = "empty" | "bad_length" | "bad_hex" | "checksum" | "bad_base58" | "not_32_bytes";

export class CxError extends Error {
  readonly code: CxErrorCode;
  constructor(code: CxErrorCode, message: string) {
    super(message);
    this.code = code;
    this.name = "CxError";
  }
}

const HEX = "0123456789abcdef";

function toHex(b: Uint8Array): string {
  let s = "";
  for (const x of b) s += HEX[x >> 4] + HEX[x & 15];
  return s;
}

function fromHex(s: string): Uint8Array {
  const out = new Uint8Array(s.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(s.slice(2 * i, 2 * i + 2), 16);
  return out;
}

/** The checksummed 64 hex characters (no prefix) for a lowercase 64-hex string. */
function checksumHex(lower: string): string {
  const h = blake3(new TextEncoder().encode(lower));
  let out = "";
  for (let i = 0; i < lower.length; i++) {
    const c = lower[i];
    const nib = i % 2 === 0 ? h[i >> 1] >> 4 : h[i >> 1] & 15;
    out += c >= "a" && c <= "f" && nib >= 8 ? c.toUpperCase() : c;
  }
  return out;
}

function asKey(key: Uint8Array | string): Uint8Array {
  if (typeof key === "string") return parseAddress(key);
  if (key.length !== 32) throw new CxError("not_32_bytes", `an address is 32 bytes, got ${key.length}`);
  return key;
}

/** Full checksummed Cx address (66 characters) for a 32-byte key, or for any accepted address string. */
export function toCx(key: Uint8Array | string): string {
  return CX_PREFIX + checksumHex(toHex(asKey(key)));
}

/** Legacy base58 form of a key (or of any accepted address string). */
export function toBase58(key: Uint8Array | string): string {
  return base58Encode(asKey(key));
}

/** Short UI form: Cx + first 4 + "..." + last 4 checksummed hex characters, e.g. Cxa380...7D06. */
export function shortCx(key: Uint8Array | string): string {
  const full = toCx(key);
  return full.slice(0, 6) + "..." + full.slice(-4);
}

/**
 * Parse an address typed or pasted by a person or sent by a client. Accepts:
 * - `Cx` + 64 hex with the correct checksum casing;
 * - `Cx` + 64 all-lowercase hex (no checksum, like an all-lowercase Ethereum address);
 * - the legacy base58 form of the 32-byte key.
 * Throws CxError otherwise; a Cx string in any other casing fails with code "checksum".
 */
export function parseAddress(input: string): Uint8Array {
  const s = (input ?? "").trim();
  if (!s) throw new CxError("empty", "address is empty");
  if (s.startsWith(CX_PREFIX)) {
    const body = s.slice(2);
    if (body.length !== 64) throw new CxError("bad_length", `a Cx address is Cx + 64 hex characters (66 in all), got ${s.length}`);
    if (!/^[0-9a-fA-F]{64}$/.test(body)) throw new CxError("bad_hex", "a Cx address has only hex characters (0-9, a-f) after Cx");
    const lower = body.toLowerCase();
    if (body !== lower && body !== checksumHex(lower)) {
      throw new CxError("checksum", "address checksum failed: the upper and lower case letters do not match this address, so it was probably mistyped (an all-lowercase Cx address is also accepted)");
    }
    return fromHex(lower);
  }
  let v: Uint8Array;
  try {
    v = base58Decode(s);
  } catch {
    throw new CxError("bad_base58", `${JSON.stringify(s.length > 80 ? s.slice(0, 80) + "..." : s)} is not a Cadre address (expected Cx + 64 hex, or a legacy base58 key)`);
  }
  if (v.length !== 32) throw new CxError("not_32_bytes", `base58 address decodes to ${v.length} bytes, not 32`);
  return v;
}

/** True when parseAddress would accept the string. */
export function isAddress(input: unknown): boolean {
  if (typeof input !== "string") return false;
  try {
    parseAddress(input);
    return true;
  } catch {
    return false;
  }
}

/** Display form of an accepted address string: its checksummed Cx (null when it does not parse). */
export function displayCx(input: string): string | null {
  try {
    return toCx(parseAddress(input));
  } catch {
    return null;
  }
}

/** Short display of an accepted address string, or the input unchanged when it does not parse. */
export function shortAddress(input: string): string {
  try {
    return shortCx(parseAddress(input));
  } catch {
    return input;
  }
}

/** Base58 form of an accepted address string, for clients and nodes that still take base58 only. */
export function normalizeToBase58(input: string): string {
  return base58Encode(parseAddress(input));
}

/** True when two accepted address strings name the same key. */
export function sameAddress(a: string, b: string): boolean {
  try {
    const x = parseAddress(a), y = parseAddress(b);
    return x.every((v, i) => v === y[i]);
  } catch {
    return false;
  }
}

// ---------- blake3 (copied from apps/explorer/src/blake3.ts, dependency free) ----------

const IV = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
const PERM = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START = 1, CHUNK_END = 2, PARENT = 4, ROOT = 8;

const rotr = (x: number, n: number) => ((x >>> n) | (x << (32 - n))) >>> 0;

function g(s: Uint32Array, a: number, b: number, c: number, d: number, x: number, y: number) {
  s[a] = (s[a] + s[b] + x) >>> 0;
  s[d] = rotr(s[d] ^ s[a], 16);
  s[c] = (s[c] + s[d]) >>> 0;
  s[b] = rotr(s[b] ^ s[c], 12);
  s[a] = (s[a] + s[b] + y) >>> 0;
  s[d] = rotr(s[d] ^ s[a], 8);
  s[c] = (s[c] + s[d]) >>> 0;
  s[b] = rotr(s[b] ^ s[c], 7);
}

function compress(cv: Uint32Array, m: Uint32Array, counter: number, len: number, flags: number): Uint32Array {
  const s = new Uint32Array([...cv, IV[0], IV[1], IV[2], IV[3], counter >>> 0, Math.floor(counter / 2 ** 32) >>> 0, len, flags]);
  let w = Uint32Array.from(m);
  for (let r = 0; r < 7; r++) {
    g(s, 0, 4, 8, 12, w[0], w[1]);
    g(s, 1, 5, 9, 13, w[2], w[3]);
    g(s, 2, 6, 10, 14, w[4], w[5]);
    g(s, 3, 7, 11, 15, w[6], w[7]);
    g(s, 0, 5, 10, 15, w[8], w[9]);
    g(s, 1, 6, 11, 12, w[10], w[11]);
    g(s, 2, 7, 8, 13, w[12], w[13]);
    g(s, 3, 4, 9, 14, w[14], w[15]);
    w = Uint32Array.from(PERM.map((i) => w[i]));
  }
  for (let i = 0; i < 8; i++) {
    s[i] ^= s[i + 8];
    s[i + 8] ^= cv[i];
  }
  return s;
}

function words(b: Uint8Array): Uint32Array {
  const p = new Uint8Array(64);
  p.set(b);
  const w = new Uint32Array(16);
  for (let i = 0; i < 16; i++) w[i] = (p[i * 4] | (p[i * 4 + 1] << 8) | (p[i * 4 + 2] << 16) | (p[i * 4 + 3] << 24)) >>> 0;
  return w;
}

interface Out {
  cv: Uint32Array;
  block: Uint32Array;
  counter: number;
  len: number;
  flags: number;
}

function chunkOut(chunk: Uint8Array, counter: number): Out {
  let cv = IV;
  const blocks = Math.max(1, Math.ceil(chunk.length / 64));
  for (let i = 0; i < blocks - 1; i++) {
    cv = compress(cv, words(chunk.subarray(i * 64, i * 64 + 64)), counter, 64, i === 0 ? CHUNK_START : 0).subarray(0, 8);
  }
  const last = chunk.subarray((blocks - 1) * 64);
  return { cv, block: words(last), counter, len: last.length, flags: (blocks === 1 ? CHUNK_START : 0) | CHUNK_END };
}

const outCv = (o: Out) => compress(o.cv, o.block, o.counter, o.len, o.flags).subarray(0, 8);
const parentOut = (l: Uint32Array, r: Uint32Array): Out => ({ cv: IV, block: Uint32Array.from([...l, ...r]), counter: 0, len: 64, flags: PARENT });

function blake3(...parts: Uint8Array[]): Uint8Array {
  const total = parts.reduce((n, p) => n + p.length, 0);
  const data = new Uint8Array(total);
  let off = 0;
  for (const p of parts) {
    data.set(p, off);
    off += p.length;
  }
  const n = Math.max(1, Math.ceil(data.length / 1024));
  const stack: Uint32Array[] = [];
  let out: Out | null = null;
  for (let i = 0; i < n; i++) {
    const o = chunkOut(data.subarray(i * 1024, i * 1024 + 1024), i);
    if (i === n - 1) {
      out = o;
      break;
    }
    let cv = outCv(o);
    let t = i + 1;
    while ((t & 1) === 0) {
      cv = outCv(parentOut(stack.pop()!, cv));
      t >>= 1;
    }
    stack.push(cv);
  }
  while (stack.length) out = parentOut(stack.pop()!, outCv(out!));
  const s = compress(out!.cv, out!.block, out!.counter, out!.len, out!.flags | ROOT);
  const res = new Uint8Array(32);
  for (let i = 0; i < 8; i++) for (let j = 0; j < 4; j++) res[i * 4 + j] = (s[i] >>> (8 * j)) & 0xff;
  return res;
}
