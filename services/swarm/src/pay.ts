// x402 "exact" native-SOL payment for a gateway top-up (the storefront's scheme,
// services/storefront-x402/src/x402.ts; same construction as services/cadre-proxy/src/pay.mjs):
// one System transfer of exactly maxAmountRequired to payTo, the storefront's key as fee payer
// (extra.feePayer; its signature slot left empty for the storefront to add), signed by the
// operator key. Built by hand (legacy message), so no Solana SDK is needed. Devnet only.
import { b58decode } from "./b58.ts";
import type { Wallet } from "./wallet.ts";

const SYSTEM = new Uint8Array(32);

function compactU16(n: number): number[] {
  const out: number[] = [];
  for (;;) {
    let b = n & 0x7f;
    n >>= 7;
    if (n) b |= 0x80;
    out.push(b);
    if (!n) return out;
  }
}
function u64le(v: string | number | bigint): number[] {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(v));
  return [...b];
}
const eq = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((x, i) => x === b[i]);

export interface Requirement {
  scheme: string;
  network: string;
  maxAmountRequired: string;
  payTo: string;
  extra?: { feePayer?: string; assetKind?: string };
}

/** Wire tx bytes paying `req` from `w`; the second value is the message the payer signed. */
export function nativeTransferTx(w: Wallet, req: Requirement, recentBlockhash: string): { tx: Uint8Array; message: Uint8Array } {
  if (req.scheme !== "exact") throw new Error(`unsupported x402 scheme ${req.scheme}`);
  if (req.extra?.assetKind && req.extra.assetKind !== "native") throw new Error("only native SOL x402 requirements are supported");
  const payer = w.publicKey;
  const payTo = b58decode(req.payTo);
  const feePayer = req.extra?.feePayer ? b58decode(req.extra.feePayer) : payer;
  if (eq(payTo, payer)) throw new Error("payTo is the operator wallet itself");
  const sep = !eq(feePayer, payer);
  const feeIsPayTo = sep && eq(feePayer, payTo);
  const keys = !sep ? [payer, payTo, SYSTEM] : feeIsPayTo ? [feePayer, payer, SYSTEM] : [feePayer, payer, payTo, SYSTEM];
  const iPayer = sep ? 1 : 0;
  const iPayTo = !sep ? 1 : feeIsPayTo ? 0 : 2;
  const nSig = sep ? 2 : 1;
  const message = Uint8Array.from([
    nSig, 0, 1,
    ...compactU16(keys.length), ...keys.flatMap((k) => [...k]),
    ...b58decode(recentBlockhash),
    ...compactU16(1),
    keys.length - 1, ...compactU16(2), iPayer, iPayTo,
    ...compactU16(12), 2, 0, 0, 0, ...u64le(req.maxAmountRequired),
  ]);
  const sigs = new Array(nSig).fill(null).map(() => new Uint8Array(64));
  sigs[iPayer] = w.sign(message);
  return { tx: Uint8Array.from([...compactU16(nSig), ...sigs.flatMap((s) => [...s]), ...message]), message };
}

export function paymentHeader(w: Wallet, req: Requirement, recentBlockhash: string): string {
  const { tx } = nativeTransferTx(w, req, recentBlockhash);
  const p = { x402Version: 1, scheme: "exact", network: req.network, payload: { transaction: Buffer.from(tx).toString("base64") } };
  return Buffer.from(JSON.stringify(p)).toString("base64");
}
