// Agent-chain transaction encoding, SPEC v2 section 2 and docs/RPC.md. Byte-checked
// against chain/agent-types/vectors/tx.json (test/tx-vectors.test.ts).
//
// Tx = version u8 (=1) | chain_id u32 | from [32] | nonce u64 | fee u64 | kind u8 | body | sig [64]
// sig = ed25519(varint(15) || "pearl-sol:tx:v1" || all preceding bytes)

import { Reader, concat, fromUtf8, u16, u32, u64, u8, utf8 } from "../core/bytes";
import { TX_NAMESPACE, blake3, signFramed, verifyRaw, frame } from "../core/crypto";

export const TX_VERSION = 1;

export const KIND = {
  Transfer: 0,
  RegisterAgent: 1,
  PostJob: 2,
  SubmitResult: 3,
  CompleteJob: 4,
  ExpireJob: 5,
  BridgeUnlock: 6,
  BridgeLock: 7,
  AdminConfig: 8,
  SetRoute: 9,
  ClearRoute: 10,
  RejectResult: 13,
} as const;
export type KindName = keyof typeof KIND;
export const KIND_NAME: Record<number, KindName> = Object.fromEntries(Object.entries(KIND).map(([k, v]) => [v, k])) as Record<number, KindName>;

export interface PostJobBody {
  m: number;
  n: number;
  k: number;
  config: Uint8Array; // 52
  hashA: Uint8Array; // 32
  hashB: Uint8Array; // 32
  dataUri: string;
  workerPay: bigint;
  protocolFee: bigint;
  deadlineHeight: bigint;
  maxBlocks: number;
  evaluators: Uint8Array[];
  threshold: number;
}

export interface SigPair {
  pubkey: Uint8Array;
  sig: Uint8Array;
}

export type TxBody =
  | { kind: "Transfer"; to: Uint8Array; amount: bigint }
  | { kind: "RegisterAgent"; name: string; uri: string }
  | ({ kind: "PostJob" } & PostJobBody)
  | { kind: "SubmitResult"; jobId: Uint8Array; resultHash: Uint8Array; uri: string }
  | { kind: "CompleteJob"; jobId: Uint8Array; sigs: SigPair[] }
  | { kind: "ExpireJob"; jobId: Uint8Array }
  | { kind: "BridgeUnlock"; solanaNonce: bigint; recipient: Uint8Array; amountSpl: bigint; jobOrder: PostJobBody | null; sigs: SigPair[] }
  | { kind: "BridgeLock"; solanaRecipient: Uint8Array; amount: bigint }
  | { kind: "AdminConfig"; key: number; value: Uint8Array }
  // Payout lien (SPEC 4.4): routeBps of the sender's matured coinbase, job pay and expiry
  // refunds go to solanaRecipient until untilHeight or cap; releaseKey all zero = cannot be
  // cleared early.
  | { kind: "SetRoute"; solanaRecipient: Uint8Array; routeBps: number; untilHeight: bigint; cap: bigint; releaseKey: Uint8Array }
  // Ends `account`'s lien early; sig = raw ed25519 by its release key over routeClearMessage.
  | { kind: "ClearRoute"; account: Uint8Array; routeNonce: bigint; sig: Uint8Array }
  // Job evaluators reject the current delivery (SPEC 4.1); sigs are raw ed25519 over rejectMessage.
  | { kind: "RejectResult"; jobId: Uint8Array; deliverer: Uint8Array; resultHash: Uint8Array; sigs: SigPair[] };

export interface UnsignedTx {
  chainId: number;
  from: Uint8Array;
  nonce: bigint;
  fee: bigint;
  body: TxBody;
}

export interface SignedTx extends UnsignedTx {
  sig: Uint8Array;
}

function fixed(b: Uint8Array, n: number, what: string) {
  if (b.length !== n) throw new Error(`${what} must be ${n} bytes, got ${b.length}`);
  return b;
}

function str8(s: string, what: string) {
  const b = utf8(s);
  if (b.length > 0xff) throw new Error(`${what} over 255 bytes`);
  return concat(u8(b.length), b);
}
function str16(s: string, what: string) {
  const b = utf8(s);
  if (b.length > 0xffff) throw new Error(`${what} over 65535 bytes`);
  return concat(u16(b.length), b);
}

