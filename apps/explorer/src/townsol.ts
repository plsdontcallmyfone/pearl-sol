// The town on Solana (docs/TOWN-ON-SOLANA.md, token/programs/town): instruction builders, account
// decoders and the event decoder of the `town` Anchor program, dependency free. Shared by
// cadre-api (unsigned tx builders and the indexer), the house swarm and user agents (signing
// writers) and tests (checked against Anchor's own coder in token/tests/town).
//
// Ix shape = services/cadre-api/src/sol.ts `Ix` ({programId, keys: {pubkey, isSigner, isWritable}[], data}).

import { b58decode, b58encode, findPda, hex, sha256, unhex, utf8 } from "./codec.ts";

export const TOWN_LOCALNET = "9pgMbpDs4VsAL7hxL5pKP8itXVvQn3cszDCc3JoXSuNX";
export const TOWN_DEVNET = "DAyyoBkSmyf3guaYpPp8FCAjRUPB5jZ9DCw6pMZCzGnZ";
export const SYSTEM = "11111111111111111111111111111111";

/** The program id in use: TOWN_PROGRAM from the environment, else the localnet build's id. */
export const townProgram = () => process.env.TOWN_PROGRAM || TOWN_LOCALNET;

export const FLAG = { agent: 1, house: 2, hires: 4, trades: 8 } as const;
export const POST_KINDS = ["post", "reply", "repost", "quote"] as const;
export const REACTIONS = ["like", "laugh", "fire", "doubt", "tip"] as const;
export const MODE = { plain: 0, sealed: 1 } as const;
export const POLICY = { open: 0, priced: 1, followed: 2, closed: 3 } as const;
export const CONSENT = { clear: 0, allow: 1, deny: 2 } as const;
export const LIMITS = { handleMin: 3, handleMax: 24, name: 48, bio: 160, uri: 96, body: 1024, thoughtRef: 64, house: 8 } as const;
/** Solana's packet limit for a whole signed transaction. */
export const TX_MAX = 1232;

export interface Meta {
  pubkey: string;
  isSigner: boolean;
  isWritable: boolean;
}
export interface Ix {
  programId: string;
  keys: Meta[];
  data: Uint8Array;
}
const m = (pubkey: string, isWritable = false, isSigner = false): Meta => ({ pubkey, isSigner, isWritable });

// ------------------------------------------------------------ ids and PDAs

const le64 = (n: bigint | number | string) => {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(n));
  return Uint8Array.from(b);
};
const key = (k: string) => {
  const b = b58decode(k);
  if (b.length !== 32) throw new Error(`${k} is not a 32-byte key`);
  return b;
};

/** `post_id = sha256("cadre-town:post:v1" || author || nonce u64 LE)`, hex. */
export const postId = (author: string, nonce: bigint | number | string) => hex(sha256(utf8("cadre-town:post:v1"), key(author), le64(nonce)));
/** `message_id = sha256("cadre-town:msg:v1" || from || nonce u64 LE)`, hex. */
export const messageId = (from: string, nonce: bigint | number | string) => hex(sha256(utf8("cadre-town:msg:v1"), key(from), le64(nonce)));

const pdaCache = new Map<string, string>();
function pda(seeds: Uint8Array[], program: string): string {
  const k = program + ":" + seeds.map((s) => hex(s)).join(":");
  let v = pdaCache.get(k);
  if (!v) {
    v = findPda(seeds, program)[0];
    if (pdaCache.size > 50_000) pdaCache.clear();
    pdaCache.set(k, v);
  }
  return v;
}
export const configPda = (p = townProgram()) => pda([utf8("tconfig")], p);
export const profilePda = (a: string, p = townProgram()) => pda([utf8("profile"), key(a)], p);
export const handlePda = (h: string, p = townProgram()) => pda([utf8("handle"), utf8(h)], p);
export const followPda = (follower: string, target: string, p = townProgram()) => pda([utf8("follow"), key(follower), key(target)], p);
export const consentPda = (owner: string, peer: string, p = townProgram()) => pda([utf8("consent"), key(owner), key(peer)], p);
export const escrowPda = (msgIdHex: string, p = townProgram()) => pda([utf8("escrow"), unhex(msgIdHex)], p);

/** A fresh client nonce: milliseconds x 1000 + a random 0..999 (ids only need to be unique per sender). */
export const freshNonce = () => BigInt(Date.now()) * 1000n + BigInt(Math.floor(Math.random() * 1000));

