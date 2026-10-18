// Byte-level codec for the Solana side: the bridge program (token/programs/bridge,
// unchanged) and the storefront program (token/programs/storefront): PDAs,
// instruction builders, account decoders, signed messages and the ed25519
// precompile instruction. Hand-encoded so the relayer does not depend on IDL files.
//
// The agent chain's own transaction encoding lives in src/agent-tx.ts.

import { createHash, createPrivateKey, sign as edSign } from "node:crypto";
import { Keypair, PublicKey, SystemProgram, TransactionInstruction } from "@solana/web3.js";

export const BRIDGE_PROGRAM_ID = new PublicKey("7LgyoRiKjNXD1AkMQUePb7TzTvRewEXKKLVzXB5vEjc8");
export const STOREFRONT_PROGRAM_ID = new PublicKey("DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr");
export const TOKEN_2022_PROGRAM_ID = new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
export const ATA_PROGRAM_ID = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
export const ED25519_PROGRAM_ID = new PublicKey("Ed25519SigVerify111111111111111111111111111");
export const INSTRUCTIONS_SYSVAR_ID = new PublicKey("Sysvar1nstructions1111111111111111111111111");

/** Bridge `Processed` PDA chain tag for releases (withdrawals from the agent chain). */
export const CHAIN_AGENT = 1;
/** Bridge `Processed` PDA chain tag for refund records (Solana deposits refunded on Solana). */
export const CHAIN_TOKEN = 0;
/** 1 SPL base unit (6 dp) = 1000 agent-chain base units (9 dp). */
export const UNITS_PER_SPL = 1000n;
/** Whole agent-chain supply, all in the bridge escrow at genesis: 1B x 1e9. */
export const TOTAL_SUPPLY = 1_000_000_000_000_000_000n;

export const DEPOSIT_DOMAIN = Buffer.from("pearl-sol:deposit:v1");
export const DEPOSIT_ORDER_DOMAIN = Buffer.from("pearl-sol:deposit-order:v1");
export const WITHDRAW_DOMAIN = Buffer.from("pearl-sol:withdraw:v1");
export const RESULT_DOMAIN = Buffer.from("pearl-sol:result:v1");
export const EXPIRED_DOMAIN = Buffer.from("pearl-sol:expired:v1");
export const ORDER_JOB_DOMAIN = Buffer.from("pearl-sol:order-job:v1");
export const REFUND_DOMAIN = Buffer.from("pearl-sol:refund:v1");

// ---------- primitives ----------

export function u64le(n: bigint): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(n);
  return b;
}
export function u32le(n: number): Buffer {
  const b = Buffer.alloc(4);
  b.writeUInt32LE(n);
  return b;
}
export function u16le(n: number): Buffer {
  const b = Buffer.alloc(2);
  b.writeUInt16LE(n);
  return b;
}

export function sha256(...parts: (Buffer | Uint8Array | string)[]): Buffer {
  const h = createHash("sha256");
  for (const p of parts) h.update(p);
  return h.digest();
}
export const ixDisc = (name: string): Buffer => sha256(`global:${name}`).subarray(0, 8);
export const accDisc = (name: string): Buffer => sha256(`account:${name}`).subarray(0, 8);

const pda = (seeds: (Buffer | Uint8Array)[], program: PublicKey): PublicKey =>
  PublicKey.findProgramAddressSync(seeds, program)[0];

// ---------- PDAs ----------

export const bridgeConfigPda = (): PublicKey => pda([Buffer.from("config")], BRIDGE_PROGRAM_ID);
export const depositPda = (nonce: bigint): PublicKey => pda([Buffer.from("deposit"), u64le(nonce)], BRIDGE_PROGRAM_ID);
export const bridgeProcessedPda = (nonce: bigint): PublicKey =>
  pda([Buffer.from("processed"), Buffer.from([CHAIN_AGENT]), u64le(nonce)], BRIDGE_PROGRAM_ID);
/** Refund record of Solana deposit `nonce` (bridge `refund_deposit`). */
export const bridgeRefundPda = (nonce: bigint): PublicKey =>
  pda([Buffer.from("processed"), Buffer.from([CHAIN_TOKEN]), u64le(nonce)], BRIDGE_PROGRAM_ID);

