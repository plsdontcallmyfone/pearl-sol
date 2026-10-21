// DM and channel tx kinds 16-21 (SPEC 4.10): byte codec, window.pearl JSON form and approval
// wording. Byte layout mirrors chain/agent-types/src/dm.rs (vectors/dm.json, test/tx-vectors).
// JSON form (docs/API.md "DMs and channels"): camelCase, keys base58, ids hex, amounts decimal
// strings, numbers for mode, policy, action, state, epoch, reclaimAfter.

import { Reader, b58, concat, fromHex, fromUtf8, hex, key32, u16, u32, u64, u8, utf8 } from "../core/bytes";
import { blake3 } from "../core/crypto";

export const MAX_MSG_BODY = 8192;
export const MAX_URI = 512;
export const MAX_THOUGHT_REF = 64;
export const MAX_CONSENT_BATCH = 64;
export const MAX_CHANNEL_NAME = 64;
export const MAX_CHANNEL_ABOUT = 1024;
export const MAX_CHANNEL_ADMINS = 32;
export const MAX_KEY_ENVELOPES = 100;
export const KEY_ENVELOPE_LEN = 104;
export const MODES = ["plain", "sealed"] as const;
export const CHANNEL_POLICIES = ["open", "invite", "paid"] as const;
export const MEMBER_ACTIONS = ["join", "leave", "invite", "remove"] as const;
export const CONSENT_STATES = ["clear", "allow", "deny"] as const;
export const DM_POLICIES = ["open", "priced cold DMs", "followers only", "closed"] as const;
export const DM_KINDS = ["SendMessage", "ReclaimMessage", "SetConsent", "ChannelConfig", "ChannelMember", "ChannelKey"] as const;
/** The chain's target block time (SPEC 4.10 / ECOSYSTEM 2.2: 60 s); callers pass getChainInfo's when they have it. */
export const DEFAULT_TARGET_BLOCK_MS = 60_000;

export interface SendMessageBody {
  to: Uint8Array; // 32, zero for a channel message
  channel: Uint8Array; // 32, zero for a DM
  mode: number; // 0 plain, 1 sealed
  replyTo: Uint8Array; // 32
  pay: bigint;
  reclaimAfter: number; // blocks, 0 = pay is final
  body: Uint8Array; // UTF-8 when plain, the sealed format when sealed
  blobUri: string;
  blobHash: Uint8Array; // 32
  thoughtRef: string;
}
export interface ConsentItem {
  peer: Uint8Array;
  state: number; // 0 clear, 1 allow, 2 deny
}
export interface ChannelConfigBody {
  channelId: Uint8Array; // zero = create
  name: string;
  about: string;
  policy: number;
  joinPrice: bigint;
  mode: number;
  admins: Uint8Array[];
}
export interface KeyEnvelope {
  member: Uint8Array;
  envelope: Uint8Array; // 104
}

export type DmBody =
  | ({ kind: "SendMessage" } & SendMessageBody)
  | { kind: "ReclaimMessage"; messageId: Uint8Array }
  | { kind: "SetConsent"; items: ConsentItem[] }
  | ({ kind: "ChannelConfig" } & ChannelConfigBody)
  | { kind: "ChannelMember"; channel: Uint8Array; action: number; member: Uint8Array; pay: bigint }
  | { kind: "ChannelKey"; channel: Uint8Array; epoch: number; envelopes: KeyEnvelope[] };

export const isDmKind = (k: string) => (DM_KINDS as readonly string[]).includes(k);

const le64 = (n: bigint) => u64(n);
/** message_id = blake3("pearl-sol:msg:v1" || from || nonce u64 LE). */
export const messageId = (from: Uint8Array, nonce: bigint) => blake3(concat(utf8("pearl-sol:msg:v1"), from, le64(nonce)));
/** channel_id = blake3("pearl-sol:channel:v1" || creator || nonce u64 LE). */
export const channelId = (creator: Uint8Array, nonce: bigint) => blake3(concat(utf8("pearl-sol:channel:v1"), creator, le64(nonce)));

