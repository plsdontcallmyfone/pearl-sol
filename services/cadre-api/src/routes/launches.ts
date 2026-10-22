// Token launches on Solana (docs/API.md "Token launches"): an unsigned pump.fun `create_v2` (+ the
// creator's dev buy in the same tx, so nobody buys ahead of it) for any wallet, launch metadata
// served from Cadre blobs, the list of AGENT-launched tokens read only from chain data, and the
// protocol halt record (the kill switch hosted and self-hosted agents check, SPEC 4.8).
//
// Labelling: a token is "agent-launched" when its create_v2 was signed by a Cadre user agent's key
// (a profile whose blob carries a signed agent spec with the launch skill) AND names that agent as
// the curve's creator. Nothing here is self-reported: the row is rebuilt from the Solana tx.
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { blake3 } from "../../../../apps/explorer/src/blake3.ts";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Api } from "../http.ts";
import { ApiError, badRequest, notFound, reply } from "../http.ts";
import type { Deps } from "../deps.ts";
import { addr } from "./wallet.ts";
import { verifyAgent } from "./useragents.ts";
import { walletFill } from "./trading.ts";
import { PUMP_FEE_BPS_MAX } from "./market.ts";
import { b58decode, b58encode } from "../../../../apps/explorer/src/codec.ts";
import {
  ata, bondingCurvePda, compileLegacy, cpQuote, createAtaIdempotent, decodeCreateV2, decodePumpGlobalInitial,
  type Ix, PUMP, pumpBuybackRecipients, pumpBuyExactQuoteInIx, pumpCreateV2Ix, pumpGlobalPda, TOKEN_2022, unsignedTx,
} from "../sol.ts";

const here = dirname(fileURLToPath(import.meta.url));
const ROOT = join(here, "../../../..");

/** Most slippage the dev buy may allow (bps), the same ceiling as the trade builders (CADRE_MAX_SLIPPAGE_BPS). */
const MAX_SLIPPAGE_BPS = (() => {
  const v = Number(process.env.CADRE_MAX_SLIPPAGE_BPS ?? 1000);
  return BigInt(Number.isInteger(v) && v >= 0 && v <= 5000 ? v : 1000);
})();
/** Optional per-deployment cap on the dev buy (lamports); unset = no cap in the builder (an agent's spec carries its own cap). */
const devBuyCap = (): bigint | null => {
  const v = process.env.CADRE_MAX_DEV_BUY_LAMPORTS;
  return v && /^\d{1,20}$/.test(v) ? BigInt(v) : null;
};
/** create_v2 + ATA + dev buy measured 230-250k CU in LiteSVM on mainnet bytes (research/NOTES-pump.md 2). */
/** Solana's packet limit for a whole wire tx (signatures + message). */
export const MAX_TX_BYTES = 1232;
const LABEL = "agent launches are labelled by GET /v1/launches when the creator is a Cadre user agent";
const lessBps = (x: bigint, b: bigint) => (x * (10_000n - b)) / 10_000n;
const pick = <T,>(xs: T[]) => xs[Math.floor(Math.random() * xs.length)];

function lamports(v: unknown, name: string): bigint {
  if (v === undefined || v === null) return 0n;
  const t = typeof v === "number" ? String(v) : v;
  if (typeof t !== "string" || !/^\d{1,20}$/.test(t)) throw badRequest(`${name} must be a decimal string of lamports`);
  const n = BigInt(t);
  if (n >= 2n ** 64n) throw badRequest(`${name} must be below 2^64`);
  return n;
}
function bpsOf(v: unknown, name: string, def: bigint): bigint {
  if (v === undefined || v === null) return def;
  const n = typeof v === "number" ? v : typeof v === "string" && /^\d+$/.test(v) ? Number(v) : NaN;
  if (!Number.isInteger(n) || n < 0 || BigInt(n) > MAX_SLIPPAGE_BPS) throw badRequest(`${name} must be an integer in [0, ${MAX_SLIPPAGE_BPS}]`);
  return BigInt(n);
}

