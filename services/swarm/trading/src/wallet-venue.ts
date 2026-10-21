// PumpSwap from the agent's OWN wallet (docs/TRADING.md 5.6, owner 2026-10-21: user agents are
// their own bosses and trade on Solana from their own wallet). The same Venue interface as the
// house venues, so the Trader's fast layer, risk rules, receipts and latency rows are the same
// code. Orders are the exact txs a person gets on /buy (cadre-api POST /v1/tx/pump-buy|pump-sell,
// with `min_*_out` set by the fast layer), signed here by the agent key and sent straight to the
// Solana RPC with preflight. Market events: websocket subscriptions on the pool's vaults
// (processed), plus the Trader's periodic poll.
//
// Book: SOL above a fee reserve, the token balance of the agent's Token-2022 account, and realized
// PnL by average cost over this agent's own fills (each fill's amounts read back from the landed
// tx's balance changes, never from the quote). The fills are kept in a JSON file so a restart keeps
// the day's loss stop honest; cadre-api recomputes the same numbers from the chain on its own.
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { Connection, PublicKey } from "@solana/web3.js";
import type { Wallet } from "../../src/wallet.ts";
import { signSolanaTx, txSignature } from "../../agents/src/chain.ts";
import { ata, poolInfo, type PoolInfo, readReserves, reservesOf, T22 } from "./solmandate.ts";
import type { Book, Fill, MarketEvent, Venue } from "./venues.ts";

/** Lamports kept back from trading for tx fees, account rent and x402 top-ups (proposed default). */
export const FEE_RESERVE = 30_000_000n;

export interface WalletFill { sig: string; side: 0 | 1; amountIn: string; out: string; slot: number; at: number }

/** Average-cost realized PnL over own fills (quote units), the same rule as cadre-api avgCostPnl. */
export function avgCost(fills: { side: 0 | 1; amountIn: bigint; out: bigint }[]) {
  let pos = 0n, cost = 0n, realized = 0n;
  for (const f of fills) {
    if (f.side === 0) { pos += f.out; cost += f.amountIn; continue; }
    if (pos <= 0n) continue;
    const take = f.amountIn > pos ? pos : f.amountIn;
    const basis = (cost * take) / pos;
    realized += (f.out * take) / (f.amountIn === 0n ? 1n : f.amountIn) - basis;
    cost -= basis;
    pos -= take;
  }
  return { position: pos, cost, realized };
}

/**
 * Amounts a landed tx moved for `owner`: SOL change (fee added back on a sell, a new token
 * account's rent taken out on a buy) and the token change of `mint`. Returns null when the tx
 * moved no tokens of that mint for that owner.
 */
export function fillFromTx(t: any, owner: string, mint: string): { side: 0 | 1; amountIn: bigint; out: bigint } | null {
  const m = t?.transaction?.message;
  const keys: string[] = (m?.accountKeys ?? m?.staticAccountKeys ?? []).map((k: any) => (typeof k === "string" ? k : k?.pubkey?.toBase58?.() ?? k?.toBase58?.() ?? String(k)));
  const i = keys.indexOf(owner);
  const meta = t?.meta;
  if (i < 0 || !meta) return null;
  const tok = (list: any[]) => (list ?? []).filter((b) => b.mint === mint && b.owner === owner).reduce((s, b) => s + BigInt(b.uiTokenAmount?.amount ?? 0), 0n);
  const dTok = tok(meta.postTokenBalances) - tok(meta.preTokenBalances);
  if (dTok === 0n) return null;
  const fee = BigInt(meta.fee ?? 0);
  const dSol = BigInt(meta.postBalances[i]) - BigInt(meta.preBalances[i]);
  if (dTok > 0n) {
    // a first buy creates the token account: its rent stays the agent's, it is not a trade cost
    const had = (meta.preTokenBalances ?? []).some((b: any) => b.mint === mint && b.owner === owner);
    let rent = 0n;
    if (!had) {
      const post = (meta.postTokenBalances ?? []).find((b: any) => b.mint === mint && b.owner === owner);
      if (post) rent = BigInt(meta.postBalances[post.accountIndex] ?? 0);
    }
    const spent = -dSol - fee - rent;
    return { side: 0, amountIn: spent > 0n ? spent : 0n, out: dTok };
  }
  const got = dSol + fee;
  return { side: 1, amountIn: -dTok, out: got > 0n ? got : 0n };
}

export class PumpSwapWallet implements Venue {
  name = "pumpswap";
  unit = "SOL";
  baseSymbol: string;
  baseDecimals = 6;
  conn: Connection;
  wallet: Wallet;
  owner: PublicKey;
  mint: PublicKey;
  apiUrl: string;
  pool!: PoolInfo;
  fillsFile: string;
  fills: WalletFill[] = [];
  subs: number[] = [];
  last: { pool?: Buffer; base?: Buffer; quote?: Buffer } = {};
  feeReserve: bigint;
  constructor(o: { rpc: string; ws?: string; wallet: Wallet; mint: string; apiUrl: string; fillsFile: string; symbol?: string; feeReserve?: bigint; conn?: Connection }) {
    this.conn = o.conn ?? new Connection(o.rpc, { commitment: "processed", wsEndpoint: o.ws });
    this.wallet = o.wallet;
    this.owner = new PublicKey(o.wallet.address);
    this.mint = new PublicKey(o.mint);
    this.apiUrl = o.apiUrl.replace(/\/$/, "");
    this.fillsFile = o.fillsFile;
    this.baseSymbol = o.symbol ?? "CADRE";
    this.feeReserve = o.feeReserve ?? FEE_RESERVE;
    if (existsSync(this.fillsFile)) { try { this.fills = JSON.parse(readFileSync(this.fillsFile, "utf8")); } catch { this.fills = []; } }
  }
  describe() { return { venue: this.name, via: "wallet", pool: this.pool?.pool.toBase58() ?? "", mint: this.mint.toBase58(), agent: this.wallet.address }; }

