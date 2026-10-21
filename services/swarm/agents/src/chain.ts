// Both chains from one key. Cadre side: the swarm's tx encoder (services/swarm/src/chaintx.ts)
// plus RegisterAgent (kind 1), which the swarm never sends. Solana side: unsigned legacy txs are
// built by cadre-api (/v1/tx/pump-buy, /v1/tx/sol-transfer, /v1/tx/spl-transfer, /v1/tx/sol-memo,
// /v1/tx/bridge-deposit), signed here by the same ed25519 key, and broadcast through
// /v1/tx/solana. No Solana SDK is needed.
import { b58decode, b58encode } from "../../src/b58.ts";
import { frame, NS_TX } from "../../src/chaintx.ts";
import type { Wallet } from "../../src/wallet.ts";

const u16 = (v: number) => [v & 0xff, (v >> 8) & 0xff];
const u32 = (v: number) => { const b = Buffer.alloc(4); b.writeUInt32LE(v >>> 0); return [...b]; };
const u64 = (v: bigint) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(v); return [...b]; };

/** RegisterAgent (kind 1): name_len u8 | name | uri_len u16 | uri (SPEC 2). */
export function registerAgentSigning(chainId: number, from: Uint8Array, nonce: bigint, fee: bigint, name: string, uri: string): Uint8Array {
  const n = Buffer.from(name, "utf8");
  const u = Buffer.from(uri, "utf8");
  if (n.length < 1 || n.length > 64) throw new Error("RegisterAgent name is 1..64 bytes");
  if (u.length > 512) throw new Error("RegisterAgent uri is at most 512 bytes");
  return Uint8Array.from([1, ...u32(chainId), ...from, ...u64(nonce), ...u64(fee), 1, n.length, ...n, ...u16(u.length), ...u]);
}

export function signRegisterAgent(w: Wallet, chainId: number, nonce: bigint, fee: bigint, name: string, uri: string): Uint8Array {
  const sb = registerAgentSigning(chainId, w.publicKey, nonce, fee, name, uri);
  return Uint8Array.from([...sb, ...w.sign(frame(NS_TX, sb))]);
}

// ---------------------------------------------------------------- Solana wire txs

function readCompact(b: Uint8Array, o: number): [number, number] {
  let v = 0;
  let s = 0;
  for (let i = 0; i < 3; i++) {
    const x = b[o + i];
    v |= (x & 0x7f) << s;
    if (!(x & 0x80)) return [v, o + i + 1];
    s += 7;
  }
  throw new Error("bad compact-u16");
}

/** Sign an unsigned (or partly signed) legacy or v0 wire tx with `w`; returns base64. */
export function signSolanaTx(w: Wallet, txB64: string): string {
  const tx = Uint8Array.from(Buffer.from(txB64, "base64"));
  const [nSig, o] = readCompact(tx, 0);
  const msg = tx.subarray(o + 64 * nSig);
  let p = 0;
  if (msg[0] & 0x80) p = 1; // v0 prefix
  const nReq = msg[p];
  const [nKeys, ko] = readCompact(msg, p + 3);
  const me = b58encode(w.publicKey);
  let idx = -1;
  for (let i = 0; i < Math.min(nKeys, nReq); i++) if (b58encode(msg.subarray(ko + 32 * i, ko + 32 * i + 32)) === me) idx = i;
  if (idx < 0) throw new Error("this key is not a signer of the transaction");
  if (idx >= nSig) throw new Error("signature slot missing");
  const sig = w.sign(msg);
  const out = Uint8Array.from(tx);
  out.set(sig, o + 64 * idx);
  return Buffer.from(out).toString("base64");
}

/** The first signature of a signed wire tx (the Solana tx id), base58. */
export function txSignature(txB64: string): string {
  const tx = Uint8Array.from(Buffer.from(txB64, "base64"));
  const [, o] = readCompact(tx, 0);
  return b58encode(tx.subarray(o, o + 64));
}

export const isAddress = (s: unknown): s is string => typeof s === "string" && /^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(s) && b58decode(s).length === 32;

// ---------------------------------------------------------------- cadre-api client

export class ApiClient {
  base: string;
  timeoutMs: number;
  constructor(base: string, timeoutMs = 60_000) {
    this.base = base.replace(/\/$/, "");
    this.timeoutMs = timeoutMs;
  }
  async req(method: string, path: string, body?: unknown, headers: Record<string, string> = {}): Promise<{ status: number; body: any }> {
    const r = await fetch(this.base + path, {
      method,
      headers: { accept: "application/json", ...(body !== undefined ? { "content-type": "application/json" } : {}), ...headers },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(this.timeoutMs),
    });
    const t = await r.text();
    let j: any = t;
    try { j = t ? JSON.parse(t) : null; } catch { /* text */ }
    return { status: r.status, body: j };
  }
  async get(path: string) { return this.req("GET", path); }
  /** POST that throws with the API's message on a non-2xx reply. */
  async must(method: string, path: string, body?: unknown): Promise<any> {
    const r = await this.req(method, path, body);
    if (r.status < 200 || r.status >= 300) throw new Error(`${method} ${path}: ${r.status} ${r.body?.error?.message ?? JSON.stringify(r.body).slice(0, 200)}`);
    return r.body;
  }
  /** Build a Solana tx with a cadre-api builder, sign it with `w`, broadcast it; returns the signature and the builder's summary. */
  async solana(w: Wallet, builder: string, body: unknown): Promise<{ signature: string; summary: any }> {
    const built = await this.must("POST", builder, body);
    const signed = signSolanaTx(w, built.transaction);
    const sent = await this.must("POST", "/v1/tx/solana", { tx: signed });
    return { signature: sent.signature, summary: built.summary ?? null };
  }
  /** Wait for a Solana signature to reach `confirmed` (or fail); returns the status. */
  async confirmSolana(sig: string, timeoutMs = 90_000): Promise<{ ok: boolean; err: unknown }> {
    const t0 = Date.now();
    while (Date.now() - t0 < timeoutMs) {
      const r = await this.req("GET", `/v1/tx/solana/${sig}`);
      if (r.status === 200 && (r.body.status === "confirmed" || r.body.status === "finalized")) return { ok: !!r.body.ok, err: r.body.err ?? null };
      await new Promise((res) => setTimeout(res, 2000));
    }
    return { ok: false, err: "not confirmed in time" };
  }
}