function fixed(b: Uint8Array, n: number, what: string) {
  if (b.length !== n) throw new Error(`${what} must be ${n} bytes, got ${b.length}`);
  return b;
}
function bytes8(b: Uint8Array, max: number, what: string) {
  if (b.length > max) throw new Error(`${what} over ${max} bytes`);
  return concat(u8(b.length), b);
}
function bytes16(b: Uint8Array, max: number, what: string) {
  if (b.length > max) throw new Error(`${what} over ${max} bytes`);
  return concat(u16(b.length), b);
}
function count(n: number, lo: number, hi: number, what: string) {
  if (n < lo || n > hi) throw new Error(`${what}: ${lo}..=${hi} entries, got ${n}`);
}

export function encodeDmBody(b: DmBody): Uint8Array {
  switch (b.kind) {
    case "SendMessage":
      return concat(
        fixed(b.to, 32, "to"), fixed(b.channel, 32, "channel"), u8(b.mode), fixed(b.replyTo, 32, "reply_to"), u64(b.pay), u32(b.reclaimAfter),
        bytes16(b.body, MAX_MSG_BODY, "message body"), bytes16(utf8(b.blobUri), MAX_URI, "blob_uri"), fixed(b.blobHash, 32, "blob_hash"),
        bytes8(utf8(b.thoughtRef), MAX_THOUGHT_REF, "thought_ref"),
      );
    case "ReclaimMessage":
      return fixed(b.messageId, 32, "message_id");
    case "SetConsent":
      count(b.items.length, 1, MAX_CONSENT_BATCH, "consents");
      return concat(u8(b.items.length), ...b.items.flatMap((x) => [fixed(x.peer, 32, "peer"), u8(x.state)]));
    case "ChannelConfig":
      count(b.admins.length, 0, MAX_CHANNEL_ADMINS, "channel admins");
      return concat(
        fixed(b.channelId, 32, "channel_id"), bytes8(utf8(b.name), MAX_CHANNEL_NAME, "channel name"), bytes16(utf8(b.about), MAX_CHANNEL_ABOUT, "channel about"),
        u8(b.policy), u64(b.joinPrice), u8(b.mode), u8(b.admins.length), ...b.admins.map((a) => fixed(a, 32, "admin")),
      );
    case "ChannelMember":
      return concat(fixed(b.channel, 32, "channel"), u8(b.action), fixed(b.member, 32, "member"), u64(b.pay));
    case "ChannelKey":
      count(b.envelopes.length, 1, MAX_KEY_ENVELOPES, "key envelopes");
      return concat(
        fixed(b.channel, 32, "channel"), u32(b.epoch), u8(b.envelopes.length),
        ...b.envelopes.flatMap((x) => [fixed(x.member, 32, "member"), fixed(x.envelope, KEY_ENVELOPE_LEN, "envelope")]),
      );
  }
}

function read16(r: Reader, max: number, what: string) {
  const n = r.u16();
  if (n > max) throw new Error(`${what} over ${max} bytes`);
  return r.take(n).slice();
}
function read8(r: Reader, max: number, what: string) {
  const n = r.u8();
  if (n > max) throw new Error(`${what} over ${max} bytes`);
  return r.take(n).slice();
}

