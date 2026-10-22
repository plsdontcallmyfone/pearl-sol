// Agent trading (docs/TRADING.md, docs/API.md "Trading"): read-only views computed only from
// chain data, plus unsigned Solana txs for the mandate grant flow.
//  - Solana: the mandate program's Mandate accounts (getProgramAccounts) and MandateSwap events
//    (the program's signatures, each tx's logs), the vaults' SOL and CADRE balances, the CADRE
//    PumpSwap pool reserves.
//  - Cadre: the AMM pools' Swap events and the Cadre mandate contract (MandateSwap events, `get`).
// PnL: realized only (mandates: equity + withdrawn + fees - deposited; house wallets on the Cadre
// AMM: average cost over the pool's own Swap events). A position's value at the pool mid is shown
// as `marked_at_mid`, labelled, never added to realized PnL. House traders are labelled and their
// trades are excluded from organic volume. Nothing is stubbed: a missing config or upstream is a
// 503 with the reason.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Api } from "../http.ts";
import { ApiError, badRequest, notFound } from "../http.ts";
import type { Deps } from "../deps.ts";
import { addr, amount } from "./wallet.ts";
import { b58decode, b58encode, findPda, sha256, utf8 } from "../../../../apps/explorer/src/codec.ts";
import { eventTopic } from "../../../../apps/explorer/src/contracts.ts";
import { verifyAgent } from "./useragents.ts";
import { decodeSocialTx } from "../../../../apps/explorer/src/social.ts";
import { isSovereign } from "../../../swarm/agents/src/spec.ts";
import { bondingCurvePda, curveCreator, decodePoolKeys } from "../sol.ts";
import { shared } from "../shared.ts";
import { ata, canonicalPool, compileLegacy, computeUnitLimit, createAtaIdempotent, type Ix, SYSTEM, TOKEN, TOKEN_2022, unsignedTx, AMM, ATA_PROGRAM } from "../sol.ts";

const here = dirname(fileURLToPath(import.meta.url));
const ROOT = join(here, "../../../..");
const VAULT_RENT = 890_880n;
const WSOL = "So11111111111111111111111111111111111111112";
const FEE_PROGRAM = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";

export interface TradingConfig {
  solana: { program: string; config: string; mint: string; pool: string; buyback_vault: string } | null;
  cadre: { amm_pool: string; agent_token: string; mandate_contract: string | null } | null;
  house: Record<string, string>; // account -> handle
}

export function loadTradingConfig(env: NodeJS.ProcessEnv = process.env): TradingConfig | null {
  const solFile = env.TRADING_SOLANA_CONFIG ?? join(ROOT, "deploy/trading-devnet-solana.json");
  const cadFile = env.TRADING_CADRE_CONFIG ?? join(ROOT, "deploy/trading-devnet.json");
  const houseFile = env.TRADING_HOUSE ?? join(ROOT, "deploy/trading-house.json");
  if (!existsSync(solFile) && !existsSync(cadFile)) return null;
  const sol = existsSync(solFile) ? JSON.parse(readFileSync(solFile, "utf8")).solana ?? null : null;
  const cad = existsSync(cadFile) ? JSON.parse(readFileSync(cadFile, "utf8")) : null;
  const house = existsSync(houseFile) ? JSON.parse(readFileSync(houseFile, "utf8")).house ?? {} : {};
  return {
    solana: sol ? { program: sol.program, config: sol.config, mint: sol.mint, pool: sol.pool, buyback_vault: sol.buyback_vault } : null,
    cadre: cad?.amm_pool ? { amm_pool: cad.amm_pool, agent_token: cad.agent_token, mandate_contract: cad.mandate_contract ?? null } : null,
    house,
  };
}

// ------------------------------------------------------------------ Solana mandate codec

const disc = (ns: string, name: string) => sha256(utf8(`${ns}:${name}`)).subarray(0, 8);
const le64 = (n: bigint) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(n); return b; };
const meta = (pubkey: string, isWritable = false, isSigner = false) => ({ pubkey, isSigner, isWritable });

export interface Limits {
  venues: number; any_mint: boolean; mints: string[]; max_in_per_trade: string; window_cap: string; window_secs: number;
  max_trades_per_window: number; max_slippage_bps: number; min_pool_quote_reserve: string; max_trade_share_bps: number; expiry_ts: string; paused: boolean;
}

export function decodeMandate(address: string, data: Uint8Array) {
  const d = Buffer.from(data);
  if (!d.subarray(0, 8).equals(Buffer.from(disc("account", "Mandate")))) return null;
  let o = 8;
  const pk = () => { const v = b58encode(d.subarray(o, o + 32)); o += 32; return v; };
  const user = pk(), agent = pk(), fee_to = pk();
  const mandate_id = d.readBigUInt64LE(o); o += 8;
  o += 2;
  const revoked = d[o++] === 1;
  const venues = d[o++], any_mint = d[o++] === 1, n = d[o++];
  const mints: string[] = [];
  for (let i = 0; i < 4; i++) { if (i < n) mints.push(b58encode(d.subarray(o, o + 32))); o += 32; }
  const limits: Limits = {
    venues, any_mint, mints, max_in_per_trade: d.readBigUInt64LE(o).toString(), window_cap: d.readBigUInt64LE(o + 8).toString(), window_secs: d.readUInt32LE(o + 16),
    max_trades_per_window: d.readUInt16LE(o + 20), max_slippage_bps: d.readUInt16LE(o + 22), min_pool_quote_reserve: d.readBigUInt64LE(o + 24).toString(),
    max_trade_share_bps: d.readUInt16LE(o + 32), expiry_ts: d.readBigInt64LE(o + 34).toString(), paused: d[o + 42] === 1,
  };
  o += 43;
  const perf_fee_bps = d.readUInt16LE(o), settle_interval_secs = d.readUInt32LE(o + 2); o += 6;
  const i64 = () => { const v = d.readBigInt64LE(o); o += 8; return v; };
  const u64 = () => { const v = d.readBigUInt64LE(o); o += 8; return v; };
  const opened_ts = i64(), window_start = i64(), window_used = u64();
  const window_trades = d.readUInt16LE(o); o += 2;
  const deposited = u64(), withdrawn = u64(), fees_agent = u64(), fees_protocol = u64(), hwm = i64(), last_settle = i64(), trades = u64(), volume = u64();
  return { address, user, agent, fee_to, mandate_id, revoked, limits, perf_fee_bps, settle_interval_secs, opened_ts, window_start, window_used, window_trades, deposited, withdrawn, fees_agent, fees_protocol, hwm, last_settle, trades, volume };
}
export type MandateRec = NonNullable<ReturnType<typeof decodeMandate>>;

