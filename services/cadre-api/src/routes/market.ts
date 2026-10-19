// Token market: pump curve / PumpSwap quote (read-only, before pump fees), the crank view
// (buyback vault + reserve limits), and UNSIGNED txs for the user's wallet to sign: a CADRE
// buy (pump curve, PumpSwap after graduation), the permissionless storefront buyback crank
// and the permissionless rate-limited reserve fund_mining. Account lists mirror
// services/crank (see src/sol.ts "market txs"); every precondition the program would
// reject on is checked first and answered with 409 / 422 instead of a tx that must fail.

import { findPda, utf8 } from "../../../../apps/explorer/src/codec.ts";
import { Api, ApiError, badRequest } from "../http.ts";
import type { Deps } from "../deps.ts";
import {
  ammBuybackRecipients,
  ammBuyExactQuoteInIx,
  ammGlobalConfigPda,
  ata,
  bondingCurvePda,
  canonicalPool,
  CLOCK,
  clockUnixTs,
  closeAccount,
  compileLegacy,
  computeUnitLimit,
  cpQuote,
  createAtaIdempotent,
  decodeBridgeConfig,
  decodeCurve,
  decodePool,
  decodePoolKeys,
  decodeReserveConfig,
  decodeReserveFundLimits,
  decodeStoreCrankConfig,
  pumpBuybackRecipients,
  pumpBuyExactQuoteInIx,
  pumpGlobalPda,
  reserveAta,
  reserveConfigPda,
  RESERVE_ID,
  reserveFundMiningIx,
  storeBuybackVault,
  storeConfigAddress,
  storeCrankBuybackAmmIx,
  storeCrankBuybackIx,
  STOREFRONT_ID,
  STORE_RENT_MIN,
  syncNative,
  systemTransfer,
  tokenAccountAmount,
  TOKEN,
  TOKEN_2022,
  unsignedTx,
  WSOL,
  type Ix,
  type PoolKeys,
} from "../sol.ts";
import { buybackModel, reserveModel } from "./stats.ts";
import { addr, amount } from "./wallet.ts";

const ZERO = "11111111111111111111111111111111";
const findCreator = (reserveProgram: string) => findPda([utf8("creator")], reserveProgram)[0];
const bridgePda = (bridgeProgram: string) => findPda([utf8("config")], bridgeProgram)[0];
/**
 * Upper bound of pump's total trade fee (protocol + creator + LP) used to turn the
 * fee-free quote into the fill of an exact-SOL-in buy: fixtures/pump fee_config has one tier
 * of 95 + 30 bps on the curve, and PumpSwap's tiers total at most 125 bps (reserve docs).
 * Over-estimating the fee only lowers min_tokens_out.
 */
export const PUMP_FEE_BPS_MAX = 125n;
/** services/crank defaults (MAX_SOL_IN, MIN_OUT_BPS) for the storefront buyback. */
const CRANK_MAX_SOL_IN = 1_000_000_000n;
const CRANK_MIN_OUT_BPS = 300n;

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

async function blockhash(d: Deps): Promise<{ blockhash: string; lastValidBlockHeight: number }> {
  const r = await d.solana<any>("getLatestBlockhash", [{ commitment: "confirmed" }]);
  return r.value;
}

/** The chain's clock (the programs' rate limits run on it, not on wall time); wall time if the sysvar is unreadable. */
async function chainNow(d: Deps): Promise<{ ts: number; source: "clock_sysvar" | "wall" }> {
  const [c] = await d.accounts([CLOCK]).catch(() => [null]);
  return c && c.data.length >= 40 ? { ts: clockUnixTs(c.data), source: "clock_sysvar" } : { ts: Math.floor(Date.now() / 1000), source: "wall" };
}

const pick = <T,>(xs: T[]) => xs[Math.floor(Math.random() * xs.length)];