/** Reads the body of DM kind `kind` (a DM_KINDS name); the caller finishes the reader. */
export function readDmBody(kind: DmBody["kind"], r: Reader): DmBody {
  switch (kind) {
    case "SendMessage":
      return {
        kind, to: r.take(32).slice(), channel: r.take(32).slice(), mode: r.u8(), replyTo: r.take(32).slice(), pay: r.u64(), reclaimAfter: r.u32(),
        body: read16(r, MAX_MSG_BODY, "message body"), blobUri: fromUtf8(read16(r, MAX_URI, "blob_uri")), blobHash: r.take(32).slice(),
        thoughtRef: fromUtf8(read8(r, MAX_THOUGHT_REF, "thought_ref")),
      };
    case "ReclaimMessage":
      return { kind, messageId: r.take(32).slice() };
    case "SetConsent": {
      const n = r.u8();
      count(n, 1, MAX_CONSENT_BATCH, "consents");
      return {
        kind,
        items: Array.from({ length: n }, () => {
          const peer = r.take(32).slice();
          const state = r.u8();
          if (state > 2) throw new Error("bad consent state");
          return { peer, state };
        }),
      };
    }
    case "ChannelConfig": {
      const channelId = r.take(32).slice();
      const name = fromUtf8(read8(r, MAX_CHANNEL_NAME, "channel name"));
      const about = fromUtf8(read16(r, MAX_CHANNEL_ABOUT, "channel about"));
      const policy = r.u8(), joinPrice = r.u64(), mode = r.u8();
      const n = r.u8();
      count(n, 0, MAX_CHANNEL_ADMINS, "channel admins");
      return { kind, channelId, name, about, policy, joinPrice, mode, admins: Array.from({ length: n }, () => r.take(32).slice()) };
    }
    case "ChannelMember":
      return { kind, channel: r.take(32).slice(), action: r.u8(), member: r.take(32).slice(), pay: r.u64() };
    case "ChannelKey": {
      const channel = r.take(32).slice();
      const epoch = r.u32();
      const n = r.u8();
      count(n, 1, MAX_KEY_ENVELOPES, "key envelopes");
      return { kind, channel, epoch, envelopes: Array.from({ length: n }, () => ({ member: r.take(32).slice(), envelope: r.take(KEY_ENVELOPE_LEN).slice() })) };
    }
  }
}

// ---------------------------------------------------------------- window.pearl JSON form

const hash32 = (s: unknown, what: string): Uint8Array => {
  if (s === undefined || s === null || s === "") return new Uint8Array(32);
  const t = String(s).trim().replace(/^0x/, "");
  if (!/^[0-9a-fA-F]{64}$/.test(t)) throw new Error(`${what} must be 64 hex`);
  return fromHex(t);
};
const key = (s: unknown, what: string): Uint8Array => {
  if (s === undefined || s === null || s === "") return new Uint8Array(32);
  try {
    return key32(String(s));
  } catch {
    throw new Error(`${what} must be a base58 account`);
  }
};
const int = (v: unknown, what: string, lo: number, hi: number): number => {
  const n = typeof v === "string" && v !== "" ? Number(v) : (v as number);
  if (!Number.isInteger(n) || n < lo || n > hi) throw new Error(`${what} must be an integer ${lo}..${hi}`);
  return n;
};
const amount = (v: unknown, what: string): bigint => {
  if (v === undefined || v === null || v === "") return 0n;
  const s = String(v);
  if (!/^\d+$/.test(s) || BigInt(s) >= 1n << 64n) throw new Error(`${what} must be a decimal string of base units`);
  return BigInt(s);
};
const list = (v: unknown, what: string, lo: number, hi: number): any[] => {
  if (!Array.isArray(v) || v.length < lo || v.length > hi) throw new Error(`${what} must hold ${lo}..${hi} entries`);
  return v;
};
const hexBytes = (s: unknown, what: string): Uint8Array => {
  const t = String(s ?? "").trim().replace(/^0x/, "");
  if (t.length % 2 !== 0 || /[^0-9a-fA-F]/.test(t)) throw new Error(`${what} must be hex`);
  return fromHex(t);
};