export function encodeLimits(l: Limits): Buffer {
  if (l.mints.length > 4) throw badRequest("at most 4 mints");
  const b = Buffer.alloc(3 + 128 + 8 + 8 + 4 + 2 + 2 + 8 + 2 + 8 + 1);
  let o = 0;
  b[o++] = l.venues; b[o++] = l.any_mint ? 1 : 0; b[o++] = l.mints.length;
  for (let i = 0; i < 4; i++) { if (i < l.mints.length) Buffer.from(b58decode(l.mints[i])).copy(b, o); o += 32; }
  b.writeBigUInt64LE(BigInt(l.max_in_per_trade), o); o += 8; b.writeBigUInt64LE(BigInt(l.window_cap), o); o += 8;
  b.writeUInt32LE(l.window_secs, o); o += 4; b.writeUInt16LE(l.max_trades_per_window, o); o += 2; b.writeUInt16LE(l.max_slippage_bps, o); o += 2;
  b.writeBigUInt64LE(BigInt(l.min_pool_quote_reserve), o); o += 8; b.writeUInt16LE(l.max_trade_share_bps, o); o += 2;
  b.writeBigInt64LE(BigInt(l.expiry_ts), o); o += 8; b[o] = l.paused ? 1 : 0;
  return b;
}

export interface SwapEvent { mandate: string; agent: string; venue: number; side: number; mint: string; amount_in: string; amount_out: string; quote: string; pool_base: string; pool_quote: string; equity_after: string; slot: number }
export function swapEvents(logs: string[]): SwapEvent[] {
  const want = Buffer.from(disc("event", "MandateSwap"));
  const out: SwapEvent[] = [];
  for (const l of logs ?? []) {
    const m = /^Program data: (.+)$/.exec(l);
    if (!m) continue;
    const d = Buffer.from(m[1], "base64");
    if (d.length < 162 || !d.subarray(0, 8).equals(want)) continue;
    let o = 8;
    const pk = () => { const v = b58encode(d.subarray(o, o + 32)); o += 32; return v; };
    const mandate = pk(), agent = pk();
    const venue = d[o++], side = d[o++];
    const mint = pk();
    const u = () => { const v = d.readBigUInt64LE(o); o += 8; return v.toString(); };
    const amount_in = u(), amount_out = u(), quote = u(), pool_base = u(), pool_quote = u(), equity_after = u();
    out.push({ mandate, agent, venue, side, mint, amount_in, amount_out, quote, pool_base, pool_quote, equity_after, slot: Number(d.readBigUInt64LE(o)) });
  }
  return out;
}

// ------------------------------------------------------------------ the index (cached reads)

interface Trade {
  chain: "solana" | "cadre"; venue: "pumpswap" | "cadre-amm"; agent: string; account: string; via: "mandate" | "wallet";
  side: "buy" | "sell"; amount_in: string; amount_out: string; unit: string; token: string; at: number; ref: string; house: boolean; flags: string[];
  kind?: "house" | "user_agent" | "mandate" | "wallet"; receipt_post?: string | null; decided_by?: string | null;
  /** The agent trading a token it launched itself (the pump curve's creator, or the PumpSwap pool's coin_creator, is the agent): labelled, never organic. */
  self_trade?: boolean;
}

export const OWN_TOKEN_FLAG = "own token: launched by this agent";

/** A user agent with the v2 trade skill, read from the chain (profile, spec in the profile blob). */
export interface UserTrader { account: string; handle: string | null; name: string | null; creator: string; hosting: string; sovereign: boolean; mints: string[]; verified: boolean }

/**
 * Amounts a landed Solana tx moved for `owner` in `mint` (the agent runtime's wallet-venue.ts
 * fillFromTx, the same rule): SOL change with the fee added back on a sell and a new token
 * account's rent taken out on a buy; the token change. Null when it moved none of that mint.
 */
export function walletFill(t: any, owner: string, mint: string): { side: "buy" | "sell"; amount_in: bigint; amount_out: bigint } | null {
  const m = t?.transaction?.message;
  const keys: string[] = (m?.accountKeys ?? []).map((k: any) => (typeof k === "string" ? k : k?.pubkey ?? String(k)));
  const i = keys.indexOf(owner);
  const meta = t?.meta;
  if (i < 0 || !meta || meta.err) return null;
  const tok = (list: any[]) => (list ?? []).filter((b) => b.mint === mint && b.owner === owner).reduce((s, b) => s + BigInt(b.uiTokenAmount?.amount ?? 0), 0n);
  const dTok = tok(meta.postTokenBalances) - tok(meta.preTokenBalances);
  if (dTok === 0n) return null;
  const fee = BigInt(meta.fee ?? 0);
  const dSol = BigInt(meta.postBalances[i]) - BigInt(meta.preBalances[i]);
  if (dTok > 0n) {
    // rent deposits for accounts this tx created (the token account, PumpSwap's per-user volume
    // account) are not a trade cost: every account that went from 0 to a balance, other than the owner
    let rent = 0n;
    for (let j = 0; j < keys.length; j++) if (j !== i && BigInt(meta.preBalances[j] ?? 0) === 0n && BigInt(meta.postBalances[j] ?? 0) > 0n) rent += BigInt(meta.postBalances[j]);
    const spent = -dSol - fee - rent;
    return { side: "buy", amount_in: spent > 0n ? spent : 0n, amount_out: dTok };
  }
  const got = dSol + fee;
  return { side: "sell", amount_in: -dTok, amount_out: got > 0n ? got : 0n };
}

const PUMP_PROGRAMS = new Set(["pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA", "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"]);

class TradingIndex {
  d: Deps;
  cfg: TradingConfig;
  sigs = new Map<string, { slot: number; blockTime: number | null; events: SwapEvent[] }>();
  mandates: { at: number; v: MandateRec[] } = { at: 0, v: [] };
  solScanAt = 0;
  cadre: { at: number; trades: Trade[]; pool: any; mandates: any[] } = { at: 0, trades: [], pool: null, mandates: [] };
  users: { at: number; v: UserTrader[] } = { at: 0, v: [] };
  userSigs = new Map<string, Map<string, { slot: number; blockTime: number | null; fills: { mint: string; side: "buy" | "sell"; amount_in: bigint; amount_out: bigint; self: boolean }[] }>>();
  creators = new Map<string, { at: number; v: string | null }>(); // mint -> its pump creator (curve, else the canonical pool's coin_creator)
  userScanAt = new Map<string, number>();
  receipts = new Map<string, { post: string; decided_by: string | null }>(); // Solana signature -> the agent's receipt post on Cadre
  constructor(d: Deps, cfg: TradingConfig) { this.d = d; this.cfg = cfg; }

