// One trading agent (docs/TRADING.md 4): the slow layer (a paid Cadre thought every
// `thinkEveryMs`) sets the stance; the fast layer reacts to every market event in-process, in
// milliseconds, inside the stance and the risk rules, and signs the order itself (direct RPC,
// never a desktop click). Every fill is anchored on Cadre by a receipt Post that names the
// trade's tx and the request id of the paid thought that set the stance ("decided by req_...",
// the user agents' receipt convention; thought_ref stays for posts whose body IS the completion).
// The latency of every stage is logged (event -> decision -> sent -> landed -> receipt -> visible).
import { appendFileSync, existsSync } from "node:fs";
import type { Wallet } from "../../src/wallet.ts";
import { blake3 } from "../../src/blake3.ts";
import type { Cadre } from "./cadre.ts";
import { parseStrategy, type Strategy, strategyPrompt, type Thought } from "./brain.ts";

/** The slow layer's paid thought: the house Brain, or a user agent's runner (useragent.ts). */
export interface BrainLike {
  ensureRows(): Promise<void>;
  think(messages: { role: string; content: string }[], maxTokens: number): Promise<Thought>;
}
import { type Book, type Fill, type MarketEvent, quoteOut, type Venue, withSlippage } from "./venues.ts";

export interface RiskRules {
  /** Largest single order, quote units (lamports or CADRE base units). */
  maxOrder: bigint;
  /** Most quote value held as tokens (at the pool's mid), quote units. */
  maxPosition: bigint;
  /** Stop trading for the UTC day once realized PnL today falls below -dailyLossStop. */
  dailyLossStop: bigint;
  /** Minimum time between two orders. */
  cooldownMs: number;
  maxOrdersPerHour: number;
  /** min_out = the program's quote minus this (must sit inside the on-chain max_slippage_bps). */
  slippageBps: number;
  /** Refuse to buy after a move up of more than this since the stance was set (no chasing). */
  maxChaseBps: number;
  /** User agents (optional): sell the whole position once its mid value is this many bps above its average cost. */
  takeProfitBps?: number;
  /** User agents (optional): sell the whole position once its mid value is this many bps below its average cost. */
  stopLossBps?: number;
}

export interface TraderOpts {
  handle: string;
  persona: string;
  house: boolean;
  venue: Venue;
  brain: BrainLike | null;
  cadre: Cadre;
  poster: Wallet;
  risk: RiskRules;
  thinkEveryMs: number;
  killFile: string;
  latencyLog: string;
  /** Pool keys of other house traders' pending orders (shared, to refuse house-vs-house crossing). */
  houseBook: Map<string, { side: 0 | 1; at: number; who: string }>;
  log: (e: string, x: Record<string, unknown>) => void;
  postReceipts: boolean;
  /** cadre-api base (https://api.../v1 root without /v1): the receipt counts as visible once the API serves it. */
  apiUrl?: string;
  /** Read the pool this often even without events (ms, default 60,000). */
  pollMs?: number;
  /** User agents (src/useragent.ts): send the receipt Post through the agent runner's own nonce
   * tracking and CADRE cap instead of `cadre.sendNative`. */
  sendPost?: (body: string) => Promise<{ txHash: string; nonce: bigint }>;
  /** User agents: act on BUY / SELL only at this conviction or higher (default 1). */
  minConviction?: { buy: number; sell: number };
  /** User agents: stop trading while this returns a reason (the runner's protocol kill switch, budget). */
  halt?: () => string;
  /** User agents: a label added to every receipt (own launched token: "self-trade in its own launched token, not organic"). */
  label?: string;
  /** User agents: the strategy prompt (persona and the agent's own strategy text) instead of the house one. */
  prompt?: (m: Parameters<typeof strategyPrompt>[1]) => { role: string; content: string }[];
}

export interface LatencyRow {
  agent: string; venue: string; side: number; event_ref: string; ok: boolean; error?: string;
  event_to_decision_ms: number; decision_to_sent_ms: number; sent_to_landed_ms: number;
  landed_at: string; receipt_tx?: string; receipt_sent_ms?: number; receipt_included_ms?: number; receipt_status?: string; receipt_risk?: string; settled_status?: string; settled_risk?: string; visible_ms?: number;
  thought_ttft_ms?: number; thought_total_ms?: number; thought_request?: string; trade_ref: string; amount_in: string; out: string;
}