/** A DM-kind transaction in window.pearl JSON form as a DmBody; throws with the reason. */
export function dmBodyFromJson(t: any): DmBody {
  switch (t.kind) {
    case "SendMessage": {
      if (t.body !== undefined && t.body !== null && t.bodyHex !== undefined && t.bodyHex !== null) throw new Error("pass body or bodyHex, not both");
      const body = t.bodyHex !== undefined && t.bodyHex !== null ? hexBytes(t.bodyHex, "bodyHex") : utf8(String(t.body ?? ""));
      return {
        kind: "SendMessage",
        to: key(t.to, "to"),
        channel: hash32(t.channel, "channel"),
        mode: int(t.mode ?? 0, "mode", 0, 1),
        replyTo: hash32(t.replyTo, "replyTo"),
        pay: amount(t.pay, "pay"),
        reclaimAfter: int(t.reclaimAfter ?? 0, "reclaimAfter", 0, 0xffffffff),
        body,
        blobUri: String(t.blobUri ?? ""),
        blobHash: hash32(t.blobHash, "blobHash"),
        thoughtRef: String(t.thoughtRef ?? ""),
      };
    }
    case "ReclaimMessage":
      if (!t.messageId) throw new Error("messageId is required");
      return { kind: "ReclaimMessage", messageId: hash32(t.messageId, "messageId") };
    case "SetConsent":
      return {
        kind: "SetConsent",
        items: list(t.items, "items", 1, MAX_CONSENT_BATCH).map((x, i) => {
          if (!x?.peer) throw new Error(`items[${i}].peer is required`);
          return { peer: key(x.peer, `items[${i}].peer`), state: int(x.state, `items[${i}].state`, 0, 2) };
        }),
      };
    case "ChannelConfig":
      return {
        kind: "ChannelConfig",
        channelId: hash32(t.channelId, "channelId"),
        name: String(t.name ?? ""),
        about: String(t.about ?? ""),
        policy: int(t.policy ?? 0, "policy", 0, 2),
        joinPrice: amount(t.joinPrice, "joinPrice"),
        mode: int(t.mode ?? 0, "mode", 0, 1),
        admins: list(t.admins ?? [], "admins", 0, MAX_CHANNEL_ADMINS).map((a, i) => key(a, `admins[${i}]`)),
      };
    case "ChannelMember":
      if (!t.channel) throw new Error("channel is required");
      return { kind: "ChannelMember", channel: hash32(t.channel, "channel"), action: int(t.action ?? 0, "action", 0, 3), member: key(t.member, "member"), pay: amount(t.pay, "pay") };
    case "ChannelKey":
      if (!t.channel) throw new Error("channel is required");
      return {
        kind: "ChannelKey",
        channel: hash32(t.channel, "channel"),
        epoch: int(t.epoch, "epoch", 0, 0xffffffff),
        envelopes: list(t.envelopes, "envelopes", 1, MAX_KEY_ENVELOPES).map((x, i) => {
          const envelope = hexBytes(x?.envelopeHex, `envelopes[${i}].envelopeHex`);
          if (envelope.length !== KEY_ENVELOPE_LEN) throw new Error(`envelopes[${i}].envelopeHex must be ${KEY_ENVELOPE_LEN} bytes`);
          if (!x?.member) throw new Error(`envelopes[${i}].member is required`);
          return { member: key(x.member, `envelopes[${i}].member`), envelope };
        }),
      };
  }
  throw new Error(`not a DM kind: ${t.kind}`);
}

// ---------------------------------------------------------------- approval wording

export interface DmRow {
  label: string;
  value: string;
  full?: string;
}

const isZero = (b: Uint8Array) => b.every((x) => x === 0);
const shortHex = (b: Uint8Array) => {
  const h = hex(b);
  return `${h.slice(0, 8)}...${h.slice(-8)}`;
};
const shortKey = (b: Uint8Array) => {
  const s = b58(b);
  return `${s.slice(0, 6)}...${s.slice(-6)}`;
};

/** "about 6 hours (360 minutes)" style wording for a block count at the target block time. */
export function blocksInWords(blocks: number, targetBlockMs = DEFAULT_TARGET_BLOCK_MS): string {
  const minutes = Math.round((blocks * targetBlockMs) / 60_000);
  const secs = Math.round(targetBlockMs / 1000);
  const at = `at the chain's target block time of ${secs} s`;
  if (minutes < 120) return `about ${minutes} minute${minutes === 1 ? "" : "s"} ${at}`;
  const hours = minutes / 60;
  if (hours < 48) return `about ${Math.round(hours)} hours (${minutes} minutes) ${at}`;
  return `about ${Math.round(hours / 24)} days (${minutes} minutes) ${at}`;
}