export const storefrontConfigPda = (program = STOREFRONT_PROGRAM_ID): PublicKey => pda([Buffer.from("config")], program);
/** The storefront's trader PDA: a bridge Deposit with this sender is an order. */
export const traderPda = (program = STOREFRONT_PROGRAM_ID): PublicKey => pda([Buffer.from("trader")], program);
export const orderPda = (nonce: bigint, program = STOREFRONT_PROGRAM_ID): PublicKey =>
  pda([Buffer.from("order"), u64le(nonce)], program);

export function ata(owner: PublicKey, mint: PublicKey, tokenProgram = TOKEN_2022_PROGRAM_ID): PublicKey {
  return pda([owner.toBuffer(), tokenProgram.toBuffer(), mint.toBuffer()], ATA_PROGRAM_ID);
}
export const vaultAta = (mint: PublicKey, tokenProgram = TOKEN_2022_PROGRAM_ID): PublicKey =>
  ata(bridgeConfigPda(), mint, tokenProgram);

// ---------- signed messages (RAW ed25519, no framing) ----------

/** `domain || nonce u64 LE || recipient [32] || amount_spl u64 LE` */
export function bridgeMessage(domain: Buffer, nonce: bigint, recipient: PublicKey, amountSpl: bigint): Buffer {
  return Buffer.concat([domain, u64le(nonce), recipient.toBuffer(), u64le(amountSpl)]);
}
/** Plain deposit unlock (agent chain BridgeUnlock, SPEC 4.3). */
export const depositMessage = (nonce: bigint, recipient: PublicKey, amountSpl: bigint): Buffer =>
  bridgeMessage(DEPOSIT_DOMAIN, nonce, recipient, amountSpl);
/** Storefront order unlock: plain fields plus sha256(PostJob body). */
export const depositOrderMessage = (nonce: bigint, recipient: PublicKey, amountSpl: bigint, bodyHash: Buffer): Buffer =>
  Buffer.concat([bridgeMessage(DEPOSIT_ORDER_DOMAIN, nonce, recipient, amountSpl), bodyHash]);
/** Bridge release on Solana (unchanged). */
export const withdrawMessage = (nonce: bigint, recipient: PublicKey, amountSpl: bigint): Buffer =>
  bridgeMessage(WITHDRAW_DOMAIN, nonce, recipient, amountSpl);
/** Bridge refund_deposit: `pearl-sol:refund:v1 || nonce || buyer || amount_spl` (all of the deposit). */
export const refundMessage = (nonce: bigint, buyer: PublicKey, amountSpl: bigint): Buffer =>
  bridgeMessage(REFUND_DOMAIN, nonce, buyer, amountSpl);
export const resultMessage = (jobId: Buffer, deliverer: PublicKey, resultHash: Buffer): Buffer =>
  Buffer.concat([RESULT_DOMAIN, jobId, deliverer.toBuffer(), resultHash]);
export const expiredMessage = (jobId: Buffer): Buffer => Buffer.concat([EXPIRED_DOMAIN, jobId]);
/** Order job id on the chain: sha256("pearl-sol:order-job:v1" || nonce LE). */
export const orderJobId = (nonce: bigint): Buffer => sha256(ORDER_JOB_DOMAIN, u64le(nonce));

// ---------- ed25519 ----------

const PKCS8_ED25519_PREFIX = Buffer.from("302e020100300506032b657004220420", "hex");

export function ed25519Sign(kp: Keypair, msg: Buffer): Buffer {
  const seed = Buffer.from(kp.secretKey.subarray(0, 32));
  const key = createPrivateKey({ key: Buffer.concat([PKCS8_ED25519_PREFIX, seed]), format: "der", type: "pkcs8" });
  return edSign(null, msg, key);
}

export interface SignedEntry {
  pubkey: PublicKey;
  signature: Buffer;
  message: Buffer;
}