/** Validates the launch text: name 1..32 bytes, symbol 1..10 of [A-Z0-9], uri http(s) and at most 200 bytes. */
export function checkLaunchText(b: { name?: unknown; symbol?: unknown; uri?: unknown }) {
  const name = b.name, symbol = b.symbol, uri = b.uri;
  if (typeof name !== "string" || !name.trim() || Buffer.byteLength(name) > 32) throw badRequest("name must be 1..32 bytes");
  if (/[\u0000-\u001f]/.test(name)) throw badRequest("name must not contain control characters");
  if (typeof symbol !== "string" || !/^[A-Z0-9]{1,10}$/.test(symbol)) throw badRequest("symbol must be 1..10 characters of A-Z and 0-9");
  if (typeof uri !== "string" || !/^https?:\/\/[^\s]+$/.test(uri) || Buffer.byteLength(uri) > 200) throw badRequest("uri must be an http(s) URL of at most 200 bytes");
  return { name, symbol, uri };
}

// ------------------------------------------------------------------ the agent-launch index

export interface LaunchRow {
  mint: string; name: string; symbol: string; uri: string; agent: string; handle: string | null; agent_creator: string | null; verified: boolean;
  signature: string; at: number | null; dev_buy_lamports: string; dev_buy_tokens_spl: string; bonding_curve: string; label: "agent-launched"; house: false;
}

/** The create_v2 in a landed tx signed by `signer` that names `signer` as the creator, or null. */
export function createV2In(t: any, signer: string): { mint: string; name: string; symbol: string; uri: string; creator: string } | null {
  const m = t?.transaction?.message;
  if (!m || t?.meta?.err) return null;
  const keys: string[] = (m.accountKeys ?? []).map((k: any) => (typeof k === "string" ? k : k?.pubkey ?? String(k)));
  const nSig = m.header?.numRequiredSignatures ?? 1;
  const si = keys.indexOf(signer);
  if (si < 0 || si >= nSig) return null;
  for (const ix of m.instructions ?? []) {
    if (keys[ix.programIdIndex] !== PUMP || typeof ix.data !== "string") continue;
    let data: Uint8Array;
    try { data = b58decode(ix.data); } catch { continue; }
    const a = decodeCreateV2(data);
    if (!a || a.creator !== signer) continue;
    const mint = keys[ix.accounts?.[0]];
    if (!mint) continue;
    return { mint, ...a };
  }
  return null;
}

const indexes = new WeakMap<Deps, LaunchIndex>();
export function launchIndex(d: Deps): LaunchIndex {
  let ix = indexes.get(d);
  if (!ix) indexes.set(d, (ix = new LaunchIndex(d)));
  return ix;
}

export class LaunchIndex {
  d: Deps;
  at = 0;
  rows: LaunchRow[] = [];
  seen = new Map<string, LaunchRow | null>(); // signature -> its launch row (null: not a launch)
  scanning: Promise<LaunchRow[]> | null = null;
  constructor(d: Deps) { this.d = d; }

  /** User agents whose signed spec has the launch skill (or a launch section). */
  async launchers() {
    const out: { account: string; handle: string | null; creator: string | null; verified: boolean }[] = [];
    for (let off = 0; off < 5000; off += 200) {
      const r = await this.d.node.call<any>("listProfiles", { offset: off, limit: 200 }).catch(() => null);
      if (!r) break;
      for (const p of r.profiles ?? []) {
        if (p.house) continue;
        const v = await verifyAgent(this.d, p.account).catch(() => null);
        const s: any = v?.doc?.spec;
        if (!s || s.agent !== p.account) continue;
        if (!s.launch && !(Array.isArray(s.skills) && s.skills.includes("launch"))) continue;
        out.push({ account: p.account, handle: p.handle ?? null, creator: s.operator ?? null, verified: !!v?.verified });
      }
      if ((r.profiles ?? []).length < 200) break;
    }
    return out;
  }

  list(): Promise<LaunchRow[]> {
    if (Date.now() - this.at < 30_000) return Promise.resolve(this.rows);
    if (!this.scanning) this.scanning = this.scan().finally(() => { this.scanning = null; });
    return this.scanning;
  }

