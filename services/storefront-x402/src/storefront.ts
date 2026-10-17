// Client side of token/programs/storefront: PDAs, instruction encoders and raw
// account readers. Hand-encoded (Borsh) against the program's IDL so the
// service needs no Anchor dependency; account layouts are the fixed offsets
// published in storefront::layout.

import { createHash } from "node:crypto";
import { PublicKey, SystemProgram, TransactionInstruction } from "@solana/web3.js";

export const STOREFRONT_ID = new PublicKey("DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr");
export const BRIDGE_ID = new PublicKey("7LgyoRiKjNXD1AkMQUePb7TzTvRewEXKKLVzXB5vEjc8");
export const PUMP = new PublicKey("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
export const AMM = new PublicKey("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
export const PUMP_FEES = new PublicKey("pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ");
export const WSOL = new PublicKey("So11111111111111111111111111111111111111112");
export const TOKEN = new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
export const TOKEN_2022 = new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
export const ATA_PROGRAM = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
export const IX_SYSVAR = new PublicKey("Sysvar1nstructions1111111111111111111111111");

export const STATUS = { 1: "pending", 2: "finalized", 3: "expired" } as Record<number, string>;
export const MAX_URI = 200;
export const MAX_EVALUATORS = 8;

const enc = (s: string) => Buffer.from(s);
export const sha256 = (...parts: Uint8Array[]) => createHash("sha256").update(Buffer.concat(parts)).digest();
const disc = (name: string) => sha256(enc(`global:${name}`)).subarray(0, 8);
export const u16 = (n: number) => { const b = Buffer.alloc(2); b.writeUInt16LE(n); return b; };
export const u32 = (n: number) => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
export const u64 = (n: bigint | number) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };

export const pda = (seeds: (Buffer | Uint8Array)[], program: PublicKey) => PublicKey.findProgramAddressSync(seeds, program)[0];
export const ata = (owner: PublicKey, mint: PublicKey, tokenProgram: PublicKey) =>
  pda([owner.toBuffer(), tokenProgram.toBuffer(), mint.toBuffer()], ATA_PROGRAM);

export function addresses(storefront = STOREFRONT_ID, bridge = BRIDGE_ID) {
  return {
    storefront,
    bridge,
    config: pda([enc("config")], storefront),
    trader: pda([enc("trader")], storefront),
    bridgeConfig: pda([enc("config")], bridge),
  };
}
export const orderPda = (nonce: bigint, storefront = STOREFRONT_ID) => pda([enc("order"), u64(nonce)], storefront);
export const draftPda = (buyer: PublicKey, id: bigint, storefront = STOREFRONT_ID) => pda([enc("draft"), buyer.toBuffer(), u64(id)], storefront);
export const depositPda = (nonce: bigint, bridge = BRIDGE_ID) => pda([enc("deposit"), u64(nonce)], bridge);
export const orderJobId = (nonce: bigint) => sha256(enc("pearl-sol:order-job:v1"), u64(nonce));

// ---------------------------------------------------------------- job spec

export interface JobSpec {
  m: number;
  n: number;
  k: number;
  config: Buffer; // 52 bytes
  hashA: Buffer; // 32
  hashB: Buffer; // 32
  dataUri: string;
  deadlineBlocks: bigint;
  maxBlocks: number;
  evaluators: PublicKey[];
  threshold: number;
}

function encodeSpec(s: JobSpec): Buffer {
  if (s.config.length !== 52 || s.hashA.length !== 32 || s.hashB.length !== 32) throw new Error("bad spec byte lengths");
  const uri = enc(s.dataUri);
  return Buffer.concat([
    u32(s.m), u32(s.n), u32(s.k), s.config, s.hashA, s.hashB,
    u32(uri.length), uri, u64(s.deadlineBlocks), u32(s.maxBlocks),
    u32(s.evaluators.length), ...s.evaluators.map((e) => e.toBuffer()), Buffer.from([s.threshold]),
  ]);
}

/** SPEC 4.1 PostJob body the chain runs for an order (deadline relative). */
export function postJobBody(s: JobSpec, workerPay: bigint, protocolFee: bigint): Buffer {
  const uri = enc(s.dataUri);
  return Buffer.concat([
    u32(s.m), u32(s.n), u32(s.k), s.config, s.hashA, s.hashB, u16(uri.length), uri,
    u64(workerPay), u64(protocolFee), u64(s.deadlineBlocks), u32(s.maxBlocks),
    Buffer.from([s.evaluators.length]), ...s.evaluators.map((e) => e.toBuffer()), Buffer.from([s.threshold]),
  ]);
}

// ---------------------------------------------------------------- instructions

const meta = (pubkey: PublicKey, isWritable = false, isSigner = false) => ({ pubkey, isWritable, isSigner });

export function openOrderIx(buyer: PublicKey, draftId: bigint, recipient: PublicKey, spec: JobSpec, a = addresses()): TransactionInstruction {
  return new TransactionInstruction({
    programId: a.storefront,
    keys: [meta(buyer, true, true), meta(a.config), meta(draftPda(buyer, draftId, a.storefront), true), meta(SystemProgram.programId)],
    data: Buffer.concat([disc("open_order"), u64(draftId), recipient.toBuffer(), encodeSpec(spec)]),
  });
}

export function cancelDraftIx(buyer: PublicKey, draftId: bigint, a = addresses()): TransactionInstruction {
  return new TransactionInstruction({
    programId: a.storefront,
    keys: [meta(buyer, true, true), meta(draftPda(buyer, draftId, a.storefront), true)],
    data: disc("cancel_draft"),
  });
}

export interface PumpAccounts {
  mint: PublicKey;
  /** pump Global buyback_fee_recipients[i] (curve) */
  curveBuyback: PublicKey;
  /** PumpSwap GlobalConfig buyback_fee_recipients[i] (pool; its wSOL ATA is passed) */
  ammBuyback: PublicKey;
  poolBaseVault?: PublicKey;
  poolQuoteVault?: PublicKey;
}

export const bondingCurvePda = (mint: PublicKey) => pda([enc("bonding-curve"), mint.toBuffer()], PUMP);
export function canonicalPool(mint: PublicKey): PublicKey {
  const auth = pda([enc("pool-authority"), mint.toBuffer()], PUMP);
  return pda([enc("pool"), u16(0), auth.toBuffer(), mint.toBuffer(), WSOL.toBuffer()], AMM);
}

function bridgeMetas(a: ReturnType<typeof addresses>, mint: PublicKey, nonce: bigint) {
  return [meta(a.bridgeConfig, true), meta(ata(a.bridgeConfig, mint, TOKEN_2022), true), meta(depositPda(nonce, a.bridge), true), meta(a.bridge)];
}

export function buyJobIx(buyer: PublicKey, draftId: bigint, nonce: bigint, solIn: bigint, minOut: bigint, p: PumpAccounts, a = addresses()): TransactionInstruction {
  const bc = bondingCurvePda(p.mint);
  return new TransactionInstruction({
    programId: a.storefront,
    keys: [
      meta(buyer, true, true), meta(a.config, true), meta(draftPda(buyer, draftId, a.storefront), true), meta(orderPda(nonce, a.storefront), true),
      meta(a.trader, true), meta(p.mint), meta(ata(a.trader, p.mint, TOKEN_2022), true), meta(ata(a.trader, WSOL, TOKEN), true),
      meta(pda([enc("global")], PUMP)), meta(bc, true), meta(ata(bc, p.mint, TOKEN_2022), true), meta(ata(bc, WSOL, TOKEN), true),
      meta(pda([enc("user_volume_accumulator"), a.trader.toBuffer()], PUMP), true), meta(pda([enc("fee_config"), PUMP.toBuffer()], PUMP_FEES)),
      meta(p.curveBuyback, true), meta(WSOL), meta(TOKEN), meta(TOKEN_2022), meta(ATA_PROGRAM), meta(SystemProgram.programId),
      meta(pda([enc("__event_authority")], PUMP)), meta(PUMP), ...bridgeMetas(a, p.mint, nonce),
    ],
    data: Buffer.concat([disc("buy_job"), u64(nonce), u64(solIn), u64(minOut)]),
  });
}

export function buyJobAmmIx(buyer: PublicKey, draftId: bigint, nonce: bigint, solIn: bigint, minOut: bigint, p: PumpAccounts, a = addresses()): TransactionInstruction {
  if (!p.poolBaseVault || !p.poolQuoteVault) throw new Error("pool vaults required");
  return new TransactionInstruction({
    programId: a.storefront,
    keys: [
      meta(buyer, true, true), meta(a.config, true), meta(draftPda(buyer, draftId, a.storefront), true), meta(orderPda(nonce, a.storefront), true),
      meta(a.trader, true), meta(p.mint), meta(ata(a.trader, p.mint, TOKEN_2022), true), meta(ata(a.trader, WSOL, TOKEN), true),
      meta(canonicalPool(p.mint), true), meta(p.poolBaseVault, true), meta(p.poolQuoteVault, true), meta(pda([enc("global_config")], AMM)),
      meta(pda([enc("user_volume_accumulator"), a.trader.toBuffer()], AMM), true), meta(pda([enc("fee_config"), AMM.toBuffer()], PUMP_FEES)),
      meta(ata(p.ammBuyback, WSOL, TOKEN), true), meta(WSOL), meta(TOKEN), meta(TOKEN_2022), meta(ATA_PROGRAM), meta(SystemProgram.programId),
      meta(pda([enc("__event_authority")], AMM)), meta(AMM), ...bridgeMetas(a, p.mint, nonce),
    ],
    data: Buffer.concat([disc("buy_job_amm"), u64(nonce), u64(solIn), u64(minOut)]),
  });
}

// ---------------------------------------------------------------- readers

const pk = (d: Uint8Array, o: number) => new PublicKey(d.subarray(o, o + 32));
const rd64 = (d: Uint8Array, o: number) => Buffer.from(d.buffer, d.byteOffset, d.byteLength).readBigUInt64LE(o);
const rd32 = (d: Uint8Array, o: number) => Buffer.from(d.buffer, d.byteOffset, d.byteLength).readUInt32LE(o);
const rd16 = (d: Uint8Array, o: number) => Buffer.from(d.buffer, d.byteOffset, d.byteLength).readUInt16LE(o);

export interface StoreConfig {
  admin: PublicKey;
  mint: PublicKey;
  bridgeProgram: PublicKey;
  paused: boolean;
  protocolFeeBps: number;
  maxSlippageBps: number;
  minSolIn: bigint;
  maxSolIn: bigint;
  maxDim: number;
  maxBlocksCap: number;
  maxDeadlineBlocks: bigint;
  threshold: number;
  attesters: PublicKey[];
}

export function decodeConfig(d: Uint8Array): StoreConfig {
  const n = rd32(d, 142);
  return {
    admin: pk(d, 8), mint: pk(d, 40), bridgeProgram: pk(d, 72), paused: d[104] !== 0,
    protocolFeeBps: rd16(d, 105), maxSlippageBps: rd16(d, 107), minSolIn: rd64(d, 109), maxSolIn: rd64(d, 117),
    maxDim: rd32(d, 125), maxBlocksCap: rd32(d, 129), maxDeadlineBlocks: rd64(d, 133), threshold: d[141],
    attesters: Array.from({ length: n }, (_, i) => pk(d, 146 + 32 * i)),
  };
}

/** Bridge Config.deposit_nonce (disc + admin, mint, vault, token_program). */
export const bridgeNonce = (d: Uint8Array) => rd64(d, 8 + 32 * 4);

export const ORDER_LEN = 1070;
const ORDER_DISC = sha256(enc("account:Order")).subarray(0, 8);

export interface OrderView {
  status: string;
  venue: "curve" | "pumpswap";
  nonce: bigint;
  jobId: string;
  buyer: string;
  recipient: string;
  bodyHash: string;
  solSpent: bigint;
  tokens: bigint;
  workerPay: bigint;
  protocolFee: bigint;
  createdSlot: bigint;
  createdTs: bigint;
  closedSlot: bigint;
  result: { deliverer: string; resultHash: string; resultUri: string } | null;
  spec: { m: number; n: number; k: number; config: string; hashA: string; hashB: string; deadlineBlocks: bigint; maxBlocks: number; threshold: number; evaluators: string[]; dataUri: string };
}

export function decodeOrder(d: Uint8Array): OrderView {
  if (d.length < ORDER_LEN || !Buffer.from(d.subarray(0, 8)).equals(ORDER_DISC)) throw new Error("not an Order account");
  const hex = (o: number, l: number) => Buffer.from(d.subarray(o, o + l)).toString("hex");
  const S = 470;
  const status = STATUS[d[9]] ?? `unknown(${d[9]})`;
  return {
    status,
    venue: d[10] === 1 ? "pumpswap" : "curve",
    nonce: rd64(d, 12),
    jobId: hex(20, 32),
    buyer: pk(d, 52).toBase58(),
    recipient: pk(d, 84).toBase58(),
    bodyHash: hex(116, 32),
    solSpent: rd64(d, 148),
    tokens: rd64(d, 156),
    workerPay: rd64(d, 164),
    protocolFee: rd64(d, 172),
    createdSlot: rd64(d, 180),
    createdTs: rd64(d, 188),
    closedSlot: rd64(d, 260),
    result: d[9] === 2
      ? { deliverer: pk(d, 196).toBase58(), resultHash: hex(228, 32), resultUri: Buffer.from(d.subarray(270, 270 + rd16(d, 268))).toString() }
      : null,
    spec: {
      m: rd32(d, S), n: rd32(d, S + 4), k: rd32(d, S + 8), config: hex(S + 12, 52), hashA: hex(S + 64, 32), hashB: hex(S + 96, 32),
      deadlineBlocks: rd64(d, S + 128), maxBlocks: rd32(d, S + 136), threshold: d[S + 140],
      evaluators: Array.from({ length: d[S + 141] }, (_, i) => pk(d, S + 142 + 32 * i).toBase58()),
      dataUri: Buffer.from(d.subarray(S + 400, S + 400 + rd16(d, S + 398))).toString(),
    },
  };
}

// pump readers (offsets as in token/programs/reserve/src/pump.rs)
export function decodeCurve(d: Uint8Array) {
  return { virtualToken: rd64(d, 8), virtualQuote: rd64(d, 16), complete: d[48] !== 0 };
}
export function decodePool(d: Uint8Array) {
  const b = Buffer.from(d.buffer, d.byteOffset, d.byteLength);
  const vq = d.length >= 261 ? (b.readBigInt64LE(253) << 64n) + b.readBigUInt64LE(245) : 0n;
  return {
    baseVault: pk(d, 139), quoteVault: pk(d, 171),
    virtualQuote: vq < 0n ? 0n : vq,
    protocolFees: rd64(d, 271), creatorFees: rd64(d, 279),
  };
}
/** buyback_fee_recipients[0] in pump Global (offset 741) / PumpSwap GlobalConfig (643), checked against the pump SDKs in the tests. */
export const pumpGlobalBuyback = (d: Uint8Array, i = 0) => pk(d, 741 + 32 * i);
export const ammGlobalBuyback = (d: Uint8Array, i = 0) => pk(d, 643 + 32 * i);
export const tokenAmount = (d: Uint8Array) => rd64(d, 64);

export function cpQuote(baseReserve: bigint, quoteReserve: bigint, quoteIn: bigint): bigint {
  const den = quoteReserve + quoteIn;
  return den === 0n ? 0n : (baseReserve * quoteIn) / den;
}
