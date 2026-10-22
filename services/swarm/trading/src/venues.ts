// The two venues behind one interface (docs/TRADING.md 3):
//  - PumpSwapMandate: CADRE on its canonical PumpSwap pool on Solana, traded by the agent key
//    through the user's mandate (token/programs/mandate). Market events: websocket account
//    subscriptions on the pool's vaults (processed commitment). Orders: direct RPC signing, a
//    compute-unit price (priority fee), blockhash kept warm, server-side preflight simulation.
//  - CadreAmm: a token against CADRE on the Cadre AMM contract, traded from the agent's own
//    wallet (house) or through the Cadre mandate contract. Market events: the node's websocket
//    ("heads" and the pending "txs" stream), then a pool query.
// Quote units: lamports (Solana) or CADRE base units (Cadre); base: token base units.
import { ComputeBudgetProgram, Connection, Keypair, PublicKey, Transaction } from "@solana/web3.js";
import type { Wallet } from "../../src/wallet.ts";
import { Abi, type Cadre, errText } from "./cadre.ts";
import { ata, cpOut, decodeMandate, type MandateProgram, pnlOf, type PoolInfo, readReserves, reservesOf, swapEventsFromLogs, VAULT_RENT } from "./solmandate.ts";

export interface MarketEvent { venue: string; base: bigint; quote: bigint; at: number; ref: string }
export interface Book { quote: bigint; base: bigint; realized: bigint }
export interface Fill {
  ok: boolean; venue: string; side: 0 | 1; amountIn: bigint; minOut: bigint; out: bigint; ref: string; error?: string;
  t: { built: number; sent: number; landed: number }; landedAt: string; fee: bigint;
}

export interface Venue {
  name: string;
  unit: string;
  baseSymbol: string;
  baseDecimals: number;
  start(onEvent: (e: MarketEvent) => void): Promise<void>;
  stop(): void;
  reserves(): Promise<{ base: bigint; quote: bigint }>;
  book(): Promise<Book>;
  trade(side: 0 | 1, amountIn: bigint, minOut: bigint): Promise<Fill>;
  /** "sol:<sig>" or "cadre:<tx hash>" for the receipt post. */
  receiptRef(f: Fill): string;
  describe(): Record<string, string>;
}

export function quoteOut(side: 0 | 1, r: { base: bigint; quote: bigint }, x: bigint) {
  return side === 0 ? cpOut(r.quote, r.base, x) : cpOut(r.base, r.quote, x);
}
export const withSlippage = (q: bigint, bps: number) => (q * BigInt(10_000 - bps)) / 10_000n;

// ------------------------------------------------------------------ Solana

export class PumpSwapMandate implements Venue {
  name = "pumpswap";
  unit = "SOL";
  baseSymbol = "CADRE";
  baseDecimals = 6; // pump.fun mints (CADRE SPL) have 6 decimals
  conn: Connection;
  wsConn: Connection;
  agent: Keypair;
  mp: MandateProgram;
  mandate: PublicKey;
  pool: PoolInfo;
  priorityMicroLamports: number;
  subs: number[] = [];
  blockhash: { hash: string; lastValid: number; at: number } | null = null;
  timer: any;
  last: { pool?: Buffer; base?: Buffer; quote?: Buffer } = {};
  constructor(o: { rpc: string; ws?: string; agent: Keypair; mp: MandateProgram; mandate: PublicKey; pool: PoolInfo; priorityMicroLamports?: number }) {
    this.conn = new Connection(o.rpc, { commitment: "processed", wsEndpoint: o.ws });
    this.wsConn = this.conn;
    this.agent = o.agent;
    this.mp = o.mp;
    this.mandate = o.mandate;
    this.pool = o.pool;
    this.priorityMicroLamports = o.priorityMicroLamports ?? 10_000;
  }
  describe() { return { venue: this.name, mandate: this.mandate.toBase58(), pool: this.pool.pool.toBase58(), agent: this.agent.publicKey.toBase58() }; }

  private async warm() {
    const b = await this.conn.getLatestBlockhash("processed");
    this.blockhash = { hash: b.blockhash, lastValid: b.lastValidBlockHeight, at: Date.now() };
  }

  async start(onEvent: (e: MarketEvent) => void) {
    await this.warm();
    this.timer = setInterval(() => this.warm().catch(() => {}), 15_000);
    const init = await this.conn.getMultipleAccountsInfo([this.pool.pool, this.pool.baseVault, this.pool.quoteVault], "processed");
    this.last = { pool: init[0]!.data, base: init[1]!.data, quote: init[2]!.data };
    const fire = (slot: number) => {
      const { pool, base, quote } = this.last;
      if (!pool || !base || !quote) return;
      const r = reservesOf(pool, base, quote, slot);
      onEvent({ venue: this.name, base: r.base, quote: r.quote, at: performance.now(), ref: `slot:${slot}` });
    };
    // vault changes arrive within the slot they happen (processed); each trade moves both vaults
    this.subs.push(this.wsConn.onAccountChange(this.pool.quoteVault, (a, ctx) => { this.last.quote = a.data; fire(ctx.slot); }, { commitment: "processed" }));
    this.subs.push(this.wsConn.onAccountChange(this.pool.baseVault, (a, ctx) => { this.last.base = a.data; fire(ctx.slot); }, { commitment: "processed" }));
    this.subs.push(this.wsConn.onAccountChange(this.pool.pool, (a) => { this.last.pool = a.data; }, { commitment: "processed" }));
  }
  stop() {
    clearInterval(this.timer);
    for (const s of this.subs) this.wsConn.removeAccountChangeListener(s).catch(() => {});
    this.subs = [];
  }
  async reserves() { return readReserves(this.conn, this.pool); }

