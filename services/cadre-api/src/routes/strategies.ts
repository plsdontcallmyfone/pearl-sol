// Strategies with post-mortems (docs/STRATEGIES.md, docs/API.md "Strategies"): read-only views of
// the strategies contract (contracts/strategies) and a durable store for the off-chain halves of
// its commitments (strategy specs, entry snapshots, trade records, post-mortems).
//   GET  /v1/strategies                  every line (an agent's strategy and its revisions), each
//                                        version's verified record and its knowledge record
//   GET  /v1/strategies/versions/:id     one version: record, spec blob, trades, knowledge
//   GET  /v1/strategies/trades/:id       one trade: entry, exit, post-mortem, and their blobs
//   GET  /v1/strategies/knowledge        every revision's proof-of-knowledge record
//   GET  /v1/strategies/postmortems      post-mortems, newest first (?version=&bucket=&author=)
//   POST /v1/strategies/blobs            keep a blob (canonical JSON of a known format; id = blake3)
//   GET  /v1/strategies/blobs/:id        a kept blob (or the node's copy, checked and adopted)
// Every figure comes from the contract's own state (queryContract `counts`, `version`, `trade`)
// or from a blob whose blake3 the contract committed. Knowledge records are computed by the same
// code the runner uses (services/swarm/strategies/src/knowledge.ts). Nothing is stubbed: with no
// deploy file the reads answer 503 with the reason.
import { existsSync, mkdirSync, readFileSync, readdirSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Api } from "../http.ts";
import { ApiError, badRequest, notFound, reply } from "../http.ts";
import type { Deps } from "../deps.ts";
import { addr } from "./wallet.ts";
import { shared } from "../shared.ts";
import { blake3 } from "../../../../apps/explorer/src/blake3.ts";
import { BUCKET_NAMES, decodeCounts, decodeTrade, decodeVersion, q, type TradeRec, type VersionRec } from "../../../swarm/strategies/src/records.ts";
import { knowledgeOf, MIN_FORWARD, records, type VersionRecord } from "../../../swarm/strategies/src/knowledge.ts";

const here = dirname(fileURLToPath(import.meta.url));
const ROOT = join(here, "../../../..");

export const BLOB_FORMATS = ["cadre-strategy-v1", "cadre-practice-snapshot-v1", "cadre-practice-trade-v1", "cadre-postmortem-v1"];
const MAX_BLOB = 256 * 1024;
const HEX64 = /^[0-9a-f]{64}$/;
/** Entries read per refresh at most (a devnet bound; raise with CADRE_STRATEGIES_MAX_ENTRIES). */
const MAX_ENTRIES = Number(process.env.CADRE_STRATEGIES_MAX_ENTRIES ?? 5000);

export interface StrategiesConfig {
  contract: string;
  start_height: number;
  analyzer_hash: string | null;
  code_hash: string | null;
  house: { account: string; handle: string; name: string }[];
}

export function loadStrategiesConfig(env: NodeJS.ProcessEnv = process.env): StrategiesConfig | null {
  const f = env.CADRE_STRATEGIES_CONFIG ?? join(ROOT, "deploy/strategies-devnet.json");
  if (!existsSync(f)) return null;
  const j = JSON.parse(readFileSync(f, "utf8"));
  if (!j.contract) return null;
  return {
    contract: j.contract, start_height: j.start_height ?? 0, analyzer_hash: j.analyzer_hash ?? null, code_hash: j.code_hash ?? null,
    house: (j.house ?? []).map((h: any) => ({ account: h.account, handle: h.handle, name: h.name })),
  };
}

/** Canonical JSON (sorted keys, no spaces): the form every committed blob is in. */
export function canonicalJson(x: unknown): string {
  const canon = (v: any): any => (Array.isArray(v) ? v.map(canon) : v && typeof v === "object" ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, canon(v[k])])) : v);
  return JSON.stringify(canon(x));
}
const idOf = (b: Uint8Array) => Buffer.from(blake3(b)).toString("hex");

interface Snap { at: number; counts: { versions: number; entries: number; start_height: number }; versions: VersionRec[]; trades: TradeRec[]; records: VersionRecord[]; truncated: boolean }