  async scan(): Promise<LaunchRow[]> {
    const out: LaunchRow[] = [];
    for (const a of await this.launchers()) {
      const sigs = await this.d.sol.getSignatures(a.account, 100).catch(() => []);
      for (const x of sigs) {
        if (x.err) continue;
        if (!this.seen.has(x.signature)) {
          const t = await this.d.sol.getTransaction(x.signature).catch(() => undefined);
          if (t === undefined) continue; // retry next scan
          const c = t ? createV2In(t, a.account) : null;
          if (!c) { this.seen.set(x.signature, null); continue; }
          const f = walletFill(t, a.account, c.mint);
          const buy = f && f.side === "buy" ? f : null;
          this.seen.set(x.signature, {
            mint: c.mint, name: c.name, symbol: c.symbol, uri: c.uri, agent: a.account, handle: a.handle, agent_creator: a.creator, verified: a.verified,
            signature: x.signature, at: x.blockTime ? x.blockTime * 1000 : null, dev_buy_lamports: (buy?.amount_in ?? 0n).toString(), dev_buy_tokens_spl: (buy?.amount_out ?? 0n).toString(),
            bonding_curve: bondingCurvePda(c.mint), label: "agent-launched", house: false,
          });
        }
        const r = this.seen.get(x.signature);
        if (r) out.push({ ...r, handle: a.handle, agent_creator: a.creator, verified: a.verified });
      }
    }
    out.sort((p, q) => (q.at ?? 0) - (p.at ?? 0));
    this.rows = out;
    this.at = Date.now();
    return out;
  }
}

// ------------------------------------------------------------------ the halt record

/** Halt scope bits (contracts/halt): 1 every agent action, 2 trading, 4 token launches. */
export const HALT = { all: 1, trade: 2, launch: 4 } as const;

export function haltContract(env: NodeJS.ProcessEnv = process.env): { contract: string | null; reason: string } {
  if (env.CADRE_HALT_CONTRACT) return { contract: env.CADRE_HALT_CONTRACT, reason: "CADRE_HALT_CONTRACT" };
  const f = env.CADRE_HALT_DEPLOY ?? join(ROOT, "deploy/halt-devnet.json");
  if (!existsSync(f)) return { contract: null, reason: `no halt contract configured (CADRE_HALT_CONTRACT unset and ${f} missing)` };
  try {
    const c = JSON.parse(readFileSync(f, "utf8")).contract;
    return typeof c === "string" && c ? { contract: c, reason: f } : { contract: null, reason: `${f} has no contract field` };
  } catch (e) {
    return { contract: null, reason: `${f}: ${(e as Error).message}` };
  }
}

/** The halt contract's `status` return: scope u8 | seq u64 | height u64 | authority [32] | reason (u32 len + utf8). */
export function decodeHaltStatus(ret: Uint8Array) {
  const b = Buffer.from(ret);
  if (b.length < 1 + 8 + 8 + 32 + 4) throw new Error(`status return is ${b.length} bytes`);
  const scope = b[0];
  const seq = b.readBigUInt64LE(1), height = b.readBigUInt64LE(9);
  const authority = b58encode(b.subarray(17, 49));
  const n = b.readUInt32LE(49);
  if (53 + n > b.length) throw new Error("reason runs past the return data");
  return { scope, seq, height, authority, reason: b.subarray(53, 53 + n).toString("utf8") };
}
export const HALT_STATUS_ARGS = Buffer.concat([Buffer.of(6), Buffer.from("status")]);

// ------------------------------------------------------------------ routes

