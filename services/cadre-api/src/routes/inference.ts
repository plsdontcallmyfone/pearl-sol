// Inference: registered models with their storefront price, ticket quotes, receipts read
// from the chain, the x402 payment tx a wallet signs, and a pass-through to the inference
// gateway's OpenAI-style endpoints (services/inference-gateway, SPEC 5.4).
//
// Prepaid balances and top-ups belong to the gateway (SPEC 5.4 prepaid API, being built
// by another agent). Until it lands they answer 501 `pending_upstream` (CADRE_PREPAID=off,
// the default); CADRE_PREPAID=gateway proxies them to the gateway. Never fake data.

import * as M from "../../../../apps/explorer/src/models.ts";
import { Api, ApiError, badRequest, notFound, reply } from "../http.ts";
import type { Deps } from "../deps.ts";
import { compileLegacy, computeUnitLimit, isAddress, systemTransfer, unsignedTx } from "../sol.ts";
import { intParam } from "./stats.ts";
import { addr, amount } from "./wallet.ts";

const HEX64 = /^[0-9a-f]{64}$/;

/** Storefront quote for a ticket of this size; null + reason when the storefront cannot price it. */
async function ticketQuote(d: Deps, modelId: string, promptTokens: number, maxTokens: number) {
  const r = await d.upstreamJson("storefront", `/quote?model_id=${modelId}&prompt_tokens=${promptTokens}&max_tokens=${maxTokens}`, { timeoutMs: 10_000 });
  if (r.status === 400 && /^m must be/.test(String(r.body?.error))) return { ok: false as const, status: 501, error: "this storefront-x402 build does not quote inference tickets (needs commit 64a4177 or later)" };
  if (r.status !== 200) return { ok: false as const, status: r.status, error: r.body?.error ?? `storefront HTTP ${r.status}` };
  if (!r.body?.ticket) return { ok: false as const, status: 501, error: "this storefront-x402 build does not quote inference tickets (needs commit 64a4177 or later)" };
  return { ok: true as const, quote: r.body };
}

function pendingPrepaid(what: string) {
  return new ApiError(501, "pending_upstream", `${what}: prepaid balances and top-ups come from the inference gateway's prepaid API (SPEC 5.4), which is not deployed here yet`, {
    flag: "CADRE_PREPAID=gateway enables the proxy once the gateway serves it",
    today: "pay per request with x402: POST /v1/openai/completions without X-PAYMENT returns the 402 requirements; POST /v1/inference/x402/payment-tx builds the tx to sign",
  });
}

