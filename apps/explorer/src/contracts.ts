// Contract codec for the explorer and cadre-api (docs/CONTRACTS.md; layouts in
// chain/agent-types/src/contracts.rs): call args ABI, event topics, contract tx bodies
// (kinds 30-34), the unsigned Call tx and its fee floor. Checked byte for byte against
// chain/agent-types/vectors/contracts.json (test/contracts.test.ts). No dependencies.

import { blake3 } from "./blake3.ts";
import { b58decode, b58encode, hex } from "./codec.ts";

export const CONTRACT_KIND = { DeployCode: 30, Instantiate: 31, Call: 32, Migrate: 33, SetContractAdmin: 34 } as const;
export type ContractKind = keyof typeof CONTRACT_KIND;
export const CONTRACT_KIND_NAMES = Object.keys(CONTRACT_KIND) as ContractKind[];
export const isContractKind = (k: unknown): k is ContractKind => typeof k === "string" && k in CONTRACT_KIND;

/** version 1 + chain_id 4 + from 32 + nonce 8 + fee 8 + kind 1 */
export const TX_HDR = 54;
export const TX_SIG = 64;
export const TX_VERSION = 1;
export const NS_TX = "pearl-sol:tx:v1";
/** Codec caps (agent-types contracts.rs). */
export const MAX_ARGS = 8192;
export const MAX_LABEL = 64;
/** GAS_PRICE_UNIT when getParams does not report it. */
export const GAS_PRICE_UNIT = 1_000_000n;

const te = new TextEncoder();
const ZERO_KEY = b58encode(new Uint8Array(32));

// ------------------------------------------------------------ byte writer / reader

class Wr {
  b: number[] = [];
  u8(v: number) {
    this.b.push(v & 0xff);
    return this;
  }
  u16(v: number) {
    this.b.push(v & 0xff, (v >> 8) & 0xff);
    return this;
  }
  u32(v: number) {
    for (let i = 0; i < 4; i++) this.b.push((v >>> (8 * i)) & 0xff);
    return this;
  }
  u64(v: bigint | number | string) {
    let n = BigInt(v);
    if (n < 0n || n >= 1n << 64n) throw new Error(`u64 out of range: ${v}`);
    for (let i = 0; i < 8; i++) {
      this.b.push(Number(n & 0xffn));
      n >>= 8n;
    }
    return this;
  }
  bytes(x: Uint8Array) {
    for (const c of x) this.b.push(c);
    return this;
  }
  out() {
    return Uint8Array.from(this.b);
  }
}

class Rd {
  o = 0;
  d: Uint8Array;
  constructor(d: Uint8Array, o = 0) {
    this.d = d;
    this.o = o;
  }
  take(n: number) {
    if (this.o + n > this.d.length) throw new Error("input too short");
    const s = this.d.subarray(this.o, this.o + n);
    this.o += n;
    return s;
  }
  u8() {
    return this.take(1)[0];
  }
  u16() {
    const s = this.take(2);
    return s[0] | (s[1] << 8);
  }
  u32() {
    const s = this.take(4);
    return (s[0] | (s[1] << 8) | (s[2] << 16)) + s[3] * 0x1000000;
  }
  u64() {
    const s = this.take(8);
    let n = 0n;
    for (let i = 7; i >= 0; i--) n = (n << 8n) | BigInt(s[i]);
    return n;
  }
  key() {
    return b58encode(this.take(32));
  }
  hash() {
    return hex(this.take(32));
  }
  end() {
    if (this.o !== this.d.length) throw new Error(`${this.d.length - this.o} trailing bytes`);
  }
}

export const keyBytes = (k: string) => {
  const b = b58decode(k);
  if (b.length !== 32) throw new Error(`not a 32-byte base58 key: ${k}`);
  return b;
};
const hexBytes = (s: string, name = "hex") => {
  const t = s.trim().replace(/^0x/, "");
  if (t.length % 2 || !/^[0-9a-fA-F]*$/.test(t)) throw new Error(`${name} is not hex`);
  return Uint8Array.from(Buffer.from(t, "hex"));
};

