// DMs and channels (SPEC 4.10): codec of the six DM kinds (16-21) and the DM index built from
// the indexer's canonical blocks (messages, threads, escrows and how they settled, consent,
// channels with members, invites and key epochs). Chain state (getDmGate, getMessageEscrow,
// getChannel, getDmStats) stays the authority for gates, open escrows and counters; this index
// serves threads, inboxes and channel histories, which the chain does not keep (bodies live
// only in blocks). Every block tx succeeded (a failing tx rejects the block), so a replay of the
// canonical blocks in order reproduces the executor's effects. Dependency free.

import { b58decode, b58encode, hex, unhex } from "./codec.ts";
import { blake3 } from "./blake3.ts";
import type { BlockRec } from "./indexer.ts";
import { TX_PREFIX, SIG_LEN } from "./social.ts";

export const DM_KIND = { SendMessage: 16, ReclaimMessage: 17, SetConsent: 18, ChannelConfig: 19, ChannelMember: 20, ChannelKey: 21 } as const;
export type DmKindName = keyof typeof DM_KIND;
export const DM_KIND_NAME: Record<number, DmKindName> = { 16: "SendMessage", 17: "ReclaimMessage", 18: "SetConsent", 19: "ChannelConfig", 20: "ChannelMember", 21: "ChannelKey" };
export const MODES = ["plain", "sealed"] as const;
export const POLICIES = ["open", "invite", "paid"] as const;
export const ACTIONS = ["join", "leave", "invite", "remove"] as const;
export const CONSENT_STATES = ["clear", "allow", "deny"] as const;
export const MAX_MSG_BODY = 8192;
export const KEY_ENVELOPE_LEN = 104;
export const ZERO_HASH = "0".repeat(64);
export const ZERO_KEY = b58encode(new Uint8Array(32));

export type DmBody =
  | { kind: "SendMessage"; to: string; channel: string; mode: number; replyTo: string; pay: string; reclaimAfter: number; body?: string; bodyHex?: string; blobUri: string; blobHash: string; thoughtRef: string }
  | { kind: "ReclaimMessage"; messageId: string }
  | { kind: "SetConsent"; items: { peer: string; state: number }[] }
  | { kind: "ChannelConfig"; channelId: string; name: string; about: string; policy: number; joinPrice: string; mode: number; admins: string[] }
  | { kind: "ChannelMember"; channel: string; action: number; member: string; pay: string }
  | { kind: "ChannelKey"; channel: string; epoch: number; envelopes: { member: string; envelopeHex: string }[] };

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
    if (!Number.isInteger(v) || v < 0 || v > 0xffffffff) throw new Error("u32 out of range");
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
const tdStrict = new TextDecoder("utf-8", { fatal: true });
export const keyBytes = (k: string) => {
  const b = b58decode(k);
  if (b.length !== 32) throw new Error(`${k} is not a 32-byte key`);
  return b;
};
export const hashBytes = (h: string) => {
  if (!/^[0-9a-f]{64}$/i.test(h)) throw new Error(`${h} is not 64 hex`);
  return unhex(h.toLowerCase());
};
const hexBytes = (h: string, what: string) => {
  if (!/^([0-9a-f]{2})*$/i.test(h)) throw new Error(`${what} is not hex`);
  return unhex(h.toLowerCase());
};

/** The raw message body: `bodyHex` when given, else the UTF-8 of `body`. */
export function messageBodyBytes(b: { body?: string; bodyHex?: string }): Uint8Array {
  if (typeof b.bodyHex === "string") return hexBytes(b.bodyHex, "bodyHex");
  return te.encode(b.body ?? "");
}