  /** User agents with the v2 trade skill: profiles flagged `trades`, not house, whose profile blob
   * carries a verifying spec with a trade section (every 60 s). */
  async userTraders(): Promise<UserTrader[]> {
    if (Date.now() - this.users.at < 60_000) return this.users.v;
    const out: UserTrader[] = [];
    for (let off = 0; off < 5000; off += 200) {
      const r = await this.d.node.call<any>("listProfiles", { offset: off, limit: 200 }).catch(() => null);
      if (!r) break;
      for (const p of r.profiles ?? []) {
        if (!p.trades || p.house || this.isHouse(p.account)) continue;
        const v = await verifyAgent(this.d, p.account).catch(() => null);
        const s = v?.doc?.spec;
        if (!s?.trade || !Array.isArray(s.trade.mints)) continue;
        out.push({ account: p.account, handle: p.handle ?? null, name: p.name ?? null, creator: s.operator, hosting: s.hosting, sovereign: isSovereign(s), mints: s.trade.mints.slice(0, 4), verified: !!v?.verified });
      }
      if ((r.profiles ?? []).length < 200) break;
    }
    this.users = { at: Date.now(), v: out };
    return out;
  }

  /** The agent's receipt posts on Cadre ("receipt sol:<sig> ... decided by req_..."). */
  async userReceipts(account: string) {
    const r = await this.d.chain<any>("getAccountTxs", { account, limit: 200 }).catch(() => null);
    for (const t of r?.txs ?? []) {
      if (t.kind !== "Post" || typeof t.tx !== "string") continue;
      let body: string | undefined;
      try { const b: any = decodeSocialTx(Uint8Array.from(Buffer.from(t.tx, "base64"))).body; body = b.kind === "Post" ? b.body : undefined; } catch { body = undefined; }
      if (typeof body !== "string") continue;
      const m = /^receipt sol:([1-9A-HJ-NP-Za-km-z]{60,90})/.exec(body);
      if (!m) continue;
      this.receipts.set(m[1], { post: t.tx_hash ?? t.hash ?? "", decided_by: /decided by (\S+)/.exec(body)?.[1] ?? null });
    }
  }

  /** Own-wallet fills of every user trader, from its Solana signatures (each tx's balance changes). */
  userTradesP: Promise<Trade[]> | null = null;
  /** One scan at a time: concurrent readers share the scan in flight (else a second caller would see the
   * scan stamped but its signatures not read yet). */
  userTrades(): Promise<Trade[]> {
    if (!this.userTradesP) this.userTradesP = this.userTradesScan().finally(() => { this.userTradesP = null; });
    return this.userTradesP;
  }

  async userTradesScan(): Promise<Trade[]> {
    const users = await this.userTraders().catch(() => []);
    for (const u of users) {
      if (Date.now() - (this.userScanAt.get(u.account) ?? 0) < 30_000) continue;
      this.userScanAt.set(u.account, Date.now());
      const seen = this.userSigs.get(u.account) ?? new Map();
      this.userSigs.set(u.account, seen);
      const list = await this.d.sol.getSignatures(u.account, 100).catch(() => []);
      const fresh = list.filter((x) => !seen.has(x.signature) && !x.err);
      for (let i = 0; i < fresh.length; i += 8) {
        await Promise.all(fresh.slice(i, i + 8).map(async (x) => {
          const t = await this.d.sol.getTransaction(x.signature).catch(() => null);
          if (!t) return;
          const keys: string[] = (t.transaction?.message?.accountKeys ?? []).map((k: any) => (typeof k === "string" ? k : k?.pubkey ?? String(k)));
          const pump = keys.some((k) => PUMP_PROGRAMS.has(k));
          const fills: { mint: string; side: "buy" | "sell"; amount_in: bigint; amount_out: bigint; self: boolean }[] = [];
          if (pump) {
            // the spec's mints, plus any other mint the agent holds in this tx whose pump creator is the agent (its own launches)
            const extra = this.ownedMints(t, u.account).filter((m) => !u.mints.includes(m));
            for (const mint of [...u.mints, ...extra]) {
              const f = walletFill(t, u.account, mint);
              if (!f) continue;
              const self = (await this.creatorOf(mint).catch(() => null)) === u.account;
              if (!u.mints.includes(mint) && !self) continue;
              fills.push({ mint, ...f, self });
            }
          }
          seen.set(x.signature, { slot: x.slot, blockTime: x.blockTime, fills });
        }));
      }
      if (fresh.length) await this.userReceipts(u.account).catch(() => {});
    }
    const out: Trade[] = [];
    for (const u of users) {
      for (const [sig, v] of this.userSigs.get(u.account) ?? []) {
        for (const f of v.fills) {
          const rc = this.receipts.get(sig);
          out.push({
            chain: "solana", venue: "pumpswap", agent: u.account, account: u.account, via: "wallet", side: f.side, amount_in: f.amount_in.toString(), amount_out: f.amount_out.toString(),
            unit: "lamports", token: f.mint, at: (v.blockTime ?? 0) * 1000, ref: `sol:${sig}`, house: false, flags: f.self ? [OWN_TOKEN_FLAG] : [], kind: "user_agent", receipt_post: rc?.post ?? null, decided_by: rc?.decided_by ?? null,
            self_trade: f.self,
          });
        }
      }
    }
    return out.sort((a, b) => b.at - a.at);
  }

  isHouse(a: string) { return !!this.cfg.house[a]; }

  /** Mints (not wSOL) in a tx's token balances owned by `owner`. */
  ownedMints(t: any, owner: string): string[] {
    const m = t?.meta;
    const xs = [...(m?.preTokenBalances ?? []), ...(m?.postTokenBalances ?? [])].filter((b: any) => b.owner === owner && typeof b.mint === "string" && b.mint !== WSOL).map((b: any) => b.mint as string);
    return [...new Set(xs)];
  }

  /** The pump creator of `mint`: BondingCurve.creator, else (curve gone) the canonical PumpSwap pool's coin_creator; cached 10 min. */
  async creatorOf(mint: string): Promise<string | null> {
    const c = this.creators.get(mint);
    if (c && Date.now() - c.at < 600_000) return c.v;
    const [curve, pool] = await this.d.sol.getMultipleAccounts([bondingCurvePda(mint), canonicalPool(mint)]);
    let v: string | null = null;
    if (curve && curve.owner === "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P") v = curveCreator(curve.data);
    if (!v && pool && pool.owner === "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA" && pool.data.length >= 243) v = decodePoolKeys(pool.data).coinCreator;
    if (v === "11111111111111111111111111111111") v = null;
    this.creators.set(mint, { at: Date.now(), v });
    return v;
  }

