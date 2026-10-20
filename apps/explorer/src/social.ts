// Social layer (SPEC 4.6): codec of the five social tx kinds and the town index built from
// the indexer's canonical blocks (posts, replies, reactions and tips, follows, the latest
// profile tx per account). Chain state (getProfile) stays the authority for profiles and
// counters; this index serves feeds, threads and per-post counts, which the chain does not
// keep. Dependency free.

import { b58decode, b58encode, hex, unhex } from "./codec.ts";
import { blake3 } from "./blake3.ts";
import type { BlockRec } from "./indexer.ts";

export const SOCIAL_KIND = { SetProfile: 14, OperatorLink: 15, Post: 22, React: 23, Follow: 24 } as const;
export type SocialKindName = keyof typeof SOCIAL_KIND;
export const SOCIAL_KIND_NAME: Record<number, SocialKindName> = { 14: "SetProfile", 15: "OperatorLink", 22: "Post", 23: "React", 24: "Follow" };
export const POST_KINDS = ["post", "reply", "repost", "quote"] as const;
export const REACTIONS = ["like", "laugh", "fire", "doubt", "tip"] as const;
export const FLAG = { agent: 1, house: 2, hires: 4, trades: 8 } as const;
export const TX_PREFIX = 54; // version 1 + chain_id 4 + from 32 + nonce 8 + fee 8 + kind 1
export const SIG_LEN = 64;
const ZERO = "0".repeat(64);
const ZERO_KEY = b58encode(new Uint8Array(32));

export type SocialBody =
  | { kind: "SetProfile"; handle: string; name: string; profileUri: string; profileHash: string; flags: number; operator: string; encKey: string; dmPrice: string; dmPolicy: number; threadFreeBlocks: number }
  | { kind: "OperatorLink"; agent: string; add: boolean }
  | { kind: "Post"; postKind: number; parent: string; channel: string; body: string; blobUri: string; blobHash: string; thoughtRef: string }
  | { kind: "React"; items: { postId: string; reaction: number; tip: string }[] }
  | { kind: "Follow"; items: { target: string; on: boolean }[] };

// ------------------------------------------------------------ codec

class Wr {
  b: number[] = [];
  u8(v: number) {
    this.b.push(v & 0xff);
    return this;
  }
  u16(v: number) {
    return this.u8(v).u8(v >> 8);
  }
  u32(v: number) {
    for (let i = 0; i < 4; i++) this.u8(v >>> (8 * i));
    return this;
  }
  u64(v: bigint | string | number) {
    let x = BigInt(v);
    if (x < 0n || x >= 1n << 64n) throw new Error("u64 out of range");
    for (let i = 0; i < 8; i++) {
      this.u8(Number(x & 0xffn));
      x >>= 8n;
    }
    return this;
  }
  bytes(v: Uint8Array) {
    for (const x of v) this.b.push(x);
    return this;
  }
  b8(v: Uint8Array) {
    if (v.length > 255) throw new Error("field above 255 bytes");
    return this.u8(v.length).bytes(v);
  }
  b16(v: Uint8Array) {
    if (v.length > 65535) throw new Error("field above 65535 bytes");
    return this.u16(v.length).bytes(v);
  }
  out() {
    return Uint8Array.from(this.b);
  }
}

class Rd {
  p = 0;
  d: Uint8Array;
  constructor(d: Uint8Array) {
    this.d = d;
  }
  take(n: number) {
    if (this.p + n > this.d.length) throw new Error(`truncated at byte ${this.p}`);
    const s = this.d.subarray(this.p, this.p + n);
    this.p += n;
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
    return (s[0] | (s[1] << 8) | (s[2] << 16) | (s[3] << 24)) >>> 0;
  }
  u64() {
    const s = this.take(8);
    let x = 0n;
    for (let i = 7; i >= 0; i--) x = (x << 8n) | BigInt(s[i]);
    return x;
  }
  b8(max: number, what: string) {
    const n = this.u8();
    if (n > max) throw new Error(`${what} above ${max} bytes`);
    return this.take(n);
  }
  b16(max: number, what: string) {
    const n = this.u16();
    if (n > max) throw new Error(`${what} above ${max} bytes`);
    return this.take(n);
  }
}

const te = new TextEncoder();
const td = new TextDecoder("utf-8", { fatal: false });
const keyBytes = (k: string) => {
  const b = b58decode(k);
  if (b.length !== 32) throw new Error(`${k} is not a 32-byte key`);
  return b;
};
const hashBytes = (h: string) => {
  if (!/^[0-9a-f]{64}$/i.test(h)) throw new Error(`${h} is not 64 hex`);
  return unhex(h.toLowerCase());
};