// ------------------------------------------------------------ event topics

/** `blake3("pearl-sol:event:v1" || name)` (the SDK's `event!`), hex. */
export const eventTopic = (name: string) => hex(blake3(te.encode("pearl-sol:event:v1"), te.encode(name)));

/** Event names the explorer decodes from topic 0 (the example contracts and common ones). */
export const KNOWN_EVENTS = ["Incremented", "Reset", "Opened", "Released", "Refunded", "Transfer", "Approval", "Posted", "Submitted", "Awarded", "JobLinked"];
let topicMap: Map<string, string> | null = null;
/** Name of a known event whose topic equals `topic` (hex), else null. */
export function topicName(topic: string | null | undefined): string | null {
  if (!topic) return null;
  topicMap ??= new Map(KNOWN_EVENTS.map((n) => [eventTopic(n), n]));
  return topicMap.get(topic.toLowerCase()) ?? null;
}

/** Contract address: `blake3("pearl-sol:contract:v1" || creator || nonce u64 LE)`. */
export const contractAddress = (creator: string, nonce: bigint | number | string) =>
  b58encode(blake3(te.encode("pearl-sol:contract:v1"), keyBytes(creator), new Wr().u64(nonce).out()));

// ------------------------------------------------------------ call args ABI

export const FIELD_TYPES = ["u8", "u16", "u32", "u64", "bool", "addr", "hash", "bytes", "str"] as const;
export type FieldType = (typeof FIELD_TYPES)[number];
/** One field: `{u64: "5"}` (the vectors' form) or `"u64:5"` (the CLI's form). bytes are hex. */
export type Field = Partial<Record<FieldType, string | number | boolean>> | string;

function parseField(f: Field, i: number): [FieldType, string | number | boolean] {
  if (typeof f === "string") {
    const c = f.indexOf(":");
    if (c < 0) throw new Error(`fields[${i}]: "type:value", type one of ${FIELD_TYPES.join(", ")}`);
    const t = f.slice(0, c) as FieldType;
    if (!FIELD_TYPES.includes(t)) throw new Error(`fields[${i}]: unknown type ${t}`);
    return [t, f.slice(c + 1)];
  }
  if (!f || typeof f !== "object") throw new Error(`fields[${i}] is an object like {"u64": "5"} or a string like "u64:5"`);
  const ks = Object.keys(f);
  if (ks.length !== 1 || !FIELD_TYPES.includes(ks[0] as FieldType)) throw new Error(`fields[${i}]: one key, one of ${FIELD_TYPES.join(", ")}`);
  return [ks[0] as FieldType, (f as any)[ks[0]]];
}

function writeField(w: Wr, t: FieldType, v: string | number | boolean, i: number) {
  const int = (bits: number) => {
    const s = String(v).trim();
    if (!/^\d+$/.test(s)) throw new Error(`fields[${i}]: ${t} is a non-negative integer`);
    const n = BigInt(s);
    if (n >= 1n << BigInt(bits)) throw new Error(`fields[${i}]: ${s} does not fit ${t}`);
    return n;
  };
  switch (t) {
    case "u8": return w.u8(Number(int(8)));
    case "u16": return w.u16(Number(int(16)));
    case "u32": return w.u32(Number(int(32)));
    case "u64": return w.u64(int(64));
    case "bool": {
      const b = v === true || v === "true" || v === 1 || v === "1" ? 1 : v === false || v === "false" || v === 0 || v === "0" ? 0 : -1;
      if (b < 0) throw new Error(`fields[${i}]: bool is true or false`);
      return w.u8(b);
    }
    case "addr": return w.bytes(keyBytes(String(v)));
    case "hash": {
      const h = hexBytes(String(v), `fields[${i}] hash`);
      if (h.length !== 32) throw new Error(`fields[${i}]: hash is 32 bytes (64 hex)`);
      return w.bytes(h);
    }
    case "bytes": {
      const b = hexBytes(String(v), `fields[${i}] bytes`);
      return w.u32(b.length).bytes(b);
    }
    case "str": {
      const b = te.encode(String(v));
      return w.u32(b.length).bytes(b);
    }
  }
}