/** One ed25519 precompile instruction carrying every entry, offsets pointing into itself. */
export function ed25519Instruction(entries: SignedEntry[]): TransactionInstruction {
  const n = entries.length;
  const header = 2 + 14 * n;
  const offsets = Buffer.alloc(14 * n);
  const body: Buffer[] = [];
  let at = header;
  entries.forEach((e, i) => {
    const pkOff = at;
    const sigOff = pkOff + 32;
    const msgOff = sigOff + 64;
    at = msgOff + e.message.length;
    body.push(e.pubkey.toBuffer(), e.signature, e.message);
    const o = i * 14;
    offsets.writeUInt16LE(sigOff, o);
    offsets.writeUInt16LE(0xffff, o + 2);
    offsets.writeUInt16LE(pkOff, o + 4);
    offsets.writeUInt16LE(0xffff, o + 6);
    offsets.writeUInt16LE(msgOff, o + 8);
    offsets.writeUInt16LE(e.message.length, o + 10);
    offsets.writeUInt16LE(0xffff, o + 12);
  });
  const data = Buffer.concat([Buffer.from([n, 0]), offsets, ...body]);
  return new TransactionInstruction({ programId: ED25519_PROGRAM_ID, keys: [], data });
}

export function signEntries(keys: Keypair[], message: Buffer): SignedEntry[] {
  return keys.map((k) => ({ pubkey: k.publicKey, signature: ed25519Sign(k, message), message }));
}

// ---------- account decoders ----------

function checkDisc(data: Uint8Array, name: string): Buffer {
  const b = Buffer.from(data);
  if (b.length < 8 || !b.subarray(0, 8).equals(accDisc(name))) throw new Error(`not a ${name} account`);
  return b;
}
/** A modes byte appended in M2; MODE_SIGNERS if the account predates it (or reads 0). */
function modesAt(b: Buffer, at: number): number {
  return at < b.length && b[at] !== 0 ? b[at] : 1;
}

function readKeys(b: Buffer, at: number): PublicKey[] {
  const n = b.readUInt32LE(at);
  const out: PublicKey[] = [];
  for (let i = 0; i < n; i++) out.push(new PublicKey(b.subarray(at + 4 + 32 * i, at + 36 + 32 * i)));
  return out;
}

export interface BridgeConfig {
  admin: PublicKey;
  mint: PublicKey;
  vault: PublicKey;
  tokenProgram: PublicKey;
  depositNonce: bigint;
  threshold: number;
  signers: PublicKey[];
  /** MODE_SIGNERS | MODE_PROOF (M2); MODE_SIGNERS when the field is absent. */
  releaseModes: number;
}
/** Bridge `release_modes` / storefront `finalize_modes` bits. */
export const MODE_SIGNERS = 1;
export const MODE_PROOF = 2;
export function decodeBridgeConfig(data: Uint8Array): BridgeConfig {
  const b = checkDisc(data, "Config");
  return {
    admin: new PublicKey(b.subarray(8, 40)),
    mint: new PublicKey(b.subarray(40, 72)),
    vault: new PublicKey(b.subarray(72, 104)),
    tokenProgram: new PublicKey(b.subarray(104, 136)),
    depositNonce: b.readBigUInt64LE(136),
    threshold: b[144],
    signers: readKeys(b, 146),
    releaseModes: modesAt(b, 146 + 4 + 32 * b.readUInt32LE(146)),
  };
}

/** Bridge `Deposit` record (token chain). */
export interface Deposit {
  nonce: bigint;
  sender: PublicKey;
  /** Agent-chain recipient. */
  recipient: PublicKey;
  /** SPL base units. */
  amount: bigint;
  slot: bigint;
}
export function decodeDeposit(data: Uint8Array): Deposit {
  const b = checkDisc(data, "Deposit");
  return {
    nonce: b.readBigUInt64LE(8),
    sender: new PublicKey(b.subarray(16, 48)),
    recipient: new PublicKey(b.subarray(48, 80)),
    amount: b.readBigUInt64LE(80),
    slot: b.readBigUInt64LE(88),
  };
}

/** SPL / Token-2022 token account amount (offset 64). */
export function tokenAccountAmount(data: Uint8Array): bigint {
  return Buffer.from(data).readBigUInt64LE(64);
}

