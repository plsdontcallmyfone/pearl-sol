// Agent-chain transaction encoding (SPEC section 2, docs/RPC.md, byte-checked
// against chain/agent-types/vectors/tx.json in test/agent-tx.test.ts).
//
//   Tx = version u8 (=1) || chain_id u32 || from [32] || nonce u64 || fee u64 || kind u8 || body || sig [64]
//   sig = ed25519 over varint(len NS) || NS || all preceding bytes, NS = b"pearl-sol:tx:v1"
//
// Only the kinds the relayer builds or the test node applies are encoded here.

import { blake3 } from "@noble/hashes/blake3";
import { Keypair, PublicKey } from "@solana/web3.js";
import { ed25519Sign, encodePostJobBody, u64le } from "./codec.ts";
import type { PostJobBody } from "./codec.ts";

export const TX_VERSION = 1;
export const TX_NS = Buffer.from("pearl-sol:tx:v1");
/** Localnet / devnet chain id (vectors). The relayer reads the real one from getChainInfo. */
export const DEFAULT_CHAIN_ID = 1347616769;

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
} as const;

export const ESCROW_ADDRESS = new PublicKey(blake3(Buffer.from("pearl-sol:escrow:v1")));
export const POOL_ADDRESS = new PublicKey(blake3(Buffer.from("pearl-sol:mining-pool:v1")));

export interface AttesterSig {
  pubkey: PublicKey;
  sig: Buffer;
}

export interface TxHeader {
  chainId: number;
  from: PublicKey;
  nonce: bigint;
  fee: bigint;
  kind: number;
}

/** Commonware `sign(ns, msg)` framing: varint(len ns) || ns || msg. */
export function framed(ns: Buffer, msg: Buffer): Buffer {
  const len: number[] = [];
  let n = ns.length;
  do {
    let b = n & 0x7f;
    n >>>= 7;
    if (n) b |= 0x80;
    len.push(b);
  } while (n);
  return Buffer.concat([Buffer.from(len), ns, msg]);
}

export function encodeUnsigned(h: TxHeader, body: Buffer): Buffer {
  const head = Buffer.alloc(1 + 4 + 32 + 8 + 8 + 1);
  head[0] = TX_VERSION;
  head.writeUInt32LE(h.chainId, 1);
  h.from.toBuffer().copy(head, 5);
  head.writeBigUInt64LE(h.nonce, 37);
  head.writeBigUInt64LE(h.fee, 45);
  head[53] = h.kind;
  return Buffer.concat([head, body]);
}

/** Signs and returns the full tx bytes. `kp.publicKey` must equal `h.from`. */
export function signTx(kp: Keypair, h: TxHeader, body: Buffer): Buffer {
  if (!kp.publicKey.equals(h.from)) throw new Error("tx signer is not tx.from");
  const unsigned = encodeUnsigned(h, body);
  return Buffer.concat([unsigned, ed25519Sign(kp, framed(TX_NS, unsigned))]);
}

/** `tx_hash = blake3(encoded tx including sig)`, lowercase hex. */
export const txHash = (tx: Buffer): string => Buffer.from(blake3(tx)).toString("hex");

function sigList(sigs: AttesterSig[]): Buffer {
  if (sigs.length > 255) throw new Error("too many attester sigs");
  return Buffer.concat([Buffer.from([sigs.length]), ...sigs.flatMap((s) => [s.pubkey.toBuffer(), s.sig])]);
}

// ---------- bodies ----------

export const transferBody = (to: PublicKey, amount: bigint): Buffer => Buffer.concat([to.toBuffer(), u64le(amount)]);

/** BridgeUnlock: `solana_nonce u64, recipient [32], amount_spl u64, job_order u8 (0/1), [PostJob body], count u8, (pubkey, sig)*`. */
export function bridgeUnlockBody(a: { solanaNonce: bigint; recipient: PublicKey; amountSpl: bigint;
  jobOrder: Buffer | null; sigs: AttesterSig[] }): Buffer {
  return Buffer.concat([
    u64le(a.solanaNonce), a.recipient.toBuffer(), u64le(a.amountSpl),
    a.jobOrder ? Buffer.concat([Buffer.from([1]), a.jobOrder]) : Buffer.from([0]),
    sigList(a.sigs),
  ]);
}

