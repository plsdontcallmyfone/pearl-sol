// x402 "exact" native-SOL payment (the storefront's scheme, services/storefront-x402/src/x402.ts):
// one System transfer of exactly maxAmountRequired to payTo, the storefront's key as fee
// payer (extra.feePayer, its signature left empty for the storefront to add), signed by
// the proxy wallet. Built by hand (legacy message) so the proxy needs no Solana SDK.
import { b58decode } from "./b58.mjs";

const SYSTEM = new Uint8Array(32);

function compactU16(n) {
  const out = [];
  for (;;) {
    let b = n & 0x7f;
    n >>= 7;
    if (n) b |= 0x80;
    out.push(b);
    if (!n) return out;
  }
}

function u64le(v) {
  const b = Buffer.alloc(8);
  b.writeBigUInt64LE(BigInt(v));
  return [...b];
}

const eq = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);

/** Wire transaction bytes paying `req` from `wallet`. */
export function nativeTransferTx(wallet, req, recentBlockhash) {
  if (req.scheme !== "exact") throw new Error(`unsupported x402 scheme ${req.scheme}`);
  if (req.extra?.assetKind !== "native") throw new Error("only native SOL x402 requirements are supported");
  const payer = wallet.publicKey;
  const payTo = b58decode(req.payTo);
  const feePayer = req.extra?.feePayer ? b58decode(req.extra.feePayer) : payer;
  if (eq(payTo, payer)) throw new Error("payTo is the proxy wallet itself");
  // storefront-x402 names its own wallet as both fee payer and payTo (it co-signs as fee payer
  // and receives the transfer); then the message has 2 signers and no separate payTo key
  const sep = !eq(feePayer, payer);
  const feeIsPayTo = sep && eq(feePayer, payTo);
  const keys = !sep ? [payer, payTo, SYSTEM] : feeIsPayTo ? [feePayer, payer, SYSTEM] : [feePayer, payer, payTo, SYSTEM];
  const iPayer = sep ? 1 : 0;
  const iPayTo = !sep ? 1 : feeIsPayTo ? 0 : 2;
  const nSig = sep ? 2 : 1;
  const msg = Uint8Array.from([
    nSig, 0, 1,
    ...compactU16(keys.length), ...keys.flatMap((k) => [...k]),
    ...b58decode(recentBlockhash),
    ...compactU16(1),
    keys.length - 1, ...compactU16(2), iPayer, iPayTo,
    ...compactU16(12), 2, 0, 0, 0, ...u64le(req.maxAmountRequired),
  ]);
  const sigs = new Array(nSig).fill(null).map(() => new Uint8Array(64));
  sigs[iPayer] = wallet.sign(msg);
  return Uint8Array.from([...compactU16(nSig), ...sigs.flatMap((s) => [...s]), ...msg]);
}

export function paymentHeader(wallet, req, recentBlockhash) {
  const tx = nativeTransferTx(wallet, req, recentBlockhash);
  const p = { x402Version: 1, scheme: "exact", network: req.network, payload: { transaction: Buffer.from(tx).toString("base64") } };
  return Buffer.from(JSON.stringify(p)).toString("base64");
}

export function isLoopbackUrl(u) {
  const h = new URL(u).hostname;
  return h === "127.0.0.1" || h === "localhost" || h === "[::1]" || h === "::1";
}

export async function latestBlockhash(solRpc) {
  const r = await fetch(solRpc, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getLatestBlockhash", params: [{ commitment: "confirmed" }] }) });
  const j = await r.json();
  const h = j?.result?.value?.blockhash;
  if (!h) throw new Error(`getLatestBlockhash failed: ${JSON.stringify(j?.error ?? j)}`);
  return h;
}
