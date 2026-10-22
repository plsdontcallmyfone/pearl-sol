// Solana wire-tx signing with the agent's key (no Solana SDK needed): find this key's signature slot
// in a legacy or v0 message, sign the message bytes, put the signature in place. Plus an outflow
// check by simulation, so spending limits apply to transactions the agent built itself.
import { b58encode } from "./vendor/b58.ts";

function readCompact(b: Uint8Array, o: number): [number, number] {
  let v = 0, s = 0;
  for (let i = 0; i < 3; i++) {
    const x = b[o + i];
    v |= (x & 0x7f) << s;
    if (!(x & 0x80)) return [v, o + i + 1];
    s += 7;
  }
  throw new Error("bad compact-u16");
}

/** Signer keys of a wire tx (base58), in slot order. */
export function signersOf(txB64: string): string[] {
  const tx = Uint8Array.from(Buffer.from(txB64, "base64"));
  const [nSig, o] = readCompact(tx, 0);
  const msg = tx.subarray(o + 64 * nSig);
  const p = msg[0] & 0x80 ? 1 : 0;
  const nReq = msg[p];
  const [nKeys, ko] = readCompact(msg, p + 3);
  const out: string[] = [];
  for (let i = 0; i < Math.min(nKeys, nReq); i++) out.push(b58encode(msg.subarray(ko + 32 * i, ko + 32 * i + 32)));
  return out;
}

/** Sign an unsigned or partly signed wire tx with `sign` for `address`; returns base64. */
export function signSolanaTx(txB64: string, address: string, sign: (m: Uint8Array) => Uint8Array): string {
  const tx = Uint8Array.from(Buffer.from(txB64, "base64"));
  const [nSig, o] = readCompact(tx, 0);
  const msg = tx.subarray(o + 64 * nSig);
  const idx = signersOf(txB64).indexOf(address);
  if (idx < 0) throw new Error("this key is not a signer of the transaction");
  if (idx >= nSig) throw new Error("signature slot missing");
  const out = Uint8Array.from(tx);
  out.set(sign(msg), o + 64 * idx);
  return Buffer.from(out).toString("base64");
}

/** The first signature (the tx id), base58. */
export function txSignature(txB64: string): string {
  const tx = Uint8Array.from(Buffer.from(txB64, "base64"));
  const [, o] = readCompact(tx, 0);
  return b58encode(tx.subarray(o, o + 64));
}

async function rpc(url: string, method: string, params: unknown[]): Promise<any> {
  const r = await fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }), signal: AbortSignal.timeout(30_000) });
  const j: any = await r.json();
  if (j.error) throw new Error(`${method}: ${j.error.message ?? JSON.stringify(j.error)}`);
  return j.result;
}

/** Lamports `address` would lose if the (unsigned) tx ran now, by simulation (fees included). */
export async function simulateOutflow(solRpc: string, txB64: string, address: string): Promise<{ outflow: bigint; err: unknown; logs: string[] }> {
  const pre = BigInt((await rpc(solRpc, "getBalance", [address, { commitment: "confirmed" }])).value);
  const sim = await rpc(solRpc, "simulateTransaction", [txB64, { encoding: "base64", sigVerify: false, replaceRecentBlockhash: true, commitment: "confirmed", accounts: { encoding: "base64", addresses: [address] } }]);
  const acct = sim.value?.accounts?.[0];
  const post = acct ? BigInt(acct.lamports) : pre;
  return { outflow: pre > post ? pre - post : 0n, err: sim.value?.err ?? null, logs: sim.value?.logs ?? [] };
}