function sigList(sigs: SigPair[]) {
  return concat(u8(sigs.length), ...sigs.flatMap((s) => [fixed(s.pubkey, 32, "sig pubkey"), fixed(s.sig, 64, "sig")]));
}

export function encodePostJob(p: PostJobBody): Uint8Array {
  return concat(
    u32(p.m), u32(p.n), u32(p.k),
    fixed(p.config, 52, "config"), fixed(p.hashA, 32, "hash_a"), fixed(p.hashB, 32, "hash_b"),
    str16(p.dataUri, "data_uri"),
    u64(p.workerPay), u64(p.protocolFee), u64(p.deadlineHeight), u32(p.maxBlocks),
    u8(p.evaluators.length), ...p.evaluators.map((e) => fixed(e, 32, "evaluator")),
    u8(p.threshold),
  );
}

export function encodeBody(b: TxBody): Uint8Array {
  switch (b.kind) {
    case "Transfer":
      return concat(fixed(b.to, 32, "to"), u64(b.amount));
    case "RegisterAgent":
      return concat(str8(b.name, "name"), str16(b.uri, "uri"));
    case "PostJob":
      return encodePostJob(b);
    case "SubmitResult":
      return concat(fixed(b.jobId, 32, "job_id"), fixed(b.resultHash, 32, "result_hash"), str16(b.uri, "uri"));
    case "CompleteJob":
      return concat(fixed(b.jobId, 32, "job_id"), sigList(b.sigs));
    case "ExpireJob":
      return fixed(b.jobId, 32, "job_id");
    case "BridgeUnlock":
      return concat(
        u64(b.solanaNonce), fixed(b.recipient, 32, "recipient"), u64(b.amountSpl),
        b.jobOrder ? concat(u8(1), encodePostJob(b.jobOrder)) : u8(0),
        sigList(b.sigs),
      );
    case "BridgeLock":
      return concat(fixed(b.solanaRecipient, 32, "solana_recipient"), u64(b.amount));
    case "AdminConfig":
      if (b.value.length > 0xffff) throw new Error("config value too long");
      return concat(u8(b.key), u16(b.value.length), b.value);
    case "SetRoute":
      if (!Number.isInteger(b.routeBps) || b.routeBps < 1 || b.routeBps > 10_000) throw new Error("route_bps must be 1..=10000");
      return concat(
        fixed(b.solanaRecipient, 32, "solana_recipient"), u16(b.routeBps), u64(b.untilHeight), u64(b.cap),
        fixed(b.releaseKey, 32, "release_key"),
      );
    case "ClearRoute":
      return concat(fixed(b.account, 32, "account"), u64(b.routeNonce), fixed(b.sig, 64, "release sig"));
    case "RejectResult":
      return concat(fixed(b.jobId, 32, "job_id"), fixed(b.deliverer, 32, "deliverer"), fixed(b.resultHash, 32, "result_hash"), sigList(b.sigs));
  }
}

/** Bytes the signature covers (everything before sig). */
export function signingBytes(t: UnsignedTx): Uint8Array {
  return concat(
    u8(TX_VERSION), u32(t.chainId), fixed(t.from, 32, "from"), u64(t.nonce), u64(t.fee),
    u8(KIND[t.body.kind]), encodeBody(t.body),
  );
}

/** What the ed25519 key actually signs: Commonware framing over the signing bytes. */
export const framedMessage = (t: UnsignedTx) => frame(TX_NAMESPACE, signingBytes(t));

export const encodeTx = (t: SignedTx) => concat(signingBytes(t), fixed(t.sig, 64, "sig"));

export function signTx(seed: Uint8Array, t: UnsignedTx): { tx: SignedTx; bytes: Uint8Array; hash: Uint8Array } {
  const sig = signFramed(seed, TX_NAMESPACE, signingBytes(t));
  const tx = { ...t, sig };
  const bytes = encodeTx(tx);
  return { tx, bytes, hash: txHash(bytes) };
}

/** tx_hash = blake3(encoded tx including sig). */
export const txHash = (bytes: Uint8Array) => blake3(bytes);

export const verifyTx = (t: SignedTx) => verifyRaw(t.sig, framedMessage(t), t.from);

// ---------------------------------------------------------------- decode

function readStr8(r: Reader) {
  return fromUtf8(r.take(r.u8()));
}
function readStr16(r: Reader) {
  return fromUtf8(r.take(r.u16()));
}
function readSigs(r: Reader): SigPair[] {
  const n = r.u8();
  return Array.from({ length: n }, () => ({ pubkey: r.take(32).slice(), sig: r.take(64).slice() }));
}