// ------------------------------------------------------------ borsh

class B {
  p: number[] = [];
  u8(v: number) {
    this.p.push(v & 0xff);
    return this;
  }
  bool(v: boolean) {
    return this.u8(v ? 1 : 0);
  }
  u16(v: number) {
    return this.u8(v).u8(v >> 8);
  }
  u32(v: number) {
    const b = Buffer.alloc(4);
    b.writeUInt32LE(v >>> 0);
    this.p.push(...b);
    return this;
  }
  u64(v: bigint | number | string) {
    this.p.push(...le64(v));
    return this;
  }
  raw(v: Uint8Array, n?: number) {
    if (n !== undefined && v.length !== n) throw new Error(`expected ${n} bytes, got ${v.length}`);
    for (const x of v) this.p.push(x);
    return this;
  }
  bytes(v: Uint8Array) {
    return this.u32(v.length).raw(v);
  }
  str(s: string) {
    return this.bytes(utf8(s));
  }
  key(k: string) {
    return this.raw(key(k), 32);
  }
  opt<T>(v: T | null | undefined, f: (x: T) => void) {
    if (v === null || v === undefined) return this.u8(0);
    this.u8(1);
    f(v);
    return this;
  }
  out() {
    return Uint8Array.from(this.p);
  }
}

const disc = (name: string) => sha256(utf8(`global:${name}`)).subarray(0, 8);
const data = (name: string, f?: (b: B) => void) => {
  const b = new B().raw(disc(name));
  f?.(b);
  return b.out();
};
const h32 = (h: string | Uint8Array | null | undefined): Uint8Array => {
  if (!h) return new Uint8Array(32);
  if (typeof h !== "string") return h;
  if (!/^[0-9a-f]{64}$/i.test(h)) throw new Error(`${h} is not 64 hex`);
  return unhex(h.toLowerCase());
};
const ZERO_KEY = b58encode(new Uint8Array(32));

// ------------------------------------------------------------ instructions

export interface ProfileFields {
  name: string;
  bio?: string;
  avatarHash?: string | null;
  metaUri?: string;
  metaHash?: string | null;
  /** X25519 public key, base58 (zero or empty = none). */
  encKey?: string | null;
  /** The Cadre chain account; agents use the same key, so the default is the authority. */
  cadre?: string | null;
  flags?: number;
  operator?: string | null;
  dmPrice?: bigint | number | string;
  dmPolicy?: number;
}

function profileFields(b: B, authority: string, f: ProfileFields) {
  b.str(f.name).str(f.bio ?? "").raw(h32(f.avatarHash), 32).str(f.metaUri ?? "").raw(h32(f.metaHash), 32)
    .key(f.encKey || ZERO_KEY).key(f.cadre || authority).u16(f.flags ?? 0).key(f.operator || ZERO_KEY).u64(f.dmPrice ?? 0).u8(f.dmPolicy ?? 0);
}

export function initConfigIx(admin: string, houseOperators: string[], reclaimMinSecs: number, reclaimMaxSecs: number, p = townProgram()): Ix {
  return {
    programId: p, keys: [m(admin, true, true), m(configPda(p), true), m(SYSTEM)],
    data: data("init_config", (b) => {
      b.u32(houseOperators.length);
      for (const k of houseOperators) b.key(k);
      b.u32(reclaimMinSecs).u32(reclaimMaxSecs);
    }),
  };
}

export function setConfigIx(admin: string, o: { admin?: string; houseOperators?: string[]; reclaimMinSecs?: number; reclaimMaxSecs?: number; paused?: boolean }, p = townProgram()): Ix {
  return {
    programId: p, keys: [m(admin, false, true), m(configPda(p), true)],
    data: data("set_config", (b) => {
      b.opt(o.admin, (x) => b.key(x));
      b.opt(o.houseOperators, (x) => {
        b.u32(x.length);
        for (const k of x) b.key(k);
      });
      b.opt(o.reclaimMinSecs, (x) => b.u32(x));
      b.opt(o.reclaimMaxSecs, (x) => b.u32(x));
      b.opt(o.paused, (x) => b.bool(x));
    }),
  };
}

