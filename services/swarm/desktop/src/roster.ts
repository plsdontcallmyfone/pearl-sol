// The house agents' jobs (owner 2026-10-23: "none of these agents are doing any actual work").
// Every house agent has one job that produces something a person can read: a sourced research
// brief delivered on the market, a source review with a verdict, a QA bug list, a monitor report
// with its series and findings, a practice trade with its post-mortem, or the digest of all of it.
// Titles and descriptions are the personas' own (services/swarm/src/personas.ts); this table says
// what each one does, how often and where its output goes. docs/HOUSE-AGENTS.md is the readable copy.
import { PERSONAS } from "../../src/personas.ts";

export type JobKind = "research" | "manager" | "reviewer" | "qa" | "monitor" | "trader" | "digest";

/** Monitor targets: each reads real devnet state (cadre-api, the node, Solana devnet, this controller). */
export type MonitorTarget = "chain" | "bridge" | "storefront" | "mining" | "buyback" | "api" | "market" | "wallets" | "desktops";
/** QA suites: the live site page by page, the API and explorer, the main flows with the vision model, phone width. */
export type QaSuite = "site" | "api_explorer" | "flows" | "mobile";
/** Hiring managers: current crypto news (RSS) or the operators' own research questions. */
export type ManagerDesk = "news" | "ops";

export interface Job {
  kind: JobKind;
  /** what the agent hands in (shown on its card and in docs/HOUSE-AGENTS.md) */
  outputs: string;
  target?: MonitorTarget;
  suite?: QaSuite;
  desk?: ManagerDesk;
}

export const ROSTER: Record<string, Job> = {
  // research desk: sells a sourced brief on the market; works only when hired (no filler between hires)
  columbinemoth56: { kind: "research", outputs: "sourced research briefs delivered on market hires" },
  lunaria38: { kind: "research", outputs: "sourced research briefs delivered on market hires" },
  nigella71: { kind: "research", outputs: "sourced research briefs delivered on market hires" },
  agrimony29: { kind: "research", outputs: "sourced research briefs delivered on market hires" },
  woodruff75: { kind: "research", outputs: "sourced research briefs delivered on market hires" },
  freesia64: { kind: "research", outputs: "sourced research briefs delivered on market hires (two at once)" },
  clematis53: { kind: "research", outputs: "sourced research briefs delivered on market hires (two at once)" },
  snapdragon88: { kind: "research", outputs: "sourced research briefs delivered on market hires (two at once)" },
  // hiring managers: real questions, paid hires (house money, labelled house)
  penstemon72: { kind: "manager", desk: "news", outputs: "market hires for briefs on current crypto news stories" },
  tansywren82: { kind: "manager", desk: "ops", outputs: "market hires for briefs on questions the Cadre operators need answered" },
  // reviewers: the named evaluator on every house research hire; verdicts from source checks, no model
  helleborefawn39: { kind: "reviewer", outputs: "verdicts on research deliveries with a per-source check table" },
  gentianbloom68: { kind: "reviewer", outputs: "verdicts on research deliveries with a per-source check table" },
  // QA
  delphinium37: { kind: "qa", suite: "site", outputs: "bug list for the live site (HTTP, console and request errors, unavailable data)" },
  hollyhock26: { kind: "qa", suite: "api_explorer", outputs: "bug list for the public API and the explorer (status, latency, shape)" },
  campion66: { kind: "qa", suite: "flows", outputs: "flow check reports for the site's main flows, nothing signed" },
  speedwell41: { kind: "qa", suite: "mobile", outputs: "phone-width layout bug list (sideways scroll, load errors)" },
  // monitors: a series, findings when a threshold is crossed, an hourly report with a chart
  feverfewvale84: { kind: "monitor", target: "chain", outputs: "chain health report: block time, finality lag, stalls" },
  celandine62: { kind: "monitor", target: "bridge", outputs: "bridge report: light-client lag, escrow invariant" },
  bellflower46: { kind: "monitor", target: "storefront", outputs: "storefront report: paid orders by stage, SOL pending" },
  hyssop58: { kind: "monitor", target: "mining", outputs: "mining report: paid-job share of new blocks, pool balance" },
  honeysuckle92: { kind: "monitor", target: "buyback", outputs: "buyback report: time since the last crank, vault waiting" },
  lupine81: { kind: "monitor", target: "api", outputs: "API report: readiness checks, node RPC queue, response time" },
  nettlemoth47: { kind: "monitor", target: "market", outputs: "market report: hires near their deadline, unjudged and expired hires" },
  scabiosa54: { kind: "monitor", target: "wallets", outputs: "house wallet report: agents too low on CADRE or SOL for their job" },
  betony83: { kind: "monitor", target: "desktops", outputs: "desktop cost report: spend against the cap, unhealthy desktops" },
  // practice traders (services/swarm/strategies, run by cadre-practice with STRAT_DESK_TRADERS)
  foxglove47: { kind: "trader", outputs: "practice trades committed on the strategies contract, each with a post-mortem" },
  meadowsweet27: { kind: "trader", outputs: "practice trades committed on the strategies contract, each with a post-mortem" },
  primrosedusk54: { kind: "trader", outputs: "practice trades committed on the strategies contract, each with a post-mortem" },
  boragemist71: { kind: "trader", outputs: "practice trades committed on the strategies contract, each with a post-mortem" },
  // the digest of everything above
  yarrowvale63: { kind: "digest", outputs: "a work digest every six hours linking every finished output" },
};

export function jobOf(handle: string): Job | null { return ROSTER[handle] ?? null; }

/** Job title and description of a house agent (its persona's own words). */
export function jobText(handle: string): { title: string; description: string } {
  const p = PERSONAS.find((x) => x.handle === handle);
  return { title: p?.archetype ?? "house agent", description: p?.persona ?? "" };
}

export const handlesOf = (k: JobKind) => Object.entries(ROSTER).filter(([, j]) => j.kind === k).map(([h]) => h);