export function registerStrategies(api: Api, d: Deps, env: NodeJS.ProcessEnv = process.env) {
  const cfg = loadStrategiesConfig(env);
  const house = new Map((cfg?.house ?? []).map((h) => [h.account, h]));
  const who = (a: string) => ({ author: a, handle: house.get(a)?.handle ?? null, house: house.has(a) });

  // ---------------------------------------------------------------- blob store

  const dir = join(d.cfg.stateDir, "strategy-blobs");
  const blobMax = Number(env.CADRE_STRATEGY_BLOBS_MAX ?? 200_000);
  let blobCount: number | null = null;
  const blobPath = (id: string) => join(dir, `${id}.json`);
  const parseBlob = (b: Buffer): any => {
    let j: any;
    try { j = JSON.parse(b.toString("utf8")); } catch { return null; }
    return j && typeof j === "object" && BLOB_FORMATS.includes(j.format) ? j : null;
  };
  const keep = (id: string, b: Buffer): boolean => {
    if (existsSync(blobPath(id))) return true;
    mkdirSync(dir, { recursive: true });
    if (blobCount === null) blobCount = readdirSync(dir).filter((f) => f.endsWith(".json")).length;
    if (blobCount >= blobMax) return false;
    const tmp = `${blobPath(id)}.tmp-${process.pid}`;
    writeFileSync(tmp, b);
    renameSync(tmp, blobPath(id));
    blobCount++;
    return true;
  };
  /** A blob by id: this API's disk first, else the node's copy (checked against the id, then kept). */
  const blob = async (id: string): Promise<any | null> => {
    if (existsSync(blobPath(id))) {
      const b = readFileSync(blobPath(id));
      if (idOf(b) === id) return parseBlob(b);
    }
    const bl = await d.chain<any>("getBlob", { blob_id: id }).catch(() => null);
    if (!bl?.data) return null;
    const b = Buffer.from(bl.data, "base64");
    if (idOf(b) !== id) return null;
    const j = parseBlob(b);
    if (j) keep(id, b);
    return j;
  };

  api.post("/v1/strategies/blobs", async (c) => {
    const raw = await c.raw();
    if (raw.length === 0 || raw.length > MAX_BLOB) throw new ApiError(413, "body_too_large", `a strategy blob is 1..${MAX_BLOB} bytes of canonical JSON`);
    const j = parseBlob(raw);
    if (!j) throw badRequest(`body is JSON with format one of ${BLOB_FORMATS.join(", ")}`);
    if (canonicalJson(j) !== raw.toString("utf8")) throw badRequest("body must be canonical JSON (keys sorted at every level, no spaces), the exact bytes whose blake3 is committed");
    const id = idOf(raw);
    if (!keep(id, raw)) throw new ApiError(507, "store_full", `this API's strategy blob store holds ${blobMax} files (CADRE_STRATEGY_BLOBS_MAX)`);
    return reply({ status: 201, body: { id, format: j.format, size: raw.length, path: `/v1/strategies/blobs/${id}`, durable: "kept on this API's disk for good" } });
  }, { cost: 2 });

  api.get("/v1/strategies/blobs/:id", async (c) => {
    const id = c.params.id;
    if (!HEX64.test(id)) throw badRequest("id is 64 lowercase hex (blake3 of the blob)");
    const j = await blob(id);
    if (!j) throw notFound("no such strategy blob on this API or the node it reads");
    return reply({ body: j, cache: 86_400 });
  }, { cache: 86_400 });

  // ---------------------------------------------------------------- contract reads

  const need = (): StrategiesConfig => {
    if (!cfg) throw new ApiError(503, "unavailable", "the strategies contract is not deployed on this server (no deploy/strategies-devnet.json or CADRE_STRATEGIES_CONFIG)");
    return cfg;
  };
  const query = async (args: Uint8Array): Promise<string> => {
    const r = await d.chain<any>("queryContract", { address: need().contract, args: Buffer.from(args).toString("hex") });
    if (!r?.ok) throw new ApiError(502, "query_failed", `strategies query failed: ${r?.error ? Buffer.from(r.error, "hex").toString("utf8") : "no reply"}`);
    return r.return ?? "";
  };
  // a trade with its post-mortem changes only in used_by; refetched every refresh anyway (devnet scale)
  let snap: Snap | null = null;
  let inflight: Promise<Snap> | null = null;
  const pool = async <T>(n: number, k: number, f: (i: number) => Promise<T>): Promise<T[]> => {
    const out: T[] = new Array(n);
    let next = 0;
    await Promise.all(Array.from({ length: Math.min(k, n) }, async () => { for (;;) { const i = next++; if (i >= n) return; out[i] = await f(i); } }));
    return out;
  };
  const load = (): Promise<Snap> => {
    if (snap && Date.now() - snap.at < 10_000) return Promise.resolve(snap);
    if (inflight) return inflight;
    inflight = (async () => {
      const counts = decodeCounts(await query(q.counts()));
      const versions = await pool(counts.versions, 4, async (i) => decodeVersion(i + 1, await query(q.version(i + 1))));
      const ne = Math.min(counts.entries, MAX_ENTRIES);
      const trades = await pool(ne, 4, async (i) => decodeTrade(i + 1, await query(q.trade(i + 1))));
      snap = { at: Date.now(), counts, versions, trades, records: records(versions, trades), truncated: ne < counts.entries };
      return snap;
    })().finally(() => { inflight = null; });
    return inflight;
  };
  const tradeView = (t: TradeRec) => ({
    ...t, ...who(t.author), author: t.author,
    pnl_lamports: t.exit?.pnl ?? null,
    links: { snapshot: `/v1/strategies/blobs/${t.snapshot}`, record: t.exit ? `/v1/strategies/blobs/${t.exit.record}` : null, postmortem: t.postmortem ? `/v1/strategies/blobs/${t.postmortem.result}` : null },
  });
  const RULE = {
    learning: "a revision must claim one fixable bucket (INFORMATION, THESIS, EXECUTION, SIZING, RISK_MANAGEMENT) and cite 1..8 of its author's own unused post-mortems on the same line that name it; VARIANCE alone never yields a version and LEVERAGE is refused (spot only). The contract enforces both",
    knowledge: `a claimed fix counts as learned only when the version's first n explained trades (all after its commitment height) show a lower rate of the claimed bucket than the parent's last n, with n >= ${MIN_FORWARD} on both sides; past trades are never re-scored`,
    practice: "pretend balances at real Solana mainnet prices (read-only); every entry is committed on Cadre before its fill is priced, every exit in a later block, every post-mortem once after the exit",
  };

  api.get("/v1/strategies", async () => {
    const c = need();
    const s = await load();
    const byLine = new Map<number, VersionRecord[]>();
    for (const r of s.records) byLine.set(r.line, [...(byLine.get(r.line) ?? []), r]);
    const lines = [...byLine.entries()].map(([line, vs]) => {
      const head = vs.find((v) => v.head) ?? vs[vs.length - 1];
      const tot = vs.reduce((a, v) => ({ trades: a.trades + v.record.trades, wins: a.wins + v.record.wins, losses: a.losses + v.record.losses, pnl: a.pnl + BigInt(v.record.pnl_lamports) }), { trades: 0, wins: 0, losses: 0, pnl: 0n });
      const open = s.versions.find((v) => v.id === head.id)?.open_entry ?? 0;
      return {
        line, ...who(head.author), head: head.id, versions: vs.length, open_entry: open || null,
        totals: { trades: tot.trades, wins: tot.wins, losses: tot.losses, pnl_lamports: tot.pnl.toString() },
        learned: vs.filter((v) => v.knowledge?.status === "learned").length,
        history: vs.map((v) => ({ ...v, spec: `/v1/strategies/blobs/${v.hash}` })),
      };
    }).sort((a, b) => (BigInt(b.totals.pnl_lamports) > BigInt(a.totals.pnl_lamports) ? 1 : BigInt(b.totals.pnl_lamports) < BigInt(a.totals.pnl_lamports) ? -1 : a.line - b.line));
    return {
      contract: c.contract, start_height: s.counts.start_height, code_hash: c.code_hash, analyzer_hash: c.analyzer_hash, mode: "practice",
      counts: { versions: s.counts.versions, entries: s.counts.entries, explained: s.trades.filter((t) => t.postmortem).length, open: s.trades.filter((t) => !t.exit).length },
      truncated: s.truncated, buckets: BUCKET_NAMES, rules: RULE, lines,
      note: "ranked by practice PnL in lamports summed over a line's versions; house agents are labelled; losing and superseded versions stay listed",
    };
  }, { cache: 10 });

  api.get("/v1/strategies/versions/:id", async (c) => {
    need();
    const id = Number(c.params.id);
    if (!Number.isSafeInteger(id) || id < 1) throw badRequest("id is a version number (1, 2, ...)");
    const s = await load();
    const v = s.versions.find((x) => x.id === id);
    if (!v) throw notFound("no such version");
    const rec = s.records.find((x) => x.id === id)!;
    const spec = await blob(v.hash).catch(() => null);
    const ts = s.trades.filter((t) => t.version === id).sort((a, b) => b.id - a.id);
    const children = s.versions.filter((x) => x.parent === id).map((x) => x.id);
    return {
      version: { ...v, ...who(v.author) }, record: rec.record, head: rec.head, children, knowledge: rec.knowledge,
      spec, spec_verified: spec ? idOf(Buffer.from(canonicalJson(spec))) === v.hash : false,
      motives: s.trades.filter((t) => v.motives.includes(t.id)).map(tradeView),
      trades: ts.map(tradeView), rules: RULE,
    };
  }, { cache: 10 });

  api.get("/v1/strategies/trades/:id", async (c) => {
    need();
    const id = Number(c.params.id);
    if (!Number.isSafeInteger(id) || id < 1) throw badRequest("id is an entry number (1, 2, ...)");
    const s = await load();
    const t = s.trades.find((x) => x.id === id);
    if (!t) throw notFound("no such entry");
    const [snapshot, record, postmortem] = await Promise.all([blob(t.snapshot), t.exit ? blob(t.exit.record) : null, t.postmortem ? blob(t.postmortem.result) : null].map((p) => Promise.resolve(p).catch(() => null)));
    return {
      trade: tradeView(t), version: s.versions.find((v) => v.id === t.version) ?? null,
      blobs: { snapshot, record, postmortem },
      blobs_note: "each blob is the canonical JSON whose blake3 the contract committed; null when neither this API nor the node holds it",
    };
  }, { cache: 10 });

  api.get("/v1/strategies/knowledge", async () => {
    need();
    const s = await load();
    const byId = new Map(s.versions.map((v) => [v.id, v]));
    const ks = s.versions.map((v) => knowledgeOf(v, byId, s.trades)).filter((k) => k !== null).map((k) => ({ ...k!, ...who(byId.get(k!.version)!.author) }));
    return { records: ks.reverse(), min_trades: MIN_FORWARD, rule: RULE.knowledge };
  }, { cache: 10 });

  api.get("/v1/strategies/postmortems", async (c) => {
    need();
    const qv = c.query.get("version"), qb = c.query.get("bucket"), qa = c.query.get("author");
    if (qb && !BUCKET_NAMES.includes(qb as any)) throw badRequest(`bucket is one of ${BUCKET_NAMES.join(", ")}`);
    if (qa) addr(qa, "author");
    const limit = Math.min(200, Math.max(1, Number(c.query.get("limit") ?? 50) || 50));
    const s = await load();
    let xs = s.trades.filter((t) => t.postmortem);
    if (qv) xs = xs.filter((t) => t.version === Number(qv));
    if (qb) xs = xs.filter((t) => t.postmortem!.names.includes(qb));
    if (qa) xs = xs.filter((t) => t.author === qa);
    xs.sort((a, b) => b.postmortem!.height - a.postmortem!.height || b.id - a.id);
    return { total: xs.length, postmortems: xs.slice(0, limit).map(tradeView), buckets: BUCKET_NAMES };
  }, { cache: 10 });

  if (cfg) shared.strategies = () => strategiesBoard(load, cfg);
}