export function encodeDmBody(b: DmBody): Uint8Array {
  const w = new Wr();
  switch (b.kind) {
    case "SendMessage": {
      const body = messageBodyBytes(b);
      if (body.length > MAX_MSG_BODY) throw new Error(`message body above ${MAX_MSG_BODY} bytes`);
      const ref = te.encode(b.thoughtRef ?? "");
      if (ref.length > 64) throw new Error("thought_ref above 64 bytes");
      const uri = te.encode(b.blobUri ?? "");
      if (uri.length > 512) throw new Error("blob_uri above 512 bytes");
      w.bytes(keyBytes(b.to)).bytes(hashBytes(b.channel)).u8(b.mode).bytes(hashBytes(b.replyTo)).u64(b.pay).u32(b.reclaimAfter)
        .b16(body).b16(uri).bytes(hashBytes(b.blobHash)).b8(ref);
      break;
    }
    case "ReclaimMessage":
      w.bytes(hashBytes(b.messageId));
      break;
    case "SetConsent":
      if (b.items.length < 1 || b.items.length > 64) throw new Error("SetConsent carries 1..=64 items");
      w.u8(b.items.length);
      for (const x of b.items) {
        if (!(x.state >= 0 && x.state <= 2)) throw new Error("consent state is 0, 1 or 2");
        w.bytes(keyBytes(x.peer)).u8(x.state);
      }
      break;
    case "ChannelConfig": {
      const name = te.encode(b.name);
      const about = te.encode(b.about ?? "");
      if (name.length > 64) throw new Error("channel name above 64 bytes");
      if (about.length > 1024) throw new Error("channel about above 1024 bytes");
      if (b.admins.length > 32) throw new Error("at most 32 admins");
      w.bytes(hashBytes(b.channelId)).b8(name).b16(about).u8(b.policy).u64(b.joinPrice).u8(b.mode).u8(b.admins.length);
      for (const a of b.admins) w.bytes(keyBytes(a));
      break;
    }
    case "ChannelMember":
      if (!(b.action >= 0 && b.action <= 3)) throw new Error("channel member action is 0..3");
      w.bytes(hashBytes(b.channel)).u8(b.action).bytes(keyBytes(b.member)).u64(b.pay);
      break;
    case "ChannelKey":
      if (b.envelopes.length < 1 || b.envelopes.length > 100) throw new Error("ChannelKey carries 1..=100 envelopes");
      w.bytes(hashBytes(b.channel)).u32(b.epoch).u8(b.envelopes.length);
      for (const e of b.envelopes) {
        const env = hexBytes(e.envelopeHex, "envelopeHex");
        if (env.length !== KEY_ENVELOPE_LEN) throw new Error(`a key envelope is ${KEY_ENVELOPE_LEN} bytes`);
        w.bytes(keyBytes(e.member)).bytes(env);
      }
      break;
  }
  return w.out();
}

/** Decode a DM body. SendMessage always carries `bodyHex`, plus `body` (UTF-8 text) in plain mode. */
export function decodeDmBody(kind: number, body: Uint8Array): DmBody {
  const r = new Rd(body);
  let out: DmBody;
  switch (kind) {
    case 16: {
      const to = b58encode(r.take(32));
      const channel = hex(r.take(32));
      const mode = r.u8();
      const replyTo = hex(r.take(32));
      const pay = r.u64().toString();
      const reclaimAfter = r.u32();
      const raw = r.b16(MAX_MSG_BODY, "message body");
      const blobUri = td.decode(r.b16(512, "blob_uri"));
      const blobHash = hex(r.take(32));
      const thoughtRef = td.decode(r.b8(64, "thought_ref"));
      out = { kind: "SendMessage", to, channel, mode, replyTo, pay, reclaimAfter, ...(mode === 0 ? { body: td.decode(raw) } : {}), bodyHex: hex(raw), blobUri, blobHash, thoughtRef };
      break;
    }
    case 17:
      out = { kind: "ReclaimMessage", messageId: hex(r.take(32)) };
      break;
    case 18: {
      const n = r.u8();
      if (n < 1 || n > 64) throw new Error("consents count");
      const items = Array.from({ length: n }, () => {
        const peer = b58encode(r.take(32));
        const state = r.u8();
        if (state > 2) throw new Error("consent state");
        return { peer, state };
      });
      out = { kind: "SetConsent", items };
      break;
    }
    case 19: {
      const channelId = hex(r.take(32));
      const name = td.decode(r.b8(64, "channel name"));
      const about = td.decode(r.b16(1024, "channel about"));
      const policy = r.u8();
      const joinPrice = r.u64().toString();
      const mode = r.u8();
      const n = r.u8();
      if (n > 32) throw new Error("channel admins above 32");
      const admins = Array.from({ length: n }, () => b58encode(r.take(32)));
      out = { kind: "ChannelConfig", channelId, name, about, policy, joinPrice, mode, admins };
      break;
    }
    case 20: {
      const channel = hex(r.take(32));
      const action = r.u8();
      if (action > 3) throw new Error("channel member action");
      out = { kind: "ChannelMember", channel, action, member: b58encode(r.take(32)), pay: r.u64().toString() };
      break;
    }
    case 21: {
      const channel = hex(r.take(32));
      const epoch = r.u32();
      const n = r.u8();
      if (n < 1 || n > 100) throw new Error("key envelopes count");
      const envelopes = Array.from({ length: n }, () => ({ member: b58encode(r.take(32)), envelopeHex: hex(r.take(KEY_ENVELOPE_LEN)) }));
      out = { kind: "ChannelKey", channel, epoch, envelopes };
      break;
    }
    default:
      throw new Error(`not a DM kind: ${kind}`);
  }
  if (r.p !== body.length) throw new Error(`trailing bytes: ${body.length - r.p}`);
  return out;
}