export function encodeSocialBody(b: SocialBody): Uint8Array {
  const w = new Wr();
  switch (b.kind) {
    case "SetProfile":
      w.b8(te.encode(b.handle)).b8(te.encode(b.name)).b16(te.encode(b.profileUri)).bytes(hashBytes(b.profileHash)).u16(b.flags)
        .bytes(keyBytes(b.operator)).bytes(keyBytes(b.encKey)).u64(b.dmPrice).u8(b.dmPolicy).u32(b.threadFreeBlocks);
      break;
    case "OperatorLink":
      w.bytes(keyBytes(b.agent)).u8(b.add ? 1 : 0);
      break;
    case "Post":
      w.u8(b.postKind).bytes(hashBytes(b.parent)).bytes(hashBytes(b.channel)).b16(te.encode(b.body)).b16(te.encode(b.blobUri)).bytes(hashBytes(b.blobHash)).b8(te.encode(b.thoughtRef));
      break;
    case "React":
      if (b.items.length < 1 || b.items.length > 64) throw new Error("React carries 1..=64 items");
      w.u8(b.items.length);
      for (const x of b.items) w.bytes(hashBytes(x.postId)).u8(x.reaction).u64(x.tip);
      break;
    case "Follow":
      if (b.items.length < 1 || b.items.length > 64) throw new Error("Follow carries 1..=64 items");
      w.u8(b.items.length);
      for (const x of b.items) w.bytes(keyBytes(x.target)).u8(x.on ? 1 : 0);
      break;
  }
  return w.out();
}

export function decodeSocialBody(kind: number, body: Uint8Array): SocialBody {
  const r = new Rd(body);
  let out: SocialBody;
  switch (kind) {
    case 14:
      out = {
        kind: "SetProfile", handle: td.decode(r.b8(24, "handle")), name: td.decode(r.b8(64, "name")), profileUri: td.decode(r.b16(512, "profile_uri")),
        profileHash: hex(r.take(32)), flags: r.u16(), operator: b58encode(r.take(32)), encKey: b58encode(r.take(32)), dmPrice: r.u64().toString(),
        dmPolicy: r.u8(), threadFreeBlocks: r.u32(),
      };
      break;
    case 15: {
      const agent = b58encode(r.take(32));
      const f = r.u8();
      if (f > 1) throw new Error("operator link add flag");
      out = { kind: "OperatorLink", agent, add: f === 1 };
      break;
    }
    case 22:
      out = {
        kind: "Post", postKind: r.u8(), parent: hex(r.take(32)), channel: hex(r.take(32)), body: td.decode(r.b16(8192, "post body")),
        blobUri: td.decode(r.b16(512, "blob_uri")), blobHash: hex(r.take(32)), thoughtRef: td.decode(r.b8(64, "thought_ref")),
      };
      break;
    case 23: {
      const n = r.u8();
      if (n < 1 || n > 64) throw new Error("reactions count");
      const items = Array.from({ length: n }, () => ({ postId: hex(r.take(32)), reaction: r.u8(), tip: r.u64().toString() }));
      out = { kind: "React", items };
      break;
    }
    case 24: {
      const n = r.u8();
      if (n < 1 || n > 64) throw new Error("follows count");
      const items = Array.from({ length: n }, () => {
        const target = b58encode(r.take(32));
        const f = r.u8();
        if (f > 1) throw new Error("follow on flag");
        return { target, on: f === 1 };
      });
      out = { kind: "Follow", items };
      break;
    }
    default:
      throw new Error(`not a social kind: ${kind}`);
  }
  if (r.p !== body.length) throw new Error(`trailing bytes: ${body.length - r.p}`);
  return out;
}

export interface SocialTx {
  chainId: number;
  from: string;
  nonce: string;
  fee: string;
  kind: number;
  body: SocialBody;
  sig: string;
}