function txReply(ixs: Ix[], feePayer: string, bh: { blockhash: string; lastValidBlockHeight: number }, extra: Record<string, unknown>) {
  const msg = compileLegacy(ixs, feePayer, bh.blockhash);
  return {
    transaction: Buffer.from(unsignedTx(msg)).toString("base64"),
    encoding: "base64",
    version: "legacy",
    fee_payer: feePayer,
    signers: msg.accountKeys.slice(0, msg.numRequiredSignatures),
    recent_blockhash: bh.blockhash,
    last_valid_block_height: bh.lastValidBlockHeight,
    ...extra,
    sign_with: "Wallet Standard solana:signAndSendTransaction (or signTransaction, then POST /v1/tx/solana)",
  };
}

function bps(v: unknown, name: string, def: bigint): bigint {
  if (v === undefined || v === null) return def;
  const n = typeof v === "number" ? v : typeof v === "string" && /^\d+$/.test(v) ? Number(v) : NaN;
  if (!Number.isInteger(n) || n < 0 || n > 5000) throw badRequest(`${name} must be an integer in [0, 5000]`);
  return BigInt(n);
}
const lessBps = (x: bigint, b: bigint) => (x * (10_000n - b)) / 10_000n;

// ---------------------------------------------------------------- fund_mining state

export interface FundState {
  ok: boolean;
  reason: string | null;
  code: string | null;
  now_ts: number;
  clock: string;
  next_allowed_ts: number | null;
  paused: boolean | null;
  /** Most this call may move now: min(max_fund_per_call, window room, reserve balance). */
  max_amount_spl: string | null;
  reserve_spl: string | null;
  limits: Record<string, unknown> | null;
}

/** What fund_mining's on-chain checks say right now (reserve fund_mining + fund_window_check). */
export async function fundState(d: Deps, amountWanted?: bigint): Promise<FundState & { raw?: { mint: string; bridgeProgram: string } }> {
  const [cfgAcc] = await d.accounts([reserveConfigPda()]);
  const now = await chainNow(d);
  const base = { now_ts: now.ts, clock: now.source, next_allowed_ts: null, paused: null, max_amount_spl: null, reserve_spl: null, limits: null };
  const cfg = cfgAcc && cfgAcc.owner === RESERVE_ID ? decodeReserveConfig(cfgAcc.data) : null;
  if (!cfg) return { ...base, ok: false, code: "reserve_missing", reason: "no reserve Config on this cluster" };
  const lim = decodeReserveFundLimits(cfgAcc!.data);
  if (!lim || lim.layout < 1) return { ...base, ok: false, code: "needs_migration", reason: "the reserve Config predates the fund limits (admin migrate_config first); fund_mining fails with NeedsMigration" };
  const limits = {
    max_per_call_spl: cfg.maxFundPerCall.toString(),
    min_interval_secs: Number(lim.minIntervalSecs),
    window_secs: Number(lim.windowSecs),
    max_per_window_spl: lim.maxPerWindow.toString(),
    window_start_ts: Number(lim.windowStart) || null,
    window_used_spl: lim.windowUsed.toString(),
    last_fund_ts: Number(lim.lastTs) || null,
    calls: Number(lim.calls),
  };
  const out = { ...base, paused: lim.paused, limits };
  if (cfg.mint === ZERO) return { ...out, ok: false, code: "not_launched", reason: "the reserve has no mint yet (launch first)" };
  if (cfg.miningPool === ZERO || cfg.bridgeProgram === ZERO) return { ...out, ok: false, code: "not_wired", reason: "the reserve's bridge_program / mining_pool are not set" };
  const [ra] = await d.accounts([reserveAta(cfg.mint)]);
  const reserve = ra ? tokenAccountAmount(ra.data) : 0n;
  const fresh = lim.windowStart === 0n || BigInt(now.ts) >= lim.windowStart + lim.windowSecs;
  const windowCapped = lim.windowSecs > 0n && lim.maxPerWindow > 0n;
  const room = windowCapped ? lim.maxPerWindow - (fresh ? 0n : lim.windowUsed) : null;
  let max = reserve;
  if (cfg.maxFundPerCall > 0n && cfg.maxFundPerCall < max) max = cfg.maxFundPerCall;
  if (room !== null && room < max) max = room < 0n ? 0n : room;
  const intervalNext = lim.lastTs === 0n ? null : Number(lim.lastTs + lim.minIntervalSecs);
  const windowNext = windowCapped && !fresh ? Number(lim.windowStart + lim.windowSecs) : null;
  let next: number | null = intervalNext !== null && intervalNext > now.ts ? intervalNext : null;
  const res = { ...out, reserve_spl: reserve.toString(), max_amount_spl: max.toString(), raw: { mint: cfg.mint, bridgeProgram: cfg.bridgeProgram } };
  if (lim.paused) return { ...res, ok: false, code: "fund_paused", reason: "fund_mining is paused by the reserve admin (FundPaused)", next_allowed_ts: null };
  if (next !== null) return { ...res, ok: false, code: "rate_limited", reason: `fund_mining ran at ${lim.lastTs}; the next call is allowed at ${next} (fund_min_interval_secs ${lim.minIntervalSecs})`, next_allowed_ts: next };
  const want = amountWanted ?? max;
  if (room !== null && want > room) {
    next = windowNext;
    return { ...res, ok: false, code: "window_full", reason: `the rolling window (${lim.windowSecs}s) has ${room > 0n ? room : 0n} SPL base units left of ${lim.maxPerWindow}; it resets at ${windowNext}`, next_allowed_ts: next };
  }
  if (cfg.maxFundPerCall > 0n && want > cfg.maxFundPerCall) return { ...res, ok: false, code: "over_fund_cap", reason: `amount ${want} is above max_fund_per_call ${cfg.maxFundPerCall}`, next_allowed_ts: now.ts };
  if (want <= 0n || reserve < want) return { ...res, ok: false, code: "insufficient_reserve", reason: `the reserve holds ${reserve} SPL base units${want > 0n ? `, the call needs ${want}` : ""}`, next_allowed_ts: null };
  return { ...res, ok: true, code: null, reason: null, next_allowed_ts: now.ts };
}