  async book(): Promise<Book> {
    const v = this.mp.vault(this.mandate);
    const [m, va, ta] = await this.conn.getMultipleAccountsInfo([this.mandate, v, ata(v, this.pool.mint, this.pool.baseTokenProgram)], "confirmed");
    if (!m) throw new Error("mandate account missing");
    const md = decodeMandate(m.data);
    const lamports = BigInt(va?.lamports ?? 0);
    return { quote: lamports > VAULT_RENT ? lamports - VAULT_RENT : 0n, base: ta && ta.data.length >= 72 ? ta.data.readBigUInt64LE(64) : 0n, realized: pnlOf(md, lamports) };
  }

  async trade(side: 0 | 1, amountIn: bigint, minOut: bigint): Promise<Fill> {
    const built = performance.now();
    if (!this.blockhash || Date.now() - this.blockhash.at > 30_000) await this.warm();
    const tx = new Transaction({ feePayer: this.agent.publicKey, blockhash: this.blockhash!.hash, lastValidBlockHeight: this.blockhash!.lastValid });
    tx.add(ComputeBudgetProgram.setComputeUnitLimit({ units: 200_000 }));
    if (this.priorityMicroLamports > 0) tx.add(ComputeBudgetProgram.setComputeUnitPrice({ microLamports: this.priorityMicroLamports }));
    tx.add(this.mp.swap(this.agent.publicKey, this.mandate, this.pool, side, amountIn, minOut));
    tx.sign(this.agent);
    const raw = tx.serialize();
    const sent = performance.now();
    const fee = 5_000n + BigInt(Math.ceil((200_000 * this.priorityMicroLamports) / 1e6));
    let sig = "";
    try {
      // server-side preflight simulation at "processed": a trade that would fail is refused before it costs a fee
      sig = await this.conn.sendRawTransaction(raw, { skipPreflight: false, preflightCommitment: "processed", maxRetries: 3 });
    } catch (e) {
      return { ok: false, venue: this.name, side, amountIn, minOut, out: 0n, ref: "", error: (e as Error).message.slice(0, 300), t: { built, sent, landed: performance.now() }, landedAt: "", fee: 0n };
    }
    const conf = await this.conn.confirmTransaction({ signature: sig, blockhash: this.blockhash!.hash, lastValidBlockHeight: this.blockhash!.lastValid }, "confirmed");
    const landed = performance.now();
    if (conf.value.err) return { ok: false, venue: this.name, side, amountIn, minOut, out: 0n, ref: sig, error: JSON.stringify(conf.value.err), t: { built, sent, landed }, landedAt: `slot:${conf.context.slot}`, fee };
    const t = await this.conn.getTransaction(sig, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
    const ev = swapEventsFromLogs(t?.meta?.logMessages ?? [])[0];
    return { ok: true, venue: this.name, side, amountIn: ev?.amountIn ?? amountIn, minOut, out: ev?.amountOut ?? 0n, ref: sig, t: { built, sent, landed }, landedAt: `slot:${t?.slot ?? conf.context.slot}`, fee: BigInt(t?.meta?.fee ?? Number(fee)) };
  }
  receiptRef(f: Fill) { return `sol:${f.ref}`; }
}

// ------------------------------------------------------------------ Cadre

export class CadreAmm implements Venue {
  name = "cadre-amm";
  unit = "CADRE";
  baseSymbol = "DESK";
  baseDecimals = 9;
  c: Cadre;
  wsUrl: string;
  wallet: Wallet;
  pool: string;
  token: string;
  mandate?: { contract: string; id: bigint };
  ws: WebSocket | null = null;
  constructor(o: { cadre: Cadre; wsUrl: string; wallet: Wallet; pool: string; token: string; mandate?: { contract: string; id: bigint } }) {
    this.c = o.cadre;
    this.wsUrl = o.wsUrl;
    this.wallet = o.wallet;
    this.pool = o.pool;
    this.token = o.token;
    this.mandate = o.mandate;
  }
  describe(): Record<string, string> { return { venue: this.name, pool: this.pool, token: this.token, agent: this.wallet.address, ...(this.mandate ? { mandate: `${this.mandate.contract}#${this.mandate.id}` } : {}) }; }