/** A whole encoded social tx (signature included); throws on anything else. */
export function decodeSocialTx(tx: Uint8Array): SocialTx {
  if (tx.length < TX_PREFIX + SIG_LEN) throw new Error("tx too short");
  if (tx[0] !== 1) throw new Error(`tx version ${tx[0]}`);
  const dv = Buffer.from(tx.buffer, tx.byteOffset, tx.byteLength);
  const kind = tx[53];
  return {
    chainId: dv.readUInt32LE(1),
    from: b58encode(tx.subarray(5, 37)),
    nonce: dv.readBigUInt64LE(37).toString(),
    fee: dv.readBigUInt64LE(45).toString(),
    kind,
    body: decodeSocialBody(kind, tx.subarray(TX_PREFIX, tx.length - SIG_LEN)),
    sig: hex(tx.subarray(tx.length - SIG_LEN)),
  };
}

/** Bytes a wallet signs (everything before the signature). */
export function socialSigningBytes(chainId: number, from: string, nonce: bigint | string | number, fee: bigint | string, body: SocialBody): Uint8Array {
  const w = new Wr();
  w.u8(1).u32(chainId).bytes(keyBytes(from)).u64(nonce).u64(fee).u8(SOCIAL_KIND[body.kind]);
  return Uint8Array.from([...w.out(), ...encodeSocialBody(body)]);
}

/** Encoded length of the signed tx (the social fee rule measures it). */
export const socialTxLen = (body: SocialBody) => TX_PREFIX + encodeSocialBody(body).length + SIG_LEN;

/** `max(min_fee, social_min_fee + social_byte_fee x len)` from getParams (defaults per SPEC 4.6 when absent). */
export function socialFee(params: any, body: SocialBody): bigint {
  const min = BigInt(params?.min_fee ?? 1000);
  const base = BigInt(params?.social_min_fee ?? 1000);
  const per = BigInt(params?.social_byte_fee ?? 0);
  // the fee field is fixed width, so the length does not depend on the fee itself
  const f = base + per * BigInt(socialTxLen(body));
  return f > min ? f : min;
}

/** `post_id = blake3("pearl-sol:post:v1" || from || nonce u64 LE)`. */
export function postId(from: string, nonce: bigint | string | number): string {
  const n = new Wr().u64(nonce).out();
  return hex(blake3(te.encode("pearl-sol:post:v1"), keyBytes(from), n));
}

/** Text avatar (no images): monogram and two hues from blake3(account). */
export function avatar(account: string, name: string | null, handle: string | null) {
  const src = (name && name.trim()) || handle || account;
  const words = src.split(/[\s_]+/).filter(Boolean);
  const monogram = (words.length >= 2 ? words[0][0] + words[1][0] : [...words[0] ?? src][0] ?? "?").toUpperCase();
  let h: Uint8Array;
  try {
    h = blake3(keyBytes(account));
  } catch {
    h = blake3(te.encode(account));
  }
  return { monogram: [...monogram].slice(0, 2).join(""), hue_a: ((h[0] << 8) | h[1]) % 360, hue_b: ((h[2] << 8) | h[3]) % 360 };
}

/** Indexer tx facts for a social kind: the decoded body and, for a Post, its id. */
export function socialFacts(kind: string, txB64: string): { social?: SocialBody; postId?: string; nonce?: string } {
  if (!(kind in SOCIAL_KIND)) return {};
  try {
    const t = decodeSocialTx(Uint8Array.from(Buffer.from(txB64, "base64")));
    return { social: t.body, nonce: t.nonce, ...(t.body.kind === "Post" ? { postId: postId(t.from, t.nonce) } : {}) };
  } catch {
    return {};
  }
}

// ------------------------------------------------------------ town index

export interface PostRec {
  postId: string;
  author: string;
  kind: number;
  parent: string | null;
  body: string;
  blobUri: string | null;
  thoughtRef: string | null;
  height: number;
  blockHash: string;
  txHash: string;
  index: number;
  time: number;
}

export interface ReactRec {
  account: string;
  reaction: number;
  tip: string;
  height: number;
  txHash: string;
}

export interface ProfileTx {
  account: string;
  handle: string | null;
  name: string;
  flags: number;
  operator: string | null;
  profileUri: string | null;
  height: number;
}

export class TownIndex {
  key = "";
  posts = new Map<string, PostRec>();
  /** newest first: (height desc, index desc) */
  order: PostRec[] = [];
  reactions = new Map<string, ReactRec[]>();
  replies = new Map<string, string[]>();
  byAuthor = new Map<string, PostRec[]>();
  /** follower -> target -> since height */
  follows = new Map<string, Map<string, number>>();
  followers = new Map<string, Map<string, number>>();
  profiles = new Map<string, ProfileTx>();
  tipsReceived = new Map<string, bigint>();
  firstSeen = new Map<string, number>();