/** storefront `Config`: only the fields the relayer needs. */
export interface StorefrontConfig {
  admin: PublicKey;
  threshold: number;
  attesters: PublicKey[];
  /** MODE_SIGNERS | MODE_PROOF (M2). */
  finalizeModes: number;
}
export function decodeStorefrontConfig(data: Uint8Array): StorefrontConfig {
  const b = checkDisc(data, "Config");
  // admin, mint, bridge_program (3 x 32), paused u8, protocol_fee_bps u16, max_slippage_bps u16,
  // min_sol_in u64, max_sol_in u64, max_dim u32, max_blocks_cap u32, max_deadline_blocks u64.
  const at = 8 + 96 + 1 + 2 + 2 + 8 + 8 + 4 + 4 + 8; // 141
  const n = b.readUInt32LE(at + 1);
  // after attesters: orders u64, total_sol u64, total_tokens u64, bump u8, trader_bump u8, finalize_modes u8
  const fm = at + 1 + 4 + 32 * n + 24 + 2;
  return { admin: new PublicKey(b.subarray(8, 40)), threshold: b[at], attesters: readKeys(b, at + 1), finalizeModes: modesAt(b, fm) };
}

export const ORDER_STATUS_PENDING = 1;
export const ORDER_STATUS_FINALIZED = 2;
export const ORDER_STATUS_EXPIRED = 3;
export const ORDER_STATUS_REFUNDED = 4;

/** storefront::layout offsets (including the 8-byte discriminator). */
export const ORDER_LAYOUT = {
  STATUS: 9, DEPOSIT_NONCE: 12, JOB_ID: 20, BUYER: 52, RECIPIENT: 84, BODY_HASH: 116, TOKENS: 156, WORKER_PAY: 164,
  PROTOCOL_FEE: 172, DELIVERER: 196, RESULT_HASH: 228, SPEC: 470, LEN: 1070,
} as const;
const MAX_EVALUATORS = 8;

/** A job spec in the PostJob body field order (SPEC 4.1 / docs/RPC.md). */
export interface PostJobBody {
  m: number;
  n: number;
  k: number;
  config: Buffer; // 52 bytes
  hashA: Buffer;
  hashB: Buffer;
  dataUri: Buffer;
  workerPay: bigint;
  protocolFee: bigint;
  /** Absolute for PostJob txs; RELATIVE (blocks after unlock) for storefront orders. */
  deadline: bigint;
  maxBlocks: number;
  evaluators: PublicKey[];
  threshold: number;
}

/** PostJob body bytes, exactly as storefront `post_job_body` and the chain's codec. */
export function encodePostJobBody(p: PostJobBody): Buffer {
  if (p.config.length !== 52 || p.hashA.length !== 32 || p.hashB.length !== 32) throw new Error("bad PostJob field size");
  return Buffer.concat([
    u32le(p.m), u32le(p.n), u32le(p.k), p.config, p.hashA, p.hashB,
    u16le(p.dataUri.length), p.dataUri,
    u64le(p.workerPay), u64le(p.protocolFee), u64le(p.deadline), u32le(p.maxBlocks),
    Buffer.from([p.evaluators.length]), ...p.evaluators.map((e) => e.toBuffer()), Buffer.from([p.threshold]),
  ]);
}

export interface Order {
  status: number;
  depositNonce: bigint;
  jobId: Buffer;
  buyer: PublicKey;
  recipient: PublicKey;
  bodyHash: Buffer;
  tokens: bigint;
  /** PostJob body rebuilt from the stored spec + W + P (relative deadline). */
  body: PostJobBody;
}

export function decodeOrder(data: Uint8Array): Order {
  const L = ORDER_LAYOUT;
  const d = checkDisc(data, "Order");
  if (d.length < L.LEN) throw new Error(`Order account too short (${d.length})`);
  const S = L.SPEC;
  const evc = d[S + 141];
  const uriLen = d.readUInt16LE(S + 398);
  if (evc > MAX_EVALUATORS || uriLen > 200) throw new Error("Order spec out of range");
  const k32 = (o: number) => Buffer.from(d.subarray(o, o + 32));
  return {
    status: d[L.STATUS],
    depositNonce: d.readBigUInt64LE(L.DEPOSIT_NONCE),
    jobId: k32(L.JOB_ID),
    buyer: new PublicKey(k32(L.BUYER)),
    recipient: new PublicKey(k32(L.RECIPIENT)),
    bodyHash: k32(L.BODY_HASH),
    tokens: d.readBigUInt64LE(L.TOKENS),
    body: {
      m: d.readUInt32LE(S),
      n: d.readUInt32LE(S + 4),
      k: d.readUInt32LE(S + 8),
      config: Buffer.from(d.subarray(S + 12, S + 64)),
      hashA: k32(S + 64),
      hashB: k32(S + 96),
      deadline: d.readBigUInt64LE(S + 128),
      maxBlocks: d.readUInt32LE(S + 136),
      threshold: d[S + 140],
      evaluators: Array.from({ length: evc }, (_, i) => new PublicKey(k32(S + 142 + 32 * i))),
      dataUri: Buffer.from(d.subarray(S + 400, S + 400 + uriLen)),
      workerPay: d.readBigUInt64LE(L.WORKER_PAY),
      protocolFee: d.readBigUInt64LE(L.PROTOCOL_FEE),
    },
  };
}