// ---------------------------------------------------------------- storefront buyback state

async function storeCrankState(d: Deps, cranker: string | null, maxSolIn: bigint) {
  const [cfgAcc, vaultAcc] = await d.accounts([storeConfigAddress(), storeBuybackVault()]);
  const c = cfgAcc && cfgAcc.owner === STOREFRONT_ID ? decodeStoreCrankConfig(cfgAcc.data) : null;
  if (!c) throw new ApiError(409, "buyback_unavailable", "the storefront Config is missing or predates the held-share buyback on this cluster");
  const now = await chainNow(d);
  if (cranker && c.cranker !== ZERO && c.cranker !== cranker) throw new ApiError(409, "cranker_restricted", `the storefront buyback crank is restricted to ${c.cranker} (buyback_cranker); the program would fail with Unauthorized`, { allowed_cranker: c.cranker });
  if (c.lastTs !== 0n && BigInt(now.ts) < c.lastTs + c.minIntervalSecs) {
    throw new ApiError(409, "too_soon", `the last buyback ran at ${c.lastTs}; the next is allowed at ${c.lastTs + c.minIntervalSecs} (buyback_min_interval_secs ${c.minIntervalSecs})`, { next_allowed_ts: Number(c.lastTs + c.minIntervalSecs), now_ts: now.ts });
  }
  const vault = BigInt(vaultAcc?.lamports ?? 0);
  const floor = STORE_RENT_MIN + c.locked;
  let spend = vault > floor ? vault - floor : 0n;
  if (c.maxPerCall > 0n && spend > c.maxPerCall) spend = c.maxPerCall;
  if (spend > maxSolIn) spend = maxSolIn;
  const minBuy = c.minBuy > 0n ? c.minBuy : 1n;
  if (spend < minBuy) throw new ApiError(409, "nothing_to_buy", `the buyback vault has ${spend} crankable lamports (min_buy ${c.minBuy}, held for unsettled orders ${c.locked}); the program would fail with NothingToBuy`, { vault_lamports: vault.toString(), held_lamports: c.locked.toString(), now_ts: now.ts });
  return { c, now, vault, spend };
}

