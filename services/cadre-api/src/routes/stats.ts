// Live stats: the explorer's own page models (apps/explorer/src/models.ts) served under
// /v1, plus supply, reserve and revenue views read from chain state. Nothing here is
// estimated: every figure is a node / Solana read or arithmetic over reads.

import * as M from "../../../../apps/explorer/src/models.ts";
import { readBuyback } from "../../../../apps/explorer/src/buyback.ts";
import { EVALUATOR_STATUSES, evaluatorJobsModel, type EvaluatorStatus } from "../../../../apps/explorer/src/evaluator.ts";
import { buybackVaultPda } from "../../../../apps/explorer/src/decode.ts";
import { big } from "../../../../apps/explorer/src/math.ts";
import { Api, ApiError, badRequest, notFound, reply } from "../http.ts";
import type { Deps } from "../deps.ts";
import { bondingCurvePda, decodeCurve, decodeReserveConfig, mintDecimals, mintSupply, reserveAta, reserveConfigPda, reserveCreatorPda, RESERVE_ID, tokenAccountAmount, isAddress } from "../sol.ts";
import { optionalExport } from "../optional.ts";

const s = (v: bigint) => v.toString();

export function intParam(q: URLSearchParams, name: string, def: number, min: number, max: number): number {
  const raw = q.get(name);
  if (raw === null || raw === "") return def;
  const n = Number(raw);
  if (!Number.isInteger(n) || n < min || n > max) throw badRequest(`${name} must be an integer in [${min}, ${max}]`);
  return n;
}

export function page<T>(items: T[], q: URLSearchParams, defLimit = 50, maxLimit = 200) {
  const limit = intParam(q, "limit", defLimit, 1, maxLimit);
  const offset = intParam(q, "offset", 0, 0, 1e9);
  return { total: items.length, offset, limit, items: items.slice(offset, offset + limit), next_offset: offset + limit < items.length ? offset + limit : null };
}

/** Light-client lag in blocks behind the chain tip (null without a light client). */
function lcLag(d: Deps) {
  const c = d.ix.lc.config;
  const tip = d.ix.chainInfo?.height ?? null;
  if (!c || tip === null) return null;
  return { tip_height: c.tipHeight, final_height: c.finalHeight, chain_height: tip, behind_tip: Math.max(0, tip - c.tipHeight), behind_final: Math.max(0, tip - c.finalHeight) };
}

/** Reserve: Config counters plus the token account and the creator PDA's SOL. */
export async function reserveModel(d: Deps) {
  const [cfgAcc, creator] = await d.accounts([reserveConfigPda(), reserveCreatorPda()]);
  const cfg = cfgAcc && cfgAcc.owner === RESERVE_ID ? decodeReserveConfig(cfgAcc.data) : null;
  if (!cfg) return null;
  const zero = "11111111111111111111111111111111";
  let tokens: string | null = null;
  let ataAddr: string | null = null;
  if (cfg.mint !== zero) {
    ataAddr = reserveAta(cfg.mint);
    const [a] = await d.accounts([ataAddr]);
    tokens = a ? s(tokenAccountAmount(a.data)) : "0";
  }
  return {
    program: RESERVE_ID,
    config: reserveConfigPda(),
    creator_pda: reserveCreatorPda(),
    creator_lamports: creator ? String(creator.lamports) : "0",
    token_account: ataAddr,
    tokens_spl: tokens,
    mint: cfg.mint === zero ? null : cfg.mint,
    mining_pool: cfg.miningPool,
    cranker: cfg.cranker === zero ? null : cfg.cranker,
    totals: { bought_spl: s(cfg.totalBought), spent_lamports: s(cfg.totalSpent), funded_spl: s(cfg.totalFunded) },
    limits: {
      max_buy_per_call_lamports: s(cfg.maxBuyPerCall),
      min_buy_lamports: s(cfg.minBuy),
      min_interval_secs: Number(cfg.minIntervalSecs),
      max_slippage_bps: cfg.maxSlippageBps,
      sol_floor_lamports: s(cfg.solFloor),
      max_fund_per_call_spl: s(cfg.maxFundPerCall),
    },
    last_crank_ts: Number(cfg.lastCrankTs) || null,
    config_tail_bytes: cfg.tailLen,
  };
}

