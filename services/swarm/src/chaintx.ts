// Agent-chain tx encoding for the kinds the swarm sends (SPEC 2 and 4.6):
// Transfer (0), SetProfile (14), OperatorLink (15), Post (22), React (23), Follow (24).
// Byte-checked against chain/agent-types/vectors/social.json and tx.json (test/vectors.test.ts).
//
// Tx  = version u8 (=1) | chain_id u32 | from [32] | nonce u64 | fee u64 | kind u8 | body | sig [64]
// sig = ed25519 over varint(len NS) || NS || signing bytes, NS = "pearl-sol:tx:v1"
import type { Wallet } from "./wallet.ts";

export const NS_TX = Buffer.from("pearl-sol:tx:v1");
export const KIND = { Transfer: 0, SetProfile: 14, OperatorLink: 15, Post: 22, React: 23, Follow: 24 } as const;
export const SOCIAL_KINDS: ReadonlySet<number> = new Set([14, 15, 22, 23, 24]);
export const REACTION = { like: 0, laugh: 1, fire: 2, doubt: 3, tip: 4 } as const;
export const FLAG = { agent: 1, house: 2, hires: 4, trades: 8 } as const;
const Z32 = new Uint8Array(32);

export interface SetProfileBody {
  handle: string;
  name: string;
  profileUri: string;
  profileHash: Uint8Array;
  flags: number;
  operator: Uint8Array;
  encKey?: Uint8Array;
  dmPrice?: bigint;
  dmPolicy?: number;
  threadFreeBlocks?: number;
}
export interface PostBody {
  postKind: number; // 0 post, 1 reply, 2 repost, 3 quote
  parent?: Uint8Array;
  channel?: Uint8Array;
  body: string;
  blobUri?: string;
  blobHash?: Uint8Array;
  thoughtRef?: string;
}
export interface ReactItem { postId: Uint8Array; reaction: number; tip: bigint }
export interface FollowItem { target: Uint8Array; on: boolean }

export type Body =
  | { kind: "Transfer"; to: Uint8Array; amount: bigint }
  | ({ kind: "SetProfile" } & SetProfileBody)
  | { kind: "OperatorLink"; agent: Uint8Array; add: boolean }
  | ({ kind: "Post" } & PostBody)
  | { kind: "React"; items: ReactItem[] }
  | { kind: "Follow"; items: FollowItem[] };

class W {
  parts: number[] = [];
  u8(v: number) { this.parts.push(v & 0xff); return this; }
  u16(v: number) { this.parts.push(v & 0xff, (v >> 8) & 0xff); return this; }
  u32(v: number) { const b = Buffer.alloc(4); b.writeUInt32LE(v >>> 0); this.parts.push(...b); return this; }
  u64(v: bigint) { const b = Buffer.alloc(8); b.writeBigUInt64LE(v); this.parts.push(...b); return this; }
  raw(v: Uint8Array, len?: number) {
    if (len !== undefined && v.length !== len) throw new Error(`expected ${len} bytes, got ${v.length}`);
    this.parts.push(...v);
    return this;
  }
  b8(v: Uint8Array) { if (v.length > 255) throw new Error("field over 255 bytes"); return this.u8(v.length).raw(v); }
  b16(v: Uint8Array) { if (v.length > 65535) throw new Error("field over 65535 bytes"); return this.u16(v.length).raw(v); }
  out() { return Uint8Array.from(this.parts); }
}
const utf8 = (s: string) => new Uint8Array(Buffer.from(s, "utf8"));

export function encodeBody(b: Body): { kind: number; bytes: Uint8Array } {
  const w = new W();
  switch (b.kind) {
    case "Transfer":
      w.raw(b.to, 32).u64(b.amount);
      break;
    case "SetProfile":
      w.b8(utf8(b.handle)).b8(utf8(b.name)).b16(utf8(b.profileUri)).raw(b.profileHash, 32).u16(b.flags)
        .raw(b.operator, 32).raw(b.encKey ?? Z32, 32).u64(b.dmPrice ?? 0n).u8(b.dmPolicy ?? 0).u32(b.threadFreeBlocks ?? 0);
      break;
    case "OperatorLink":
      w.raw(b.agent, 32).u8(b.add ? 1 : 0);
      break;
    case "Post":
      w.u8(b.postKind).raw(b.parent ?? Z32, 32).raw(b.channel ?? Z32, 32).b16(utf8(b.body)).b16(utf8(b.blobUri ?? ""))
        .raw(b.blobHash ?? Z32, 32).b8(utf8(b.thoughtRef ?? ""));
      break;
    case "React":
      if (b.items.length < 1 || b.items.length > 64) throw new Error("React carries 1..=64 items");
      w.u8(b.items.length);
      for (const x of b.items) w.raw(x.postId, 32).u8(x.reaction).u64(x.tip);
      break;
    case "Follow":
      if (b.items.length < 1 || b.items.length > 64) throw new Error("Follow carries 1..=64 items");
      w.u8(b.items.length);
      for (const x of b.items) w.raw(x.target, 32).u8(x.on ? 1 : 0);
      break;
  }
  return { kind: KIND[b.kind], bytes: w.out() };
}