/** Fields only (return data and Instantiate args have no method name). */
export function encodeFields(fields: Field[]): Uint8Array {
  const w = new Wr();
  fields.forEach((f, i) => {
    const [t, v] = parseField(f, i);
    writeField(w, t, v, i);
  });
  return w.out();
}

/** `method (u8 len + UTF-8) | fields`. */
export function encodeCallArgs(method: string, fields: Field[] = []): Uint8Array {
  const m = te.encode(method);
  if (m.length < 1 || m.length > 255) throw new Error("method is 1..255 bytes of UTF-8");
  const out = Uint8Array.from([m.length, ...m, ...encodeFields(fields)]);
  if (out.length > MAX_ARGS) throw new Error(`args are ${out.length} bytes, at most ${MAX_ARGS}`);
  return out;
}

/** The method name at the head of call args, and the field bytes after it; null when the args do not start with one. */
export function splitMethod(args: Uint8Array | string): { method: string; rest: string } | null {
  const a = typeof args === "string" ? hexBytes(args) : args;
  if (a.length < 2) return null;
  const n = a[0];
  if (n < 1 || 1 + n > a.length) return null;
  const s = a.subarray(1, 1 + n);
  if (![...s].every((c) => c >= 0x20 && c < 0x7f)) return null;
  return { method: Buffer.from(s).toString("utf8"), rest: hex(a.subarray(1 + n)) };
}

/** Decode `data` as fields of the given types (return data has no ABI on chain; the caller names the types). */
export function decodeFields(data: Uint8Array | string, types: FieldType[]): (string | boolean)[] {
  const r = new Rd(typeof data === "string" ? hexBytes(data) : data);
  const out = types.map((t, i) => {
    if (!FIELD_TYPES.includes(t)) throw new Error(`returns[${i}]: unknown type ${t}`);
    switch (t) {
      case "u8": return String(r.u8());
      case "u16": return String(r.u16());
      case "u32": return String(r.u32());
      case "u64": return r.u64().toString();
      case "bool": return r.u8() !== 0;
      case "addr": return r.key();
      case "hash": return r.hash();
      case "bytes": return hex(r.take(r.u32()));
      case "str": return Buffer.from(r.take(r.u32())).toString("utf8");
    }
  });
  r.end();
  return out;
}

// ------------------------------------------------------------ tx bodies

export interface CallBody {
  contract: string;
  value: bigint | string;
  gasLimit: bigint | string;
  args: Uint8Array;
}

export function encodeCallBody(b: CallBody): Uint8Array {
  if (b.args.length > MAX_ARGS) throw new Error(`args are ${b.args.length} bytes, at most ${MAX_ARGS}`);
  return new Wr().bytes(keyBytes(b.contract)).u64(b.value).u64(b.gasLimit).u16(b.args.length).bytes(b.args).out();
}

/** Bytes a wallet signs (everything before the signature). */
export function contractSigningBytes(chainId: number, from: string, nonce: bigint | number | string, fee: bigint | string, kind: ContractKind, body: Uint8Array): Uint8Array {
  const w = new Wr().u8(TX_VERSION).u32(chainId).bytes(keyBytes(from)).u64(nonce).u64(fee).u8(CONTRACT_KIND[kind]);
  return Uint8Array.from([...w.out(), ...body]);
}

/** What ed25519 signs: `varint(len NS) || NS || signing_bytes` (NS shorter than 128). */
export function framedMessage(signing: Uint8Array): Uint8Array {
  const ns = te.encode(NS_TX);
  return Uint8Array.from([ns.length, ...ns, ...signing]);
}