  async solMandates(): Promise<MandateRec[]> {
    const s = this.cfg.solana;
    if (!s) return [];
    if (Date.now() - this.mandates.at < 15_000) return this.mandates.v;
    const accts = await this.d.sol.getProgramAccounts(s.program, b58encode(disc("account", "Mandate")));
    this.mandates = { at: Date.now(), v: accts.map((a) => decodeMandate(a.address, a.data)).filter((x): x is MandateRec => !!x) };
    return this.mandates.v;
  }

  /** New program signatures since the last scan, each tx's MandateSwap events (kept for good). */
  async solTrades(): Promise<Trade[]> {
    const s = this.cfg.solana;
    if (!s) return [];
    if (Date.now() - this.solScanAt > 20_000) {
      this.solScanAt = Date.now();
      const list = await this.d.sol.getSignatures(s.program, 100);
      const fresh = list.filter((x) => !this.sigs.has(x.signature) && !x.err);
      // 8 at a time: public RPCs rate-limit bursts
      for (let i = 0; i < fresh.length; i += 8) {
        await Promise.all(fresh.slice(i, i + 8).map(async (x) => {
          const t = await this.d.sol.getTransaction(x.signature).catch(() => null);
          if (t) this.sigs.set(x.signature, { slot: x.slot, blockTime: x.blockTime, events: swapEvents(t.meta?.logMessages ?? []) });
        }));
      }
    }
    const mand = new Map((await this.solMandates()).map((m) => [m.address, m]));
    const out: Trade[] = [];
    for (const [sig, v] of this.sigs) {
      for (const e of v.events) {
        const m = mand.get(e.mandate);
        out.push({
          chain: "solana", venue: "pumpswap", agent: e.agent, account: e.mandate, via: "mandate", side: e.side === 0 ? "buy" : "sell",
          amount_in: e.amount_in, amount_out: e.amount_out, unit: "lamports", token: e.mint, at: (v.blockTime ?? 0) * 1000, ref: `sol:${sig}`,
          house: this.isHouse(e.agent), flags: m && m.user === e.agent ? ["agent is its own grantor"] : [], kind: this.isHouse(e.agent) ? "house" : "mandate",
        });
      }
    }
    return out.sort((a, b) => b.at - a.at);
  }

  async cadreState() {
    const c = this.cfg.cadre;
    if (!c) return this.cadre;
    if (Date.now() - this.cadre.at < 15_000) return this.cadre;
    const info = await this.d.chain<any>("queryContract", { address: c.amm_pool, args: Buffer.from(methodArgs("info")).toString("hex") });
    const pool = info?.ok ? readInfo(info.return) : null;
    const ev = await this.d.chain<any>("getContractEvents", { address: c.amm_pool, limit: 200, topic: eventTopic("Swap") });
    const trades: Trade[] = [];
    const heights = new Map<number, number>();
    for (const e of ev?.events ?? []) {
      const data = Buffer.from(e.data, "hex");
      const trader = b58encode(Buffer.from(e.topics[1], "hex"));
      const origin = b58encode(data.subarray(33, 65));
      const viaMandate = c.mandate_contract && trader === c.mandate_contract;
      trades.push({
        chain: "cadre", venue: "cadre-amm", agent: origin, account: viaMandate ? `${c.mandate_contract}` : trader, via: viaMandate ? "mandate" : "wallet",
        side: data[0] === 0 ? "buy" : "sell", amount_in: data.readBigUInt64LE(1).toString(), amount_out: data.readBigUInt64LE(9).toString(), unit: "CADRE base units",
        token: c.agent_token, at: e.height, ref: `cadre:${e.tx_hash}`, house: this.isHouse(origin), flags: [], kind: this.isHouse(origin) ? "house" : viaMandate ? "mandate" : "wallet" as any,
      });
      heights.set(e.height, (heights.get(e.height) ?? 0) + 1);
    }
    // self front-run flag: the agent's own wallet trade in the same block as, and before, its mandate trade
    for (const t of trades) {
      if (t.via !== "mandate") continue;
      const before = trades.find((x) => x.via === "wallet" && x.agent === t.agent && x.at === t.at && x.ref !== t.ref);
      if (before) t.flags.push("agent's own wallet traded the same pool in the same block");
    }
    const mandates: any[] = [];
    if (c.mandate_contract) {
      const cnt = await this.d.chain<any>("queryContract", { address: c.mandate_contract, args: Buffer.from(methodArgs("count")).toString("hex") });
      const n = cnt?.ok ? Number(Buffer.from(cnt.return, "hex").readBigUInt64LE(0)) : 0;
      for (let i = 0; i < Math.min(n, 200); i++) {
        const g = await this.d.chain<any>("queryContract", { address: c.mandate_contract, args: Buffer.from([...methodArgs("get"), ...le64(BigInt(i))]).toString("hex") });
        if (!g?.ok) continue;
        const p = await this.d.chain<any>("queryContract", { address: c.mandate_contract, args: Buffer.from([...methodArgs("position"), ...le64(BigInt(i)), ...b58decode(c.agent_token)]).toString("hex") });
        mandates.push({ id: i, ...readCadreMandate(g.return), position: p?.ok ? Buffer.from(p.return, "hex").readBigUInt64LE(0).toString() : "0" });
      }
    }
    this.cadre = { at: Date.now(), trades, pool, mandates };
    return this.cadre;
  }
}

// Cadre ABI bits (docs/CONTRACTS.md 4)
const methodArgs = (m: string) => Uint8Array.from([m.length, ...utf8(m)]);
function readInfo(hex: string) {
  const b = Buffer.from(hex, "hex");
  return { token: b58encode(b.subarray(0, 32)), reserve_cadre: b.readBigUInt64LE(32).toString(), reserve_token: b.readBigUInt64LE(40).toString(), lp_supply: b.readBigUInt64LE(48).toString(), fee_bps: b.readUInt16LE(56) };
}
function readCadreMandate(hex: string) {
  const b = Buffer.from(hex, "hex");
  let o = 0;
  const pk = () => { const v = b58encode(b.subarray(o, o + 32)); o += 32; return v; };
  const user = pk(), agent = pk(), fee_to = pk();
  const perf_fee_bps = b.readUInt16LE(o); o += 2;
  const settle_blocks = b.readBigUInt64LE(o).toString(); o += 8;
  const revoked = b[o++] === 1;
  const u = () => { const v = b.readBigUInt64LE(o); o += 8; return v; };
  const limits = { max_in: u().toString(), window_cap: u().toString(), window_blocks: u().toString(), max_trades: b.readUInt16LE(o), max_slippage_bps: b.readUInt16LE(o + 2) };
  o += 4;
  const min_pool_cadre = u().toString();
  const max_trade_share_bps = b.readUInt16LE(o); o += 2;
  const expiry_height = u().toString();
  const paused = b[o++] === 1, any_token = b[o++] === 1, n = b[o++];
  o += 32 * n;
  const equity = u(), deposited = u(), withdrawn = u(), fees_agent = u(), fees_protocol = u();
  const hwm = u(); u(); u(); u(); o += 2;
  const trades = u(), volume = u();
  return {
    user, agent: agent === b58encode(new Uint8Array(32)) ? null : agent, fee_to, perf_fee_bps, settle_blocks, revoked,
    limits: { ...limits, min_pool_cadre, max_trade_share_bps, expiry_height, paused, any_token },
    equity: equity.toString(), deposited: deposited.toString(), withdrawn: withdrawn.toString(), fees_agent: fees_agent.toString(), fees_protocol: fees_protocol.toString(),
    realized_pnl: (equity + withdrawn + fees_agent + fees_protocol - deposited).toString(), hwm: hwm.toString(), trades: trades.toString(), volume: volume.toString(),
  };
}