  async init() { if (!this.pool) this.pool = await poolInfo(this.conn, this.mint, T22); }

  async start(onEvent: (e: MarketEvent) => void) {
    await this.init();
    const init = await this.conn.getMultipleAccountsInfo([this.pool.pool, this.pool.baseVault, this.pool.quoteVault], "processed");
    this.last = { pool: init[0]!.data, base: init[1]!.data, quote: init[2]!.data };
    const fire = (slot: number) => {
      const { pool, base, quote } = this.last;
      if (!pool || !base || !quote) return;
      const r = reservesOf(pool, base, quote, slot);
      onEvent({ venue: this.name, base: r.base, quote: r.quote, at: performance.now(), ref: `slot:${slot}` });
    };
    this.subs.push(this.conn.onAccountChange(this.pool.quoteVault, (a, ctx) => { this.last.quote = a.data; fire(ctx.slot); }, { commitment: "processed" }));
    this.subs.push(this.conn.onAccountChange(this.pool.baseVault, (a, ctx) => { this.last.base = a.data; fire(ctx.slot); }, { commitment: "processed" }));
    this.subs.push(this.conn.onAccountChange(this.pool.pool, (a) => { this.last.pool = a.data; }, { commitment: "processed" }));
  }
  stop() { for (const s of this.subs) this.conn.removeAccountChangeListener(s).catch(() => {}); this.subs = []; }
  async reserves() { await this.init(); return readReserves(this.conn, this.pool); }

  realized() {
    return avgCost(this.fills.map((f) => ({ side: f.side, amountIn: BigInt(f.amountIn), out: BigInt(f.out) }))).realized;
  }

  async book(): Promise<Book> {
    const [sol, ta] = await this.conn.getMultipleAccountsInfo([this.owner, ata(this.owner, this.mint, T22)], "confirmed");
    const lam = BigInt(sol?.lamports ?? 0);
    return { quote: lam > this.feeReserve ? lam - this.feeReserve : 0n, base: ta && ta.data.length >= 72 ? ta.data.readBigUInt64LE(64) : 0n, realized: this.realized() };
  }

  private async build(side: 0 | 1, amountIn: bigint, minOut: bigint): Promise<{ transaction: string; summary: any; recent_blockhash?: string; last_valid_block_height?: number }> {
    const a = this.wallet.address;
    const [path, body] = side === 0
      ? ["/v1/tx/pump-buy", { buyer: a, sol_in: amountIn.toString(), min_tokens_out: minOut.toString(), mint: this.mint.toBase58() }]
      : ["/v1/tx/pump-sell", { seller: a, amount_spl: amountIn.toString(), min_sol_out: minOut.toString(), mint: this.mint.toBase58() }];
    const r = await fetch(this.apiUrl + path, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body), signal: AbortSignal.timeout(15_000) });
    const j: any = await r.json().catch(() => ({}));
    if (r.status !== 200 || typeof j.transaction !== "string") throw new Error(`${path} ${r.status}: ${j?.error?.message ?? j?.message ?? JSON.stringify(j).slice(0, 200)}`);
    return j;
  }

  async trade(side: 0 | 1, amountIn: bigint, minOut: bigint): Promise<Fill> {
    const built = performance.now();
    let sent = built;
    let sig = "";
    try {
      const b = await this.build(side, amountIn, minOut);
      const signed = signSolanaTx(this.wallet, b.transaction);
      sig = txSignature(signed);
      const bh = b.recent_blockhash && b.last_valid_block_height ? { blockhash: b.recent_blockhash, lastValidBlockHeight: b.last_valid_block_height } : await this.conn.getLatestBlockhash("confirmed");
      sent = performance.now();
      // preflight at "processed": an order that would fail now (slippage, balance) costs no fee
      await this.conn.sendRawTransaction(Buffer.from(signed, "base64"), { skipPreflight: false, preflightCommitment: "processed", maxRetries: 3 });
      const conf = await this.conn.confirmTransaction({ signature: sig, blockhash: bh.blockhash, lastValidBlockHeight: bh.lastValidBlockHeight }, "confirmed");
      const landed = performance.now();
      if (conf.value.err) return { ok: false, venue: this.name, side, amountIn, minOut, out: 0n, ref: sig, error: JSON.stringify(conf.value.err), t: { built, sent, landed }, landedAt: `slot:${conf.context.slot}`, fee: 0n };
      const t = await this.conn.getTransaction(sig, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
      const f = fillFromTx(t, this.wallet.address, this.mint.toBase58());
      const realIn = f?.amountIn ?? amountIn;
      const out = f?.out ?? 0n;
      this.fills.push({ sig, side, amountIn: realIn.toString(), out: out.toString(), slot: t?.slot ?? conf.context.slot, at: Date.now() });
      try { writeFileSync(this.fillsFile, JSON.stringify(this.fills.slice(-5_000))); } catch { /* the chain keeps the record */ }
      return { ok: true, venue: this.name, side, amountIn: realIn, minOut, out, ref: sig, t: { built, sent, landed }, landedAt: `slot:${t?.slot ?? conf.context.slot}`, fee: BigInt(t?.meta?.fee ?? 0) };
    } catch (e) {
      return { ok: false, venue: this.name, side, amountIn, minOut, out: 0n, ref: sig, error: (e as Error).message.slice(0, 300), t: { built, sent, landed: performance.now() }, landedAt: "", fee: 0n };
    }
  }
  receiptRef(f: Fill) { return `sol:${f.ref}`; }
}