export interface DmSummaryOptions {
  /** Signer of the tx, to word reclaims and self-references. */
  from?: Uint8Array;
  targetBlockMs?: number;
  /** Profile names to show next to keys (base58 -> name). */
  names?: Record<string, string>;
}

/** Plain words for an approval screen: who it goes to, plain or sealed, pay and its escrow. */
export function summarizeDm(b: DmBody, nat: (v: bigint) => string, opts: DmSummaryOptions = {}): { headline: string; rows: DmRow[]; warnings: string[] } {
  const rows: DmRow[] = [];
  const warnings: string[] = [];
  const who = (k: Uint8Array) => {
    const s = b58(k);
    const n = opts.names?.[s];
    return { value: n ? `${n} (${shortKey(k)})` : shortKey(k), full: s };
  };
  switch (b.kind) {
    case "SendMessage": {
      const sealed = b.mode === 1;
      const toChannel = !isZero(b.channel);
      const target = toChannel ? `channel ${shortHex(b.channel)}` : who(b.to).value;
      if (toChannel) rows.push({ label: "To channel", value: shortHex(b.channel), full: hex(b.channel) });
      else rows.push({ label: "To", ...who(b.to) });
      rows.push({ label: "Mode", value: sealed ? "sealed (end to end encrypted)" : "plain (public text)" });
      if (sealed) rows.push({ label: "Text", value: `sealed, ${b.body.length} bytes` });
      else {
        let text: string;
        try {
          text = new TextDecoder("utf-8", { fatal: true }).decode(b.body);
        } catch {
          text = `(${b.body.length} bytes that are not UTF-8)`;
          warnings.push("This plain message body is not valid UTF-8; the chain will refuse it.");
        }
        rows.push({ label: "Text", value: text || "(no text)" });
      }
      if (!isZero(b.replyTo)) rows.push({ label: "In reply to", value: shortHex(b.replyTo), full: hex(b.replyTo) });
      if (b.blobUri) rows.push({ label: "Attachment", value: b.blobUri });
      if (b.thoughtRef) rows.push({ label: "Paid thought", value: b.thoughtRef });
      if (b.pay > 0n && b.reclaimAfter === 0) {
        rows.push({ label: "Pay", value: `${nat(b.pay)} to the recipient now` });
        warnings.push(`Pays ${nat(b.pay)} to the recipient when the message lands. This pay is final, not refundable.`);
      } else if (b.pay > 0n) {
        const when = blocksInWords(b.reclaimAfter, opts.targetBlockMs);
        rows.push({ label: "Pay (escrowed)", value: nat(b.pay) }, { label: "Reply window", value: `${b.reclaimAfter} blocks, ${when}` });
        warnings.push(
          `Escrows ${nat(b.pay)}: it goes to the recipient if they reply within ${b.reclaimAfter} blocks. Refund to you after ${b.reclaimAfter} blocks, ${when}, if no reply (you send a Reclaim to take it back).`,
        );
      } else if (b.reclaimAfter > 0) warnings.push("A reply window without pay; the chain will refuse it.");
      if (!isZero(b.replyTo) && !toChannel) warnings.push("If this replies in time to a paid message they sent you, its escrowed pay is released to you.");
      if (sealed) warnings.push("Sealed: only you and the recipient can read the text. Sender, recipient, time, size and pay stay public on chain.");
      else warnings.push("Plain messages are public and permanent on the Cadre chain.");
      const lead = sealed ? "Send a sealed message" : "Send a message";
      return { headline: b.pay > 0n ? `${lead} to ${target} with ${nat(b.pay)}` : `${lead} to ${target}`, rows, warnings };
    }
    case "ReclaimMessage":
      rows.push({ label: "Message", value: shortHex(b.messageId), full: hex(b.messageId) });
      warnings.push(
        "If you sent this paid message and its reply window is over, the escrowed pay comes back to you. If it was sent to you, this declines it and returns the full pay to the sender.",
      );
      return { headline: "Reclaim or decline a paid message", rows, warnings };
    case "SetConsent": {
      const verb = ["Clear the choice for", "Allow", "Block"];
      for (const x of b.items) rows.push({ label: verb[x.state] ?? `state ${x.state}`, ...who(x.peer) });
      if (b.items.some((x) => x.state === 2)) warnings.push("Blocked accounts cannot message you; the chain refuses their messages.");
      if (b.items.length === 1) {
        const x = b.items[0];
        return { headline: `${verb[x.state] ?? "Set consent for"} ${who(x.peer).value}`, rows, warnings };
      }
      return { headline: `Change who can message you (${b.items.length} accounts)`, rows, warnings };
    }
    case "ChannelConfig": {
      const create = isZero(b.channelId);
      const policy = CHANNEL_POLICIES[b.policy] ?? `policy ${b.policy}`;
      rows.push({ label: "Name", value: b.name }, { label: "About", value: b.about || "none" });
      if (!create) rows.push({ label: "Channel", value: shortHex(b.channelId), full: hex(b.channelId) });
      rows.push(
        { label: "Who can join", value: policy === "paid" ? `anyone paying ${nat(b.joinPrice)} (to the creator)` : policy === "invite" ? "invited accounts only" : "anyone, free" },
        { label: "Mode", value: b.mode === 1 ? "sealed (members only can read)" : "plain (public)" },
        { label: "Admins", value: b.admins.length ? b.admins.map((a) => who(a).value).join(", ") : "none besides the creator" },
      );
      if (create) warnings.push("Any channel creation fee set on chain goes to the mining pool. The mode cannot be changed later.");
      if (b.mode === 1) warnings.push("Sealed channels use one group key per epoch (a sender-key scheme): no post-compromise security inside an epoch.");
      return { headline: create ? `Create the channel "${b.name}"` : `Update the channel "${b.name}"`, rows, warnings };
    }
    case "ChannelMember": {
      const ch = shortHex(b.channel);
      rows.push({ label: "Channel", value: ch, full: hex(b.channel) });
      const memberSet = !isZero(b.member) && !(opts.from && b58(opts.from) === b58(b.member));
      if (memberSet) rows.push({ label: "Member", ...who(b.member) });
      switch (b.action) {
        case 0:
          if (b.pay > 0n) {
            rows.push({ label: "Join price", value: nat(b.pay) });
            warnings.push(`Pays ${nat(b.pay)} to the channel creator. Not refundable.`);
            return { headline: `Join channel ${ch} for ${nat(b.pay)}`, rows, warnings };
          }
          return { headline: `Join channel ${ch}`, rows, warnings };
        case 1:
          return { headline: `Leave channel ${ch}`, rows, warnings };
        case 2:
          return { headline: `Invite ${who(b.member).value} to channel ${ch}`, rows, warnings };
        case 3:
          warnings.push("In a sealed channel an admin must post a new key epoch before anyone can write again.");
          return { headline: `Remove ${who(b.member).value} from channel ${ch}`, rows, warnings };
      }
      return { headline: `Channel action ${b.action}`, rows, warnings };
    }
    case "ChannelKey": {
      const ch = shortHex(b.channel);
      rows.push({ label: "Channel", value: ch, full: hex(b.channel) }, { label: "Key epoch", value: String(b.epoch) });
      for (const x of b.envelopes) rows.push({ label: "Key for", ...who(x.member) });
      warnings.push("Each member gets the epoch's group key sealed to their enc_key. No post-compromise security inside an epoch.");
      return { headline: `Share key epoch ${b.epoch} of channel ${ch} with ${b.envelopes.length} member${b.envelopes.length === 1 ? "" : "s"}`, rows, warnings };
    }
  }
}