// ------------------------------------------------------------------ PnL

/** Average-cost realized PnL over one account's own fills (quote units). */
export function avgCostPnl(fills: { side: "buy" | "sell"; amount_in: string; amount_out: string }[]) {
  let pos = 0n, cost = 0n, realized = 0n, bought = 0n, sold = 0n;
  for (const f of fills) {
    const a = BigInt(f.amount_in), b = BigInt(f.amount_out);
    if (f.side === "buy") { pos += b; cost += a; bought += a; }
    else if (pos > 0n) {
      const take = a > pos ? pos : a;
      const basis = (cost * take) / pos;
      realized += (b * take) / a - basis;
      cost -= basis; pos -= take; sold += b;
    }
  }
  return { position: pos, cost_basis: cost, realized, bought_quote: bought, sold_quote: sold };
}

const midValue = (pos: bigint, base: bigint, quote: bigint) => (base === 0n ? 0n : (pos * quote) / base);

// ------------------------------------------------------------------ routes

export function registerTrading(api: Api, d: Deps, cfgIn?: TradingConfig | null) {
  const cfg = cfgIn === undefined ? loadTradingConfig() : cfgIn;
  const ix = cfg ? new TradingIndex(d, cfg) : null;
  const need = () => {
    if (!ix || !cfg) throw new ApiError(503, "unavailable", "trading is not configured on this server (deploy/trading-devnet*.json or TRADING_*_CONFIG)");
    return { ix, cfg };
  };

  async function solPool(mint?: string) {
    const { cfg } = need();
    if (!cfg.solana && !mint) return null;
    const poolAddr = !mint || mint === cfg.solana?.mint ? cfg.solana!.pool : canonicalPool(mint);
    const [p] = await d.sol.getMultipleAccounts([poolAddr]);
    if (!p) return null;
    const pd = Buffer.from(p.data);
    const bv = b58encode(pd.subarray(139, 171)), qv = b58encode(pd.subarray(171, 203));
    const [b, q] = await d.sol.getMultipleAccounts([bv, qv]);
    let vq = pd.length >= 261 ? pd.readBigInt64LE(245) + (pd.readBigInt64LE(253) << 64n) : 0n;
    if (vq < 0n) vq = 0n;
    const base = b ? Buffer.from(b.data).readBigUInt64LE(64) : 0n;
    const quote = q ? Buffer.from(q.data).readBigUInt64LE(64) - pd.readBigUInt64LE(271) - pd.readBigUInt64LE(279) + vq : 0n;
    return { pool: poolAddr, mint: mint ?? cfg.solana!.mint, base_reserve: base, quote_reserve: quote };
  }

  /** Own-wallet positions of user agents (Solana, PumpSwap), by average cost over their own fills. */
  async function walletPositions(agent?: string) {
    const { ix } = need();
    const users = (await ix.userTraders().catch(() => [])).filter((u) => !agent || u.account === agent);
    if (!users.length) return [];
    const trades = (await ix.userTrades().catch(() => [])).filter((t) => !agent || t.agent === agent);
    const pools = new Map<string, Awaited<ReturnType<typeof solPool>>>();
    const out: any[] = [];
    for (const u of users) {
      const own = new Set(trades.filter((t) => t.agent === u.account && t.self_trade).map((t) => t.token));
      for (const mint of [...new Set([...u.mints, ...own])]) {
        const fills = trades.filter((t) => t.agent === u.account && t.token === mint).reverse();
        if (!pools.has(mint)) pools.set(mint, await solPool(mint).catch(() => null));
        const pool = pools.get(mint);
        const p = avgCostPnl(fills);
        out.push({
          chain: "solana", venue: "pumpswap", via: "wallet", kind: "user_agent", agent: u.account, handle: u.handle, name: u.name, creator: u.creator, hosting: u.hosting, sovereign: u.sovereign, verified: u.verified, mint,
          own_token: own.has(mint),
          position_spl: p.position.toString(), cost_basis_lamports: p.cost_basis.toString(), realized_pnl_lamports: p.realized.toString(),
          marked_at_mid_lamports: pool ? midValue(p.position, pool.base_reserve, pool.quote_reserve).toString() : null,
          trades: fills.length, last_trade_at: fills.length ? fills[fills.length - 1].at : null,
          method: "average cost over this agent's own PumpSwap fills, each read from the landed Solana tx's balance changes (the tx fee and rent deposits for accounts the tx created excluded)",
        });
      }
    }
    return out;
  }

  async function solPositions(filter: (m: MandateRec) => boolean) {
    const { ix, cfg } = need();
    const ms = (await ix.solMandates()).filter(filter);
    if (!ms.length || !cfg.solana) return [];
    const vaults = ms.map((m) => findPda([utf8("vault"), b58decode(m.address)], cfg.solana!.program)[0]);
    const atas = vaults.map((v) => ata(v, cfg.solana!.mint, TOKEN_2022));
    const accts = await d.sol.getMultipleAccounts([...vaults, ...atas]);
    const pool = await solPool();
    return ms.map((m, i) => {
      const lam = BigInt(accts[i]?.lamports ?? 0);
      const equity = lam > VAULT_RENT ? lam - VAULT_RENT : 0n;
      const tok = accts[ms.length + i] ? Buffer.from(accts[ms.length + i]!.data).readBigUInt64LE(64) : 0n;
      const realized = equity + m.withdrawn + m.fees_agent + m.fees_protocol - m.deposited;
      return {
        chain: "solana", venue: "pumpswap", mandate: m.address, user: m.user, agent: m.revoked ? null : m.agent, revoked: m.revoked, house: ix.isHouse(m.agent),
        vault: vaults[i], sol_equity_lamports: equity.toString(), cadre_position_spl: tok.toString(),
        marked_at_mid_lamports: pool ? midValue(tok, pool.base_reserve, pool.quote_reserve).toString() : null,
        realized_pnl_lamports: realized.toString(), deposited_lamports: m.deposited.toString(), withdrawn_lamports: m.withdrawn.toString(),
        fees_agent_lamports: m.fees_agent.toString(), fees_protocol_lamports: m.fees_protocol.toString(), hwm_lamports: m.hwm.toString(),
        trades: Number(m.trades), volume_lamports: m.volume.toString(), limits: m.limits, perf_fee_bps: m.perf_fee_bps, settle_interval_secs: m.settle_interval_secs,
        opened_ts: Number(m.opened_ts),
      };
    });
  }

  async function cadrePositions(agent?: string) {
    const { ix, cfg } = need();
    if (!cfg.cadre) return { wallets: [], mandates: [] };
    const st = await ix.cadreState();
    const pool = st.pool;
    const by = new Map<string, Trade[]>();
    for (const t of [...st.trades].reverse()) if (t.via === "wallet") by.set(t.agent, [...(by.get(t.agent) ?? []), t]);
    const wallets = [...by.entries()].filter(([a]) => !agent || a === agent).map(([a, fills]) => {
      const p = avgCostPnl(fills);
      return {
        chain: "cadre", venue: "cadre-amm", account: a, house: ix.isHouse(a), handle: cfg.house[a] ?? null, token: cfg.cadre!.agent_token,
        position_token: p.position.toString(), cost_basis_cadre: p.cost_basis.toString(), realized_pnl_cadre: p.realized.toString(),
        marked_at_mid_cadre: pool ? midValue(p.position, BigInt(pool.reserve_token), BigInt(pool.reserve_cadre)).toString() : null,
        trades: fills.length, method: "average cost over this account's own Swap events on the pool",
      };
    });
    const mandates = st.mandates.filter((m) => !agent || m.agent === agent).map((m) => ({ chain: "cadre", venue: "cadre-amm", contract: cfg.cadre!.mandate_contract, ...m, house: m.agent ? ix.isHouse(m.agent) : false }));
    return { wallets, mandates };
  }

  async function allTrades() {
    const { ix } = need();
    const [s, c, u] = await Promise.all([ix.solTrades().catch(() => []), ix.cadreState().then((x) => x.trades).catch(() => []), ix.userTrades().catch(() => [])]);
    return { solana: [...s, ...u].sort((a, b) => b.at - a.at), cadre: c };
  }
  if (ix && cfg) {
    shared.trades = allTrades;
    shared.tradingHouse = cfg.house;
  }

  api.get("/v1/trading", async () => {
    const { cfg } = need();
    const [pool, t] = await Promise.all([solPool().catch(() => null), allTrades()]);
    const st = cfg.cadre ? await need().ix.cadreState().catch(() => null) : null;
    // quote side of each fill: what a buy spent, what a sell received
    const sum = (xs: Trade[]) => xs.reduce((s, x) => s + BigInt(x.side === "buy" ? x.amount_in : x.amount_out), 0n);
    const vol = (xs: Trade[], house: boolean) => sum(xs.filter((x) => x.house === house && !x.self_trade));
    const selfVol = (xs: Trade[]) => sum(xs.filter((x) => !x.house && x.self_trade));
    return {
      venues: {
        solana: cfg.solana ? { program: cfg.solana.program, mint: cfg.solana.mint, pool: cfg.solana.pool, reserves: pool ? { cadre_spl: pool.base_reserve.toString(), sol_lamports: pool.quote_reserve.toString() } : null, buyback_vault: cfg.solana.buyback_vault } : null,
        cadre: cfg.cadre ? { amm_pool: cfg.cadre.amm_pool, token: cfg.cadre.agent_token, mandate_contract: cfg.cadre.mandate_contract, reserves: st?.pool ?? null } : null,
      },
      house_traders: Object.entries(cfg.house).map(([account, handle]) => ({ account, handle })),
      user_agents: (await need().ix.userTraders().catch(() => [])).length,
      volume: {
        note: "quote-side volume per venue unit; house traders are excluded from organic and shown apart; user agents trading from their own wallets count as organic, except an agent's trades in a token it launched itself (self_trade), which are shown apart and never organic",
        solana_lamports: { organic: vol(t.solana, false).toString(), house: vol(t.solana, true).toString(), self_trade: selfVol(t.solana).toString() },
        cadre_base_units: { organic: vol(t.cadre, false).toString(), house: vol(t.cadre, true).toString(), self_trade: selfVol(t.cadre).toString() },
      },
      trades: { solana: t.solana.length, cadre: t.cadre.length },
    };
  }, { cache: 10 });

  api.get("/v1/trading/trades", async (c) => {
    const q = c.query;
    const agent = q.get("agent");
    if (agent) addr(agent, "agent");
    const venue = q.get("venue");
    const t = await allTrades();
    let xs = [...t.solana, ...t.cadre];
    if (venue) xs = xs.filter((x) => x.venue === venue);
    if (agent) xs = xs.filter((x) => x.agent === agent);
    if (q.get("organic") === "1") xs = xs.filter((x) => !x.house && !x.self_trade);
    const limit = Math.min(200, Math.max(1, Number(q.get("limit") ?? 50) || 50));
    return { trades: xs.slice(0, limit), total: xs.length, note: "Solana trades carry the block time in ms; Cadre trades carry the block height in `at`" };
  }, { cache: 10 });

  api.get("/v1/trading/agents/:account", async (c) => {
    const a = addr(c.params.account, "account");
    const { cfg } = need();
    const [sol, cad, t, w] = await Promise.all([solPositions((m) => m.agent === a || m.user === a), cadrePositions(a), allTrades(), walletPositions(a)]);
    const trades = [...t.solana, ...t.cadre].filter((x) => x.agent === a).slice(0, 100);
    if (!sol.length && !cad.wallets.length && !cad.mandates.length && !trades.length && !w.length) throw notFound("no trading on chain for this account");
    return {
      account: a, handle: cfg.house[a] ?? w[0]?.handle ?? null, house: !!cfg.house[a], kind: cfg.house[a] ? "house" : w.length ? "user_agent" : "mandate",
      sovereign: w[0]?.sovereign ?? null, creator: w[0]?.creator ?? null,
      positions: { solana_wallet: w, solana_mandates: sol, cadre_wallet: cad.wallets, cadre_mandates: cad.mandates },
      pnl_note: "realized only, from chain data; marked_at_mid is the position at the pool's current price, not profit",
      trades,
    };
  }, { cache: 10 });

  api.get("/v1/trading/leaderboard", async () => {
    const { cfg } = need();
    const [sol, cad, w] = await Promise.all([solPositions(() => true), cadrePositions(), walletPositions()]);
    const ownBoard = w.filter((x) => x.own_token);
    const userBoard = w.filter((x) => !x.own_token).map((x) => ({ ...x })).sort((x, y) => (BigInt(y.realized_pnl_lamports) > BigInt(x.realized_pnl_lamports) ? 1 : BigInt(y.realized_pnl_lamports) < BigInt(x.realized_pnl_lamports) ? -1 : 0));
    const solBoard = sol.map((m) => ({ agent: m.agent ?? "(revoked)", handle: m.agent ? cfg.house[m.agent] ?? null : null, house: m.house, kind: m.house ? "house" : "mandate", mandate: m.mandate, user: m.user, realized_pnl_lamports: m.realized_pnl_lamports, position_cadre_spl: m.cadre_position_spl, position_marked_at_mid_lamports: m.marked_at_mid_lamports, trades: m.trades, revoked: m.revoked, client_capital: m.user !== m.agent }))
      .sort((x, y) => (BigInt(y.realized_pnl_lamports) > BigInt(x.realized_pnl_lamports) ? 1 : -1));
    const cadBoard = [
      ...cad.mandates.map((m: any) => ({ agent: m.agent ?? "(revoked)", handle: m.agent ? cfg.house[m.agent] ?? null : null, house: m.house, kind: m.house ? "house" : "mandate", mandate: `${m.contract}#${m.id}`, realized_pnl_cadre: m.realized_pnl, position_token: m.position, trades: Number(m.trades), revoked: m.revoked, client_capital: m.user !== m.agent })),
      ...cad.wallets.map((w) => ({ agent: w.account, handle: w.handle, house: w.house, kind: w.house ? "house" : "wallet", mandate: null, realized_pnl_cadre: w.realized_pnl_cadre, position_token: w.position_token, position_marked_at_mid_cadre: w.marked_at_mid_cadre, trades: w.trades, revoked: false, client_capital: false })),
    ].sort((x, y) => (BigInt(y.realized_pnl_cadre) > BigInt(x.realized_pnl_cadre) ? 1 : -1));
    return {
      note: "ranked by realized PnL per venue unit (no conversion between SOL and CADRE). Mandate PnL is realized in SOL or CADRE only: buying moves value into a token position, shown apart at the pool mid and never counted as profit. Revoked and losing agents stay listed; house traders are labelled",
      user_agents: userBoard, user_agents_note: "user agents (organic): sovereign agents created by users, trading on Solana PumpSwap from their own wallets; realized PnL by average cost over their own fills, read from the landed txs",
      user_agents_own_token: ownBoard,
      user_agents_own_token_note: "user agents trading tokens they launched themselves (the pump creator is the agent): self-trades, labelled and left out of the user_agents ranking and of organic volume; their PnL by the same average-cost rule",
      solana: solBoard, cadre: cadBoard,
    };
  }, { cache: 15 });

  api.get("/v1/trading/mandates", async (c) => {
    const user = c.query.get("user");
    const agent = c.query.get("agent");
    if (!user && !agent) throw badRequest("pass user or agent");
    if (user) addr(user, "user");
    if (agent) addr(agent, "agent");
    const sol = await solPositions((m) => (!user || m.user === user) && (!agent || m.agent === agent));
    return { solana: sol };
  }, { cache: 5 });

  // ---------------------------------------------------------------- unsigned txs (the grant flow)

  async function blockhash() {
    const r = await d.solana<any>("getLatestBlockhash", [{ commitment: "confirmed" }]);
    return r.value as { blockhash: string; lastValidBlockHeight: number };
  }
  async function reply(ixs: Ix[], feePayer: string, summary: Record<string, unknown>, words: string) {
    const bh = await blockhash();
    const msg = compileLegacy(ixs, feePayer, bh.blockhash);
    return {
      transaction: Buffer.from(unsignedTx(msg)).toString("base64"), encoding: "base64", version: "legacy", fee_payer: feePayer,
      signers: msg.accountKeys.slice(0, msg.numRequiredSignatures), recent_blockhash: bh.blockhash, last_valid_block_height: bh.lastValidBlockHeight,
      summary, in_words: words, sign_with: "Wallet Standard solana:signAndSendTransaction (or signTransaction, then POST /v1/tx/solana)",
    };
  }
  const prog = () => { const s = need().cfg.solana; if (!s) throw new ApiError(503, "unavailable", "the Solana mandate program is not configured"); return s; };
  const mandatePda = (program: string, user: string, id: bigint) => findPda([utf8("mandate"), b58decode(user), le64(id)], program)[0];
  const vaultPda = (program: string, m: string) => findPda([utf8("vault"), b58decode(m)], program)[0];
  const bpsOf = (v: unknown, name: string, d0: number, max: number) => {
    const n = v === undefined || v === null || v === "" ? d0 : Number(v);
    if (!Number.isInteger(n) || n < 0 || n > max) throw badRequest(`${name} must be an integer in 0..${max}`);
    return n;
  };
  function limitsFrom(b: any, mint: string): Limits {
    const l = b.limits ?? {};
    return {
      venues: 1 << 1, any_mint: false, mints: [mint],
      max_in_per_trade: amount(l.max_in_per_trade ?? "20000000", "limits.max_in_per_trade").toString(),
      window_cap: amount(l.window_cap ?? "200000000", "limits.window_cap").toString(),
      window_secs: bpsOf(l.window_secs, "limits.window_secs", 86_400, 30 * 86_400) || 86_400,
      max_trades_per_window: bpsOf(l.max_trades_per_window, "limits.max_trades_per_window", 24, 65_535),
      max_slippage_bps: bpsOf(l.max_slippage_bps, "limits.max_slippage_bps", 300, 2_000),
      min_pool_quote_reserve: amount(l.min_pool_quote_reserve ?? "1000000000", "limits.min_pool_quote_reserve").toString(),
      max_trade_share_bps: bpsOf(l.max_trade_share_bps, "limits.max_trade_share_bps", 100, 10_000),
      expiry_ts: String(l.expiry_ts ?? 0), paused: !!l.paused,
    };
  }
  const sol = (x: string | bigint) => (Number(x) / 1e9).toString();
  const limitWords = (l: Limits) => `trades at most ${sol(l.max_in_per_trade)} SOL each, ${sol(l.window_cap)} SOL and ${l.max_trades_per_window} trades per ${l.window_secs / 3600} h, slippage at most ${l.max_slippage_bps / 100}%, only CADRE on its PumpSwap pool${l.expiry_ts !== "0" ? `, until ${new Date(Number(l.expiry_ts) * 1000).toISOString()}` : ""}`;

  api.post("/v1/tx/mandate-open", async (c) => {
    const s = prog();
    const b = await c.json();
    const user = addr(b.user, "user");
    const agent = addr(b.agent, "agent");
    const deposit = amount(b.deposit_lamports, "deposit_lamports");
    const perf = bpsOf(b.perf_fee_bps, "perf_fee_bps", 2_000, 5_000);
    const settle = bpsOf(b.settle_interval_secs, "settle_interval_secs", 86_400, 365 * 86_400);
    const limits = limitsFrom(b, s.mint);
    const mine = (await need().ix.solMandates()).filter((m) => m.user === user).map((m) => m.mandate_id);
    const id = b.mandate_id !== undefined ? amount(b.mandate_id, "mandate_id") : (mine.length ? mine.reduce((a, x) => (x > a ? x : a), 0n) + 1n : 1n);
    const m = mandatePda(s.program, user, id), v = vaultPda(s.program, m);
    const openIx: Ix = { programId: s.program, data: Buffer.concat([disc("global", "open"), le64(id), b58decode(agent), b58decode(agent), encodeLimits(limits), Buffer.from([perf & 0xff, perf >> 8]), (() => { const x = Buffer.alloc(4); x.writeUInt32LE(settle); return x; })(), le64(deposit)]),
      keys: [meta(user, true, true), meta(m, true), meta(v, true), meta(SYSTEM)] };
    const uva: Ix = { programId: AMM, data: Uint8Array.from([94, 6, 202, 115, 255, 96, 232, 183]), keys: [meta(user, true, true), meta(v), meta(findPda([utf8("user_volume_accumulator"), b58decode(v)], AMM)[0], true), meta(SYSTEM), meta(findPda([utf8("__event_authority")], AMM)[0]), meta(AMM)] };
    const add: Ix = { programId: s.program, data: disc("global", "add_mint"), keys: [meta(user, true, true), meta(m), meta(v), meta(s.mint), meta(ata(v, s.mint, TOKEN_2022), true), meta(TOKEN_2022), meta(ATA_PROGRAM), meta(SYSTEM)] };
    const words = `Open trading account #${id} that you own. Deposit ${sol(deposit)} SOL. Agent ${agent} may only swap SOL and CADRE inside it: ${limitWords(limits)}. It can never withdraw. You can withdraw or revoke at any time. Fee: ${perf / 100}% of realized SOL profit above the high-water mark, settled at most every ${settle / 3600} h; 10% of that fee buys CADRE back.`;
    return reply([computeUnitLimit(200_000), openIx, uva, add], user, { mandate: m, vault: v, mandate_id: id.toString(), agent, deposit_lamports: deposit.toString(), limits, perf_fee_bps: perf, settle_interval_secs: settle }, words);
  });

  const ownMandate = async (user: string, mandate: string) => {
    const m = (await need().ix.solMandates()).find((x) => x.address === mandate);
    if (!m) throw notFound("no such mandate");
    if (m.user !== user) throw badRequest("that mandate belongs to another user");
    return m;
  };
  api.post("/v1/tx/mandate-revoke", async (c) => {
    const s = prog();
    const b = await c.json();
    const user = addr(b.user, "user");
    const mandate = addr(b.mandate, "mandate");
    const m = await ownMandate(user, mandate);
    const ix0: Ix = { programId: s.program, data: disc("global", "revoke"), keys: [meta(user, false, true), meta(mandate, true)] };
    return reply([ix0], user, { mandate, agent: m.agent }, `Revoke agent ${m.agent} on trading account #${m.mandate_id}. It can no longer trade; your funds stay in the account until you withdraw.`);
  });
  api.post("/v1/tx/mandate-withdraw", async (c) => {
    const s = prog();
    const b = await c.json();
    const user = addr(b.user, "user");
    const mandate = addr(b.mandate, "mandate");
    const m = await ownMandate(user, mandate);
    const v = vaultPda(s.program, mandate);
    const [va, ta] = await d.sol.getMultipleAccounts([v, ata(v, s.mint, TOKEN_2022)]);
    const eq = BigInt(va?.lamports ?? 0) > VAULT_RENT ? BigInt(va!.lamports) - VAULT_RENT : 0n;
    const tok = ta ? Buffer.from(ta.data).readBigUInt64LE(64) : 0n;
    const lam = b.lamports === undefined || b.lamports === "all" ? eq : amount(b.lamports, "lamports");
    const ixs: Ix[] = [computeUnitLimit(100_000)];
    if (tok > 0n && b.tokens !== "none") {
      ixs.push({ programId: s.program, data: Buffer.concat([disc("global", "withdraw_token"), le64(tok)]), keys: [meta(user, true, true), meta(mandate), meta(v, true), meta(s.mint), meta(ata(v, s.mint, TOKEN_2022), true), meta(ata(user, s.mint, TOKEN_2022), true), meta(TOKEN_2022), meta(ATA_PROGRAM), meta(SYSTEM)] });
    }
    if (lam > 0n) ixs.push({ programId: s.program, data: Buffer.concat([disc("global", "withdraw_sol"), le64(lam)]), keys: [meta(user, true, true), meta(mandate, true), meta(v, true), meta(SYSTEM)] });
    if (ixs.length === 1) throw new ApiError(422, "nothing_to_withdraw", "the account holds no SOL above rent and no CADRE");
    return reply(ixs, user, { mandate, sol_lamports: lam.toString(), cadre_spl: b.tokens === "none" ? "0" : tok.toString() }, `Withdraw ${sol(lam)} SOL${tok > 0n && b.tokens !== "none" ? ` and ${(Number(tok) / 1e6).toString()} CADRE` : ""} from trading account #${m.mandate_id} to your wallet.`);
  });
  api.post("/v1/tx/mandate-limits", async (c) => {
    const s = prog();
    const b = await c.json();
    const user = addr(b.user, "user");
    const mandate = addr(b.mandate, "mandate");
    const m = await ownMandate(user, mandate);
    const limits = limitsFrom(b, s.mint);
    const ix0: Ix = { programId: s.program, data: Buffer.concat([disc("global", "set_limits"), encodeLimits(limits)]), keys: [meta(user, false, true), meta(mandate, true)] };
    return reply([ix0], user, { mandate, limits }, `${limits.paused ? "Pause" : "Set new limits on"} trading account #${m.mandate_id}: ${limitWords(limits)}.`);
  });
  void WSOL; void FEE_PROGRAM; void TOKEN; void canonicalPool; void createAtaIdempotent;
}
