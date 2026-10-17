// x402 "exact" scheme on Solana (x402 v1, coinbase/x402 specs/schemes/exact,
// SVM variant), facilitator side, done in-process by this service.
//
// - The 402 response lists PaymentRequirements with `extra.feePayer` = the
//   service wallet. The client builds a transaction that pays exactly
//   `maxAmountRequired` to `payTo`, signs it (partially, when the service is
//   the fee payer), and sends it base64 in the X-PAYMENT header:
//   base64(JSON {x402Version: 1, scheme: "exact", network, payload: {transaction}}).
// - The SVM spec pays an SPL token with TransferChecked. This service also
//   accepts native SOL (`asset` = the wSOL mint address, `extra.assetKind` =
//   "native"): one System transfer of exactly the amount to `payTo`. Localnet
//   runs native SOL; the SPL path (USDC) is verified the same way but buying
//   with it is the storefront's TODO(usdc).
// - Verification is strict: only ComputeBudget (bounded price), one transfer,
//   and (SPL) an idempotent ATA create are allowed; no lookup tables; the
//   service key may only appear as the fee payer, never as a source,
//   authority or ATA payer; every other required signature must verify.

import { createPublicKey, verify as edVerify } from "node:crypto";
import { ComputeBudgetProgram, Keypair, PublicKey, SystemProgram, TransactionMessage, VersionedTransaction } from "@solana/web3.js";
import { ata, TOKEN, TOKEN_2022 } from "./storefront.ts";

export const X402_VERSION = 1;
const COMPUTE_BUDGET = new PublicKey("ComputeBudget111111111111111111111111111111");
const SYSTEM = new PublicKey("11111111111111111111111111111111");
const ATA_PROGRAM = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

export interface PaymentRequirements {
  scheme: "exact";
  network: string;
  maxAmountRequired: string;
  resource: string;
  description: string;
  mimeType: string;
  payTo: string;
  maxTimeoutSeconds: number;
  asset: string;
  extra: { feePayer: string; assetKind: "native" | "spl"; decimals: number };
}

export interface PaymentPayload {
  x402Version: number;
  scheme: string;
  network: string;
  payload: { transaction: string };
}

export interface VerifiedPayment {
  tx: VersionedTransaction;
  payer: PublicKey;
  amount: bigint;
  /** The service must add the fee payer signature before sending. */
  needsFeePayerSig: boolean;
  /** Replay key: the payer's signature (base64). */
  id: string;
}

export class PaymentError extends Error {}

export function decodePaymentHeader(h: string): PaymentPayload {
  let j: unknown;
  try {
    j = JSON.parse(Buffer.from(h, "base64").toString("utf8"));
  } catch {
    throw new PaymentError("X-PAYMENT is not base64 JSON");
  }
  const p = j as PaymentPayload;
  if (!p || typeof p !== "object" || typeof p.payload?.transaction !== "string") throw new PaymentError("X-PAYMENT has no payload.transaction");
  return p;
}

export function encodePaymentHeader(p: PaymentPayload): string {
  return Buffer.from(JSON.stringify(p)).toString("base64");
}

function edOk(pubkey: PublicKey, msg: Uint8Array, sig: Uint8Array): boolean {
  const key = createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: Buffer.from(pubkey.toBytes()).toString("base64url") }, format: "jwk" });
  return edVerify(null, msg, key, sig);
}

const u64At = (d: Uint8Array, o: number) => Buffer.from(d).readBigUInt64LE(o);