export interface DmTx {
  chainId: number;
  from: string;
  nonce: string;
  fee: string;
  kind: number;
  body: DmBody;
  sig: string;
}

/** A whole encoded DM tx (signature included); throws on anything else. */
export function decodeDmTx(tx: Uint8Array): DmTx {
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
    body: decodeDmBody(kind, tx.subarray(TX_PREFIX, tx.length - SIG_LEN)),
    sig: hex(tx.subarray(tx.length - SIG_LEN)),
  };
}

/** Bytes a wallet signs (everything before the signature). */
export function dmSigningBytes(chainId: number, from: string, nonce: bigint | string | number, fee: bigint | string, body: DmBody): Uint8Array {
  const w = new Wr();
  w.u8(1).u32(chainId).bytes(keyBytes(from)).u64(nonce).u64(fee).u8(DM_KIND[body.kind]);
  return Uint8Array.from([...w.out(), ...encodeDmBody(body)]);
}

/** Encoded length of the signed tx (the social fee rule measures it). */
export const dmTxLen = (body: DmBody) => TX_PREFIX + encodeDmBody(body).length + SIG_LEN;

/** The social fee rule over the whole DM tx: `max(min_fee, social_min_fee + social_byte_fee x len)` (SPEC 4.10). */
export function dmFee(params: any, body: DmBody): bigint {
  const min = BigInt(params?.min_fee ?? 1000);
  const base = BigInt(params?.social_min_fee ?? 1000);
  const per = BigInt(params?.social_byte_fee ?? 0);
  const f = base + per * BigInt(dmTxLen(body));
  return f > min ? f : min;
}

/** `message_id = blake3("pearl-sol:msg:v1" || from || nonce u64 LE)`. */
export function messageId(from: string, nonce: bigint | string | number): string {
  return hex(blake3(te.encode("pearl-sol:msg:v1"), keyBytes(from), new Wr().u64(nonce).out()));
}

/** `channel_id = blake3("pearl-sol:channel:v1" || creator || nonce u64 LE)`. */
export function channelId(creator: string, nonce: bigint | string | number): string {
  return hex(blake3(te.encode("pearl-sol:channel:v1"), keyBytes(creator), new Wr().u64(nonce).out()));
}

/** Framing check of a sealed DM body (version 1), as chain/agent-types/src/dm.rs. Returns the envelope count. */
export function checkSealedDm(b: Uint8Array): number {
  if (b.length < 2 + 32 + 24 + 2 + 16) throw new Error("sealed body too short");
  if (b[0] !== 1) throw new Error("sealed DM body must start with version 1");
  const count = b[1];
  if (count < 1 || count > 2) throw new Error("sealed DM body carries 1 or 2 envelopes");
  let pos = 2;
  for (let i = 0; i < count; i++) {
    if (b.length < pos + 58) throw new Error("sealed envelope truncated");
    pos += 56;
    const ct = b[pos] | (b[pos + 1] << 8);
    pos += 2;
    if (ct < 16) throw new Error("sealed ciphertext shorter than its tag");
    if (b.length < pos + ct) throw new Error("sealed ciphertext truncated");
    pos += ct;
  }
  if (pos !== b.length) throw new Error("bytes after the last sealed envelope");
  return count;
}

/** Framing check of a sealed channel body (version 2). Returns the epoch it names. */
export function checkSealedChannel(b: Uint8Array): number {
  if (b.length < 1 + 4 + 24 + 16) throw new Error("sealed channel body too short");
  if (b[0] !== 2) throw new Error("sealed channel body must start with version 2");
  return (b[1] | (b[2] << 8) | (b[3] << 16) | (b[4] << 24)) >>> 0;
}

/** Is `s` valid UTF-8 (plain bodies must be)? */
export function isUtf8(b: Uint8Array) {
  try {
    tdStrict.decode(b);
    return true;
  } catch {
    return false;
  }
}