export function registerLaunches(api: Api, d: Deps) {
  api.post("/v1/tx/pump-create", async (c) => {
    const b = await c.json();
    const creator = addr(b.creator, "creator");
    const mint = addr(b.mint, "mint");
    if (mint === creator) throw badRequest("mint must be a fresh keypair's public key, not the creator");
    const { name, symbol, uri } = checkLaunchText(b);
    const devBuy = lamports(b.dev_buy_lamports, "dev_buy_lamports");
    const cap = devBuyCap();
    if (cap !== null && devBuy > cap) throw new ApiError(422, "dev_buy_above_cap", `dev_buy_lamports ${devBuy} is above this server's cap ${cap} (CADRE_MAX_DEV_BUY_LAMPORTS)`, { cap_lamports: cap.toString() });
    const slip = bpsOf(b.slippage_bps, "slippage_bps", 100n);
    const minOverride = b.min_tokens_out === undefined || b.min_tokens_out === null ? null : lamports(b.min_tokens_out, "min_tokens_out");

    const bc = bondingCurvePda(mint);
    const [m, curve, g] = await d.accounts([mint, bc, pumpGlobalPda()]);
    if (m) throw new ApiError(409, "mint_exists", `an account already exists at ${mint}: create_v2 needs a fresh mint keypair`);
    if (curve) throw new ApiError(409, "curve_exists", `a pump bonding curve already exists for ${mint}`);
    if (!g) throw new ApiError(503, "pump_global_missing", "pump Global account missing on this cluster");
    const gl = decodePumpGlobalInitial(g.data);
    if (!gl.createV2Enabled) throw new ApiError(503, "create_v2_disabled", "pump Global has create_v2 disabled on this cluster");

    const bal = await d.solana<any>("getBalance", [creator, { commitment: "confirmed" }]);
    const have = BigInt(bal?.value ?? 0);
    if (have < devBuy) throw new ApiError(422, "insufficient_funds", `the creator holds ${have} lamports, the dev buy spends up to ${devBuy} (plus the tx fee and the new accounts' rent)`);

    // no compute-budget instruction: the 1,232-byte packet limit is tight for create_v2 + ATA + buy
    // (about 30 accounts), and the default budget (200k per instruction, 600k here) covers the
    // measured 230-250k CU (research/NOTES-pump.md)
    const ixs: Ix[] = [pumpCreateV2Ix(creator, mint, name, symbol, uri, creator)];
    let devQuote: Record<string, unknown> = { dev_buy_lamports: "0" };
    if (devBuy > 0n) {
      // the curve starts at Global's initial reserves; exact-SOL-in buys swap sol_in / (1 + fee)
      const net = (devBuy * 10_000n) / (10_000n + PUMP_FEE_BPS_MAX);
      const quoted = cpQuote(gl.virtualToken, gl.virtualSol, net);
      if (quoted >= gl.realToken) {
        throw new ApiError(422, "dev_buy_completes_curve", `a dev buy of ${devBuy} lamports would buy ${quoted} of the curve's ${gl.realToken} real tokens and complete (graduate) the curve at once; this cluster's Global starts the curve at ${gl.virtualSol} lamports of virtual SOL`, { initial_virtual_sol_lamports: gl.virtualSol.toString(), initial_real_token_spl: gl.realToken.toString(), quote_tokens_out_spl: quoted.toString() });
      }
      const floor = lessBps(quoted, MAX_SLIPPAGE_BPS);
      if (minOverride !== null && minOverride < floor) throw new ApiError(422, "unsafe_min_out", `min_tokens_out ${minOverride} is more than ${MAX_SLIPPAGE_BPS} bps below the quote (${quoted})`, { floor: floor.toString(), quoted: quoted.toString() });
      const minOut = minOverride ?? lessBps(quoted, slip);
      if (minOut <= 0n) throw new ApiError(422, "amount_too_small", "dev_buy_lamports buys less than one base unit after fees and slippage");
      const recipient = pick(pumpBuybackRecipients(g.data));
      ixs.push(createAtaIdempotent(creator, creator, mint, TOKEN_2022), pumpBuyExactQuoteInIx(creator, mint, devBuy, minOut, recipient));
      devQuote = { dev_buy_lamports: devBuy.toString(), quote_tokens_out_spl: quoted.toString(), min_tokens_out_spl: minOut.toString(), slippage_bps: Number(slip), fee_allowance_bps: Number(PUMP_FEE_BPS_MAX), buyback_fee_recipient: recipient, dev_buy_instruction: "buy_exact_quote_in_v3" };
    }
    const r = await d.solana<any>("getLatestBlockhash", [{ commitment: "confirmed" }]);
    const bh = r.value as { blockhash: string; lastValidBlockHeight: number };
    const msg = compileLegacy(ixs, creator, bh.blockhash);
    const wire = unsignedTx(msg);
    if (wire.length > MAX_TX_BYTES) throw new ApiError(422, "tx_too_large", `the create tx is ${wire.length} bytes, ${wire.length - MAX_TX_BYTES} over Solana's ${MAX_TX_BYTES}-byte limit: shorten the name, symbol or uri (a short metadata uri is ${"<api>"}/v1/l/<blob id in base58>)`, { bytes: wire.length, max: MAX_TX_BYTES });
    return {
      transaction: Buffer.from(wire).toString("base64"), encoding: "base64", version: "legacy", fee_payer: creator, tx_bytes: wire.length,
      signers: msg.accountKeys.slice(0, msg.numRequiredSignatures), recent_blockhash: bh.blockhash, last_valid_block_height: bh.lastValidBlockHeight,
      summary: {
        creator, mint, name, symbol, uri, program: PUMP, instruction: "create_v2", bonding_curve: bc, token_account: ata(creator, mint, TOKEN_2022), token_program: TOKEN_2022,
        curve_start: { initial_virtual_sol_lamports: gl.virtualSol.toString(), initial_virtual_token_spl: gl.virtualToken.toString(), initial_real_token_spl: gl.realToken.toString(), token_total_supply_spl: gl.tokenTotalSupply.toString() },
        ...devQuote,
      },
      label: LABEL,
      note: "two signers: the creator (fee payer, curve creator, dev buyer) and the fresh mint keypair; the dev buy rides in the same tx, so nobody can buy ahead of it, and fails without moving funds below min_tokens_out",
      sign_with: "sign with the creator wallet and the mint keypair (partialSign), then POST /v1/tx/solana",
    };
  });

  // Durable copy of every launch metadata blob (2026-10-22): node blobs that no chain state names
  // are pruned after grace_blocks (RPC.md "Retention", the blob lifecycle rule), and a token's
  // metadata uri must never 404 (pump.fun and wallets read it). So this API keeps the exact bytes
  // on its own disk under <STATE_DIR>/launch-meta/<blob id>.json (the id is blake3 of the bytes,
  // so the copy is checkable against the uri), serves the disk copy first, and adopts any launch
  // metadata it finds on the node. CADRE_LAUNCH_META_MAX (default 20,000 files) bounds the store.
  const metaDir = join(d.cfg.stateDir, "launch-meta");
  const metaMax = Number(process.env.CADRE_LAUNCH_META_MAX ?? 20_000);
  const META_MAX_BYTES = 2048;
  const metaPath = (id: string) => join(metaDir, `${id}.json`);
  const parseMeta = (b: Buffer): any => {
    let j: any;
    try { j = JSON.parse(b.toString("utf8")); } catch { return null; }
    return j?.format === "cadre-launch-meta-v1" && typeof j.name === "string" && typeof j.symbol === "string" ? j : null;
  };
  const keepMeta = (id: string, b: Buffer): boolean => {
    if (existsSync(metaPath(id))) return true;
    mkdirSync(metaDir, { recursive: true });
    if (readdirSync(metaDir).length >= metaMax) return false;
    const tmp = `${metaPath(id)}.tmp-${process.pid}`;
    writeFileSync(tmp, b);
    renameSync(tmp, metaPath(id));
    return true;
  };
  const b58id = (hexId: string) => b58encode(Buffer.from(hexId, "hex"));

  api.post("/v1/launches/metadata", async (c) => {
    const raw = await c.raw();
    if (raw.length === 0 || raw.length > META_MAX_BYTES) throw new ApiError(413, "body_too_large", `launch metadata is 1..${META_MAX_BYTES} bytes of JSON`);
    const j = parseMeta(raw);
    if (!j) throw badRequest("body is launch metadata JSON: {format: \"cadre-launch-meta-v1\", name, symbol, description, agent_launched, agent, ...}");
    if (Buffer.byteLength(j.name) > 32 || !/^[A-Za-z0-9]{1,10}$/.test(j.symbol)) throw badRequest("name is at most 32 bytes and symbol 1..10 letters or digits (create_v2 limits)");
    if (j.agent !== undefined && j.agent !== null) addr(j.agent, "agent");
    const id = Buffer.from(blake3(raw)).toString("hex");
    if (!keepMeta(id, raw)) throw new ApiError(507, "store_full", `this API's launch metadata store holds ${metaMax} files (CADRE_LAUNCH_META_MAX)`);
    // also a Cadre blob, so anyone can fetch it by id from a node while the node keeps it
    const put = await d.chain<any>("putBlob", { data: raw.toString("base64") }).catch((e: Error) => ({ error: e.message }));
    return reply({ status: 201, body: { id, id_b58: b58id(id), path: `/v1/l/${b58id(id)}`, size: raw.length, durable: "kept on this API's disk for good", node_blob: put?.blob_id === id ? "stored" : `not stored: ${put?.error ?? "unexpected reply"}` } });
  }, { cost: 10 });

  const metadata = async (raw: string) => {
    // 64 lowercase hex, or the same 32 bytes in base58 (the short form keeps the create tx small)
    let id = raw;
    if (!/^[0-9a-f]{64}$/.test(id)) {
      let k: Uint8Array | null = null;
      try { k = /^[1-9A-HJ-NP-Za-km-z]{32,44}$/.test(raw) ? b58decode(raw) : null; } catch { k = null; }
      if (!k || k.length !== 32) throw badRequest("id must be a Cadre blob id: 64 lowercase hex or its 32 bytes in base58");
      id = Buffer.from(k).toString("hex");
    }
    if (existsSync(metaPath(id))) {
      const j = parseMeta(readFileSync(metaPath(id)));
      if (j) return reply({ body: j, cache: 86_400 });
    }
    const bl = await d.chain<any>("getBlob", { blob_id: id }).catch(() => null);
    if (!bl?.data) throw notFound("no such launch metadata on this API or the node it reads");
    const bytes = Buffer.from(bl.data, "base64");
    const j = parseMeta(bytes);
    if (!j) throw notFound("the blob is not launch metadata (format cadre-launch-meta-v1)");
    if (Buffer.from(blake3(bytes)).toString("hex") === id) keepMeta(id, bytes); // adopt: served once, kept for good
    return reply({ body: j, cache: 86_400 });
  };
  api.get("/v1/launches/metadata/:id", (c) => metadata(c.params.id), { cache: 86_400 });
  api.get("/v1/l/:id", (c) => metadata(c.params.id), { cache: 86_400 });

  api.get("/v1/launches", async (c) => {
    const agent = c.query.get("agent");
    if (agent) addr(agent, "agent");
    const limit = Math.min(200, Math.max(1, Number(c.query.get("limit") ?? 50) || 50));
    let rows = await launchIndex(d).list();
    if (agent) rows = rows.filter((r) => r.agent === agent);
    return {
      launches: rows.slice(0, limit), total: rows.length,
      rule: "agent-launched = a pump create_v2 signed by a Cadre user agent's key (its profile carries a signed spec with the launch skill) naming that agent as the curve's creator; rebuilt from the Solana txs, never self-reported. The agent's own trades in these tokens are labelled self_trade on /v1/trading and excluded from organic volume",
    };
  }, { cache: 30 });

  api.get("/v1/launches/:mint", async (c) => {
    const mint = addr(c.params.mint, "mint");
    const r = (await launchIndex(d).list()).find((x) => x.mint === mint);
    if (!r) throw notFound("not an agent-launched token (no create_v2 signed by a Cadre user agent with the launch skill names this mint)");
    return r;
  }, { cache: 30 });

  api.get("/v1/halt", async () => {
    const h = haltContract();
    if (!h.contract) throw new ApiError(503, "unavailable", h.reason);
    const r = await d.chain<any>("queryContract", { address: h.contract, args: HALT_STATUS_ARGS.toString("hex") });
    if (!r?.ok) throw new ApiError(502, "halt_query_failed", `queryContract status on ${h.contract}: ${r?.error ?? "failed"}`);
    let s: ReturnType<typeof decodeHaltStatus>;
    try { s = decodeHaltStatus(Buffer.from(r.return ?? "", "hex")); } catch (e) { throw new ApiError(502, "halt_decode_failed", (e as Error).message); }
    return {
      contract: h.contract, scope: s.scope,
      halted: { all: (s.scope & HALT.all) !== 0, trade: (s.scope & HALT.trade) !== 0, launch: (s.scope & HALT.launch) !== 0 },
      seq: s.seq.toString(), height: s.height.toString(), authority: s.authority, reason: s.reason,
      note: "signed by the halt authority on Cadre; hosted agents obey it; a self-hosted cadre-agent checks it before acting, but whoever runs one can modify it, so for self-hosted agents it is advisory",
    };
  }, { cache: 5 });
}