/** Buyback: the explorer's view (program counters + recorded buys) plus the vault's live lamports. */
export async function buybackModel(d: Deps) {
  const bb = await readBuyback(d.ix);
  const vault = buybackVaultPda();
  const [v] = await d.accounts([vault]);
  return { vault, vault_lamports: v ? String(v.lamports) : "0", ...(bb ?? { active: false, note: "the deployed storefront has no buyback fields or no buyback activity yet" }) };
}

/**
 * Supply: every component read from chain state. `circulating_spl` comes from the
 * token-side definition (apps/explorer/src/supply.ts, `supplyModel(ix)`) when it is in
 * the tree; until then it is null and the components are what a reader can sum.
 */
export async function supplyModel(d: Deps) {
  const fromExplorer = await optionalExport<(ix: unknown) => unknown>("../../../apps/explorer/src/supply.ts", ["supplyModel", "circulatingSupply"]);
  const b = await d.bridge();
  const [mintAcc, vaultAcc, curveAcc] = await d.accounts([b.mint, b.vault, bondingCurvePda(b.mint)]);
  if (!mintAcc) throw new ApiError(503, "mint_missing", "the bridge's mint account is missing");
  const supply = mintSupply(mintAcc.data);
  const decimals = mintDecimals(mintAcc.data);
  const curve = curveAcc ? decodeCurve(curveAcc.data) : null;
  const reserve = await reserveModel(d).catch(() => null);
  const escrow = await d.chain<any>("getEscrow", {}).catch(() => null);
  const initial = curve ? curve.tokenTotalSupply : null;
  const explorer = fromExplorer ? await (fromExplorer as any)(d.ix) : null;
  return {
    mint: b.mint,
    decimals,
    initial_supply_spl: initial === null ? null : s(initial),
    initial_supply_source: initial === null ? null : "pump bonding curve token_total_supply (fixed at create)",
    mint_supply_spl: s(supply),
    burned_spl: initial === null ? null : s(initial - supply),
    reserve_spl: reserve?.tokens_spl ?? null,
    bridge_vault_spl: vaultAcc ? s(tokenAccountAmount(vaultAcc.data)) : null,
    curve: curve ? { complete: curve.complete, real_token_reserves_spl: s(curve.realToken), real_sol_reserves_lamports: s(curve.realQuote) } : null,
    agent_chain: escrow
      ? {
          total_supply: escrow.total_supply,
          escrow_balance: escrow.escrow_balance,
          outside_escrow: escrow.outside_escrow,
          outside_escrow_spl: escrow.outside_escrow_spl,
          mining_pool: escrow.pool,
          mining_pool_balance: escrow.pool_balance,
          note: "chain units are 9 dp; 1 SPL base unit (6 dp) = 1000 chain units. Tokens outside the escrow are backed 1:1 by the Solana bridge vault.",
        }
      : null,
    circulating_spl: explorer && typeof explorer === "object" && "circulating_spl" in (explorer as any) ? (explorer as any).circulating_spl : null,
    circulating_definition: explorer ? (explorer as any).definition ?? "apps/explorer supply module" : "pending: the token-side definition (apps/explorer circulating-supply) is not in this build",
    ...(explorer ? { explorer } : {}),
  };
}

