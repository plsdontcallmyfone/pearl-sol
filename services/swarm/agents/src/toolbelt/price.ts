// What a tool call costs and what the agent pays for it (docs/TOOLBELT.md 4).
//
// Price rule (owner 2026-10-22, matching inference decision D2 in docs/ECOSYSTEM.md 5.1): the agent
// pays 2 x the measured or list cost of the outside provider, converted to lamports at a fixed SOL
// price the runtime operator sets (the stack has no price source). Every input below names where it
// comes from; a call whose outside cost is zero (a plain page fetch, a blob put, an allowlisted HTTP
// call) is priced at zero and is still capped by its call counts and rates.
//
// Costs are booked from what the provider reports when it reports it (OpenRouter's usage.cost for the
// vision model), from measured seconds x list rate for sandboxes (E2B bills per second), and from the
// provider's list price per unit otherwise (search credits, emails).
import { E2B_RATES, usdPerSecond } from "../../../desktop/src/config.ts";

export const MARKUP = 2; // D2: 2x the measured cost

export interface PriceConfig {
  /** SOL price in USD used to convert. Default: the D2 conversion (Coinbase spot 2026-10-20 20:00 UTC). */
  solUsd: number;
  /** Firecrawl list price per credit: Hobby plan 19 USD for 5,000 credits (firecrawl.dev/pricing, read 2026-10-22). */
  firecrawlUsdPerCredit: number;
  /** Brave Search API list price: 5 USD per 1,000 requests (brave.com/search/api, read 2026-10-22). */
  braveUsdPerRequest: number;
  /** AgentMail Developer plan: 20 USD per month for 10,000 emails (agentmail.to/pricing, read 2026-10-22). */
  emailUsdPerMessage: number;
}

export const PRICE_SOURCES = {
  solUsd: "105.985 USD/SOL: the conversion used for inference price D2 (Coinbase spot 2026-10-20 20:00 UTC, docs/ECOSYSTEM.md 5.1); set TOOLBELT_SOL_USD when SOL moves",
  firecrawl: "Firecrawl Hobby: 19 USD / 5,000 credits; search 2 credits per 10 results, scrape 1 credit per page (firecrawl.dev/pricing, read 2026-10-22)",
  brave: "Brave Search API: 5 USD per 1,000 requests (brave.com/search/api, read 2026-10-22)",
  e2b: `E2B: ${E2B_RATES.usd_per_vcpu_s} USD per vCPU-second + ${E2B_RATES.usd_per_gib_s} USD per GiB-second (${E2B_RATES.source}, read ${E2B_RATES.read_at})`,
  vision: "OpenRouter usage.cost of every vision-model response (provider-reported, per call)",
  email: "AgentMail Developer: 20 USD/month for 10,000 emails (agentmail.to/pricing, read 2026-10-22)",
  zero: "no outside provider: a direct page fetch, a Cadre blob put, an allowlisted HTTP call and GitHub API calls cost the runtime nothing outside, so they are priced 0 and limited by counts and rates",
} as const;

export function priceConfigFromEnv(env: Record<string, string | undefined> = process.env): PriceConfig {
  const n = (k: string, d: number) => { const v = Number(env[k]); return Number.isFinite(v) && v > 0 ? v : d; };
  return {
    solUsd: n("TOOLBELT_SOL_USD", 105.985),
    firecrawlUsdPerCredit: n("TOOLBELT_FIRECRAWL_USD_PER_CREDIT", 19 / 5000),
    braveUsdPerRequest: n("TOOLBELT_BRAVE_USD_PER_REQUEST", 5 / 1000),
    emailUsdPerMessage: n("TOOLBELT_EMAIL_USD_PER_MESSAGE", 20 / 10000),
  };
}

/** Lamports the agent pays for an outside cost of `usd`: ceil(2 x usd / solUsd x 1e9). */
export function lamportsFor(usd: number, p: PriceConfig): bigint {
  if (!(usd > 0)) return 0n;
  return BigInt(Math.ceil((MARKUP * usd * 1e9) / p.solUsd));
}

export const sandboxUsd = (seconds: number, vcpu: number, ramGib: number) => Math.max(0, seconds) * usdPerSecond(vcpu, ramGib);
export const firecrawlSearchUsd = (results: number, p: PriceConfig) => 2 * Math.max(1, Math.ceil(results / 10)) * p.firecrawlUsdPerCredit;
export const firecrawlScrapeUsd = (pages: number, p: PriceConfig) => pages * p.firecrawlUsdPerCredit;
