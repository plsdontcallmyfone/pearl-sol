// Agent services market (docs/MARKET.md, docs/API.md "Services market"): offers, hires, their
// timelines and the provider records, all read from the market contract (contracts/market) via
// the node's contract RPC, plus unsigned Call txs for the Cadre Wallet with a plain-words review
// of what the signer pays, to whom, and when it comes back. Reputation follows the indexer rules
// (apps/explorer/src/market.ts `providerRecord`): money from distinct payers, self-dealing and
// house activity shown apart and never counted as organic. Market blobs (requests, deliveries,
// verdicts) are referenced only from contract storage, so the node prunes them after its grace
// window; this route keeps a copy of every market blob it reads under STATE_DIR/market-blobs.
// No config or no contract RPC: 503 with the reason. Nothing is stubbed.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import type { Api } from "../http.ts";
import { ApiError, badRequest, notFound } from "../http.ts";
import type { Deps } from "../deps.ts";
import { addr } from "./wallet.ts";
import { TownState } from "./town.ts";
import { agentTables } from "../../../../apps/explorer/src/profile.ts";
import { blake3 } from "../../../../apps/explorer/src/blake3.ts";
import { hex } from "../../../../apps/explorer/src/codec.ts";
import { TX_HDR, TX_SIG, contractFeeFloor, contractSigningBytes, encodeCallBody, framedMessage, isUnknownMethod } from "../../../../apps/explorer/src/contracts.ts";
import {
  MAX, MarketIndex, STATUS, exclusion, feeOf, isKey, marketArgs, providerRecord, type Context, type Hire, type Offer,
} from "../../../../apps/explorer/src/market.ts";

const here = dirname(fileURLToPath(import.meta.url));
const ROOT = join(here, "../../../..");
const BLOCK_S = 60;
const HEX64 = /^[0-9a-fA-F]{64}$/;

export interface MarketDeploy { market: string; code_hash?: string; admin?: string; judge?: string | null; house: Record<string, string>; deployed_height?: number }

export function loadMarketDeploy(env: NodeJS.ProcessEnv = process.env): MarketDeploy | null {
  const f = env.MARKET_CONFIG ?? join(ROOT, "deploy/market-devnet.json");
  if (!existsSync(f)) return null;
  const j = JSON.parse(readFileSync(f, "utf8"));
  if (!isKey(j.market)) return null;
  return { market: j.market, code_hash: j.code_hash, admin: j.admin, judge: j.judge ?? null, house: j.house ?? {}, deployed_height: j.deployed_height };
}

const cad = (v: string | bigint) => {
  const b = BigInt(v);
  const whole = b / 1_000_000_000n, frac = (b % 1_000_000_000n).toString().padStart(9, "0").replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : `${whole}`;
};
const hours = (blocks: number) => {
  const h = (blocks * BLOCK_S) / 3600;
  return h < 1 ? `about ${Math.max(1, Math.round(h * 60))} minutes` : `about ${h < 10 ? h.toFixed(1) : Math.round(h)} hours`;
};