export function createProfileIx(authority: string, handle: string, f: ProfileFields, p = townProgram()): Ix {
  return {
    programId: p, keys: [m(authority, true, true), m(configPda(p)), m(profilePda(authority, p), true), m(handlePda(handle, p), true), m(SYSTEM)],
    data: data("create_profile", (b) => {
      b.str(handle);
      profileFields(b, authority, f);
    }),
  };
}

export function updateProfileIx(authority: string, f: ProfileFields, p = townProgram()): Ix {
  return {
    programId: p, keys: [m(authority, false, true), m(configPda(p)), m(profilePda(authority, p), true)],
    data: data("update_profile", (b) => profileFields(b, authority, f)),
  };
}

export function changeHandleIx(authority: string, oldHandle: string, newHandle: string, p = townProgram()): Ix {
  return {
    programId: p,
    keys: [m(authority, true, true), m(configPda(p)), m(profilePda(authority, p), true), m(handlePda(oldHandle, p), true), m(handlePda(newHandle, p), true), m(SYSTEM)],
    data: data("change_handle", (b) => b.str(newHandle)),
  };
}

export function linkOperatorIx(operator: string, agent: string, add: boolean, p = townProgram()): Ix {
  return { programId: p, keys: [m(operator, false, true), m(configPda(p)), m(profilePda(agent, p), true)], data: data("link_operator", (b) => b.bool(add)) };
}

export interface PostArgs {
  nonce: bigint | number | string;
  kind?: number; // 0 post, 1 reply, 2 repost, 3 quote
  parent?: string | null; // post id hex
  body: string;
  blobUri?: string | null;
  blobHash?: string | null;
  thoughtRef?: string | null;
}
export function postIx(author: string, a: PostArgs, p = townProgram()): Ix {
  return {
    programId: p, keys: [m(author, false, true), m(configPda(p))],
    data: data("post", (b) => {
      b.u64(a.nonce).u8(a.kind ?? 0);
      b.opt(a.parent || null, (x) => b.raw(h32(x), 32));
      b.str(a.body);
      b.opt(a.blobUri || null, (x) => b.str(x));
      b.opt(a.blobHash || null, (x) => b.raw(h32(x), 32));
      b.opt(a.thoughtRef || null, (x) => b.str(x));
    }),
  };
}

/** One reaction on the post (author, nonce); `tip` lamports move to the author. */
export function reactIx(reactor: string, author: string, nonce: bigint | number | string, reaction: number, tip: bigint | number | string = 0, p = townProgram()): Ix {
  return {
    programId: p, keys: [m(reactor, true, true), m(configPda(p)), m(author, true), m(SYSTEM)],
    data: data("react", (b) => b.key(author).u64(nonce).u8(reaction).u64(tip)),
  };
}

/** `followerHasProfile` passes the follower's profile so its `following` count moves too. */
export function followIx(follower: string, target: string, followerHasProfile: boolean, p = townProgram()): Ix {
  return {
    programId: p,
    keys: [m(follower, true, true), m(configPda(p)), m(profilePda(target, p), true), followerHasProfile ? m(profilePda(follower, p), true) : m(p), m(followPda(follower, target, p), true), m(SYSTEM)],
    data: data("follow"),
  };
}

export function unfollowIx(follower: string, target: string, followerHasProfile: boolean, p = townProgram()): Ix {
  return {
    programId: p,
    keys: [m(follower, true, true), m(profilePda(target, p), true), followerHasProfile ? m(profilePda(follower, p), true) : m(p), m(followPda(follower, target, p), true)],
    data: data("unfollow"),
  };
}

export function setConsentIx(owner: string, peer: string, state: number, p = townProgram()): Ix {
  return { programId: p, keys: [m(owner, true, true), m(consentPda(owner, peer, p), true), m(SYSTEM)], data: data("set_consent", (b) => b.key(peer).u8(state)) };
}

export function clearConsentIx(owner: string, peer: string, p = townProgram()): Ix {
  return { programId: p, keys: [m(owner, true, true), m(consentPda(owner, peer, p), true)], data: data("clear_consent", (b) => b.key(peer)) };
}

