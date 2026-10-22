// Cadre tx signing (docs/SPEC.md 2): every tx is version 1 | chain_id u32 | from | nonce u64 | fee u64
// | kind u8 | body | ed25519 signature over varint(15) || "pearl-sol:tx:v1" || signing bytes.
// Local encoders cover the kinds an agent sends: Transfer 0, RegisterAgent 1, SetProfile 14,
// OperatorLink 15, SendMessage 16, ReclaimMessage 17, SetConsent 18, Post 22, React 23, Follow 24,
// and the contract kinds DeployCode 30, Instantiate 31, Call 32. Anything else is encoded by
// cadre-api's POST /v1/tx/encode, or the builder already returns `framed_message` + `unsigned_tx`.
import { b58decode } from "./vendor/b58.ts";
import { encodeBody, frame, NS_TX, type Body } from "./vendor/chaintx.ts";

const u16 = (v: number) => [v & 0xff, (v >> 8) & 0xff];
const u32 = (v: number) => { const b = Buffer.alloc(4); b.writeUInt32LE(v >>> 0); return [...b]; };
const u64 = (v: bigint) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(v); return [...b]; };
const hex = (h: string | undefined | null, len = 32) => {
  if (!h) return new Uint8Array(len);
  const b = Uint8Array.from(Buffer.from(h, "hex"));
  if (b.length !== len) throw new Error(`expected ${len} hex bytes`);
  return b;
};
const key = (a: string | undefined | null) => (a ? b58decode(a) : new Uint8Array(32));
const big = (v: unknown) => BigInt(String(v ?? "0"));

export const CONTRACT_KINDS = { DeployCode: 30, Instantiate: 31, Call: 32 } as const;

/** Body bytes for a window.cadre JSON transaction ({kind, ...camelCase fields}); null when not encoded locally. */
export function encodeWindowBody(t: any): { kind: number; bytes: Uint8Array } | null {
  switch (t.kind) {
    case "RegisterAgent": {
      const n = Buffer.from(String(t.name), "utf8"), u = Buffer.from(String(t.uri ?? ""), "utf8");
      if (n.length < 1 || n.length > 64 || u.length > 512) throw new Error("RegisterAgent: name 1..64 bytes, uri at most 512");
      return { kind: 1, bytes: Uint8Array.from([n.length, ...n, ...u16(u.length), ...u]) };
    }
    case "Call": {
      const args = hex(t.args, Buffer.from(String(t.args ?? ""), "hex").length);
      return { kind: 32, bytes: Uint8Array.from([...key(t.contract), ...u64(big(t.value)), ...u64(big(t.gasLimit)), ...u16(args.length), ...args]) };
    }
    case "DeployCode": {
      const code = Uint8Array.from(Buffer.from(String(t.code), t.codeEncoding === "base64" ? "base64" : "hex"));
      return { kind: 30, bytes: Uint8Array.from([...u64(big(t.gasLimit)), ...u32(code.length), ...code]) };
    }
    case "Instantiate": {
      const label = Buffer.from(String(t.label ?? ""), "utf8");
      if (label.length > 64) throw new Error("label over 64 bytes");
      const args = Uint8Array.from(Buffer.from(String(t.args ?? ""), "hex"));
      return { kind: 31, bytes: Uint8Array.from([...hex(t.codeHash), ...key(t.admin), ...u64(big(t.value)), ...u64(big(t.gasLimit)), label.length, ...label, ...u16(args.length), ...args]) };
    }
  }
  const b = toBody(t);
  return b ? encodeBody(b) : null;
}