export function verifyExact(p: PaymentPayload, req: PaymentRequirements, maxCuPriceMicroLamports = 5_000_000n): VerifiedPayment {
  if (p.x402Version !== X402_VERSION) throw new PaymentError(`x402Version ${p.x402Version} not supported`);
  if (p.scheme !== "exact") throw new PaymentError(`scheme ${p.scheme} not supported`);
  if (p.network !== req.network) throw new PaymentError(`network ${p.network}, expected ${req.network}`);
  let tx: VersionedTransaction;
  try {
    tx = VersionedTransaction.deserialize(Buffer.from(p.payload.transaction, "base64"));
  } catch {
    throw new PaymentError("transaction does not deserialize");
  }
  const msg = tx.message;
  if (msg.addressTableLookups.length > 0) throw new PaymentError("address lookup tables not allowed");
  const keys = msg.staticAccountKeys;
  const service = new PublicKey(req.extra.feePayer);
  const payTo = new PublicKey(req.payTo);
  const asset = new PublicKey(req.asset);
  const want = BigInt(req.maxAmountRequired);
  const feePayer = keys[0];
  const serviceIsFeePayer = feePayer.equals(service);

  let transfer: { payer: PublicKey; amount: bigint } | null = null;
  for (const ix of msg.compiledInstructions) {
    const prog = keys[ix.programIdIndex];
    const acc = (i: number) => keys[ix.accountKeyIndexes[i]];
    const d = ix.data;
    if (prog.equals(COMPUTE_BUDGET)) {
      if (d[0] === 2 && d.length === 5) continue; // SetComputeUnitLimit
      if (d[0] === 3 && d.length === 9) {
        if (u64At(d, 1) > maxCuPriceMicroLamports) throw new PaymentError("compute unit price too high");
        continue;
      }
      throw new PaymentError("compute budget instruction not allowed");
    }
    if (transfer) throw new PaymentError("only one transfer allowed, and it must be last");
    if (prog.equals(SYSTEM)) {
      if (req.extra.assetKind !== "native") throw new PaymentError("native transfer for an SPL requirement");
      if (d.length !== 12 || Buffer.from(d).readUInt32LE(0) !== 2 || ix.accountKeyIndexes.length !== 2) throw new PaymentError("system instruction is not a transfer");
      const from = acc(0), to = acc(1);
      if (!to.equals(payTo)) throw new PaymentError("transfer does not go to payTo");
      if (from.equals(service)) throw new PaymentError("transfer source is the service");
      transfer = { payer: from, amount: u64At(d, 4) };
      continue;
    }
    if (prog.equals(ATA_PROGRAM)) {
      if (req.extra.assetKind !== "spl") throw new PaymentError("ATA create for a native requirement");
      if (!(d.length === 0 || (d.length === 1 && (d[0] === 0 || d[0] === 1)))) throw new PaymentError("ATA instruction not allowed");
      if (acc(0).equals(service)) throw new PaymentError("ATA create paid by the service");
      if (!acc(2).equals(payTo) || !acc(3).equals(asset)) throw new PaymentError("ATA create is not payTo's");
      continue;
    }
    if (prog.equals(TOKEN) || prog.equals(TOKEN_2022)) {
      if (req.extra.assetKind !== "spl") throw new PaymentError("token transfer for a native requirement");
      if (d.length !== 10 || d[0] !== 12 || ix.accountKeyIndexes.length < 4) throw new PaymentError("token instruction is not TransferChecked");
      const mint = acc(1), dest = acc(2), authority = acc(3);
      if (!mint.equals(asset)) throw new PaymentError("wrong asset");
      if (d[9] !== req.extra.decimals) throw new PaymentError("wrong decimals");
      if (!dest.equals(ata(payTo, asset, prog))) throw new PaymentError("destination is not payTo's ATA");
      if (authority.equals(service) || acc(0).equals(service)) throw new PaymentError("transfer authority is the service");
      transfer = { payer: authority, amount: u64At(d, 1) };
      continue;
    }
    throw new PaymentError(`program ${prog.toBase58()} not allowed`);
  }
  if (!transfer) throw new PaymentError("no transfer");
  if (transfer.amount !== want) throw new PaymentError(`amount ${transfer.amount}, required ${want}`);

  const nsig = msg.header.numRequiredSignatures;
  const bytes = msg.serialize();
  let payerSig: Uint8Array | null = null;
  for (let i = 0; i < nsig; i++) {
    const k = keys[i];
    const sig = tx.signatures[i];
    if (k.equals(service)) {
      if (i !== 0) throw new PaymentError("service may only sign as fee payer");
      continue;
    }
    if (!sig || sig.every((b) => b === 0) || !edOk(k, bytes, sig)) throw new PaymentError(`missing or bad signature for ${k.toBase58()}`);
    if (k.equals(transfer.payer)) payerSig = sig;
  }
  if (!payerSig) throw new PaymentError("payer did not sign");
  return { tx, payer: transfer.payer, amount: transfer.amount, needsFeePayerSig: serviceIsFeePayer, id: Buffer.from(payerSig).toString("base64") };
}

/**
 * Client helper: an X-PAYMENT header paying a native-SOL requirement. The
 * service is the fee payer (extra.feePayer) and adds its signature at
 * settlement, so the payer only signs the transfer.
 */
export function buildNativePayment(payer: { publicKey: PublicKey; secretKey: Uint8Array }, req: PaymentRequirements, recentBlockhash: string): string {
  const feePayer = new PublicKey(req.extra.feePayer);
  const msg = new TransactionMessage({
    payerKey: feePayer,
    recentBlockhash,
    instructions: [
      ComputeBudgetProgram.setComputeUnitLimit({ units: 10_000 }),
      SystemProgram.transfer({ fromPubkey: payer.publicKey, toPubkey: new PublicKey(req.payTo), lamports: BigInt(req.maxAmountRequired) }),
    ],
  }).compileToV0Message();
  const tx = new VersionedTransaction(msg);
  tx.sign([Keypair.fromSecretKey(payer.secretKey)]);
  return encodePaymentHeader({ x402Version: X402_VERSION, scheme: "exact", network: req.network, payload: { transaction: Buffer.from(tx.serialize()).toString("base64") } });
}