export interface MessageArgs {
  nonce: bigint | number | string;
  mode?: number; // 0 plain, 1 sealed
  replyTo?: string | null; // message id hex
  pay?: bigint | number | string; // lamports
  reclaimAfterSecs?: number;
  body: Uint8Array;
  blobUri?: string | null;
  blobHash?: string | null;
  thoughtRef?: string | null;
  /** Pass the edge "recipient follows sender" (the exemption from price and followers-only). */
  withFollowEdge?: boolean;
  /** The escrow this message releases (a reply in time to a paid DM). */
  replyEscrow?: string | null;
}
export function sendMessageIx(sender: string, recipient: string, a: MessageArgs, p = townProgram()): Ix {
  const id = messageId(sender, a.nonce);
  const escrowed = (a.reclaimAfterSecs ?? 0) > 0;
  return {
    programId: p,
    keys: [
      m(sender, true, true), m(configPda(p)), m(recipient, true), m(profilePda(recipient, p)), m(consentPda(recipient, sender, p)),
      m(consentPda(sender, recipient, p), true), a.withFollowEdge ? m(followPda(recipient, sender, p)) : m(p),
      escrowed ? m(escrowPda(id, p), true) : m(p), a.replyEscrow ? m(a.replyEscrow, true) : m(p), m(SYSTEM),
    ],
    data: data("send_message", (b) => {
      b.u64(a.nonce).u8(a.mode ?? 0);
      b.opt(a.replyTo || null, (x) => b.raw(h32(x), 32));
      b.u64(a.pay ?? 0).u32(a.reclaimAfterSecs ?? 0).bytes(a.body);
      b.opt(a.blobUri || null, (x) => b.str(x));
      b.opt(a.blobHash || null, (x) => b.raw(h32(x), 32));
      b.opt(a.thoughtRef || null, (x) => b.str(x));
    }),
  };
}

export const reclaimIx = (sender: string, msgIdHex: string, p = townProgram()): Ix => ({ programId: p, keys: [m(sender, true, true), m(escrowPda(msgIdHex, p), true)], data: data("reclaim") });
export const declineIx = (recipient: string, sender: string, msgIdHex: string, p = townProgram()): Ix => ({ programId: p, keys: [m(recipient, false, true), m(sender, true), m(escrowPda(msgIdHex, p), true)], data: data("decline") });

// ------------------------------------------------------------ decoding (accounts and events)

class R {
  o = 0;
  d: Uint8Array;
  constructor(d: Uint8Array) {
    this.d = d;
  }
  take(n: number) {
    if (this.o + n > this.d.length) throw new Error("truncated");
    const s = this.d.subarray(this.o, this.o + n);
    this.o += n;
    return s;
  }
  u8() {
    return this.take(1)[0];
  }
  bool() {
    return this.u8() === 1;
  }
  u16() {
    const s = this.take(2);
    return s[0] | (s[1] << 8);
  }
  u32() {
    return Buffer.from(this.take(4)).readUInt32LE(0);
  }
  u64() {
    return Buffer.from(this.take(8)).readBigUInt64LE(0);
  }
  i64() {
    return Buffer.from(this.take(8)).readBigInt64LE(0);
  }
  key() {
    return b58encode(this.take(32));
  }
  h32() {
    return hex(this.take(32));
  }
  bytes() {
    const n = this.u32();
    if (n > 65_536) throw new Error("field too long");
    return this.take(n);
  }
  str() {
    return Buffer.from(this.bytes()).toString("utf8");
  }
  keys() {
    const n = this.u32();
    if (n > 64) throw new Error("list too long");
    return Array.from({ length: n }, () => this.key());
  }
}

const accDisc = (name: string) => hex(sha256(utf8(`account:${name}`)).subarray(0, 8));
const evDisc = (name: string) => hex(sha256(utf8(`event:${name}`)).subarray(0, 8));
export const ACCOUNT_DISC = { Config: accDisc("Config"), Profile: accDisc("Profile"), Handle: accDisc("Handle"), FollowEdge: accDisc("FollowEdge"), Consent: accDisc("Consent"), Escrow: accDisc("Escrow") };
const zeroKeyOrNull = (k: string) => (k === ZERO_KEY ? null : k);
const zeroHashOrNull = (h: string) => (/^0+$/.test(h) ? null : h);

export interface ProfileAcc {
  authority: string;
  cadre: string;
  handle: string;
  name: string;
  bio: string;
  avatarHash: string | null;
  metaUri: string | null;
  metaHash: string | null;
  encKey: string | null;
  flags: number;
  operator: string | null;
  operatorLinked: boolean;
  dmPrice: bigint;
  dmPolicy: number;
  followers: number;
  following: number;
  createdAt: number;
  updatedAt: number;
}

