// Byte-level codec for the bridge (token chain) and escrow (agent chain)
// programs: PDAs, instruction builders, account decoders, SPEC section 6
// messages and the ed25519 precompile instruction. Hand-encoded so the relayer
// does not depend on generated IDL files.

import { createHash, createPrivateKey, sign as edSign } from "node:crypto";
import { Keypair, PublicKey, SystemProgram, TransactionInstruction } from "@solana/web3.js";

export const BRIDGE_PROGRAM_ID = new PublicKey("7LgyoRiKjNXD1AkMQUePb7TzTvRewEXKKLVzXB5vEjc8");
export const ESCROW_PROGRAM_ID = new PublicKey("4EgFJ3Vnck4EqEtuDFnmF8Zrpcz8mmTU2DbnZBHyQuum");
export const TOKEN_2022_PROGRAM_ID = new PublicKey("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
export const ATA_PROGRAM_ID = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
export const ED25519_PROGRAM_ID = new PublicKey("Ed25519SigVerify111111111111111111111111111");
export const INSTRUCTIONS_SYSVAR_ID = new PublicKey("Sysvar1nstructions1111111111111111111111111");

export const CHAIN_TOKEN = 0;
export const CHAIN_AGENT = 1;
/** 1 SPL base unit (6 dp) = 1000 lamports (9 dp). */
export const LAMPORTS_PER_SPL_UNIT = 1000n;
/** Whole agent-chain supply held by the escrow at genesis: 1B x 1e9. */
export const GENESIS_ESCROW_LAMPORTS = 1_000_000_000_000_000_000n;

export const DEPOSIT_DOMAIN = Buffer.from("pearl-sol:deposit:v1");
export const WITHDRAW_DOMAIN = Buffer.from("pearl-sol:withdraw:v1");

// ---------- primitives ----------

export function u64le(n: bigint): Buffer {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(n);
  return b;
}

function sha256(s: string): Buffer {
  return createHash("sha256").update(s).digest();
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

export const escrowConfigPda = (): PublicKey => pda([Buffer.from("config")], ESCROW_PROGRAM_ID);
export const escrowPda = (): PublicKey => pda([Buffer.from("escrow")], ESCROW_PROGRAM_ID);
export const withdrawalPda = (nonce: bigint): PublicKey =>
  pda([Buffer.from("withdrawal"), u64le(nonce)], ESCROW_PROGRAM_ID);
export const escrowProcessedPda = (nonce: bigint): PublicKey =>
  pda([Buffer.from("processed"), Buffer.from([CHAIN_TOKEN]), u64le(nonce)], ESCROW_PROGRAM_ID);

export function ata(owner: PublicKey, mint: PublicKey, tokenProgram = TOKEN_2022_PROGRAM_ID): PublicKey {
  return pda([owner.toBuffer(), tokenProgram.toBuffer(), mint.toBuffer()], ATA_PROGRAM_ID);
}
export const vaultAta = (mint: PublicKey, tokenProgram = TOKEN_2022_PROGRAM_ID): PublicKey =>
  ata(bridgeConfigPda(), mint, tokenProgram);

// ---------- SPEC section 6 messages ----------

/** `domain || nonce u64 LE || recipient [32] || amount_spl u64 LE` */
export function bridgeMessage(domain: Buffer, nonce: bigint, recipient: PublicKey, amountSpl: bigint): Buffer {
  return Buffer.concat([domain, u64le(nonce), recipient.toBuffer(), u64le(amountSpl)]);
}
export const depositMessage = (nonce: bigint, recipient: PublicKey, amountSpl: bigint): Buffer =>
  bridgeMessage(DEPOSIT_DOMAIN, nonce, recipient, amountSpl);
export const withdrawMessage = (nonce: bigint, recipient: PublicKey, amountSpl: bigint): Buffer =>
  bridgeMessage(WITHDRAW_DOMAIN, nonce, recipient, amountSpl);

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
}
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
  };
}

export interface EscrowConfig {
  admin: PublicKey;
  withdrawNonce: bigint;
  threshold: number;
  signers: PublicKey[];
}
export function decodeEscrowConfig(data: Uint8Array): EscrowConfig {
  const b = checkDisc(data, "Config");
  return {
    admin: new PublicKey(b.subarray(8, 40)),
    withdrawNonce: b.readBigUInt64LE(40),
    threshold: b[48],
    signers: readKeys(b, 51),
  };
}

/** Deposit (token chain) and Withdrawal (agent chain) share one layout. */
export interface Transfer {
  nonce: bigint;
  sender: PublicKey;
  recipient: PublicKey;
  /** SPL base units. */
  amount: bigint;
  slot: bigint;
}
function decodeTransfer(data: Uint8Array, name: string): Transfer {
  const b = checkDisc(data, name);
  return {
    nonce: b.readBigUInt64LE(8),
    sender: new PublicKey(b.subarray(16, 48)),
    recipient: new PublicKey(b.subarray(48, 80)),
    amount: b.readBigUInt64LE(80),
    slot: b.readBigUInt64LE(88),
  };
}
export const decodeDeposit = (d: Uint8Array): Transfer => decodeTransfer(d, "Deposit");
export const decodeWithdrawal = (d: Uint8Array): Transfer => decodeTransfer(d, "Withdrawal");

/** SPL / Token-2022 token account amount (offset 64). */
export function tokenAccountAmount(data: Uint8Array): bigint {
  return Buffer.from(data).readBigUInt64LE(64);
}

// ---------- instruction builders ----------

const w = (pubkey: PublicKey, isSigner = false) => ({ pubkey, isSigner, isWritable: true });
const r = (pubkey: PublicKey, isSigner = false) => ({ pubkey, isSigner, isWritable: false });

function keyVec(keys: PublicKey[]): Buffer {
  const len = Buffer.alloc(4);
  len.writeUInt32LE(keys.length);
  return Buffer.concat([len, ...keys.map((k) => k.toBuffer())]);
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

export function escrowInitializeIx(admin: PublicKey, signers: PublicKey[], threshold: number): TransactionInstruction {
  return new TransactionInstruction({
    programId: ESCROW_PROGRAM_ID,
    keys: [w(admin, true), w(escrowConfigPda()), r(escrowPda()), r(SystemProgram.programId)],
    data: Buffer.concat([ixDisc("initialize"), keyVec(signers), Buffer.from([threshold])]),
  });
}

export function escrowUnlockIx(a: { payer: PublicKey; nonce: bigint; recipient: PublicKey; amountSpl: bigint }):
  TransactionInstruction {
  return new TransactionInstruction({
    programId: ESCROW_PROGRAM_ID,
    keys: [w(a.payer, true), r(escrowConfigPda()), w(escrowPda()), w(a.recipient), w(escrowProcessedPda(a.nonce)),
      r(INSTRUCTIONS_SYSVAR_ID), r(SystemProgram.programId)],
    data: Buffer.concat([ixDisc("unlock"), u64le(a.nonce), a.recipient.toBuffer(), u64le(a.amountSpl)]),
  });
}

export function escrowLockIx(a: { sender: PublicKey; nonce: bigint; amountLamports: bigint; solanaRecipient: PublicKey }):
  TransactionInstruction {
  return new TransactionInstruction({
    programId: ESCROW_PROGRAM_ID,
    keys: [w(a.sender, true), w(escrowConfigPda()), w(escrowPda()), w(withdrawalPda(a.nonce)), r(SystemProgram.programId)],
    data: Buffer.concat([ixDisc("lock"), u64le(a.amountLamports), a.solanaRecipient.toBuffer()]),
  });
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