export function registerInference(api: Api, d: Deps) {
  api.get("/v1/inference/models", async (c) => {
    const pt = intParam(c.query, "prompt_tokens", 64, 1, 4096);
    const mt = intParam(c.query, "max_tokens", 64, 1, 4096);
    const chain = await d.chain<any>("listModels", {});
    let served: any[] | null = null;
    let gatewayError: string | null = null;
    if (d.cfg.gatewayUrl) {
      const g = await d.upstreamJson("gateway", "/v1/models", { timeoutMs: 5000 }).catch((e) => ({ status: 0, body: { error: (e as Error).message } }));
      if (g.status === 200 && Array.isArray(g.body?.data)) served = g.body.data;
      else gatewayError = g.body?.error?.message ?? g.body?.error ?? `gateway HTTP ${g.status}`;
    }
    const models = await Promise.all(
      (chain.models ?? []).map(async (m: any) => {
        const q = await ticketQuote(d, m.model_id, pt, mt).catch((e) => ({ ok: false as const, status: 503, error: (e as Error).message }));
        const gw = served?.find((s) => s.id === m.model_id) ?? null;
        return {
          ...m,
          served_by_gateway: served ? !!gw : null,
          gateway: gw,
          price: q.ok
            ? { prompt_tokens: pt, max_tokens: mt, price_lamports: q.quote.price_lamports, sol_in_lamports: q.quote.sol_in_lamports, buyback_lamports: q.quote.buyback_lamports, work_flops: q.quote.work_flops, venue: q.quote.venue, pay_to: q.quote.payment?.pay_to ?? null, quote: `/v1/inference/quote?model_id=${m.model_id}&prompt_tokens=${pt}&max_tokens=${mt}` }
            : null,
          price_error: q.ok ? null : q.error,
        };
      }),
    );
    return {
      total: models.length,
      models,
      gateway: d.cfg.gatewayUrl ? { reachable: served !== null, error: gatewayError, openai_base_url: "/v1/openai" } : null,
      pricing_rule: "price = sol_in + overhead; sol_in covers base + lamports_per_gflop x 2 x macs_per_row x rows, rows = round_up(prompt + max tokens, row period), grossed up by the buyback share (storefront-x402 GET /quote)",
    };
  }, { cache: 15 });

  api.get("/v1/inference/quote", async (c) => {
    const model = (c.query.get("model_id") ?? "").toLowerCase();
    if (!HEX64.test(model)) throw badRequest("model_id must be 32 bytes hex");
    const q = await ticketQuote(d, model, intParam(c.query, "prompt_tokens", 0, 1, 4096), intParam(c.query, "max_tokens", 0, 1, 4096));
    if (!q.ok) throw new ApiError(q.status === 400 ? 400 : 502, q.status === 400 ? "bad_request" : "storefront_error", q.error);
    return q.quote;
  }, { cache: 5 });

  // receipts: inference tickets from the chain (via the shared index), by poster
  api.get("/v1/inference/receipts", (c) => {
    const account = addr(c.query.get("account"), "account");
    const ord = M.orderByJob(d.ix);
    const tickets = [...d.ix.jobs.values()].filter((j) => j.kind === "ticket" && j.poster === account).sort((a, b) => b.posted_height - a.posted_height);
    const limit = intParam(c.query, "limit", 50, 1, 200);
    return {
      account,
      total: tickets.length,
      receipts: tickets.slice(0, limit).map((j) => {
        const o = ord.get(j.job_id);
        return {
          ticket_id: j.job_id,
          status: j.status,
          model_id: j.standing?.model_id ?? null,
          server: j.standing?.server ?? null,
          prompt_tokens: j.standing?.prompt_tokens ?? j.m,
          max_new_tokens: j.standing?.max_new_tokens ?? j.n,
          worker_pay: j.worker_pay,
          protocol_fee: j.protocol_fee,
          blocks_mined: j.standing?.blocks_mined ?? null,
          subsidy_paid: j.standing?.subsidy_paid ?? null,
          posted_height: j.posted_height,
          closed_height: j.closed_height || null,
          order: o ? { nonce: o.nonce, status: o.status, sol_spent: o.solSpent, payer: o.recipient, buyer: o.buyer } : null,
          links: { receipt: `/v1/inference/receipts/${j.job_id}`, job: `/v1/jobs/${j.job_id}`, ...(d.cfg.explorerUrl ? { explorer: `${d.cfg.explorerUrl.replace(/\/$/, "")}/job/${j.job_id}` } : {}) },
        };
      }),
    };
  }, { cache: 3 });

  api.get("/v1/inference/receipts/:id", async (c) => {
    const id = c.params.id.toLowerCase();
    if (!HEX64.test(id)) throw badRequest("receipt id is the ticket id (64 hex)");
    const [ticket, job] = await Promise.all([d.chain<any>("getTicket", { ticket_id: id }).catch(() => null), M.jobModel(d.ix, id)]);
    let gateway: unknown = null;
    if (d.cfg.gatewayUrl) {
      const g = await d.upstreamJson("gateway", `/v1/requests/${id}`, { timeoutMs: 5000 }).catch(() => null);
      if (g && g.status === 200) gateway = g.body?.gateway ?? null;
    }
    if (!ticket && !job) throw notFound("no such ticket");
    return {
      ticket_id: id,
      ticket,
      job,
      gateway_record: gateway,
      links: { job: `/v1/jobs/${id}`, ...(job?.order ? { order: `/v1/orders/${job.order.nonce}` } : {}), ...(d.cfg.explorerUrl ? { explorer: `${d.cfg.explorerUrl.replace(/\/$/, "")}/job/${id}` } : {}) },
    };
  }, { cache: 3 });

  api.get("/v1/inference/requests/:id", async (c) => {
    const id = c.params.id.toLowerCase();
    if (!HEX64.test(id)) throw badRequest("request id is the ticket id (64 hex)");
    const g = await d.upstreamJson("gateway", `/v1/requests/${id}`, { timeoutMs: 10_000 });
    return reply({ status: g.status, body: g.status < 400 ? g.body : { error: { code: "gateway_error", message: g.body?.error?.message ?? g.body?.error ?? `gateway HTTP ${g.status}` } } });
  }, { cache: 2 });

  // x402: the payment tx the payer's wallet signs (the service is the fee payer and co-signs at settlement)
  api.post("/v1/inference/x402/payment-tx", async (c) => {
    const b = await c.json();
    const payer = addr(b.payer, "payer");
    const req = Array.isArray(b.requirements?.accepts) ? b.requirements.accepts[0] : b.requirements;
    if (!req || typeof req !== "object") throw badRequest("requirements: one PaymentRequirements object from the 402 reply (or the whole 402 body)");
    if (req.scheme !== "exact") throw badRequest("only the exact scheme is supported");
    if (req.extra?.assetKind !== "native") throw badRequest("only native SOL payments are built here (extra.assetKind = native)");
    const payTo = addr(req.payTo, "requirements.payTo");
    const feePayer = addr(req.extra?.feePayer, "requirements.extra.feePayer");
    const lamports = amount(req.maxAmountRequired, "requirements.maxAmountRequired");
    // only pay the storefront this deployment talks to
    const h = await d.upstreamJson("storefront", "/health", { timeoutMs: 5000 });
    const wallet = h.body?.wallet;
    if (!isAddress(wallet) || wallet !== payTo || wallet !== feePayer) throw new ApiError(422, "unknown_pay_to", "requirements.payTo / feePayer is not this deployment's storefront wallet", { storefront_wallet: wallet ?? null });
    if (h.body?.network && req.network !== h.body.network) throw badRequest(`requirements.network ${req.network} is not the storefront's ${h.body.network}`);
    if (payer === feePayer) throw badRequest("payer must not be the storefront wallet");
    const bh = await d.solana<any>("getLatestBlockhash", [{ commitment: "confirmed" }]);
    const msg = compileLegacy([computeUnitLimit(10_000), systemTransfer(payer, payTo, lamports)], feePayer, bh.value.blockhash);
    return {
      transaction: Buffer.from(unsignedTx(msg)).toString("base64"),
      encoding: "base64",
      version: "legacy",
      fee_payer: feePayer,
      signers: msg.accountKeys.slice(0, msg.numRequiredSignatures),
      payer_signs: payer,
      lamports: lamports.toString(),
      network: req.network,
      recent_blockhash: bh.value.blockhash,
      last_valid_block_height: bh.value.lastValidBlockHeight,
      x_payment: {
        how: "sign with the wallet's solana:signTransaction (partial: the storefront adds the fee-payer signature at settlement), then X-PAYMENT = base64(JSON {x402Version: 1, scheme: 'exact', network, payload: {transaction: <signed tx base64>}}); the client's encodeXPayment() does this",
        x402Version: 1,
        scheme: "exact",
      },
    };
  });

  // prepaid (SPEC 5.4, gateway): proxied when CADRE_PREPAID=gateway, else 501 pending_upstream
  const prepaid = (what: string, path: (c: any) => string, method: "GET" | "POST") => async (c: any) => {
    if (d.cfg.prepaid !== "gateway") throw pendingPrepaid(what);
    const body = method === "POST" ? await c.raw() : undefined;
    const r = await d.upstream("gateway", path(c), { method, body, headers: { "content-type": "application/json" }, timeoutMs: 20_000 });
    return reply({ upstream: r });
  };
  api.get("/v1/inference/prepaid/:account", prepaid("prepaid balance", (c) => `/v1/prepaid/${encodeURIComponent(addr(c.params.account, "account"))}`, "GET"), { cache: "no-store" });
  api.post("/v1/inference/prepaid/topup", prepaid("prepaid top-up", () => "/v1/prepaid/topup", "POST"));
  api.post("/v1/inference/prepaid/topup/quote", prepaid("prepaid top-up quote", () => "/v1/prepaid/topup/quote", "POST"));

  // OpenAI-style pass-through: base URL /v1/openai -> gateway /v1/*
  const pass = async (c: any) => {
    const rest = c.params.rest ?? "";
    if (!/^[a-z0-9/_.-]*$/i.test(rest) || rest.includes("..")) throw badRequest("bad path");
    const allowed = ["models", "completions", "chat/completions", "embeddings"];
    if (!allowed.some((a) => rest === a || rest.startsWith(a + "/")) && !rest.startsWith("requests/")) throw new ApiError(404, "not_found", `the gateway pass-through serves ${allowed.join(", ")} and requests/{id}`);
    const headers: Record<string, string> = {};
    for (const k of ["content-type", "authorization", "x-payment", "x-cadre-account", "x-cadre-signature", "x-cadre-timestamp", "x-cadre-nonce", "accept"]) {
      const v = c.headers[k];
      if (typeof v === "string") headers[k] = v;
    }
    headers["x-forwarded-for"] = c.ip;
    const body = c.method === "POST" ? await c.raw() : undefined;
    const r = await d.upstream("gateway", `/v1/${rest}${c.query.toString() ? "?" + c.query.toString() : ""}`, { method: c.method === "HEAD" ? "GET" : c.method, headers, body, timeoutMs: 300_000 });
    return reply({ upstream: r });
  };
  api.get("/v1/openai/:rest*", pass, { cache: "no-store" });
  api.post("/v1/openai/:rest*", pass, { cost: 10 });
}