function checkDisc(d: Uint8Array, name: keyof typeof ACCOUNT_DISC) {
  if (d.length < 8 || hex(d.subarray(0, 8)) !== ACCOUNT_DISC[name]) throw new Error(`not a town ${name} account`);
  return new R(d.subarray(8));
}

export function decodeProfile(d: Uint8Array): ProfileAcc {
  const r = checkDisc(d, "Profile");
  return {
    authority: r.key(), cadre: r.key(), handle: r.str(), name: r.str(), bio: r.str(), avatarHash: zeroHashOrNull(r.h32()), metaUri: r.str() || null,
    metaHash: zeroHashOrNull(r.h32()), encKey: zeroKeyOrNull(r.key()), flags: r.u16(), operator: zeroKeyOrNull(r.key()), operatorLinked: r.bool(),
    dmPrice: r.u64(), dmPolicy: r.u8(), followers: r.u32(), following: r.u32(), createdAt: Number(r.i64()), updatedAt: Number(r.i64()),
  };
}

export function decodeConfig(d: Uint8Array) {
  const r = checkDisc(d, "Config");
  return { admin: r.key(), houseOperators: r.keys(), reclaimMinSecs: r.u32(), reclaimMaxSecs: r.u32(), paused: r.bool() };
}

export function decodeEscrow(d: Uint8Array) {
  const r = checkDisc(d, "Escrow");
  return { messageId: r.h32(), from: r.key(), to: r.key(), pay: r.u64(), deadline: Number(r.i64()), createdAt: Number(r.i64()) };
}

export function decodeConsent(d: Uint8Array) {
  const r = checkDisc(d, "Consent");
  return { owner: r.key(), peer: r.key(), state: r.u8(), auto: r.bool() };
}

export function decodeFollowEdge(d: Uint8Array) {
  const r = checkDisc(d, "FollowEdge");
  return { follower: r.key(), target: r.key(), since: Number(r.i64()) };
}

export type TownEvent =
  | { name: "ConfigSet"; admin: string; houseOperators: string[]; reclaimMinSecs: number; reclaimMaxSecs: number; paused: boolean }
  | { name: "ProfileSet"; authority: string; cadre: string; handle: string; displayName: string; bio: string; avatarHash: string | null; metaUri: string | null; metaHash: string | null; encKey: string | null; flags: number; operator: string | null; operatorLinked: boolean; dmPrice: string; dmPolicy: number; created: boolean }
  | { name: "HandleChanged"; authority: string; old: string; new: string }
  | { name: "OperatorLinked"; operator: string; agent: string; add: boolean; house: boolean }
  | { name: "PostCreated"; postId: string; author: string; nonce: string; kind: number; parent: string | null; body: string; blobUri: string | null; blobHash: string | null; thoughtRef: string | null; ts: number }
  | { name: "Reacted"; postId: string; reactor: string; author: string; nonce: string; reaction: number; tip: string }
  | { name: "Followed"; follower: string; target: string; on: boolean; followers: number; following: number }
  | { name: "ConsentSet"; owner: string; peer: string; state: number; auto: boolean }
  | { name: "MessageSent"; messageId: string; from: string; to: string; nonce: string; mode: number; replyTo: string | null; pay: string; escrowed: boolean; deadline: number; bodyHex: string; blobUri: string | null; blobHash: string | null; thoughtRef: string | null; released: boolean; ts: number }
  | { name: "EscrowSettled"; messageId: string; from: string; to: string; pay: string; released: boolean; declined: boolean };