function readPostJob(r: Reader): PostJobBody {
  const m = r.u32(), n = r.u32(), k = r.u32();
  const config = r.take(52).slice(), hashA = r.take(32).slice(), hashB = r.take(32).slice();
  const dataUri = readStr16(r);
  const workerPay = r.u64(), protocolFee = r.u64(), deadlineHeight = r.u64(), maxBlocks = r.u32();
  const ec = r.u8();
  const evaluators = Array.from({ length: ec }, () => r.take(32).slice());
  const threshold = r.u8();
  return { m, n, k, config, hashA, hashB, dataUri, workerPay, protocolFee, deadlineHeight, maxBlocks, evaluators, threshold };
}

export function decodePostJob(b: Uint8Array): PostJobBody {
  const r = new Reader(b);
  const p = readPostJob(r);
  r.finish();
  return p;
}

function readBody(kind: number, r: Reader): TxBody {
  switch (KIND_NAME[kind]) {
    case "Transfer":
      return { kind: "Transfer", to: r.take(32).slice(), amount: r.u64() };
    case "RegisterAgent":
      return { kind: "RegisterAgent", name: readStr8(r), uri: readStr16(r) };
    case "PostJob":
      return { kind: "PostJob", ...readPostJob(r) };
    case "SubmitResult":
      return { kind: "SubmitResult", jobId: r.take(32).slice(), resultHash: r.take(32).slice(), uri: readStr16(r) };
    case "CompleteJob":
      return { kind: "CompleteJob", jobId: r.take(32).slice(), sigs: readSigs(r) };
    case "ExpireJob":
      return { kind: "ExpireJob", jobId: r.take(32).slice() };
    case "BridgeUnlock": {
      const solanaNonce = r.u64(), recipient = r.take(32).slice(), amountSpl = r.u64();
      const flag = r.u8();
      if (flag > 1) throw new Error("bad job_order flag");
      const jobOrder = flag === 1 ? readPostJob(r) : null;
      return { kind: "BridgeUnlock", solanaNonce, recipient, amountSpl, jobOrder, sigs: readSigs(r) };
    }
    case "BridgeLock":
      return { kind: "BridgeLock", solanaRecipient: r.take(32).slice(), amount: r.u64() };
    case "AdminConfig": {
      const key = r.u8();
      return { kind: "AdminConfig", key, value: r.take(r.u16()).slice() };
    }
    case "SetRoute":
      return {
        kind: "SetRoute", solanaRecipient: r.take(32).slice(), routeBps: r.u16(), untilHeight: r.u64(), cap: r.u64(),
        releaseKey: r.take(32).slice(),
      };
    case "ClearRoute":
      return { kind: "ClearRoute", account: r.take(32).slice(), routeNonce: r.u64(), sig: r.take(64).slice() };
    case "RejectResult":
      return { kind: "RejectResult", jobId: r.take(32).slice(), deliverer: r.take(32).slice(), resultHash: r.take(32).slice(), sigs: readSigs(r) };
    default:
      throw new Error(`unknown tx kind ${kind}`);
  }
}

/** Parses an unsigned tx (signing bytes, no sig). */
export function decodeUnsigned(b: Uint8Array): UnsignedTx {
  const r = new Reader(b);
  const t = readHead(r);
  r.finish();
  return t;
}

function readHead(r: Reader): UnsignedTx {
  const v = r.u8();
  if (v !== TX_VERSION) throw new Error(`unsupported tx version ${v}`);
  const chainId = r.u32();
  const from = r.take(32).slice();
  const nonce = r.u64();
  const fee = r.u64();
  const kind = r.u8();
  return { chainId, from, nonce, fee, body: readBody(kind, r) };
}

export function decodeTx(b: Uint8Array): SignedTx {
  const r = new Reader(b);
  const t = readHead(r);
  const sig = r.take(64).slice();
  r.finish();
  return { ...t, sig };
}

/** Accepts either signing bytes or a full signed tx; the trailing 64 bytes decide. */
export function decodeEither(b: Uint8Array): { tx: UnsignedTx; signed: boolean } {
  try {
    return { tx: decodeUnsigned(b), signed: false };
  } catch {
    return { tx: decodeTx(b), signed: true };
  }
}