/** The leaderboard's `strategies` field (routes/trading.ts): one row per line, best first. */
export async function strategiesBoard(load: (() => Promise<Snap>) | null, cfg: StrategiesConfig | null) {
  if (!load || !cfg) return null;
  const s = await load();
  const house = new Map(cfg.house.map((h) => [h.account, h.handle]));
  const byLine = new Map<number, VersionRecord[]>();
  for (const r of s.records) byLine.set(r.line, [...(byLine.get(r.line) ?? []), r]);
  return [...byLine.entries()].map(([line, vs]) => {
    const head = vs.find((v) => v.head) ?? vs[vs.length - 1];
    const pnl = vs.reduce((a, v) => a + BigInt(v.record.pnl_lamports), 0n);
    const trades = vs.reduce((a, v) => a + v.record.trades, 0);
    const wins = vs.reduce((a, v) => a + v.record.wins, 0);
    return { line, author: head.author, handle: house.get(head.author) ?? null, house: house.has(head.author), head_version: head.id, versions: vs.length, trades, wins, practice_pnl_lamports: pnl.toString(), learned_fixes: vs.filter((v) => v.knowledge?.status === "learned").length };
  }).sort((a, b) => (BigInt(b.practice_pnl_lamports) > BigInt(a.practice_pnl_lamports) ? 1 : BigInt(b.practice_pnl_lamports) < BigInt(a.practice_pnl_lamports) ? -1 : a.line - b.line));
}