  /** Rebuild from the indexer's canonical blocks when they changed. */
  update(blocks: Map<number, BlockRec>, tipHeight: number) {
    let top = "";
    const b = blocks.get(tipHeight);
    if (b) top = b.hash;
    const key = `${tipHeight}:${top}:${blocks.size}`;
    if (key === this.key) return this;
    this.reset();
    this.key = key;
    const hs = [...blocks.keys()].sort((x, y) => x - y);
    for (const h of hs) {
      const blk = blocks.get(h)!;
      blk.txs.forEach((t, i) => this.apply(blk, t as any, i));
    }
    this.order = [...this.posts.values()].sort((x, y) => y.height - x.height || y.index - x.index);
    for (const p of this.order) {
      const l = this.byAuthor.get(p.author) ?? [];
      l.push(p);
      this.byAuthor.set(p.author, l);
    }
    return this;
  }

  private reset() {
    this.posts.clear();
    this.order = [];
    this.reactions.clear();
    this.replies.clear();
    this.byAuthor.clear();
    this.follows.clear();
    this.followers.clear();
    this.profiles.clear();
    this.tipsReceived.clear();
    this.firstSeen.clear();
  }

  private apply(blk: BlockRec, t: { hash: string; kind: string; from: string; facts: any }, index: number) {
    const s: SocialBody | undefined = t.facts?.social;
    if (!s) return;
    if (!this.firstSeen.has(t.from)) this.firstSeen.set(t.from, blk.height);
    switch (s.kind) {
      case "Post": {
        const id = t.facts.postId as string;
        const parent = s.parent === ZERO ? null : s.parent;
        this.posts.set(id, {
          postId: id, author: t.from, kind: s.postKind, parent, body: s.body, blobUri: s.blobUri || null, thoughtRef: s.thoughtRef || null,
          height: blk.height, blockHash: blk.hash, txHash: t.hash, index, time: blk.time,
        });
        if (parent) {
          const l = this.replies.get(parent) ?? [];
          l.push(id);
          this.replies.set(parent, l);
        }
        break;
      }
      case "React":
        for (const x of s.items) {
          const l = this.reactions.get(x.postId) ?? [];
          l.push({ account: t.from, reaction: x.reaction, tip: x.tip, height: blk.height, txHash: t.hash });
          this.reactions.set(x.postId, l);
          const author = this.posts.get(x.postId)?.author;
          if (author && x.tip !== "0") this.tipsReceived.set(author, (this.tipsReceived.get(author) ?? 0n) + BigInt(x.tip));
        }
        break;
      case "Follow":
        for (const x of s.items) {
          const out = this.follows.get(t.from) ?? new Map();
          const inn = this.followers.get(x.target) ?? new Map();
          if (x.on) {
            out.set(x.target, blk.height);
            inn.set(t.from, blk.height);
          } else {
            out.delete(x.target);
            inn.delete(t.from);
          }
          this.follows.set(t.from, out);
          this.followers.set(x.target, inn);
        }
        break;
      case "SetProfile":
        this.profiles.set(t.from, {
          account: t.from, handle: s.handle || null, name: s.name, flags: s.flags, operator: s.operator === ZERO_KEY ? null : s.operator,
          profileUri: s.profileUri || null, height: blk.height,
        });
        break;
      case "OperatorLink":
        if (!s.add) {
          const p = this.profiles.get(s.agent);
          if (p && p.operator === t.from && p.flags & FLAG.house) p.flags &= ~FLAG.house;
        }
        break;
    }
  }

  reactionCounts(postId: string) {
    const out = { like: 0, laugh: 0, fire: 0, doubt: 0, tip_count: 0, tip_total: "0" };
    let tips = 0n;
    for (const r of this.reactions.get(postId) ?? []) {
      if (r.reaction < 4) out[REACTIONS[r.reaction] as "like"]++;
      if (r.tip !== "0") {
        out.tip_count++;
        tips += BigInt(r.tip);
      }
    }
    out.tip_total = tips.toString();
    return out;
  }

  /** ProfileCard from the latest SetProfile tx (chain-validated when it was included). */
  card(account: string) {
    const p = this.profiles.get(account);
    return {
      account,
      handle: p?.handle ?? null,
      name: p?.name ?? null,
      is_agent: !!(p && p.flags & FLAG.agent),
      house: !!(p && p.flags & FLAG.house),
      operator: p?.operator ?? null,
      avatar: avatar(account, p?.name ?? null, p?.handle ?? null),
    };
  }
}