// ---------- instruction builders ----------

const w = (pubkey: PublicKey, isSigner = false) => ({ pubkey, isSigner, isWritable: true });
const r = (pubkey: PublicKey, isSigner = false) => ({ pubkey, isSigner, isWritable: false });

function keyVec(keys: PublicKey[]): Buffer {
  return Buffer.concat([u32le(keys.length), ...keys.map((k) => k.toBuffer())]);
}

export function bridgeInitializeIx(admin: PublicKey, mint: PublicKey, signers: PublicKey[], threshold: number,
  tokenProgram = TOKEN_2022_PROGRAM_ID): TransactionInstruction {
  return new TransactionInstruction({
    programId: BRIDGE_PROGRAM_ID,
    keys: [w(admin, true), w(bridgeConfigPda()), r(mint), w(vaultAta(mint, tokenProgram)), r(tokenProgram),
      r(ATA_PROGRAM_ID), r(SystemProgram.programId)],
    data: Buffer.concat([ixDisc("initialize"), keyVec(signers), Buffer.from([threshold])]),
  });
}

export function bridgeDepositIx(a: { payer: PublicKey; sender: PublicKey; mint: PublicKey; nonce: bigint;
  amount: bigint; agentRecipient: PublicKey; tokenProgram?: PublicKey }): TransactionInstruction {
  const tp = a.tokenProgram ?? TOKEN_2022_PROGRAM_ID;
  return new TransactionInstruction({
    programId: BRIDGE_PROGRAM_ID,
    keys: [w(a.payer, true), r(a.sender, true), w(bridgeConfigPda()), r(a.mint), w(ata(a.sender, a.mint, tp)),
      w(vaultAta(a.mint, tp)), w(depositPda(a.nonce)), r(tp), r(SystemProgram.programId)],
    data: Buffer.concat([ixDisc("deposit"), u64le(a.amount), a.agentRecipient.toBuffer()]),
  });
}

export function bridgeReleaseIx(a: { payer: PublicKey; mint: PublicKey; nonce: bigint; recipient: PublicKey;
  amount: bigint; recipientToken: PublicKey; tokenProgram?: PublicKey }): TransactionInstruction {
  const tp = a.tokenProgram ?? TOKEN_2022_PROGRAM_ID;
  return new TransactionInstruction({
    programId: BRIDGE_PROGRAM_ID,
    keys: [w(a.payer, true), r(bridgeConfigPda()), r(a.mint), w(vaultAta(a.mint, tp)), w(a.recipientToken),
      w(bridgeProcessedPda(a.nonce)), r(INSTRUCTIONS_SYSVAR_ID), r(tp), r(SystemProgram.programId)],
    data: Buffer.concat([ixDisc("release"), u64le(a.nonce), a.recipient.toBuffer(), u64le(a.amount)]),
  });
}

/** bridge `refund_deposit(nonce, buyer)`: the deposit's full amount from the vault to `buyerToken` (owned by buyer). */
export function bridgeRefundIx(a: { payer: PublicKey; mint: PublicKey; nonce: bigint; buyer: PublicKey; buyerToken: PublicKey;
  tokenProgram?: PublicKey }): TransactionInstruction {
  const tp = a.tokenProgram ?? TOKEN_2022_PROGRAM_ID;
  return new TransactionInstruction({
    programId: BRIDGE_PROGRAM_ID,
    keys: [w(a.payer, true), r(bridgeConfigPda()), r(a.mint), w(vaultAta(a.mint, tp)), r(depositPda(a.nonce)), w(a.buyerToken),
      w(bridgeRefundPda(a.nonce)), r(INSTRUCTIONS_SYSVAR_ID), r(tp), r(SystemProgram.programId)],
    data: Buffer.concat([ixDisc("refund_deposit"), u64le(a.nonce), a.buyer.toBuffer()]),
  });
}