export class Trader {
  o: TraderOpts;
  /** The exit rule behind the order in flight (take profit / stop loss), named in its receipt. */
  exitWhy = "";
  strategy: Strategy = { stance: "hold", conviction: 1, setAt: 0, requestId: "", text: "" };
  lastThought: { ttftMs: number; totalMs: number } | null = null;
  refPrice = 0;
  lastOrderAt = 0;
  ordersThisHour: number[] = [];
  busy = false;
  thinking = false;
  dayStartRealized: bigint | null = null;
  day = "";
  halted = "";
  book: Book | null = null;
  prices: { t: number; p: number }[] = [];
  thinkTimer: any;
  pollTimer: any;

  constructor(o: TraderOpts) { this.o = o; }

  private price(e: { base: bigint; quote: bigint }) { return e.base === 0n ? 0 : Number(e.quote) / Number(e.base); }

  async start() {
    this.book = await this.o.venue.book();
    await this.o.venue.start((e) => this.onEvent(e));
    // a quiet pool sends no events: read it every pollMs too, so a stance is acted on without outside trades
    this.pollTimer = setInterval(async () => {
      const r = await this.o.venue.reserves().catch(() => null);
      if (r) this.onEvent({ venue: this.o.venue.name, base: r.base, quote: r.quote, at: performance.now(), ref: "poll" });
    }, this.o.pollMs ?? 60_000);
    const loop = async () => {
      await this.think().catch((e) => this.o.log("think_error", { agent: this.o.handle, error: (e as Error).message }));
      this.thinkTimer = setTimeout(loop, this.o.thinkEveryMs);
    };
    loop();
  }
  stop() { clearTimeout(this.thinkTimer); clearInterval(this.pollTimer); this.o.venue.stop(); }

  change(sinceMs: number): number | null {
    const now = Date.now();
    const old = this.prices.find((x) => x.t >= now - sinceMs);
    const last = this.prices.at(-1);
    if (!old || !last || old === last) return null;
    return last.p / old.p - 1;
  }

  /** The slow layer: one paid thought sets stance + conviction. */
  async think() {
    if (!this.o.brain || this.thinking) return;
    this.thinking = true;
    try {
      const r = await this.o.venue.reserves();
      const p = this.price(r);
      this.book = await this.o.venue.book();
      const held = this.book.base === 0n ? 0n : quoteOut(1, r, this.book.base);
      await this.o.brain.ensureRows();
      const m = {
        venue: this.o.venue.name, price: p, change1h: this.change(3_600_000), change24h: this.change(86_400_000),
        positionShare: Number(held) / Number(this.o.risk.maxPosition), realized: Number(this.book.realized) / 1e9, unit: this.o.venue.unit,
      };
      const th = await this.o.brain.think(this.o.prompt ? this.o.prompt(m) : strategyPrompt(this.o.persona, m), 4);
      const s = parseStrategy(th.text);
      this.strategy = { ...s, setAt: Date.now(), requestId: th.requestId, text: th.text.slice(0, 40) };
      this.lastThought = { ttftMs: th.ttftMs, totalMs: th.totalMs };
      this.refPrice = p;
      this.o.log("stance", { agent: this.o.handle, venue: this.o.venue.name, ...s, request_id: th.requestId, rows: th.rows, ttft_ms: Math.round(th.ttftMs), total_ms: Math.round(th.totalMs) });
    } finally {
      this.thinking = false;
    }
  }

  /** The fast layer: deterministic, runs on every market event. Returns the order or null. */
  decide(e: MarketEvent, book: Book): { side: 0 | 1; amountIn: bigint; minOut: bigint; exit?: string } | null {
    const s = this.strategy;
    const r = { base: e.base, quote: e.quote };
    const p = this.price(r);
    const held = book.base === 0n ? 0n : quoteOut(1, r, book.base);
    const ex = this.exit(r, book, held);
    if (ex) return ex;
    const size = (this.o.risk.maxOrder * BigInt(s.conviction)) / 5n;
    if (size <= 0n) return null;
    const mc = this.o.minConviction;
    if (mc && ((s.stance === "buy" && s.conviction < mc.buy) || (s.stance === "sell" && s.conviction < mc.sell))) return null;
    if (s.stance === "buy") {
      if (this.refPrice > 0 && p > this.refPrice * (1 + this.o.risk.maxChaseBps / 10_000)) return null; // no chasing
      const room = this.o.risk.maxPosition - held;
      const x = room < size ? room : size;
      if (x <= 0n || x > book.quote) return null;
      return { side: 0, amountIn: x, minOut: withSlippage(quoteOut(0, r, x), this.o.risk.slippageBps) };
    }
    if (s.stance === "sell" && book.base > 0n) {
      // sell a clip worth about `size` quote units, or everything if less is held
      const clip = held <= size ? book.base : (book.base * size) / (held === 0n ? 1n : held);
      if (clip <= 0n) return null;
      return { side: 1, amountIn: clip, minOut: withSlippage(quoteOut(1, r, clip), this.o.risk.slippageBps) };
    }
    return null;
  }