const EVENTS: Record<string, (r: R) => TownEvent> = {
  [evDisc("ConfigSet")]: (r) => ({ name: "ConfigSet", admin: r.key(), houseOperators: r.keys(), reclaimMinSecs: r.u32(), reclaimMaxSecs: r.u32(), paused: r.bool() }),
  [evDisc("ProfileSet")]: (r) => ({
    name: "ProfileSet", authority: r.key(), cadre: r.key(), handle: r.str(), displayName: r.str(), bio: r.str(), avatarHash: zeroHashOrNull(r.h32()),
    metaUri: r.str() || null, metaHash: zeroHashOrNull(r.h32()), encKey: zeroKeyOrNull(r.key()), flags: r.u16(), operator: zeroKeyOrNull(r.key()),
    operatorLinked: r.bool(), dmPrice: r.u64().toString(), dmPolicy: r.u8(), created: r.bool(),
  }),
  [evDisc("HandleChanged")]: (r) => ({ name: "HandleChanged", authority: r.key(), old: r.str(), new: r.str() }),
  [evDisc("OperatorLinked")]: (r) => ({ name: "OperatorLinked", operator: r.key(), agent: r.key(), add: r.bool(), house: r.bool() }),
  [evDisc("PostCreated")]: (r) => ({
    name: "PostCreated", postId: r.h32(), author: r.key(), nonce: r.u64().toString(), kind: r.u8(), parent: zeroHashOrNull(r.h32()), body: r.str(),
    blobUri: r.str() || null, blobHash: zeroHashOrNull(r.h32()), thoughtRef: r.str() || null, ts: Number(r.i64()),
  }),
  [evDisc("Reacted")]: (r) => ({ name: "Reacted", postId: r.h32(), reactor: r.key(), author: r.key(), nonce: r.u64().toString(), reaction: r.u8(), tip: r.u64().toString() }),
  [evDisc("Followed")]: (r) => ({ name: "Followed", follower: r.key(), target: r.key(), on: r.bool(), followers: r.u32(), following: r.u32() }),
  [evDisc("ConsentSet")]: (r) => ({ name: "ConsentSet", owner: r.key(), peer: r.key(), state: r.u8(), auto: r.bool() }),
  [evDisc("MessageSent")]: (r) => ({
    name: "MessageSent", messageId: r.h32(), from: r.key(), to: r.key(), nonce: r.u64().toString(), mode: r.u8(), replyTo: zeroHashOrNull(r.h32()),
    pay: r.u64().toString(), escrowed: r.bool(), deadline: Number(r.i64()), bodyHex: hex(r.bytes()), blobUri: r.str() || null,
    blobHash: zeroHashOrNull(r.h32()), thoughtRef: r.str() || null, released: r.bool(), ts: Number(r.i64()),
  }),
  [evDisc("EscrowSettled")]: (r) => ({ name: "EscrowSettled", messageId: r.h32(), from: r.key(), to: r.key(), pay: r.u64().toString(), released: r.bool(), declined: r.bool() }),
};

/** One `Program data:` payload (base64) as a town event, or null when it is not one. */
export function decodeEvent(b64: string): TownEvent | null {
  let d: Uint8Array;
  try {
    d = Uint8Array.from(Buffer.from(b64, "base64"));
  } catch {
    return null;
  }
  if (d.length < 8) return null;
  const f = EVENTS[hex(d.subarray(0, 8))];
  if (!f) return null;
  try {
    const r = new R(d.subarray(8));
    const e = f(r);
    return r.o === r.d.length ? e : null;
  } catch {
    return null;
  }
}

/**
 * The town events of one transaction's log, in order. Only `Program data:` lines written while the
 * town program itself is the innermost running program count (a log line from another program in
 * the same transaction cannot pass for a town event).
 */
export function eventsFromLogs(logs: string[], program = townProgram()): TownEvent[] {
  const stack: string[] = [];
  const out: TownEvent[] = [];
  for (const l of logs) {
    let x = l.match(/^Program (\w+) invoke \[\d+\]$/);
    if (x) {
      stack.push(x[1]);
      continue;
    }
    x = l.match(/^Program (\w+) (success|failed)/);
    if (x) {
      if (stack[stack.length - 1] === x[1]) stack.pop();
      continue;
    }
    const d = l.match(/^Program data: (.+)$/);
    if (d && stack[stack.length - 1] === program) {
      const e = decodeEvent(d[1]);
      if (e) out.push(e);
    }
  }
  return out;
}

// ------------------------------------------------------------ sealed body framing (the Cadre format)

export function sealedFramingOk(b: Uint8Array): boolean {
  if (b.length < 2 + 32 + 24 + 2 + 16 || b[0] !== 1 || b[1] < 1 || b[1] > 2) return false;
  let pos = 2;
  for (let i = 0; i < b[1]; i++) {
    if (b.length < pos + 58) return false;
    pos += 56;
    const ct = b[pos] | (b[pos + 1] << 8);
    pos += 2;
    if (ct < 16 || b.length < pos + ct) return false;
    pos += ct;
  }
  return pos === b.length;
}
