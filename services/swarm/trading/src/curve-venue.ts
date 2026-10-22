// A pump.fun bonding curve from the agent's OWN wallet: the venue for a token the agent launched
// itself, before it graduates to PumpSwap (docs/TRADING.md 5.7). Orders are the same cadre-api
// builders as PumpSwapWallet (/v1/tx/pump-buy|pump-sell route a curve coin to buy_exact_quote_in_v3
// / sell_v3), so only the market data differs: the curve account's virtual reserves (BondingCurve:
// disc 8, virtual_token_reserves u64 at 8, virtual_sol_reserves u64 at 16, complete bool at 48).
// When the curve completes the venue stops quoting (reserves() throws), so the Trader halts; a
// restart picks PumpSwapWallet once the canonical pool exists.
import { PublicKey } from "@solana/web3.js";
import { canonicalPool, pda, PUMP } from "./solmandate.ts";
import type { MarketEvent } from "./venues.ts";
import { PumpSwapWallet } from "./wallet-venue.ts";

export const bondingCurve = (mint: PublicKey) => pda(["bonding-curve", mint], PUMP);

export function curveReserves(d: Buffer): { base: bigint; quote: bigint; complete: boolean } {
  if (d.length < 49) throw new Error("bonding curve account too short");
  return { base: d.readBigUInt64LE(8), quote: d.readBigUInt64LE(16), complete: d[48] === 1 };
}

export class PumpCurveWallet extends PumpSwapWallet {
  override name = "pump-curve";
  curve!: PublicKey;
  override describe() { return { venue: this.name, via: "wallet", pool: this.curve?.toBase58() ?? "", mint: this.mint.toBase58(), agent: this.wallet.address }; }
  override async init() { if (!this.curve) this.curve = bondingCurve(this.mint); }
  override async start(onEvent: (e: MarketEvent) => void) {
    await this.init();
    this.subs.push(this.conn.onAccountChange(this.curve, (a, ctx) => {
      const r = curveReserves(a.data);
      if (!r.complete) onEvent({ venue: this.name, base: r.base, quote: r.quote, at: performance.now(), ref: `slot:${ctx.slot}` });
    }, { commitment: "processed" }));
  }
  override async reserves() {
    await this.init();
    const a = await this.conn.getAccountInfo(this.curve, "processed");
    if (!a) throw new Error(`no pump bonding curve for ${this.mint.toBase58()}`);
    const r = curveReserves(a.data);
    if (r.complete) throw new Error("the curve is complete (graduated): restart to trade it on PumpSwap");
    return { base: r.base, quote: r.quote };
  }
}

/** The venue for a pump coin now: its canonical PumpSwap pool when it exists, else its curve. */
export async function walletVenueFor(o: ConstructorParameters<typeof PumpSwapWallet>[0]): Promise<PumpSwapWallet> {
  const v = new PumpSwapWallet(o);
  const pool = await v.conn.getAccountInfo(canonicalPool(new PublicKey(o.mint)), "confirmed").catch(() => null);
  return pool ? v : new PumpCurveWallet({ ...o, conn: v.conn });
}