/** Revenue with wash screens: P from distinct payers, SOL settled, self-dealt work shown and excluded. */
export function revenueModel(d: Deps, days: number) {
  const ix = d.ix;
  const ord = M.orderByJob(ix);
  const jobs = [...ix.jobs.values()].filter((j) => j.kind !== "segment");
  let pAll = 0n, pSelf = 0n, pSelfMined = 0n, wAll = 0n, solFinal = 0n, pHouse = 0n, solHouse = 0n, jobsHouse = 0;
  const house = new Set<string>(Array.isArray(ix.params?.house_operators) ? ix.params.house_operators : []);
  const payers = new Map<string, bigint>();
  const day = new Map<string, { p: bigint; sol: bigint; payers: Set<string> }>();
  const minerOf = new Map<string, Set<string>>();
  for (const [jid, hs] of ix.jobBlocks) minerOf.set(jid, new Set(hs.map((h) => ix.blocks.get(h)?.miner).filter(Boolean) as string[]));
  for (const j of jobs) {
    const p = big(j.protocol_fee);
    pAll += p;
    wAll += big(j.worker_pay);
    const isHouse = house.has(j.poster);
    const self = !isHouse && j.deliverer && j.deliverer === j.poster;
    if (self) pSelf += p;
    const miners = minerOf.get(j.job_id);
    if (miners && miners.size === 1 && miners.has(j.poster)) pSelfMined += p;
    const o = ord.get(j.job_id);
    const sol = o && o.status === "finalized" ? big(o.solSpent) : 0n;
    if (isHouse) {
      pHouse += p;
      solHouse += sol;
      jobsHouse++;
    } else if (!self) payers.set(j.poster, (payers.get(j.poster) ?? 0n) + p);
    solFinal += sol;
    const blk = ix.blocks.get(j.posted_height);
    const t = o?.createdTs ? o.createdTs * (o.createdTs < 1e12 ? 1000 : 1) : blk?.time ?? null;
    if (t) {
      const k = new Date(t).toISOString().slice(0, 10);
      const e = day.get(k) ?? { p: 0n, sol: 0n, payers: new Set() };
      e.p += p;
      e.sol += sol;
      e.payers.add(j.poster);
      day.set(k, e);
    }
  }
  const pScreened = pAll - pSelf - pHouse;
  const ranked = [...payers.values()].sort((a, b) => (b > a ? 1 : b < a ? -1 : 0));
  const top = ranked[0] ?? 0n;
  const keys = [...day.keys()].sort().slice(-days);
  return {
    units: { protocol_fee: "chain units (9 dp)", sol: "lamports" },
    protocol_fees_paid: s(pAll),
    worker_pay: s(wAll),
    sol_settled_via_storefront: s(solFinal),
    screens: {
      self_dealt_protocol_fees: s(pSelf),
      self_dealt_rule: "jobs whose deliverer is also the poster; excluded from protocol_fees_screened and from payer counts",
      self_mined_protocol_fees: s(pSelfMined),
      self_mined_rule: "jobs whose every mined block came from the poster's own key; shown, not excluded (mining your own job still pays (1 - alpha) x P to the pool)",
    },
    house_spend: {
      house_operators: [...house], jobs: jobsHouse, protocol_fees: s(pHouse), sol_settled: s(solHouse),
      rule: "jobs whose payer is a declared house operator (chain config house_operators): the house paying for its own agents' thoughts; kept out of protocol_fees_screened and the payer counts",
    },
    protocol_fees_screened: s(pScreened),
    unique_payers_screened: payers.size,
    top_payer_share_ppm: pScreened === 0n ? 0 : Number((top * 1_000_000n) / pScreened),
    by_day: keys.map((k) => ({ day: k, protocol_fees: s(day.get(k)!.p), sol_settled: s(day.get(k)!.sol), unique_payers: day.get(k)!.payers.size })),
    note: "No USD figures: there is no price source. Counts of txs or calls are never revenue.",
  };
}

