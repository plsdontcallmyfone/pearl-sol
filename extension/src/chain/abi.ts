// Contract call ABI (crates/cadre-contract-sdk/src/abi.rs): input = method (u8 len, UTF-8) |
// field*, little-endian fields, bytes/str with a u32 length. No crypto imports, so the page
// script (window.pearl.contracts) can bundle it alone.

import { b58, concat, fromHex, hex, key32, u16, u32, u64, u8, utf8 } from "../core/bytes";

/** Call / Instantiate / Migrate args cap (chain/agent-types/src/contracts.rs MAX_ARGS). */
export const MAX_ARGS = 8192;


/**
 * One ABI field (crates/cadre-contract-sdk/src/abi.rs): integers as numbers or decimal
 * strings, addr base58 (or 64 hex), hash and bytes hex, str text. Encoded little-endian;
 * bytes and str carry a u32 length.
 */
export type AbiField =
  | { u8: number | string }
  | { u16: number | string }
  | { u32: number | string }
  | { u64: number | string | bigint }
  | { bool: boolean }
  | { addr: string }
  | { hash: string }
  | { bytes: string }
  | { str: string };

export const uint = (v: unknown, bits: number, what: string): bigint => {
  const s = typeof v === "bigint" ? v.toString() : String(v ?? "");
  if (!/^\d+$/.test(s)) throw new Error(`${what} must be a non-negative integer`);
  const n = BigInt(s);
  if (n >= 1n << BigInt(bits)) throw new Error(`${what} does not fit in u${bits}`);
  return n;
};
export const hexArg = (s: unknown, what: string): Uint8Array => {
  try {
    return fromHex(String(s ?? "").trim());
  } catch {
    throw new Error(`${what} must be hex`);
  }
};

export function encodeAbiField(f: AbiField): Uint8Array {
  const [[t, v]] = Object.entries(f) as [string, unknown][];
  switch (t) {
    case "u8": return u8(Number(uint(v, 8, "u8")));
    case "u16": return u16(Number(uint(v, 16, "u16")));
    case "u32": return u32(Number(uint(v, 32, "u32")));
    case "u64": return u64(uint(v, 64, "u64"));
    case "bool":
      if (typeof v !== "boolean") throw new Error("bool must be true or false");
      return u8(v ? 1 : 0);
    case "addr":
      try {
        return key32(String(v));
      } catch {
        throw new Error("addr must be a base58 account");
      }
    case "hash": {
      const h = hexArg(v, "hash");
      if (h.length !== 32) throw new Error("hash must be 64 hex");
      return h;
    }
    case "bytes": {
      const b = hexArg(v, "bytes");
      return concat(u32(b.length), b);
    }
    case "str": {
      const b = utf8(String(v));
      return concat(u32(b.length), b);
    }
  }
  throw new Error(`unknown ABI field type ${t}`);
}

/** Fields only: the input of `instantiate` and `migrate`, and the tail of a call input. */
export const encodeAbiFields = (fields: AbiField[] = []) => concat(...fields.map(encodeAbiField));

/** Call input: method (u8 len, UTF-8) then fields. */
export function encodeCallArgs(method: string, fields: AbiField[] = []): Uint8Array {
  const m = utf8(method);
  if (m.length < 1 || m.length > 0xff) throw new Error("method must be 1..255 bytes");
  const out = concat(u8(m.length), m, encodeAbiFields(fields));
  if (out.length > MAX_ARGS) throw new Error(`args over ${MAX_ARGS} bytes`);
  return out;
}

const PRINTABLE = /^[\p{L}\p{N}\p{P}\p{S} ]+$/u;
export const strictUtf8 = (b: Uint8Array): string | null => {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(b);
  } catch {
    return null;
  }
};

/** Method name and field bytes when `args` reads as the call ABI with a plain-text method; null otherwise. */
export function parseCallArgs(args: Uint8Array): { method: string; fields: Uint8Array } | null {
  if (args.length < 1) return null;
  const n = args[0];
  if (n < 1 || 1 + n > args.length) return null;
  const method = strictUtf8(args.subarray(1, 1 + n));
  if (method === null || !PRINTABLE.test(method)) return null;
  return { method, fields: args.subarray(1 + n) };
}

export const int = (v: bigint) => v.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
export const shortHex = (b: Uint8Array, n = 8) => {
  const h = hex(b);
  return h.length > 2 * n ? `${h.slice(0, n)}...` : h;
};
export const shortKey = (b: Uint8Array) => {
  const s = b58(b);
  return s.length > 11 ? `${s.slice(0, 4)}...${s.slice(-3)}` : s;
};

/**
 * Plain words for ABI fields. The encoding carries no types, so a value is read by its length
 * (8 bytes as a u64, 32 as an address, u32 length + UTF-8 text as a str, ...) and labelled
 * with that reading; anything else is shown as hex.
 */
export function describeFields(f: Uint8Array): { text: string; reading: string | null } {
  const dv = new DataView(f.buffer, f.byteOffset, f.byteLength);
  switch (f.length) {
    case 0: return { text: "no arguments", reading: null };
    case 1: return { text: `${f[0]} (u8)`, reading: "u8" };
    case 2: return { text: `${int(BigInt(dv.getUint16(0, true)))} (u16)`, reading: "u16" };
    case 4: return { text: `${int(BigInt(dv.getUint32(0, true)))} (u32)`, reading: "u32" };
    case 8: return { text: `${int(dv.getBigUint64(0, true))} (u64)`, reading: "u64" };
    case 32: return { text: `${shortKey(f)} (address)`, reading: "addr" };
  }
  if (f.length >= 4 && dv.getUint32(0, true) === f.length - 4) {
    const s = strictUtf8(f.subarray(4));
    if (s !== null && s.length > 0 && PRINTABLE.test(s)) return { text: `"${s.length > 48 ? `${s.slice(0, 48)}...` : s}" (str)`, reading: "str" };
  }
  return { text: `${f.length} bytes ${shortHex(f, 16)}`, reading: null };
}