export function registerServices(api: Api, d: Deps, depIn?: MarketDeploy | null) {
  const dep = depIn === undefined ? loadMarketDeploy() : depIn;
  const town = new TownState(d);
  const blobDir = join(d.cfg.stateDir, "market-blobs");
  let ix: MarketIndex | null = dep ? new MarketIndex(dep.market, (m, p) => d.chain(m, p)) : null;
  let refreshing: Promise<MarketIndex> | null = null;

  function need(): MarketDeploy {
    if (!dep || !ix) throw new ApiError(503, "unavailable", "no services market is configured on this cadre-api (deploy/market-devnet.json or MARKET_CONFIG)");
    return dep;
  }
  async function index(maxAgeMs = 10_000): Promise<MarketIndex> {
    need();
    if (Date.now() - ix!.at < maxAgeMs) return ix!;
    refreshing ??= ix!.refresh().catch((e) => {
      if (isUnknownMethod(e)) throw new ApiError(503, "unavailable", "this node does not serve the contract RPC");
      throw e;
    }).finally(() => { refreshing = null; });
    return refreshing;
  }

  // ---------------------------------------------------------------- labels and records

  function ctx(): Context {
    const t = town.idx();
    const tables = agentTables(d.ix);
    return {
      isHouse: (a) => !!dep?.house[a] || t.card(a).house,
      operatorOf: (a) => t.card(a).operator,
      transfersTo: (to) => tables.transfers.get(to) ?? [],
    };
  }
  function card(a: string) {
    const c = town.idx().card(a);
    const label = dep?.house[a] ?? null;
    return { account: a, handle: c.handle ?? label, name: c.name, house: c.house || !!label, is_agent: c.is_agent, operator: c.operator, avatar: c.avatar };
  }
  function offerView(o: Offer, hires: Hire[], x: Context) {
    const rec = providerRecord(hires, o.provider, x);
    const mine = hires.filter((h) => h.offer === o.id);
    return {
      ...o,
      price_cadre: cad(o.price),
      fee_bps: ix!.config?.fee_bps ?? null,
      fee: ix!.config ? feeOf(BigInt(o.price), ix!.config.fee_bps).toString() : null,
      turnaround_hint: hours(o.turnaround),
      provider_card: card(o.provider),
      evaluator_cards: o.evaluators.map(card),
      record: { paid_volume_organic: rec.paid_volume_organic, distinct_payers: rec.distinct_payers, completed: rec.completed, rejected: rec.rejected, delivered_then_expired: rec.delivered_then_expired, house_volume: rec.house_volume },
      offer_stats: { hires: mine.length, completed: mine.filter((h) => h.status === "completed").length, open: mine.filter((h) => h.status === "open" || h.status === "delivered").length },
    };
  }
  function hireView(h: Hire, x: Context) {
    const ex = exclusion(h, x);
    const o = ix!.offer(h.offer);
    return {
      ...h,
      offer_title: o?.title ?? null,
      category: o?.category ?? null,
      buyer_card: card(h.buyer),
      provider_card: card(h.provider),
      evaluator_cards: h.evaluators.map(card),
      house: ex?.kind === "house",
      counts_as_organic: h.status === "completed" && !ex,
      excluded: ex,
      blocks_left: Math.max(0, h.deadline - ix!.height),
      deadline_hint: hours(Math.max(0, h.deadline - ix!.height)),
    };
  }

  // ---------------------------------------------------------------- blobs (kept after pruning)

  async function blob(uri: string | null): Promise<{ uri: string; size: number; json: any | null; text: string | null; archived: boolean } | null> {
    if (!uri || !/^blob:[0-9a-f]{64}$/i.test(uri)) return null;
    const id = uri.slice(5).toLowerCase();
    const f = join(blobDir, id);
    let data: Buffer | null = existsSync(f) ? readFileSync(f) : null;
    let archived = !!data;
    if (!data) {
      const r = await d.chain<any>("getBlob", { blob_id: uri }).catch(() => null);
      if (r?.data) {
        const b = Buffer.from(r.data, "base64");
        if (hex(blake3(b)) === id) {
          data = b;
          mkdirSync(blobDir, { recursive: true });
          writeFileSync(f, b);
          archived = false;
        }
      }
    }
    if (!data) return null;
    const text = data.length <= 64 * 1024 ? data.toString("utf8") : null;
    let json = null;
    try { json = text ? JSON.parse(text) : null; } catch { /* not JSON */ }
    return { uri, size: data.length, json, text, archived };
  }

  /** A thought_ref checked against the gateway receipt and the text it should have written. */
  async function thought(ref: string | null, text: string | null) {
    if (!ref) return null;
    const r = await town.receipt(ref);
    const sha = text !== null ? createHash("sha256").update(text).digest("hex") : null;
    return {
      thought_ref: ref,
      receipt_found: !!r,
      verified: !!(r && sha && r.text_sha256 === sha),
      rows: r?.rows ?? null,
      charged_lamports: r?.charged_lamports ?? null,
      wallet: r?.wallet ?? null,
      status_url: r?.status_url ?? null,
      rule: "verified = the gateway receipt's text_sha256 equals sha256 of the text shown",
    };
  }

  // ---------------------------------------------------------------- reads

  api.get("/v1/market", async () => {
    const m = await index();
    const x = ctx();
    const hs = [...m.hires.values()];
    let organic = 0n, house = 0n, selfDealt = 0n;
    const payers = new Set<string>();
    for (const h of hs.filter((h) => h.status === "completed")) {
      const ex = exclusion(h, x);
      if (!ex) { organic += BigInt(h.price); payers.add(h.buyer); } else if (ex.kind === "house") house += BigInt(h.price); else selfDealt += BigInt(h.price);
    }
    const byStatus = Object.fromEntries(STATUS.map((s) => [s, hs.filter((h) => h.status === s).length]));
    return {
      contract: dep!.market, code_hash: dep!.code_hash ?? null, deployed_height: dep!.deployed_height ?? null, height: m.height,
      config: m.config, fee_rule: "fee = ceil(price x fee_bps / 10000), paid by the buyer on top of the price, kept only when the hire completes (decision D6: floor 2,000 bps, admin-editable)",
      judge: dep!.judge ? card(dep!.judge) : null,
      offers: m.offers.length, active_offers: m.offers.filter((o) => o.active).length, hires: hs.length, by_status: byStatus,
      volume: { organic: organic.toString(), organic_distinct_payers: payers.size, house: house.toString(), self_dealt_excluded: selfDealt.toString() },
      categories: [...new Set(m.offers.filter((o) => o.active).map((o) => o.category).filter(Boolean))].sort(),
      house_accounts: Object.entries(dep!.house).map(([a, h]) => ({ account: a, handle: h })),
      note: "House accounts are the market desk and its LLM judge; their hires are house activity and never count as organic volume.",
    };
  }, { cache: 10 });

  api.get("/v1/market/offers", async (c) => {
    const m = await index();
    const x = ctx();
    const hs = [...m.hires.values()];
    const q = c.query;
    const cat = q.get("category"), prov = q.get("provider"), text = (q.get("q") ?? "").toLowerCase();
    const maxPrice = q.get("max_price") ? BigInt(q.get("max_price")!) : null;
    const all = q.get("active") === "all";
    const house = q.get("house"); // "only" | "exclude" | null
    let list = m.offers.filter((o) => (all || o.active) && (!cat || o.category === cat) && (!prov || o.provider === prov) && (!maxPrice || BigInt(o.price) <= maxPrice) && (!text || o.title.toLowerCase().includes(text) || o.category.toLowerCase().includes(text)));
    let views = list.map((o) => offerView(o, hs, x));
    if (house === "only") views = views.filter((v) => v.provider_card.house);
    if (house === "exclude") views = views.filter((v) => !v.provider_card.house);
    const sort = q.get("sort") ?? "volume";
    const by: Record<string, (a: any, b: any) => number> = {
      volume: (a, b) => Number(BigInt(b.record.paid_volume_organic) - BigInt(a.record.paid_volume_organic)) || b.offer_stats.completed - a.offer_stats.completed || a.id - b.id,
      price: (a, b) => Number(BigInt(a.price) - BigInt(b.price)) || a.id - b.id,
      new: (a, b) => b.id - a.id,
    };
    views.sort(by[sort] ?? by.volume);
    return { contract: dep!.market, height: m.height, fee_bps: m.config?.fee_bps ?? null, count: views.length, offers: views, sort_rule: "volume = organic paid volume from distinct payers (house and self-dealt excluded), then completed hires" };
  }, { cache: 10 });

  api.get("/v1/market/offers/:id", async (c) => {
    const m = await index();
    const id = Number(c.params.id);
    const o = Number.isInteger(id) ? m.offer(id) : null;
    if (!o) throw notFound("no such offer");
    const x = ctx();
    const hs = [...m.hires.values()];
    const spec = await blob(o.spec_uri);
    const specOk = spec ? hex(blake3(Buffer.from(spec.text ?? ""))) === o.spec_hash || null : null;
    return {
      offer: offerView(o, hs, x),
      spec: spec?.json ?? (spec?.text ? { text: spec.text } : null), spec_available: !!spec, spec_hash_ok: specOk,
      provider_record: providerRecord(hs, o.provider, x),
      recent_hires: hs.filter((h) => h.offer === o.id).sort((a, b) => b.id - a.id).slice(0, 25).map((h) => hireView(h, x)),
      quote: await m.quote(o.id, 0n),
    };
  }, { cache: 10 });

  api.get("/v1/market/quote", async (c) => {
    const m = await index();
    const id = Number(c.query.get("offer"));
    if (!Number.isInteger(id) || !m.offer(id)) throw badRequest("offer must be an existing offer id");
    const jf = BigInt(c.query.get("judge_fee") ?? "0");
    return { offer: id, ...(await m.quote(id, jf)), fee_bps: m.config?.fee_bps ?? null };
  }, { cache: 5 });

  api.get("/v1/market/hires", async (c) => {
    const m = await index();
    const x = ctx();
    const q = c.query;
    const who = (k: string) => { const v = q.get(k); if (v && !isKey(v)) throw badRequest(`${k} must be an address`); return v; };
    const buyer = who("buyer"), provider = who("provider"), evaluator = who("evaluator"), account = who("account");
    const status = q.get("status");
    if (status && !STATUS.includes(status as any)) throw badRequest(`status is one of ${STATUS.join(", ")}`);
    const limit = Math.min(200, Math.max(1, Number(q.get("limit") ?? 50) || 50));
    const list = [...m.hires.values()]
      .filter((h) => (!buyer || h.buyer === buyer) && (!provider || h.provider === provider) && (!evaluator || h.evaluators.includes(evaluator)) && (!account || [h.buyer, h.provider, ...h.evaluators].includes(account)) && (!status || h.status === status))
      .sort((a, b) => b.id - a.id);
    return { contract: dep!.market, height: m.height, count: list.length, hires: list.slice(0, limit).map((h) => hireView(h, x)) };
  }, { cache: 5 });

  api.get("/v1/market/hires/:id", async (c) => {
    const m = await index(5_000);
    const id = Number(c.params.id);
    const h = Number.isInteger(id) ? m.hires.get(id) : undefined;
    if (!h) throw notFound("no such hire");
    const x = ctx();
    const [req, del] = await Promise.all([blob(h.request_uri), blob(h.result_uri)]);
    const ev = m.hireEvents(id);
    const delivered = ev.find((e) => e.name === "Delivered");
    const delText = typeof del?.json?.text === "string" ? del.json.text : del?.text ?? null;
    const verdicts = await Promise.all(ev.filter((e) => e.name === "Verdict").map(async (e) => {
      const vb = await blob((e.data.reason_uri as string) ?? null);
      const ans = typeof vb?.json?.answer === "string" ? vb.json.answer : null;
      return {
        evaluator: e.who, evaluator_card: e.who ? card(e.who) : null, accept: e.data.accept, height: e.height, tx_hash: e.tx_hash,
        reason: vb?.json?.reason ?? null, reasoning_uri: e.data.reason_uri ?? null, reasoning: vb?.json ?? null,
        reason_hash_ok: vb?.text ? hex(blake3(Buffer.from(vb.text))) === e.data.reason_hash : null,
        thought: await thought((e.data.thought_ref as string) ?? null, ans),
        judge_kind: vb?.json?.format === "cadre-verdict-v1" ? "llm_judge" : "manual",
        limits: vb?.json?.limits ?? null,
      };
    }));
    return {
      hire: hireView(h, x),
      request: req ? { uri: req.uri, text: req.json?.request ?? req.text, archived: req.archived, hash_ok: req.text ? hex(blake3(Buffer.from(req.text))) === h.request_hash : null } : null,
      delivery: del ? { uri: del.uri, text: delText, archived: del.archived, hash_ok: del.text ? hex(blake3(Buffer.from(del.text))) === h.result_hash : null, thought: await thought((delivered?.data.thought_ref as string) ?? null, delText) } : null,
      verdicts,
      timeline: ev.map((e) => ({ event: e.name, height: e.height, tx_hash: e.tx_hash, by: e.who, data: e.data })),
    };
  }, { cache: 5 });

  api.get("/v1/market/agents/:account", async (c) => {
    const a = addr(c.params.account, "account");
    const m = await index();
    const x = ctx();
    const hs = [...m.hires.values()];
    return {
      account: a, card: card(a), house: x.isHouse(a),
      offers: m.offers.filter((o) => o.provider === a).map((o) => offerView(o, hs, x)),
      record: providerRecord(hs, a, x),
      as_buyer: hs.filter((h) => h.buyer === a).sort((p, q) => q.id - p.id).slice(0, 50).map((h) => hireView(h, x)),
      as_provider: hs.filter((h) => h.provider === a).sort((p, q) => q.id - p.id).slice(0, 50).map((h) => hireView(h, x)),
      as_evaluator: hs.filter((h) => h.evaluators.includes(a)).sort((p, q) => q.id - p.id).slice(0, 50).map((h) => hireView(h, x)),
    };
  }, { cache: 10 });

  // ---------------------------------------------------------------- unsigned txs

  const big = (v: unknown, name: string, min = 0n): bigint => {
    if (v === undefined || v === null || v === "") throw badRequest(`${name} is required`);
    let b: bigint;
    try { b = BigInt(v as any); } catch { throw badRequest(`${name} must be an integer (base units)`); }
    if (b < min || b > 18446744073709551615n) throw badRequest(`${name} out of range`);
    return b;
  };
  const evalList = (v: unknown): string[] => {
    if (!Array.isArray(v)) throw badRequest("evaluators must be an array of addresses");
    if (v.length > MAX.evaluators) throw badRequest("at most 4 evaluators");
    return v.map((e, i) => addr(e, `evaluators[${i}]`));
  };
  const h32 = (v: unknown, name: string, dflt?: string) => {
    if ((v === undefined || v === null || v === "") && dflt !== undefined) return dflt;
    if (typeof v !== "string" || !HEX64.test(v)) throw badRequest(`${name} must be 64 hex chars`);
    return v.toLowerCase();
  };

  api.post("/v1/tx/market", async (c) => {
    const b = await c.json();
    const from = addr(b.from, "from");
    const m = await index(3_000);
    const action = String(b.action ?? "");
    let args: Uint8Array;
    let value = 0n;
    let review: string[];
    const name = (a: string) => { const k = card(a); return k.handle ? `@${k.handle}` : a; };
    try {
      switch (action) {
        case "offer":
        case "update_offer": {
          const o = { title: String(b.title ?? ""), category: String(b.category ?? ""), spec_uri: String(b.spec_uri ?? ""), spec_hash: h32(b.spec_hash, "spec_hash", "00".repeat(32)), price: big(b.price, "price", 1n), turnaround: Number(b.turnaround ?? 0), policy: Number(b.policy ?? 0), evaluators: evalList(b.evaluators ?? []), active: b.active !== false };
          if (!o.title) throw badRequest("title is required");
          if (!Number.isInteger(o.turnaround) || o.turnaround < 1 || o.turnaround > MAX.turnaround) throw badRequest(`turnaround must be 1..${MAX.turnaround} blocks`);
          if (o.policy !== 0 && o.policy !== 1) throw badRequest("policy is 0 (buyer names evaluators) or 1 (from the offer's list)");
          let id = -1;
          if (action === "update_offer") {
            id = Number(b.offer);
            const cur = m.offer(id);
            if (!cur) throw notFound("no such offer");
            if (cur.provider !== from) throw badRequest("only the offer's provider can update it");
          }
          args = action === "offer" ? marketArgs.offer(o) : marketArgs.updateOffer(id, o);
          const fee = feeOf(o.price, m.config?.fee_bps ?? 0);
          review = [
            `${action === "offer" ? "List" : "Update"} the service "${o.title}" at ${cad(o.price)} CADRE, delivered within ${o.turnaround} blocks (${hours(o.turnaround)}).`,
            `Buyers pay the price plus a ${cad(fee)} CADRE market fee (${(m.config?.fee_bps ?? 0) / 100}%) into escrow; you are paid the price when their evaluators accept your delivery.`,
            o.policy === 1 ? `Buyers may only name these evaluators: ${o.evaluators.map(name).join(", ")}.` : "Buyers name their own evaluators (themselves, a person, an agent or an LLM judge).",
            o.active ? "The offer is open for hires." : "The offer is paused (no new hires).",
            "This tx moves no CADRE except the tx fee and a refundable storage deposit.",
          ];
          break;
        }
        case "hire": {
          const id = Number(b.offer);
          const o = m.offer(id);
          if (!o) throw notFound("no such offer");
          if (!o.active) throw new ApiError(409, "offer_inactive", "the offer is not active");
          if (o.provider === from) throw new ApiError(409, "self_hire", "a provider cannot hire its own offer");
          const evaluators = evalList(b.evaluators);
          if (!evaluators.length) throw badRequest("name at least one evaluator");
          if (evaluators.includes(o.provider)) throw badRequest("the provider cannot judge its own work");
          if (o.policy === "offer_list" && evaluators.some((e) => !o.evaluators.includes(e))) throw badRequest("this offer accepts only evaluators from its list");
          const threshold = Number(b.threshold ?? 1);
          if (!Number.isInteger(threshold) || threshold < 1 || threshold > evaluators.length) throw badRequest("threshold must be 1..evaluators");
          const deadline = Number(b.deadline_blocks ?? o.turnaround * 2);
          if (!Number.isInteger(deadline) || deadline < o.turnaround || deadline > MAX.deadline) throw badRequest(`deadline_blocks must be ${o.turnaround}..${MAX.deadline}`);
          const judgeFee = big(b.judge_fee ?? "0", "judge_fee");
          const q = await m.quote(id, judgeFee);
          if (b.price !== undefined && String(b.price) !== q.price) throw new ApiError(409, "price_changed", `the offer's price is now ${q.price}`);
          const reqUri = String(b.request_uri ?? "");
          if (reqUri.length > MAX.uri) throw badRequest("request_uri over 256 bytes");
          args = marketArgs.hire({ offer: id, price: q.price, request_uri: reqUri, request_hash: h32(b.request_hash, "request_hash"), deadline_blocks: deadline, judge_fee: judgeFee, evaluators, threshold });
          value = BigInt(q.total);
          review = [
            `You pay ${cad(q.total)} CADRE now, held by the market contract ${dep!.market} (not by ${name(o.provider)}).`,
            `Of it: ${cad(q.price)} CADRE goes to ${name(o.provider)} only if your evaluators accept the delivery; ${cad(q.fee)} CADRE is the market fee (${(m.config?.fee_bps ?? 0) / 100}%), kept only on acceptance; ${cad(q.judge_fee)} CADRE pays the evaluators who decide.`,
            `Evaluators: ${evaluators.map(name).join(", ")}; ${threshold} of ${evaluators.length} must accept.`,
            `If they reject, ${cad(BigInt(q.price) + BigInt(q.fee))} CADRE comes back to you (the evaluators keep their ${cad(q.judge_fee)}).`,
            `If nothing is delivered and decided within ${deadline} blocks (${hours(deadline)}), anyone can send a refund of all ${cad(q.total)} CADRE to you.`,
            `${name(o.provider)} may decline before delivering; then all of it comes back at once.`,
          ];
          break;
        }
        case "deliver": {
          const h = m.hires.get(Number(b.hire));
          if (!h) throw notFound("no such hire");
          if (h.provider !== from) throw badRequest("only the hire's provider delivers");
          if (h.status !== "open") throw new ApiError(409, "not_open", `the hire is ${h.status}`);
          const uri = String(b.result_uri ?? "");
          args = marketArgs.deliver(h.id, h32(b.result_hash, "result_hash"), uri, String(b.thought_ref ?? ""));
          review = [`Deliver your work for hire #${h.id} (${uri || "no URI"}).`, `Its evaluators then decide; on acceptance you receive ${cad(h.price)} CADRE.`, "This tx moves no CADRE except the tx fee."];
          break;
        }
        case "verdict": {
          const h = m.hires.get(Number(b.hire));
          if (!h) throw notFound("no such hire");
          if (!h.evaluators.includes(from)) throw badRequest("you are not an evaluator of this hire");
          if (h.status !== "delivered") throw new ApiError(409, "not_delivered", `the hire is ${h.status}`);
          const accept = b.accept === true;
          args = marketArgs.verdict(h.id, accept, String(b.reason_uri ?? ""), h32(b.reason_hash, "reason_hash", "00".repeat(32)), String(b.thought_ref ?? ""));
          review = [
            `${accept ? "Accept" : "Reject"} the delivery for hire #${h.id}.`,
            accept ? `If this completes the vote (${h.threshold} of ${h.evaluators.length}), ${cad(h.price)} CADRE goes to ${name(h.provider)} and the fee to the market.` : `If this decides the vote, ${cad(BigInt(h.price) + BigInt(h.fee))} CADRE goes back to the buyer.`,
            BigInt(h.judge_fee) > 0n ? `Evaluators on the deciding side share ${cad(h.judge_fee)} CADRE.` : "No evaluator fee on this hire.",
          ];
          break;
        }
        case "decline": {
          const h = m.hires.get(Number(b.hire));
          if (!h) throw notFound("no such hire");
          if (h.provider !== from) throw badRequest("only the hire's provider can decline");
          if (h.status !== "open") throw new ApiError(409, "not_open", `the hire is ${h.status}`);
          args = marketArgs.decline(h.id, String(b.reason ?? ""));
          review = [`Decline hire #${h.id}: all ${cad(h.total)} CADRE goes back to the buyer now.`];
          break;
        }
        case "expire": {
          const h = m.hires.get(Number(b.hire));
          if (!h) throw notFound("no such hire");
          if (h.status !== "open" && h.status !== "delivered") throw new ApiError(409, "closed", `the hire is ${h.status}`);
          if (m.height + 1 <= h.deadline) throw new ApiError(409, "deadline_not_passed", `the deadline is block ${h.deadline}; now ${m.height}`);
          args = marketArgs.expire(h.id);
          review = [`Refund hire #${h.id}: its deadline passed, so all ${cad(h.total)} CADRE goes back to the buyer.`];
          break;
        }
        default:
          throw badRequest("action is one of offer, update_offer, hire, deliver, verdict, decline, expire");
      }
    } catch (e) {
      if (e instanceof ApiError) throw e;
      throw badRequest((e as Error).message);
    }
    const [acct, params, info] = await Promise.all([d.chain<any>("getAccount", { account: from }), d.chain<any>("getParams", {}), d.chain<any>("getChainInfo", {})]);
    const sim = await d.chain<any>("simulateCall", { from, address: dep!.market, args: hex(args), value: Number(value) });
    if (!sim?.ok) throw new ApiError(409, "would_fail", `the market contract would refuse this: ${sim?.error ? Buffer.from(sim.error, "hex").toString("utf8") : "simulation failed"}`);
    const gasLimit = BigInt(Math.ceil(sim.gas_used * 1.25)) + 20_000n;
    const body = encodeCallBody({ contract: dep!.market, value, gasLimit, args });
    const txLen = TX_HDR + body.length + TX_SIG;
    const floor = contractFeeFloor(params, txLen, gasLimit);
    if (BigInt(acct.balance) < floor.fee + value) throw new ApiError(422, "insufficient_funds", `balance ${cad(acct.balance)} CADRE < ${cad(floor.fee + value)} CADRE needed (value + tx fee) on the Cadre chain`);
    const signing = contractSigningBytes(info.chain_id, from, acct.nonce, floor.fee, "Call", body);
    return {
      action, review, review_text: review.join(" "),
      window_pearl: { method: "signAndSendTransaction", params: { transaction: { kind: "Call", contract: dep!.market, value: value.toString(), gasLimit: gasLimit.toString(), args: hex(args), fee: floor.fee.toString() } } },
      chain_id: info.chain_id, nonce: acct.nonce, fee: floor.fee.toString(), value: value.toString(), gas_limit: gasLimit.toString(), simulated_gas: sim.gas_used,
      unsigned_tx: hex(signing), framed_message: hex(framedMessage(signing)), contract: dep!.market,
      sign_with: "window.pearl.signAndSendTransaction({ transaction: window_pearl.params.transaction }) -> { txHash }; then GET /v1/contracts/receipts/{txHash}",
    };
  });

  return { index, dep };
}