async function poolKeysFor(d: Deps, mint: string): Promise<PoolKeys & { baseReserve: bigint; quoteReserve: bigint }> {
  const pool = canonicalPool(mint);
  const [pa] = await d.accounts([pool]);
  if (!pa) throw new ApiError(409, "pool_missing", "the curve is complete but the canonical PumpSwap pool does not exist yet (migration pending)");
  const k = decodePoolKeys(pa.data);
  const pl = decodePool(pa.data);
  if (k.baseMint !== mint || k.quoteMint !== WSOL) throw new ApiError(503, "pool_unexpected", "the canonical pool's mints are not (mint, wSOL)");
  const [bv, qv] = await d.accounts([k.baseVault, k.quoteVault]);
  if (!bv || !qv) throw new ApiError(503, "pool_vaults_missing", "PumpSwap pool vaults missing");
  let q = tokenAccountAmount(qv.data) - pl.protocolFees - pl.creatorFees;
  if (q < 0n) q = 0n;
  return { pool, baseMint: k.baseMint, quoteMint: k.quoteMint, baseVault: k.baseVault, quoteVault: k.quoteVault, baseReserve: tokenAccountAmount(bv.data), quoteReserve: q + pl.virtualQuote };
}

export function registerMarket(api: Api, d: Deps) {
  api.get("/v1/pump/quote", async (c) => {
    const solIn = amount(c.query.get("sol_in"), "sol_in");
    const q = await pumpQuote(d, solIn);
    return { ...q, note: "fee-free constant-product quote; the fill is lower by pump's fee (1.25% on the curve, tiered on PumpSwap) and by slippage" };
  }, { cache: 3 });

  api.get("/v1/crank", async () => {
    const [bb, res, fund] = await Promise.all([buybackModel(d), reserveModel(d).catch(() => null), fundState(d).catch(() => null)]);
    return {
      buyback: bb,
      reserve: res,
      fund_mining: fund
        ? { ok: fund.ok, code: fund.code, reason: fund.reason, next_allowed_ts: fund.next_allowed_ts, now_ts: fund.now_ts, clock: fund.clock, paused: fund.paused, max_amount_spl: fund.max_amount_spl, reserve_spl: fund.reserve_spl, max_fund_per_call_spl: res?.limits.max_fund_per_call_spl ?? null, limits: fund.limits }
        : null,
      txs: { crank_buyback: "POST /v1/tx/crank-buyback {cranker}", fund_mining: "POST /v1/tx/fund-mining {cranker, amount_spl?}" },
    };
  }, { cache: 5 });

  api.post("/v1/tx/pump-buy", async (c) => {
    const b0 = await c.json();
    const buyer = addr(b0.buyer, "buyer");
    const solIn = amount(b0.sol_in, "sol_in");
    const slip = bps(b0.slippage_bps, "slippage_bps", 100n);
    const minOverride = b0.min_tokens_out === undefined || b0.min_tokens_out === null ? null : amount(b0.min_tokens_out, "min_tokens_out");
    const cfg = await d.bridge();
    const mint = cfg.mint;
    if (cfg.tokenProgram !== TOKEN_2022) throw new ApiError(503, "unexpected_token_program", "the bridge mint is not a Token-2022 mint (pump create_v2 coins are)");
    const bal = await d.solana<any>("getBalance", [buyer, { commitment: "confirmed" }]);
    const have = BigInt(bal?.value ?? 0);
    if (have < solIn) throw new ApiError(422, "insufficient_funds", `the buyer holds ${have} lamports, the buy spends up to ${solIn} (plus the tx fee and first-time account rent)`);
    // pump's exact-SOL-in buys swap sol_in / (1 + fee): quote that net amount, fee-free
    const net = (solIn * 10_000n) / (10_000n + PUMP_FEE_BPS_MAX);
    const q = await pumpQuote(d, net);
    const quoted = BigInt(q.tokens_out_spl);
    const minOut = minOverride ?? lessBps(quoted, slip);
    if (minOut <= 0n) throw new ApiError(422, "amount_too_small", "sol_in buys less than one base unit after fees and slippage");
    // same order as the pump SDKs: (wSOL wrap on PumpSwap), the buyer's token ATA (idempotent), the buy, (unwrap)
    const ixs: Ix[] = [computeUnitLimit(400_000)];
    const baseAta = createAtaIdempotent(buyer, buyer, mint, TOKEN_2022);
    let route: Record<string, unknown>;
    if (q.venue === "curve") {
      const [g] = await d.accounts([pumpGlobalPda()]);
      if (!g) throw new ApiError(503, "pump_global_missing", "pump Global account missing on this cluster");
      const recipient = pick(pumpBuybackRecipients(g.data));
      ixs.push(baseAta, pumpBuyExactQuoteInIx(buyer, mint, solIn, minOut, recipient));
      route = { venue: "curve", program: "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P", instruction: "buy_exact_quote_in_v3", bonding_curve: bondingCurvePda(mint), buyback_fee_recipient: recipient };
    } else {
      const p = await poolKeysFor(d, mint);
      const [gc] = await d.accounts([ammGlobalConfigPda()]);
      if (!gc) throw new ApiError(503, "amm_global_missing", "PumpSwap GlobalConfig missing on this cluster");
      const owner = pick(ammBuybackRecipients(gc.data));
      const wsol = ata(buyer, WSOL, TOKEN);
      ixs.push(createAtaIdempotent(buyer, buyer, WSOL, TOKEN), systemTransfer(buyer, wsol, solIn), syncNative(wsol), baseAta, ammBuyExactQuoteInIx(buyer, p, solIn, minOut, owner), closeAccount(wsol, buyer, buyer));
      route = { venue: "pumpswap", program: "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA", instruction: "buy_exact_quote_in_v2", pool: p.pool, buyback_fee_recipient: ata(owner, WSOL, TOKEN), wsol_account: wsol };
    }
    const bh = await blockhash(d);
    return txReply(ixs, buyer, bh, {
      summary: {
        buyer,
        mint,
        sol_in_lamports: solIn.toString(),
        quote_tokens_out_spl: quoted.toString(),
        min_tokens_out_spl: minOut.toString(),
        slippage_bps: Number(slip),
        fee_allowance_bps: Number(PUMP_FEE_BPS_MAX),
        token_account: ata(buyer, mint, TOKEN_2022),
        ...route,
      },
      note: "spends at most sol_in (pump's fee included) and fails without moving funds if the fill is below min_tokens_out; the quote is constant product on sol_in net of a 1.25% fee allowance",
    });
  });

  api.post("/v1/tx/crank-buyback", async (c) => {
    const b0 = await c.json();
    const cranker = addr(b0.cranker, "cranker");
    const maxSolIn = b0.max_sol_in === undefined ? CRANK_MAX_SOL_IN : amount(b0.max_sol_in, "max_sol_in");
    const minOutBps = bps(b0.min_out_bps, "min_out_bps", CRANK_MIN_OUT_BPS);
    const { c: sc, spend, vault, now } = await storeCrankState(d, cranker, maxSolIn);
    const [curveAcc] = await d.accounts([bondingCurvePda(sc.mint)]);
    if (!curveAcc) throw new ApiError(503, "curve_missing", "no pump bonding curve for the storefront's mint");
    const curve = decodeCurve(curveAcc.data);
    let ix: Ix, quote: bigint, route: Record<string, unknown>;
    if (!curve.complete) {
      const [g] = await d.accounts([pumpGlobalPda()]);
      if (!g) throw new ApiError(503, "pump_global_missing", "pump Global account missing on this cluster");
      quote = cpQuote(curve.virtualToken, curve.virtualQuote, spend);
      const recipient = pick(pumpBuybackRecipients(g.data));
      ix = storeCrankBuybackIx(cranker, sc, maxSolIn, lessBps(quote, minOutBps), recipient);
      route = { venue: "curve", instruction: "crank_buyback", buyback_fee_recipient: recipient };
    } else {
      const p = await poolKeysFor(d, sc.mint);
      const [gc] = await d.accounts([ammGlobalConfigPda()]);
      if (!gc) throw new ApiError(503, "amm_global_missing", "PumpSwap GlobalConfig missing on this cluster");
      quote = cpQuote(p.baseReserve, p.quoteReserve, spend);
      const owner = ammBuybackRecipients(gc.data)[0];
      ix = storeCrankBuybackAmmIx(cranker, sc, p, maxSolIn, lessBps(quote, minOutBps), owner);
      route = { venue: "pumpswap", instruction: "crank_buyback_amm", pool: p.pool, buyback_fee_recipient: ata(owner, WSOL, TOKEN) };
    }
    const [ra] = await d.accounts([ata(findCreator(sc.reserveProgram), sc.mint, TOKEN_2022)]);
    const reserveTokens = ra ? tokenAccountAmount(ra.data) : 0n;
    const bh = await blockhash(d);
    return txReply([computeUnitLimit(600_000), ix], cranker, bh, {
      summary: {
        cranker,
        program: STOREFRONT_ID,
        ...route,
        vault: storeBuybackVault(),
        vault_lamports: vault.toString(),
        held_lamports: sc.locked.toString(),
        spend_lamports: spend.toString(),
        max_sol_in_lamports: maxSolIn.toString(),
        quote_tokens_out_spl: quote.toString(),
        min_tokens_out_spl: lessBps(quote, minOutBps).toString(),
        min_out_bps: Number(minOutBps),
        reserve_cap_spl: sc.reserveCap.toString(),
        reserve_spl: reserveTokens.toString(),
        destination: sc.reserveCap === 0n || reserveTokens < sc.reserveCap ? (sc.reserveCap === 0n ? "reserve" : `reserve up to ${sc.reserveCap - reserveTokens}, rest burned`) : "burn (reserve at cap)",
        now_ts: now.ts,
      },
      note: "permissionless; the cranker pays the tx fee and, the first time, the vault's token account rent. Shares of orders not settled yet stay held (services/crank sf-settle releases finalized ones first)",
    });
  });

  api.post("/v1/tx/fund-mining", async (c) => {
    const b0 = await c.json();
    const cranker = addr(b0.cranker, "cranker");
    const want = b0.amount_spl === undefined || b0.amount_spl === null ? undefined : amount(b0.amount_spl, "amount_spl");
    const st = await fundState(d, want);
    if (!st.ok) {
      const status = st.code === "insufficient_reserve" || st.code === "over_fund_cap" ? 422 : 409;
      throw new ApiError(status, st.code ?? "refused", st.reason ?? "fund_mining would fail", { next_allowed_ts: st.next_allowed_ts, now_ts: st.now_ts, paused: st.paused, max_amount_spl: st.max_amount_spl, limits: st.limits });
    }
    const amt = want ?? BigInt(st.max_amount_spl!);
    const { mint, bridgeProgram } = st.raw!;
    const bcfgKey = bridgePda(bridgeProgram);
    const [bcfg] = await d.accounts([bcfgKey]);
    const bc = bcfg ? decodeBridgeConfig(bcfg.data) : null;
    if (!bc) throw new ApiError(503, "bridge_not_initialized", "the reserve's bridge Config is missing");
    const bh = await blockhash(d);
    return txReply([computeUnitLimit(600_000), reserveFundMiningIx(cranker, mint, bridgeProgram, bc.depositNonce, amt)], cranker, bh, {
      deposit_nonce: bc.depositNonce.toString(),
      summary: { cranker, program: RESERVE_ID, instruction: "fund_mining", amount_spl: amt.toString(), receive_chain: (amt * 1000n).toString(), reserve_spl: st.reserve_spl, mint, bridge_program: bridgeProgram, now_ts: st.now_ts, limits: st.limits },
      note: "permissionless; moves reserve tokens into the bridge for the agent-chain mining pool. The cranker pays the bridge Deposit record rent. The Deposit PDA is keyed by the bridge's deposit nonce at build time: if another deposit lands first the tx fails without moving funds; build it again",
    });
  });
}