  async reserves() {
    const r = await this.c.query(this.pool, Abi.method("info").out());
    r.addr();
    const quote = r.u64();
    const base = r.u64();
    return { base, quote };
  }

  async start(onEvent: (e: MarketEvent) => void) {
    const fire = async (ref: string) => {
      const at = performance.now();
      const r = await this.reserves().catch(() => null);
      if (r) onEvent({ venue: this.name, base: r.base, quote: r.quote, at, ref });
    };
    const open = () => {
      const ws = new WebSocket(this.wsUrl);
      this.ws = ws;
      ws.onopen = () => ws.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "subscribe", params: { topics: ["heads"] } }));
      ws.onmessage = (m: MessageEvent) => {
        try {
          const j = JSON.parse(String(m.data));
          const d = j?.params?.data;
          if (j?.params?.topic === "heads" && d) fire(`height:${d.height ?? "?"}`);
        } catch { /* ignore */ }
      };
      ws.onclose = () => { if (this.ws === ws) setTimeout(open, 3_000); };
      ws.onerror = () => { try { ws.close(); } catch { /* */ } };
    };
    open();
    await fire("start");
  }
  stop() { const w = this.ws; this.ws = null; try { w?.close(); } catch { /* */ } }

  async tokenBalance(owner: string) {
    return (await this.c.query(this.token, Abi.method("balance_of").addr(owner).out())).u64();
  }

  async book(): Promise<Book> {
    if (this.mandate) {
      const r = await this.c.query(this.mandate.contract, Abi.method("get").u64(this.mandate.id).out());
      r.o = 96 + 2 + 8 + 1;
      r.o += 8 * 3 + 2 + 2 + 8 + 2 + 8 + 1 + 1;
      const n = r.u8();
      r.o += 32 * n;
      const equity = r.u64(), deposited = r.u64(), withdrawn = r.u64(), fa = r.u64(), fp = r.u64();
      const pos = (await this.c.query(this.mandate.contract, Abi.method("position").u64(this.mandate.id).addr(this.token).out())).u64();
      return { quote: equity, base: pos, realized: equity + withdrawn + fa + fp - deposited };
    }
    // a house wallet: realized PnL is computed by the indexer from Swap events (pnl.ts); here the live book
    return { quote: await this.c.balance(this.wallet.address), base: await this.tokenBalance(this.wallet.address), realized: 0n };
  }

  async trade(side: 0 | 1, amountIn: bigint, minOut: bigint): Promise<Fill> {
    const built = performance.now();
    let sent = built;
    try {
      const h = await this.c.height();
      const maxHeight = BigInt(h + 3); // a stale order cannot land later than 3 blocks from now
      let s;
      if (this.mandate) {
        const args = Abi.method("swap").u64(this.mandate.id).addr(this.pool).u8(side).u64(amountIn).u64(minOut).u64(maxHeight).out();
        sent = performance.now();
        s = await this.c.sendContract(this.wallet, { kind: "Call", contract: this.mandate.contract, value: 0n, gasLimit: 0n, args });
      } else if (side === 0) {
        sent = performance.now();
        s = await this.c.sendContract(this.wallet, { kind: "Call", contract: this.pool, value: amountIn, gasLimit: 0n, args: Abi.method("swap_cadre_for_token").u64(minOut).addr(this.wallet.address).u64(maxHeight).out() });
      } else {
        const ap = await this.c.sendContract(this.wallet, { kind: "Call", contract: this.token, value: 0n, gasLimit: 0n, args: Abi.method("approve").addr(this.pool).u64(amountIn).out() });
        sent = performance.now();
        // the approve and the swap go back to back in one block (consecutive nonces); the swap is not simulated first
        s = await this.c.sendContract(this.wallet, { kind: "Call", contract: this.pool, value: 0n, gasLimit: 1_500_000n, args: Abi.method("swap_token_for_cadre").u64(amountIn).u64(minOut).addr(this.wallet.address).u64(maxHeight).out() });
        void ap;
      }
      const inc = await this.c.waitIncluded(s.txHash, 600_000, 500);
      const landed = performance.now();
      const ok = inc.receipt ? inc.receipt.ok : true;
      const out = ok && inc.receipt?.return ? Buffer.from(inc.receipt.return, "hex").readBigUInt64LE(0) : 0n;
      return { ok, venue: this.name, side, amountIn, minOut, out, ref: s.txHash, error: ok ? undefined : errText(inc.receipt?.error), t: { built, sent, landed }, landedAt: `height:${inc.height}`, fee: BigInt(inc.receipt?.fee_charged ?? s.fee) };
    } catch (e) {
      return { ok: false, venue: this.name, side, amountIn, minOut, out: 0n, ref: "", error: (e as Error).message.slice(0, 300), t: { built, sent, landed: performance.now() }, landedAt: "", fee: 0n };
    }
  }
  receiptRef(f: Fill) { return `cadre:${f.ref}`; }
}