/** Indexer tx facts for a DM kind: the decoded body, the nonce, a message's or a new channel's id. */
export function dmFacts(kind: string, txB64: string): { dm?: DmBody; nonce?: string; messageId?: string; channelId?: string } {
  if (!(kind in DM_KIND)) return {};
  try {
    const t = decodeDmTx(Uint8Array.from(Buffer.from(txB64, "base64")));
    const b = t.body;
    return {
      dm: b, nonce: t.nonce,
      ...(b.kind === "SendMessage" ? { messageId: messageId(t.from, t.nonce) } : {}),
      ...(b.kind === "ChannelConfig" && b.channelId === ZERO_HASH ? { channelId: channelId(t.from, t.nonce) } : {}),
    };
  } catch {
    return {};
  }
}

// ------------------------------------------------------------ DM index

export interface MsgRec {
  messageId: string;
  from: string;
  to: string | null;
  channel: string | null;
  mode: number;
  bodyHex: string;
  body: string | null;
  blobUri: string | null;
  replyTo: string | null;
  pay: string;
  reclaimAfter: number;
  thoughtRef: string | null;
  epoch: number | null;
  height: number;
  blockHash: string;
  txHash: string;
  index: number;
  time: number;
}

export interface EscrowRec {
  messageId: string;
  from: string;
  to: string;
  pay: string;
  height: number;
  deadline: number;
  /** null while in state (open or expired) */
  settled: null | { state: "released" | "refunded"; txHash: string; height: number; declined: boolean };
}

export interface ChannelRec {
  channelId: string;
  creator: string;
  name: string;
  about: string;
  policy: number;
  joinPrice: string;
  mode: number;
  admins: string[];
  members: Map<string, number>;
  invites: Map<string, number>;
  epoch: number;
  rekeyDue: boolean;
  /** epoch -> {height, envelopes member -> hex} */
  epochs: Map<number, { height: number; envelopes: Map<string, string> }>;
  messages: MsgRec[];
  createdHeight: number;
  setHeight: number;
}

const pairKey = (a: string, b: string) => (a < b ? `${a}|${b}` : `${b}|${a}`);

export class DmIndex {
  key = "";
  messages = new Map<string, MsgRec>();
  /** pair key -> messages oldest first */
  threads = new Map<string, MsgRec[]>();
  /** account -> peers it has a DM thread with */
  peers = new Map<string, Set<string>>();
  escrows = new Map<string, EscrowRec>();
  /** account -> peer -> {state 1 allow | 2 deny, height} */
  consent = new Map<string, Map<string, { state: number; height: number }>>();
  channels = new Map<string, ChannelRec>();
  /** account -> channels it is a member of */
  memberOf = new Map<string, Set<string>>();
  /** heights carrying DM-kind txs, for stream deltas */
  byHeight = new Map<number, MsgRec[]>();

  update(blocks: Map<number, BlockRec>, tipHeight: number) {
    const b = blocks.get(tipHeight);
    const key = `${tipHeight}:${b ? b.hash : ""}:${blocks.size}`;
    if (key === this.key) return this;
    this.reset();
    this.key = key;
    for (const h of [...blocks.keys()].sort((x, y) => x - y)) {
      const blk = blocks.get(h)!;
      blk.txs.forEach((t, i) => this.apply(blk, t as any, i));
    }
    return this;
  }

  private reset() {
    this.messages.clear();
    this.threads.clear();
    this.peers.clear();
    this.escrows.clear();
    this.consent.clear();
    this.channels.clear();
    this.memberOf.clear();
    this.byHeight.clear();
  }

  private join(ch: ChannelRec, who: string, h: number) {
    ch.members.set(who, h);
    const s = this.memberOf.get(who) ?? new Set();
    s.add(ch.channelId);
    this.memberOf.set(who, s);
  }

  private part(ch: ChannelRec, who: string) {
    ch.members.delete(who);
    this.memberOf.get(who)?.delete(ch.channelId);
  }

