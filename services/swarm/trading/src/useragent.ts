// The full trading engine for user agents (docs/TRADING.md 5.6). A user agent with the trade skill
// (agent spec v2, services/swarm/agents/src/spec.ts TradeSpec) gets one Trader per mint: the same
// fast layer, risk rules, receipts and latency rows as the house traders, on PumpSwap from the
// agent's OWN wallet (wallet-venue.ts). Its paid strategy thoughts go through the agent runner, so
// they come out of the agent's own prepaid balance and its spec's daily row and SOL caps. Every
// number comes from the spec and is fixed at creation; the paid thought only picks the stance.
// Nothing here takes orders from the creator: the only stops are the protocol-wide kill file and
// the runner's own halt (budget), never a creator request.
import type { Wallet } from "../../src/wallet.ts";
import type { Thought } from "./brain.ts";
import type { Cadre } from "./cadre.ts";
import { type BrainLike, type RiskRules, Trader } from "./trader.ts";
import { PumpSwapWallet } from "./wallet-venue.ts";
import { walletVenueFor } from "./curve-venue.ts";

/** The trade section of a v2 agent spec (kept structural here so this package has no import cycle). */
export interface TradeSpecLike {
  venue: "pumpswap"; mints: string[]; max_order_lamports: string; max_position_lamports: string; daily_loss_stop_lamports: string;
  cooldown_s: number; max_orders_per_hour: number; slippage_bps: number; max_chase_bps: number; stance_every_min: number;
  buy_min_conviction: number; sell_min_conviction: number; strategy: string;
}

export function riskFromSpec(t: TradeSpecLike): RiskRules {
  return {
    maxOrder: BigInt(t.max_order_lamports), maxPosition: BigInt(t.max_position_lamports), dailyLossStop: BigInt(t.daily_loss_stop_lamports),
    cooldownMs: t.cooldown_s * 1000, maxOrdersPerHour: t.max_orders_per_hour, slippageBps: t.slippage_bps, maxChaseBps: t.max_chase_bps,
  };
}

/** The user agent's strategy prompt: its persona and its current strategy in words, then numbers only. */
export function userStrategyPrompt(persona: string, strategy: string, m: { venue: string; price: number; change1h: number | null; change24h: number | null; positionShare: number; realized: number; unit: string }) {
  const pct = (x: number | null) => (x === null ? "unknown" : `${(x * 100).toFixed(2)}%`);
  const st = strategy.trim() ? ` Your strategy: ${strategy.trim().slice(0, 600)}` : "";
  return [
    { role: "system", content: `You are ${persona.slice(0, 1200)}, a trading agent on Cadre that trades its own money.${st} You only see numbers. Answer with exactly one word, BUY, SELL or HOLD, then a conviction number from 1 to 5.` },
    { role: "user", content: `Market ${m.venue}. Price ${m.price.toPrecision(6)} ${m.unit} per token. Change 1h ${pct(m.change1h)}, 24h ${pct(m.change24h)}. Your position is ${(m.positionShare * 100).toFixed(0)}% of your limit. Realized PnL ${m.realized.toFixed(6)} ${m.unit}. Decide.` },
  ];
}

export interface UserTradingOpts {
  handle: string;
  persona: string;
  strategy: () => string; // the creator's initial strategy or the agent's own rewrite (agent_self)
  trade: TradeSpecLike;
  wallet: Wallet;
  cadre: Cadre; // inclusion waits for receipts
  apiUrl: string; // cadre-api: tx builders (the same as /buy) and receipt visibility
  solRpc: string;
  solWs?: string;
  stateDir: string;
  killFile: string;
  latencyLog: string;
  think: (messages: { role: string; content: string }[], maxTokens: number) => Promise<Thought | null>;
  ensureRows: () => Promise<void>;
  sendPost: (body: string) => Promise<{ txHash: string; nonce: bigint }>;
  halt: () => string;
  log: (e: string, x: Record<string, unknown>) => void;
  venueFor?: (mint: string) => PumpSwapWallet; // tests
}

export class UserTrading {
  traders: Trader[] = [];
  o: UserTradingOpts;
  constructor(o: UserTradingOpts) { this.o = o; }

  brain(): BrainLike {
    return {
      ensureRows: () => this.o.ensureRows(),
      think: async (messages, maxTokens) => {
        const t = await this.o.think(messages, maxTokens);
        if (!t) throw new Error("budget: the spec's daily rows or SOL cap refused this paid thought");
        return t;
      },
    };
  }

  build(): Trader[] {
    const o = this.o;
    this.traders = o.trade.mints.map((mint) => {
      const venue = o.venueFor?.(mint) ?? new PumpSwapWallet({ rpc: o.solRpc, ws: o.solWs, wallet: o.wallet, mint, apiUrl: o.apiUrl, fillsFile: `${o.stateDir}/fills-${o.wallet.address}-${mint}.json` });
      return this.trader(venue);
    });
    return this.traders;
  }

  /** One Trader on `venue` with the spec's risk rules (shared by build and addOwnToken). */
  trader(venue: PumpSwapWallet, label?: string): Trader {
    const o = this.o;
    return new Trader({
      handle: o.handle, persona: o.persona, house: false, venue, brain: this.brain(), cadre: o.cadre, poster: o.wallet, risk: riskFromSpec(o.trade),
      thinkEveryMs: o.trade.stance_every_min * 60_000, killFile: o.killFile, latencyLog: o.latencyLog, houseBook: new Map(), log: o.log,
      postReceipts: true, apiUrl: o.apiUrl, sendPost: o.sendPost, halt: o.halt,
      minConviction: { buy: o.trade.buy_min_conviction, sell: o.trade.sell_min_conviction },
      prompt: (m) => userStrategyPrompt(o.persona, o.strategy(), m), label,
    });
  }

  /** launch.trade_own (docs/TRADING.md 5.7): also trade a token this agent launched, inside the same
   * risk rules, on its curve until it graduates (then its PumpSwap pool). Every receipt says it is a
   * self-trade in the agent's own token; cadre-api keeps those fills out of organic volume. */
  async addOwnToken(mint: string): Promise<Trader | null> {
    const o = this.o;
    if (this.traders.some((t) => t.o.venue.describe().mint === mint)) return null;
    const venue = o.venueFor?.(mint) ?? await walletVenueFor({ rpc: o.solRpc, ws: o.solWs, wallet: o.wallet, mint, apiUrl: o.apiUrl, fillsFile: `${o.stateDir}/fills-${o.wallet.address}-${mint}.json`, symbol: "OWN" });
    const t = this.trader(venue, "self-trade in its own launched token, not organic");
    this.traders.push(t);
    await t.start();
    o.log("trader_started", { agent: o.handle, own_token: true, ...venue.describe() });
    return t;
  }

  async start() {
    if (!this.traders.length) this.build();
    for (const t of this.traders) {
      await t.start();
      this.o.log("trader_started", { agent: this.o.handle, ...t.o.venue.describe() });
    }
  }
  stop() { for (const t of this.traders) t.stop(); }

  /** Status for the runner's statusJson (numbers from the live book and the stance). */
  status() {
    return this.traders.map((t) => ({
      venue: t.o.venue.describe(), stance: t.strategy.stance, conviction: t.strategy.conviction, decided_by: t.strategy.requestId || null,
      stance_set_ms: t.strategy.setAt || null, halted: t.halted || null,
      book: t.book ? { sol_available_lamports: t.book.quote.toString(), position_spl: t.book.base.toString(), realized_pnl_lamports: t.book.realized.toString() } : null,
    }));
  }
}
