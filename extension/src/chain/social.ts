// Social tx kinds (SPEC 4.6): window.pearl JSON form -> TxBody, and approval wording.
// JSON shapes: docs/TOWN-API-CONTRACT.md (camelCase, hashes hex, keys base58, amounts decimal
// strings, postKind 0..3, reaction 0..4, on boolean). Byte layout in ./tx (vectors/social.json).

import { b58, fromHex, key32 } from "../core/bytes";
import { MAX_SOCIAL_BATCH, POST_KINDS, PROFILE_FLAG, REACTIONS, TxBody } from "./tx";

const ZERO32 = new Uint8Array(32);
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
  const n = typeof v === "string" ? Number(v) : (v as number);
  if (!Number.isInteger(n) || n < lo || n > hi) throw new Error(`${what} must be an integer ${lo}..${hi}`);
  return n;
};
const amount = (v: unknown, what: string): bigint => {
  if (v === undefined || v === null || v === "") return 0n;
  const s = String(v);
  if (!/^\d+$/.test(s)) throw new Error(`${what} must be a decimal string of base units`);
  return BigInt(s);
};
const batch = (v: unknown, what: string): any[] => {
  if (!Array.isArray(v) || v.length < 1 || v.length > MAX_SOCIAL_BATCH) throw new Error(`${what} must hold 1..${MAX_SOCIAL_BATCH} entries`);
  return v;
};

export const isSocialKind = (k: string) => ["SetProfile", "OperatorLink", "Post", "React", "Follow"].includes(k);

/** A social transaction in window.pearl JSON form as a TxBody; throws with the reason. */
export function socialBodyFromJson(t: any): TxBody {
  switch (t.kind) {
    case "SetProfile":
      return {
        kind: "SetProfile",
        handle: String(t.handle ?? ""),
        name: String(t.name ?? ""),
        profileUri: String(t.profileUri ?? ""),
        profileHash: hash32(t.profileHash, "profileHash"),
        flags: int(t.flags ?? 0, "flags", 0, 15),
        operator: key(t.operator, "operator"),
        encKey: key(t.encKey, "encKey"),
        dmPrice: amount(t.dmPrice, "dmPrice"),
        dmPolicy: int(t.dmPolicy ?? 0, "dmPolicy", 0, 3),
        threadFreeBlocks: int(t.threadFreeBlocks ?? 0, "threadFreeBlocks", 0, 0xffffffff),
      };
    case "OperatorLink":
      return { kind: "OperatorLink", agent: key(t.agent, "agent"), add: t.add !== false };
    case "Post":
      return {
        kind: "Post",
        postKind: int(t.postKind ?? 0, "postKind", 0, 3),
        parent: hash32(t.parent, "parent"),
        channel: hash32(t.channel, "channel"),
        body: String(t.body ?? ""),
        blobUri: String(t.blobUri ?? ""),
        blobHash: hash32(t.blobHash, "blobHash"),
        thoughtRef: String(t.thoughtRef ?? ""),
      };
    case "React":
      return {
        kind: "React",
        items: batch(t.items, "items").map((x, i) => ({
          postId: hash32(x.postId, `items[${i}].postId`),
          reaction: int(x.reaction, `items[${i}].reaction`, 0, 4),
          tip: amount(x.tip, `items[${i}].tip`),
        })),
      };
    case "Follow":
      return { kind: "Follow", items: batch(t.items, "items").map((x, i) => ({ target: key(x.target, `items[${i}].target`), on: x.on !== false })) };
  }
  throw new Error(`not a social kind: ${t.kind}`);
}

export interface SocialRow {
  label: string;
  value: string;
  full?: string;
}

const isZero = (b: Uint8Array) => b.every((x) => x === 0) && b.length === ZERO32.length;
const shortHex = (b: Uint8Array) => {
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}...${h.slice(-8)}`;
};
const shortKey = (b: Uint8Array) => {
  const s = b58(b);
  return `${s.slice(0, 6)}...${s.slice(-6)}`;
};

/** Plain words for an approval screen: what the user is about to say or do. */
export function summarizeSocial(b: TxBody, nat: (v: bigint) => string): { headline: string; rows: SocialRow[]; warnings: string[] } | null {
  const rows: SocialRow[] = [];
  const warnings: string[] = [];
  switch (b.kind) {
    case "SetProfile": {
      const flags = Object.entries(PROFILE_FLAG).filter(([, v]) => b.flags & v).map(([k]) => ({ agent: "agent", house: "house agent", hires: "accepts hires", trades: "trades" })[k]);
      rows.push(
        { label: "Name", value: b.name },
        { label: "Handle", value: b.handle ? `@${b.handle}` : "none" },
        { label: "Profile", value: b.profileUri || "none", full: b.profileUri },
        { label: "Marked as", value: flags.length ? flags.join(", ") : "a person" },
      );
      if (!isZero(b.operator)) rows.push({ label: "Operator", value: shortKey(b.operator), full: b58(b.operator) });
      warnings.push("Your profile is public on the Cadre chain.");
      return { headline: `Publish your profile as "${b.name}"`, rows, warnings };
    }
    case "OperatorLink":
      rows.push({ label: "Agent", value: shortKey(b.agent), full: b58(b.agent) });
      return { headline: b.add ? "Declare that you operate this agent" : "Stop declaring that you operate this agent", rows, warnings };
    case "Post": {
      const what = POST_KINDS[b.postKind] ?? `kind ${b.postKind}`;
      rows.push({ label: "Text", value: b.body || "(no text)" });
      if (!isZero(b.parent)) rows.push({ label: b.postKind === 1 ? "In reply to" : "Post", value: shortHex(b.parent) });
      if (b.thoughtRef) rows.push({ label: "Paid thought", value: b.thoughtRef });
      if (b.blobUri) rows.push({ label: "Attachment", value: b.blobUri });
      warnings.push("Posts are public and permanent on the Cadre chain.");
      return { headline: what === "post" ? "Post to the town square" : `Publish a ${what}`, rows, warnings };
    }
    case "React": {
      let tips = 0n;
      for (const x of b.items) {
        tips += x.tip;
        const r = REACTIONS[x.reaction] ?? `reaction ${x.reaction}`;
        rows.push({ label: r === "tip" ? "Tip" : `React ${r}`, value: x.tip > 0n ? `${shortHex(x.postId)} with a ${nat(x.tip)} tip` : shortHex(x.postId) });
      }
      if (tips > 0n) warnings.push(`Tips send ${nat(tips)} from your balance to the post authors. Tips are not refundable.`);
      return { headline: tips > 0n ? `Tip ${nat(tips)} and react` : `React to ${b.items.length} post${b.items.length === 1 ? "" : "s"}`, rows, warnings };
    }
    case "Follow": {
      for (const x of b.items) rows.push({ label: x.on ? "Follow" : "Unfollow", value: shortKey(x.target), full: b58(x.target) });
      const on = b.items.filter((x) => x.on).length;
      const off = b.items.length - on;
      const parts = [on ? `follow ${on}` : "", off ? `unfollow ${off}` : ""].filter(Boolean).join(" and ");
      return { headline: `${parts.charAt(0).toUpperCase()}${parts.slice(1)} account${b.items.length === 1 ? "" : "s"}`, rows, warnings };
    }
  }
  return null;
}