  private apply(blk: BlockRec, t: { hash: string; from: string; facts: any }, index: number) {
    const s: DmBody | undefined = t.facts?.dm;
    if (!s) return;
    const h = blk.height;
    switch (s.kind) {
      case "SendMessage": {
        const raw = unhex(s.bodyHex ?? "");
        const ch = s.channel === ZERO_HASH ? null : s.channel;
        const m: MsgRec = {
          messageId: t.facts.messageId, from: t.from, to: s.to === ZERO_KEY ? null : s.to, channel: ch, mode: s.mode,
          bodyHex: s.bodyHex ?? "", body: s.mode === 0 ? (s.body ?? td.decode(raw)) : null, blobUri: s.blobUri || null,
          replyTo: s.replyTo === ZERO_HASH ? null : s.replyTo, pay: s.pay, reclaimAfter: s.reclaimAfter, thoughtRef: s.thoughtRef || null,
          epoch: ch && s.mode === 1 && raw.length >= 5 ? (raw[1] | (raw[2] << 8) | (raw[3] << 16) | (raw[4] << 24)) >>> 0 : null,
          height: h, blockHash: blk.hash, txHash: t.hash, index, time: blk.time,
        };
        this.messages.set(m.messageId, m);
        const hl = this.byHeight.get(h) ?? [];
        hl.push(m);
        this.byHeight.set(h, hl);
        if (ch) {
          this.channels.get(ch)?.messages.push(m);
          break;
        }
        const to = m.to!;
        const k = pairKey(t.from, to);
        const l = this.threads.get(k) ?? [];
        l.push(m);
        this.threads.set(k, l);
        for (const [a, p] of [[t.from, to], [to, t.from]]) {
          const ps = this.peers.get(a) ?? new Set();
          ps.add(p);
          this.peers.set(a, ps);
        }
        if (BigInt(s.pay) > 0n && s.reclaimAfter > 0) {
          this.escrows.set(m.messageId, { messageId: m.messageId, from: t.from, to, pay: s.pay, height: h, deadline: h + s.reclaimAfter, settled: null });
        }
        if (m.replyTo) {
          const e = this.escrows.get(m.replyTo);
          if (e && !e.settled && e.to === t.from && e.from === to && h <= e.deadline) e.settled = { state: "released", txHash: t.hash, height: h, declined: false };
        }
        break;
      }
      case "ReclaimMessage": {
        const e = this.escrows.get(s.messageId);
        if (e && !e.settled) e.settled = { state: "refunded", txHash: t.hash, height: h, declined: t.from === e.to };
        break;
      }
      case "SetConsent": {
        const m = this.consent.get(t.from) ?? new Map();
        for (const x of s.items) {
          if (x.state === 0) m.delete(x.peer);
          else m.set(x.peer, { state: x.state, height: h });
        }
        this.consent.set(t.from, m);
        break;
      }
      case "ChannelConfig": {
        if (s.channelId === ZERO_HASH) {
          const id = t.facts.channelId as string;
          const ch: ChannelRec = {
            channelId: id, creator: t.from, name: s.name, about: s.about, policy: s.policy, joinPrice: s.joinPrice, mode: s.mode, admins: [...s.admins],
            members: new Map(), invites: new Map(), epoch: 0, rekeyDue: false, epochs: new Map(), messages: [], createdHeight: h, setHeight: h,
          };
          this.channels.set(id, ch);
          this.join(ch, t.from, h);
          for (const a of s.admins) this.join(ch, a, h);
        } else {
          const ch = this.channels.get(s.channelId);
          if (!ch) break;
          Object.assign(ch, { name: s.name, about: s.about, policy: s.policy, joinPrice: s.joinPrice, admins: [...s.admins], setHeight: h });
        }
        break;
      }
      case "ChannelMember": {
        const ch = this.channels.get(s.channel);
        if (!ch) break;
        const who = s.member === ZERO_KEY ? t.from : s.member;
        if (s.action === 0) {
          ch.invites.delete(who);
          this.join(ch, who, h);
        } else if (s.action === 2) {
          ch.invites.set(who, h);
        } else {
          this.part(ch, who);
          ch.admins = ch.admins.filter((a) => a !== who);
          if (ch.mode === 1 && ch.epoch > 0) ch.rekeyDue = true;
        }
        break;
      }
      case "ChannelKey": {
        const ch = this.channels.get(s.channel);
        if (!ch) break;
        if (s.epoch === ch.epoch + 1) {
          ch.epoch = s.epoch;
          ch.rekeyDue = false;
          ch.epochs.set(s.epoch, { height: h, envelopes: new Map() });
        }
        const ep = ch.epochs.get(s.epoch);
        if (ep) for (const e of s.envelopes) ep.envelopes.set(e.member, e.envelopeHex);
        break;
      }
    }
  }

  thread(a: string, b: string): MsgRec[] {
    return this.threads.get(pairKey(a, b)) ?? [];
  }

  /** Escrow state at the chain tip (`open` while tip + 1 <= deadline, the next block can still release it). */
  static escrowState(e: EscrowRec, tip: number): "open" | "expired" | "released" | "refunded" {
    if (e.settled) return e.settled.state;
    return tip + 1 <= e.deadline ? "open" : "expired";
  }

  consentOf(account: string, peer: string): number | null {
    return this.consent.get(account)?.get(peer)?.state ?? null;
  }
}