  /** Take profit / stop loss (user agents, optional): deterministic, ahead of the stance. Sells the
   * whole position when its mid value crosses the bound against its average cost. */
  exit(r: { base: bigint; quote: bigint }, book: Book, held: bigint): { side: 1; amountIn: bigint; minOut: bigint; exit: string } | null {
    const { takeProfitBps: tp, stopLossBps: sl } = this.o.risk;
    if ((tp === undefined && sl === undefined) || book.base <= 0n || !book.cost || book.cost <= 0n) return null;
    const bps = ((held - book.cost) * 10_000n) / book.cost;
    const why = tp !== undefined && bps >= BigInt(tp) ? `take profit at ${bps} bps` : sl !== undefined && bps <= -BigInt(sl) ? `stop loss at ${bps} bps` : "";
    if (!why) return null;
    return { side: 1, amountIn: book.base, minOut: withSlippage(quoteOut(1, r, book.base), this.o.risk.slippageBps), exit: why };
  }

  riskBlock(now: number, book: Book): string {
    if (existsSync(this.o.killFile)) return "kill switch file present";
    const h = this.o.halt?.();
    if (h) return h;
    if (this.halted) return this.halted;
    const day = new Date().toISOString().slice(0, 10);
    if (day !== this.day) { this.day = day; this.dayStartRealized = book.realized; this.halted = ""; }
    if (this.dayStartRealized !== null && book.realized - this.dayStartRealized < -this.o.risk.dailyLossStop) {
      this.halted = `daily loss stop: realized ${book.realized - this.dayStartRealized} today`;
      return this.halted;
    }
    if (now - this.lastOrderAt < this.o.risk.cooldownMs) return "cooldown";
    this.ordersThisHour = this.ordersThisHour.filter((t) => now - t < 3_600_000);
    if (this.ordersThisHour.length >= this.o.risk.maxOrdersPerHour) return "orders per hour";
    return "";
  }

  async onEvent(e: MarketEvent) {
    const p = this.price(e);
    if (p > 0) { this.prices.push({ t: Date.now(), p }); if (this.prices.length > 5_000) this.prices.splice(0, 1_000); }
    if (this.busy || !this.book) return;
    const now = Date.now();
    const block = this.riskBlock(now, this.book);
    if (block) return;
    const order = this.decide(e, this.book);
    const decidedAt = performance.now();
    if (!order) return;
    if (order.exit) this.o.log("exit", { agent: this.o.handle, venue: this.o.venue.name, why: order.exit });
    this.exitWhy = order.exit ?? "";
    // house traders never cross each other on the same pool inside 2 minutes (no house wash trades)
    const key = `${this.o.venue.name}:${this.o.venue.describe().pool}`;
    const other = this.o.houseBook.get(key);
    if (this.o.house && other && other.who !== this.o.handle && other.side !== order.side && now - other.at < 120_000) {
      this.o.log("refused", { agent: this.o.handle, reason: "would cross another house trader", other: other.who });
      return;
    }
    this.busy = true;
    if (this.o.house) this.o.houseBook.set(key, { side: order.side, at: now, who: this.o.handle });
    this.lastOrderAt = now;
    this.ordersThisHour.push(now);
    try {
      const f = await this.o.venue.trade(order.side, order.amountIn, order.minOut);
      const row: LatencyRow = {
        agent: this.o.handle, venue: f.venue, side: f.side, event_ref: e.ref, ok: f.ok, error: f.error,
        event_to_decision_ms: +(decidedAt - e.at).toFixed(3), decision_to_sent_ms: +(f.t.sent - decidedAt).toFixed(1), sent_to_landed_ms: +(f.t.landed - f.t.sent).toFixed(0),
        landed_at: f.landedAt, trade_ref: f.ref, amount_in: f.amountIn.toString(), out: f.out.toString(),
        thought_ttft_ms: this.lastThought ? Math.round(this.lastThought.ttftMs) : undefined, thought_total_ms: this.lastThought ? Math.round(this.lastThought.totalMs) : undefined, thought_request: this.strategy.requestId,
      };
      this.o.log(f.ok ? "fill" : "order_failed", { agent: this.o.handle, ...row });
      if (f.ok && f.venue === "cadre-amm") {
        // Cadre-side trade: act on it only once it reaches the stage its CADRE notional needs (preconf gate)
        const notional = f.side === 0 ? f.amountIn : f.out;
        const st = await this.o.cadre.waitSettled(f.ref, notional).catch((err: Error) => ({ status: "unsettled", risk: `not acted: ${err.message}` }));
        row.settled_status = st.status;
        row.settled_risk = st.risk;
        this.o.log("settled", { agent: this.o.handle, tx: f.ref, status: st.status, risk: st.risk });
      }
      if (f.ok) {
        this.book = await this.o.venue.book().catch(() => this.book);
        if (this.o.postReceipts) await this.receipt(f, row);
      }
      appendFileSync(this.o.latencyLog, JSON.stringify({ t: new Date().toISOString(), ...row }) + "\n");
    } finally {
      this.busy = false;
    }
  }