export function frame(ns: Uint8Array, msg: Uint8Array): Uint8Array {
  const out: number[] = [];
  let n = ns.length;
  for (;;) {
    const byte = n & 0x7f;
    n >>= 7;
    if (n === 0) { out.push(byte); break; }
    out.push(byte | 0x80);
  }
  return Uint8Array.from([...out, ...ns, ...msg]);
}

export function signingBytes(chainId: number, from: Uint8Array, nonce: bigint, fee: bigint, body: Body): Uint8Array {
  const { kind, bytes } = encodeBody(body);
  return new W().u8(1).u32(chainId).raw(from, 32).u64(nonce).u64(fee).u8(kind).raw(bytes).out();
}

export function signTx(w: Wallet, chainId: number, nonce: bigint, fee: bigint, body: Body): Uint8Array {
  const sb = signingBytes(chainId, w.publicKey, nonce, fee, body);
  const sig = w.sign(frame(NS_TX, sb));
  return Uint8Array.from([...sb, ...sig]);
}

/** Encoded length of a tx with this body (the signature is 64 bytes whatever it is). */
export function txLen(body: Body): number {
  return 54 + encodeBody(body).bytes.length + 64;
}

/** Fee rule of SPEC 4.6: max(min_fee, social_min_fee + social_byte_fee x len) for social kinds. */
export function feeFor(body: Body, params: { min_fee: string; social_min_fee?: string; social_byte_fee?: string }): bigint {
  const min = BigInt(params.min_fee);
  const k = KIND[body.kind];
  if (!SOCIAL_KINDS.has(k)) return min;
  const s = BigInt(params.social_min_fee ?? params.min_fee) + BigInt(params.social_byte_fee ?? "0") * BigInt(txLen(body));
  return s > min ? s : min;
}

// ------------------------------------------------------------------ decode (fakes, tests)

export interface Decoded {
  chainId: number;
  from: Uint8Array;
  nonce: bigint;
  fee: bigint;
  kind: number;
  body: Body;
  signing: Uint8Array;
  sig: Uint8Array;
}

export function decodeTx(t: Uint8Array): Decoded {
  const b = Buffer.from(t);
  let p = 0;
  const take = (n: number) => { if (p + n > b.length) throw new Error("truncated"); const s = new Uint8Array(b.subarray(p, p + n)); p += n; return s; };
  const u8 = () => take(1)[0];
  const u16 = () => { const v = b.readUInt16LE(p); p += 2; return v; };
  const u32 = () => { const v = b.readUInt32LE(p); p += 4; return v; };
  const u64 = () => { const v = b.readBigUInt64LE(p); p += 8; return v; };
  const s8 = () => Buffer.from(take(u8())).toString("utf8");
  const s16 = () => Buffer.from(take(u16())).toString("utf8");
  if (u8() !== 1) throw new Error("version");
  const chainId = u32();
  const from = take(32);
  const nonce = u64();
  const fee = u64();
  const kind = u8();
  let body: Body;
  switch (kind) {
    case 0: body = { kind: "Transfer", to: take(32), amount: u64() }; break;
    case 14: body = { kind: "SetProfile", handle: s8(), name: s8(), profileUri: s16(), profileHash: take(32), flags: u16(), operator: take(32), encKey: take(32), dmPrice: u64(), dmPolicy: u8(), threadFreeBlocks: u32() }; break;
    case 15: body = { kind: "OperatorLink", agent: take(32), add: u8() === 1 }; break;
    case 22: body = { kind: "Post", postKind: u8(), parent: take(32), channel: take(32), body: s16(), blobUri: s16(), blobHash: take(32), thoughtRef: s8() }; break;
    case 23: { const n = u8(); const items: ReactItem[] = []; for (let i = 0; i < n; i++) items.push({ postId: take(32), reaction: u8(), tip: u64() }); body = { kind: "React", items }; break; }
    case 24: { const n = u8(); const items: FollowItem[] = []; for (let i = 0; i < n; i++) items.push({ target: take(32), on: u8() === 1 }); body = { kind: "Follow", items }; break; }
    default: throw new Error(`kind ${kind} not decoded here`);
  }
  const signing = new Uint8Array(b.subarray(0, p));
  const sig = take(64);
  if (p !== b.length) throw new Error("trailing bytes");
  return { chainId, from, nonce, fee, kind, body, signing, sig };
}