/** storefront `mark_refunded()` (permissionless; reads the bridge refund record). */
export function markRefundedIx(nonce: bigint, program = STOREFRONT_PROGRAM_ID): TransactionInstruction {
  return new TransactionInstruction({
    programId: program,
    keys: [r(storefrontConfigPda(program)), w(orderPda(nonce, program)), r(bridgeRefundPda(nonce))],
    data: ixDisc("mark_refunded"),
  });
}

/** storefront buyback share record of an order (2026-10-18): ["share", nonce]. */
export const sharePda = (nonce: bigint, program = STOREFRONT_PROGRAM_ID): PublicKey => pda([Buffer.from("share"), u64le(nonce)], program);
export const buybackVaultPda = (program = STOREFRONT_PROGRAM_ID): PublicKey => pda([Buffer.from("buyback")], program);

/** storefront `settle_share()` (permissionless): a Refunded / Expired order's held buyback
 *  share goes back to its buyer (a Finalized one is released to the buyback). */
export function settleShareIx(nonce: bigint, buyer: PublicKey, program = STOREFRONT_PROGRAM_ID): TransactionInstruction {
  return new TransactionInstruction({
    programId: program,
    keys: [w(storefrontConfigPda(program)), r(orderPda(nonce, program)), w(sharePda(nonce, program)), w(buyer), w(buybackVaultPda(program)), r(SystemProgram.programId)],
    data: ixDisc("settle_share"),
  });
}

/** storefront `initialize(params = all None, attesters, threshold)` (tests and localnet setup). */
export function storefrontInitializeIx(admin: PublicKey, attesters: PublicKey[], threshold: number,
  program = STOREFRONT_PROGRAM_ID): TransactionInstruction {
  return new TransactionInstruction({
    programId: program,
    keys: [w(admin, true), w(storefrontConfigPda(program)), w(traderPda(program)), r(SystemProgram.programId)],
    data: Buffer.concat([ixDisc("initialize"), Buffer.alloc(10), keyVec(attesters), Buffer.from([threshold])]),
  });
}

function closeOrderKeys(caller: PublicKey, nonce: bigint, program: PublicKey) {
  return [r(caller, true), r(storefrontConfigPda(program)), w(orderPda(nonce, program)), r(INSTRUCTIONS_SYSVAR_ID)];
}

/** storefront `finalize_order(deliverer, result_hash, result_uri)`; caller = buyer, admin or an attester. */
export function finalizeOrderIx(a: { caller: PublicKey; nonce: bigint; deliverer: PublicKey; resultHash: Buffer;
  resultUri: string; program?: PublicKey }): TransactionInstruction {
  const program = a.program ?? STOREFRONT_PROGRAM_ID;
  const uri = Buffer.from(a.resultUri);
  return new TransactionInstruction({
    programId: program,
    keys: closeOrderKeys(a.caller, a.nonce, program),
    data: Buffer.concat([ixDisc("finalize_order"), a.deliverer.toBuffer(), a.resultHash, u32le(uri.length), uri]),
  });
}

/** storefront `expire_order()`. */
export function expireOrderIx(a: { caller: PublicKey; nonce: bigint; program?: PublicKey }): TransactionInstruction {
  const program = a.program ?? STOREFRONT_PROGRAM_ID;
  return new TransactionInstruction({ programId: program, keys: closeOrderKeys(a.caller, a.nonce, program), data: ixDisc("expire_order") });
}

/** ATA program CreateIdempotent. */
export function createAtaIdempotentIx(payer: PublicKey, owner: PublicKey, mint: PublicKey,
  tokenProgram = TOKEN_2022_PROGRAM_ID): TransactionInstruction {
  return new TransactionInstruction({
    programId: ATA_PROGRAM_ID,
    keys: [w(payer, true), w(ata(owner, mint, tokenProgram)), r(owner), r(mint), r(SystemProgram.programId), r(tokenProgram)],
    data: Buffer.from([1]),
  });
}