  /** The receipt Post on Cadre: names the trade tx and the deciding thought (thought_ref). */
  async receipt(f: Fill, row: LatencyRow) {
    const side = f.side === 0 ? "buy" : "sell";
    const u = this.o.venue.unit;
    const q = (x: bigint) => (Number(x) / 1e9).toString(); // SOL and chain CADRE: 9 decimals
    const b = (x: bigint) => (Number(x) / 10 ** this.o.venue.baseDecimals).toString();
    const tk = this.o.venue.baseSymbol;
    const what = f.side === 0 ? `${q(f.amountIn)} ${u} for ${b(f.out)} ${tk}` : `${b(f.amountIn)} ${tk} for ${q(f.out)} ${u}`;
    const body = `receipt ${this.o.venue.receiptRef(f)} ${this.o.venue.name} ${side}: ${what}; ${this.o.house ? "house trade; " : "own wallet; "}${this.o.label ? `${this.o.label}; ` : ""}${this.exitWhy ? `${this.exitWhy}; ` : ""}stance ${this.strategy.stance} ${this.strategy.conviction} decided by ${this.strategy.requestId || "none"}`;
    // machine-written (no model text in it), so the model-output filter does not apply; signatures are base58 and would trip its key check
    if (Buffer.byteLength(body) > 1024) { this.o.log("receipt_refused", { agent: this.o.handle, reason: "over 1024 bytes" }); return; }
    const t0 = performance.now();
    try {
      const s = this.o.sendPost ? await this.o.sendPost(body) : await this.o.cadre.sendNative(this.o.poster, { kind: "Post", postKind: 0, body });
      row.receipt_tx = s.txHash;
      row.receipt_sent_ms = +(performance.now() - t0).toFixed(0);
      // a receipt moves no value: the gate lets it count from preconfirmed (medium confidence), with the stated risk
      const st = await this.o.cadre.waitSettled(s.txHash, 0n, undefined, { timeoutMs: 900_000, pollMs: 500 });
      if (st.status === "included" || st.status === "final") row.receipt_included_ms = +(performance.now() - t0).toFixed(0);
      row.receipt_status = st.status;
      row.receipt_risk = st.risk;
      this.o.log("receipt", { agent: this.o.handle, tx: s.txHash, status: st.status, risk: st.risk, height: st.tx_status.height ?? null, settled_ms: st.waited_ms, trade: f.ref });
      if (this.o.apiUrl) {
        const n = Buffer.alloc(8);
        n.writeBigUInt64LE(s.nonce);
        const pid = Buffer.from(blake3(Buffer.from("pearl-sol:post:v1"), this.o.poster.publicKey, n)).toString("hex");
        for (let i = 0; i < 240; i++) {
          const r = await fetch(`${this.o.apiUrl}/v1/posts/${pid}`, { signal: AbortSignal.timeout(10_000) }).catch(() => null);
          if (r?.status === 200) { row.visible_ms = +(performance.now() - t0).toFixed(0); break; }
          await new Promise((res) => setTimeout(res, 1_000));
        }
      }
    } catch (e) {
      this.o.log("receipt_failed", { agent: this.o.handle, error: (e as Error).message });
    }
  }
}