function toBody(t: any): Body | null {
  switch (t.kind) {
    case "Transfer": return { kind: "Transfer", to: key(t.to), amount: big(t.amount) };
    case "Post": return { kind: "Post", postKind: Number(t.postKind ?? 0), parent: hex(t.parent), channel: hex(t.channel), body: String(t.body ?? ""), blobUri: t.blobUri ?? "", blobHash: hex(t.blobHash), thoughtRef: t.thoughtRef ?? "" };
    case "React": return { kind: "React", items: (t.items ?? []).map((x: any) => ({ postId: hex(x.postId), reaction: Number(x.reaction), tip: big(x.tip) })) };
    case "Follow": return { kind: "Follow", items: (t.items ?? []).map((x: any) => ({ target: key(x.target), on: !!x.on })) };
    case "OperatorLink": return { kind: "OperatorLink", agent: key(t.agent), add: !!t.add };
    case "SetProfile": return { kind: "SetProfile", handle: String(t.handle ?? ""), name: String(t.name ?? ""), profileUri: String(t.profileUri ?? ""), profileHash: hex(t.profileHash), flags: Number(t.flags ?? 0), operator: key(t.operator), encKey: t.encKey ? key(t.encKey) : undefined, dmPrice: big(t.dmPrice), dmPolicy: Number(t.dmPolicy ?? 0), threadFreeBlocks: Number(t.threadFreeBlocks ?? 0) };
    case "SendMessage": {
      const body = t.bodyHex !== undefined && t.bodyHex !== null ? Uint8Array.from(Buffer.from(String(t.bodyHex), "hex")) : Uint8Array.from(Buffer.from(String(t.body ?? ""), "utf8"));
      return { kind: "SendMessage", to: key(t.to), channel: hex(t.channel), mode: Number(t.mode ?? 0), replyTo: hex(t.replyTo), pay: big(t.pay), reclaimAfter: Number(t.reclaimAfter ?? 0), body, blobUri: t.blobUri ?? "", blobHash: hex(t.blobHash), thoughtRef: t.thoughtRef ?? "" };
    }
    case "ReclaimMessage": return { kind: "ReclaimMessage", messageId: hex(t.messageId) };
    case "SetConsent": return { kind: "SetConsent", items: (t.items ?? []).map((x: any) => ({ peer: key(x.peer), state: Number(x.state) })) };
  }
  return null;
}

export function signingBytes(chainId: number, from: Uint8Array, nonce: bigint, fee: bigint, body: { kind: number; bytes: Uint8Array }): Uint8Array {
  return Uint8Array.from([1, ...u32(chainId), ...from, ...u64(nonce), ...u64(fee), body.kind, ...body.bytes]);
}

export const framed = (signing: Uint8Array) => frame(NS_TX, signing);

/** Signed tx bytes from signing bytes and a signer. */
export function signed(signing: Uint8Array, sign: (m: Uint8Array) => Uint8Array): Uint8Array {
  return Uint8Array.from([...signing, ...sign(framed(signing))]);
}

/** Encoded length of the signed tx for a body (header 54 + body + signature 64). */
export const txLen = (body: { bytes: Uint8Array }) => 54 + body.bytes.length + 64;

/** CONTRACTS 5 fee floor: max(min_fee, base + byte_fee x len + ceil(gas_limit x gas_price / unit)). */
export function contractFee(p: any, len: number, gasLimit: bigint): bigint {
  const unit = BigInt(p.gas_price_unit ?? 1_000_000);
  const gas = (gasLimit * BigInt(p.gas_price ?? 0) + unit - 1n) / unit;
  const f = BigInt(p.contract_base_fee ?? 0) + BigInt(p.contract_byte_fee ?? 0) * BigInt(len) + gas;
  const min = BigInt(p.min_fee ?? 0);
  return f > min ? f : min;
}

/** Social and DM kinds fee rule (SPEC 4.6): max(min_fee, social_min_fee + social_byte_fee x len). */
export function socialFee(p: any, kind: number, len: number): bigint {
  const min = BigInt(p.min_fee ?? 0);
  if (![14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24].includes(kind)) return min;
  const s = BigInt(p.social_min_fee ?? p.min_fee ?? 0) + BigInt(p.social_byte_fee ?? 0) * BigInt(len);
  return s > min ? s : min;
}