export function registerStats(api: Api, d: Deps) {
  api.get("/v1/summary", async () => {
    const sm = M.summaryModel(d.ix);
    const [bb, res] = await Promise.all([buybackModel(d).catch((e) => ({ error: (e as Error).message })), reserveModel(d).catch(() => null)]);
    let supply: unknown = null;
    try {
      supply = await supplyModel(d);
    } catch (e) {
      supply = { error: (e as Error).message };
    }
    return { ...sm, light_client_lag: lcLag(d), buyback: bb, reserve: res, supply, as_of_ms: d.ix.status.lastPoll || null };
  }, { cache: 3 });

  api.get("/v1/feed", async (c) => {
    const f = c.query.get("filter") ?? "all";
    if (!["all", "paid", "fallback"].includes(f)) throw badRequest("filter must be all, paid or fallback");
    const before = c.query.get("before") ? intParam(c.query, "before", 0, 1, 1e12) : undefined;
    return M.feedModel(d.ix, { filter: f as any, before, limit: intParam(c.query, "limit", 40, 1, 200) });
  }, { cache: 3 });

  api.get("/v1/blocks/:id/proof", async (c) => {
    const id = c.params.id;
    if (!/^\d+$/.test(id) && !/^[0-9a-fA-F]{64}$/.test(id)) throw badRequest("id is a height or a 64-hex block hash");
    const h = await d.chain<any>("getHeader", /^\d+$/.test(id) ? { height: Number(id) } : { hash: id.toLowerCase() }).catch(() => null);
    if (!h) throw notFound("block not found");
    return { height: h.header_fields.height, hash: h.hash, proof_hash: h.header_fields.proof_hash, proof_hash_rule: "proof_hash = blake3(proof bytes), committed in the header", bytes: Buffer.from(h.proof, "base64").length, proof: h.proof };
  }, { cache: 60 });

  api.get("/v1/blocks/:id", async (c) => {
    const id = c.params.id;
    if (!/^\d+$/.test(id) && !/^[0-9a-fA-F]{64}$/.test(id)) throw badRequest("id is a height or a 64-hex block hash");
    const b = await M.blockModel(d.ix, id);
    if (!b) throw notFound("block not found");
    return reply({ body: b, cache: b.final ? 30 : 3 });
  }, { cache: 3 });

  api.get("/v1/jobs", (c) => {
    const ev = c.query.get("evaluator") ?? (c.query.get("role") === "evaluator" ? c.query.get("account") : null);
    if (c.query.get("role") === "evaluator" && !ev) throw badRequest("role=evaluator needs account=<address>");
    if (ev !== null) {
      if (!isAddress(ev)) throw badRequest("evaluator must be an address (Cx, or the legacy base58 key)");
      const est = c.query.get("status") || "all";
      if (!(EVALUATOR_STATUSES as readonly string[]).includes(est)) throw badRequest(`with evaluator, status is one of ${EVALUATOR_STATUSES.join(", ")}`);
      const m = evaluatorJobsModel(d.ix, ev, est as EvaluatorStatus);
      const p = page(m.jobs, c.query);
      return { evaluator: m.evaluator, status: m.status, counts: m.counts, total: p.total, offset: p.offset, limit: p.limit, next_offset: p.next_offset, jobs: p.items, indexed: m.indexed };
    }
    const st = c.query.get("status");
    const kind = c.query.get("kind");
    const poster = c.query.get("poster");
    let jobs = M.jobsModel(d.ix).jobs;
    if (st) jobs = jobs.filter((j) => j.status === st);
    if (kind) jobs = jobs.filter((j) => j.kind === kind);
    if (poster) jobs = jobs.filter((j) => j.poster === poster);
    const p = page(jobs, c.query);
    return { total: p.total, offset: p.offset, limit: p.limit, next_offset: p.next_offset, jobs: p.items };
  }, { cache: 3 });

  api.get("/v1/jobs/:id", async (c) => {
    if (!/^[0-9a-fA-F]{6,64}$/.test(c.params.id)) throw badRequest("job id is hex (6 to 64 chars; a prefix of at least 6 works)");
    const j = await M.jobModel(d.ix, c.params.id);
    if (!j) throw notFound("job not found");
    return j;
  }, { cache: 3 });

  api.get("/v1/orders", (c) => {
    const st = c.query.get("status");
    let orders = M.ordersModel(d.ix).orders;
    if (st) orders = orders.filter((o) => o.status === st);
    const payer = c.query.get("payer");
    if (payer) orders = orders.filter((o) => o.payer === payer || o.buyer === payer);
    const p = page(orders, c.query);
    return { total: p.total, offset: p.offset, limit: p.limit, next_offset: p.next_offset, orders: p.items };
  }, { cache: 3 });

  api.get("/v1/orders/:nonce", (c) => {
    if (!/^\d+$/.test(c.params.nonce)) throw badRequest("nonce is a decimal integer");
    const o = M.orderPageModel(d.ix, c.params.nonce);
    if (!o) throw notFound("order not found");
    return o;
  }, { cache: 3 });

  api.get("/v1/payers", (c) => {
    const m = M.payersModel(d.ix);
    const p = page(m.payers, c.query);
    return { total_protocol_fees: m.total_protocol_fees, total: p.total, offset: p.offset, limit: p.limit, next_offset: p.next_offset, payers: p.items };
  }, { cache: 5 });

  api.get("/v1/buyback", () => buybackModel(d), { cache: 5 });
  api.get("/v1/reserve", async () => {
    const r = await reserveModel(d);
    if (!r) throw notFound("no reserve Config on this cluster");
    return r;
  }, { cache: 5 });
  api.get("/v1/supply", () => supplyModel(d), { cache: 10 });
  api.get("/v1/revenue", (c) => revenueModel(d, intParam(c.query, "days", 30, 1, 3650)), { cache: 10 });

  api.get("/v1/sol/tx/:sig", async (c) => {
    if (!/^[1-9A-HJ-NP-Za-km-z]{64,90}$/.test(c.params.sig)) throw badRequest("sig is a base58 Solana signature");
    const t = await d.solana<any>("getTransaction", [c.params.sig, { encoding: "json", commitment: "confirmed", maxSupportedTransactionVersion: 0 }]);
    if (!t) throw notFound("not in the Solana RPC's ledger (pruned or unknown)");
    return reply({ body: t, cache: 60 });
  }, { cache: 30 });
}
