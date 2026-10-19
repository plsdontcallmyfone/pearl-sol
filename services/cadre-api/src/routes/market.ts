// Token market reads: pump curve / PumpSwap quote (read-only, before pump fees), and the
// crank view (buyback vault + reserve limits). Unsigned pump-buy and crank txs are not
// built yet (501 pending): building them needs the exact pump account list kept in step
// with services/crank, which the token-side agent is changing now.

import { Api, ApiError } from "../http.ts";
import type { Deps } from "../deps.ts";
import { bondingCurvePda, canonicalPool, cpQuote, decodeCurve, decodePool, tokenAccountAmount } from "../sol.ts";
import { buybackModel, reserveModel } from "./stats.ts";
import { amount } from "./wallet.ts";

export async function pumpQuote(d: Deps, solIn: bigint) {
  const b = await d.bridge();
  const [curveAcc, poolAcc] = await d.accounts([bondingCurvePda(b.mint), canonicalPool(b.mint)]);
  if (!curveAcc) throw new ApiError(503, "curve_missing", "no pump bonding curve for the mint on this cluster");
  const curve = decodeCurve(curveAcc.data);
  if (!curve.complete) {
    return { venue: "curve", sol_in_lamports: solIn.toString(), tokens_out_spl: cpQuote(curve.virtualToken, curve.virtualQuote, solIn).toString(), reserves: { virtual_token: curve.virtualToken.toString(), virtual_sol: curve.virtualQuote.toString(), real_token: curve.realToken.toString() } };
  }
  if (!poolAcc) throw new ApiError(503, "pool_missing", "the curve is complete but the PumpSwap pool is missing");
  const pl = decodePool(poolAcc.data);
  const [bv, qv] = await d.accounts([pl.baseVault, pl.quoteVault]);
  if (!bv || !qv) throw new ApiError(503, "pool_vaults_missing", "PumpSwap pool vaults missing");
  let q = tokenAccountAmount(qv.data) - pl.protocolFees - pl.creatorFees;
  if (q < 0n) q = 0n;
  const base = tokenAccountAmount(bv.data);
  return { venue: "pumpswap", sol_in_lamports: solIn.toString(), tokens_out_spl: cpQuote(base, q + pl.virtualQuote, solIn).toString(), reserves: { base_spl: base.toString(), quote_lamports: (q + pl.virtualQuote).toString() } };
}

const pending = (what: string) =>
  new ApiError(501, "pending", `${what} is not built by this API yet`, { why: "the pump / crank account lists are being changed by the token-side agent (buyback to reserve, permissionless fund_mining); this endpoint lands once they settle" });

export function registerMarket(api: Api, d: Deps) {
  api.get("/v1/pump/quote", async (c) => {
    const solIn = amount(c.query.get("sol_in"), "sol_in");
    const q = await pumpQuote(d, solIn);
    return { ...q, note: "fee-free constant-product quote; the fill is lower by pump's fee (1.25% on the curve, tiered on PumpSwap) and by slippage" };
  }, { cache: 3 });

  api.get("/v1/crank", async () => {
    const [bb, res] = await Promise.all([buybackModel(d), reserveModel(d).catch(() => null)]);
    const now = Math.floor(Date.now() / 1000);
    const last = res?.last_crank_ts ?? null;
    const interval = res?.limits.min_interval_secs ?? null;
    return {
      buyback: bb,
      reserve: res,
      fund_mining: res ? { next_allowed_ts: last !== null && interval !== null ? last + interval : null, now_ts: now, max_fund_per_call_spl: res.limits.max_fund_per_call_spl } : null,
      txs: { crank_buyback: "POST /v1/tx/crank-buyback (pending)", fund_mining: "POST /v1/tx/fund-mining (pending)" },
    };
  }, { cache: 5 });

  api.post("/v1/tx/pump-buy", () => {
    throw pending("the unsigned pump buy tx");
  });
  api.post("/v1/tx/crank-buyback", () => {
    throw pending("the unsigned crank buyback tx");
  });
  api.post("/v1/tx/fund-mining", () => {
    throw pending("the unsigned fund_mining tx");
  });
}
