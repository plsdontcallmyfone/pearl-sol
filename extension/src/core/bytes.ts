// Byte helpers shared by every encoder. No Buffer: this runs in the MV3 service
// worker, the popup and the page, so it sticks to Uint8Array and DataView.

import { base58, base64 } from "@scure/base";

export const utf8 = (s: string) => new TextEncoder().encode(s);
export const fromUtf8 = (b: Uint8Array) => new TextDecoder().decode(b);

export function hex(b: Uint8Array): string {
  let s = "";
  for (const x of b) s += x.toString(16).padStart(2, "0");
  return s;
}

export function fromHex(s: string): Uint8Array {
  const t = s.startsWith("0x") ? s.slice(2) : s;
  if (t.length % 2 !== 0 || /[^0-9a-fA-F]/.test(t)) throw new Error("bad hex");
  const out = new Uint8Array(t.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(t.slice(2 * i, 2 * i + 2), 16);
  return out;
}

export const b58 = (b: Uint8Array) => base58.encode(b);
export const fromB58 = (s: string) => base58.decode(s);
export const b64 = (b: Uint8Array) => base64.encode(b);
export const fromB64 = (s: string) => base64.decode(s);

/** 32-byte key from base58 or hex (64 chars). */
export function key32(s: string): Uint8Array {
  const t = s.trim();
  const b = /^(0x)?[0-9a-fA-F]{64}$/.test(t) ? fromHex(t) : fromB58(t);
  if (b.length !== 32) throw new Error("expected a 32-byte key");
  return b;
}

export function isAddress(s: string): boolean {
  try {
    return fromB58(s.trim()).length === 32;
  } catch {
    return false;
  }
}

export function concat(...parts: Uint8Array[]): Uint8Array {
  const n = parts.reduce((a, p) => a + p.length, 0);
  const out = new Uint8Array(n);
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

export function equal(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

export const U64_MAX = (1n << 64n) - 1n;

export function u8(n: number) {
  if (!Number.isInteger(n) || n < 0 || n > 0xff) throw new Error(`u8 out of range: ${n}`);
  return Uint8Array.of(n);
}
export function u16(n: number) {
  if (!Number.isInteger(n) || n < 0 || n > 0xffff) throw new Error(`u16 out of range: ${n}`);
  const b = new Uint8Array(2);
  new DataView(b.buffer).setUint16(0, n, true);
  return b;
}
export function u32(n: number) {
  if (!Number.isInteger(n) || n < 0 || n > 0xffffffff) throw new Error(`u32 out of range: ${n}`);
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, n, true);
  return b;
}
export function u64(n: bigint | number) {
  const v = BigInt(n);
  if (v < 0n || v > U64_MAX) throw new Error(`u64 out of range: ${v}`);
  const b = new Uint8Array(8);
  new DataView(b.buffer).setBigUint64(0, v, true);
  return b;
}

/** Commonware / protobuf style unsigned LEB128 varint. */
export function varint(n: number): Uint8Array {
  const out: number[] = [];
  let v = n;
  do {
    let byte = v & 0x7f;
    v = Math.floor(v / 128);
    if (v > 0) byte |= 0x80;
    out.push(byte);
  } while (v > 0);
  return Uint8Array.from(out);
}

/** Little-endian cursor with bounds checks; `finish()` refuses trailing bytes. */
export class Reader {
  o = 0;
  private dv: DataView;
  constructor(public b: Uint8Array) {
    this.dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
  }
  take(n: number): Uint8Array {
    if (this.o + n > this.b.length) throw new Error(`truncated: need ${n} bytes at ${this.o}`);
    const s = this.b.subarray(this.o, this.o + n);
    this.o += n;
    return s;
  }
  u8() {
    return this.take(1)[0];
  }
  u16() {
    const v = this.dv.getUint16(this.o, true);
    this.take(2);
    return v;
  }
  u32() {
    if (this.o + 4 > this.b.length) throw new Error("truncated u32");
    const v = this.dv.getUint32(this.o, true);
    this.o += 4;
    return v;
  }
  u64() {
    if (this.o + 8 > this.b.length) throw new Error("truncated u64");
    const v = this.dv.getBigUint64(this.o, true);
    this.o += 8;
    return v;
  }
  remaining() {
    return this.b.length - this.o;
  }
  finish() {
    if (this.o !== this.b.length) throw new Error(`${this.b.length - this.o} trailing bytes`);
  }
}