/** `max(min_fee, contract_base_fee + contract_byte_fee x tx_len + ceil(gas_limit x gas_price / gas_price_unit))` from getParams. */
export function contractFeeFloor(params: any, txLen: number, gasLimit: bigint | string | number) {
  const need = (k: string) => {
    if (params?.[k] === undefined || params?.[k] === null) throw new Error(`getParams has no ${k} (the node predates the contracts upgrade)`);
    return BigInt(params[k]);
  };
  const min = need("min_fee");
  const base = need("contract_base_fee");
  const per = need("contract_byte_fee");
  const price = need("gas_price");
  const unit = params.gas_price_unit !== undefined ? BigInt(params.gas_price_unit) : GAS_PRICE_UNIT;
  const g = BigInt(gasLimit);
  const gasFee = (g * price + unit - 1n) / unit;
  const sum = base + per * BigInt(txLen) + gasFee;
  return { fee: sum > min ? sum : min, base, byte_fee: per * BigInt(txLen), gas_fee: gasFee, min_fee: min, tx_len: txLen };
}

export interface DecodedContractTx {
  kind: ContractKind;
  chain_id: number;
  from: string;
  nonce: string;
  fee: string;
  body: Record<string, unknown>;
}

/** Decode a contract tx (kinds 30-34) from its encoded bytes; null for any other kind. */
export function decodeContractTx(tx: Uint8Array | string): DecodedContractTx | null {
  const d = typeof tx === "string" ? Uint8Array.from(Buffer.from(tx, "base64")) : tx;
  if (d.length < TX_HDR + TX_SIG) return null;
  const kindByte = d[TX_HDR - 1];
  const kind = CONTRACT_KIND_NAMES.find((k) => CONTRACT_KIND[k] === kindByte);
  if (!kind) return null;
  const h = new Rd(d);
  h.u8();
  const chain_id = h.u32();
  const from = h.key();
  const nonce = h.u64().toString();
  const fee = h.u64().toString();
  h.u8();
  const r = new Rd(d.subarray(0, d.length - TX_SIG), TX_HDR);
  let body: Record<string, unknown>;
  const args16 = () => hex(r.take(r.u16()));
  switch (kind) {
    case "DeployCode": {
      const gas_limit = r.u64().toString();
      const code = r.take(r.u32());
      body = { gas_limit, code_size: code.length, code_hash: hex(blake3(code)) };
      break;
    }
    case "Instantiate": {
      const code_hash = r.hash();
      const admin = r.key();
      const value = r.u64().toString();
      const gas_limit = r.u64().toString();
      const label = Buffer.from(r.take(r.u8())).toString("utf8");
      body = { code_hash, admin: admin === ZERO_KEY ? null : admin, value, gas_limit, label, args: args16(), contract: contractAddress(from, nonce) };
      break;
    }
    case "Call": {
      body = { contract: r.key(), value: r.u64().toString(), gas_limit: r.u64().toString(), args: args16() };
      break;
    }
    case "Migrate": {
      body = { contract: r.key(), code_hash: r.hash(), gas_limit: r.u64().toString(), args: args16() };
      break;
    }
    case "SetContractAdmin": {
      const contract = r.key();
      const admin = r.key();
      body = { contract, admin: admin === ZERO_KEY ? null : admin };
      break;
    }
  }
  r.end();
  if (typeof body.args === "string") body.method = splitMethod(body.args as string)?.method ?? null;
  return { kind, chain_id, from, nonce, fee, body };
}

/** Events with the decoded name of topic 0 when it is a known event. */
export function withTopicNames<T extends { topics?: string[] }>(events: T[] | null | undefined): (T & { name: string | null })[] {
  return (events ?? []).map((e) => ({ ...e, name: topicName(e.topics?.[0]) }));
}

/** A node error that says the method does not exist: the contracts upgrade is not on that node. */
export const isUnknownMethod = (e: unknown) => /unknown method|method not found|no such method|-32601/i.test((e as Error)?.message ?? String(e));