/** BridgeLock: `solana_recipient [32], amount u64` (chain units, positive multiple of 1000). */
export const bridgeLockBody = (solanaRecipient: PublicKey, amount: bigint): Buffer =>
  Buffer.concat([solanaRecipient.toBuffer(), u64le(amount)]);

export { encodePostJobBody };
export type { PostJobBody };

// ---------- decoding (used by the test node and the CLI) ----------

class Reader {
  at = 0;
  readonly b: Buffer;
  constructor(b: Buffer) {
    this.b = b;
  }
  take(n: number): Buffer {
    if (this.at + n > this.b.length) throw new Error("tx truncated");
    const s = this.b.subarray(this.at, this.at + n);
    this.at += n;
    return Buffer.from(s);
  }
  u8 = () => this.take(1)[0];
  u16 = () => this.take(2).readUInt16LE();
  u32 = () => this.take(4).readUInt32LE();
  u64 = () => this.take(8).readBigUInt64LE();
  key = () => new PublicKey(this.take(32));
}

function readPostJobBody(rd: Reader): PostJobBody {
  const m = rd.u32(), n = rd.u32(), k = rd.u32();
  const config = rd.take(52), hashA = rd.take(32), hashB = rd.take(32);
  const dataUri = rd.take(rd.u16());
  const workerPay = rd.u64(), protocolFee = rd.u64(), deadline = rd.u64(), maxBlocks = rd.u32();
  const evc = rd.u8();
  const evaluators = Array.from({ length: evc }, () => rd.key());
  const threshold = rd.u8();
  return { m, n, k, config, hashA, hashB, dataUri, workerPay, protocolFee, deadline, maxBlocks, evaluators, threshold };
}

export type DecodedBody =
  | { kind: 0; to: PublicKey; amount: bigint }
  | { kind: 6; solanaNonce: bigint; recipient: PublicKey; amountSpl: bigint; jobOrder: PostJobBody | null; sigs: AttesterSig[] }
  | { kind: 7; solanaRecipient: PublicKey; amount: bigint };

export interface DecodedTx extends TxHeader {
  body: DecodedBody;
  /** Bytes the signature covers (before framing). */
  unsigned: Buffer;
  sig: Buffer;
}

export function decodeTx(tx: Buffer): DecodedTx {
  if (tx.length < 54 + 64) throw new Error("tx too short");
  const rd = new Reader(tx.subarray(0, tx.length - 64));
  if (rd.u8() !== TX_VERSION) throw new Error("bad tx version");
  const chainId = rd.u32(), from = rd.key(), nonce = rd.u64(), fee = rd.u64(), kind = rd.u8();
  let body: DecodedBody;
  if (kind === KIND.Transfer) body = { kind, to: rd.key(), amount: rd.u64() };
  else if (kind === KIND.BridgeLock) body = { kind, solanaRecipient: rd.key(), amount: rd.u64() };
  else if (kind === KIND.BridgeUnlock) {
    const solanaNonce = rd.u64(), recipient = rd.key(), amountSpl = rd.u64();
    const flag = rd.u8();
    if (flag > 1) throw new Error("bad job_order flag");
    const jobOrder = flag ? readPostJobBody(rd) : null;
    const count = rd.u8();
    const sigs = Array.from({ length: count }, () => ({ pubkey: rd.key(), sig: rd.take(64) }));
    body = { kind, solanaNonce, recipient, amountSpl, jobOrder, sigs };
  } else throw new Error(`tx kind ${kind} not supported by this decoder`);
  if (rd.at !== rd.b.length) throw new Error("trailing bytes in tx body");
  return { chainId, from, nonce, fee, kind, body, unsigned: Buffer.from(tx.subarray(0, tx.length - 64)), sig: Buffer.from(tx.subarray(tx.length - 64)) };
}
