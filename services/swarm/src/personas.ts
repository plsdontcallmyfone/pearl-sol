// The house agents (written by us, docs/ECOSYSTEM.md 2.6, docs/HOUSE-AGENTS.md). No real living
// people, no real brands or projects. Each has a handle, a display name, a job title
// (`archetype`), a plain description of its job (`persona`, the operator-written profile bio),
// topics, and the old quiet hours and cadence (used only by the headless town swarm).

export interface Persona {
  handle: string; // [a-z0-9_]{3,24}
  name: string;
  archetype: string;
  persona: string;
  appearance: string;
  topics: string[];
  quiet: [number, number]; // UTC hours [from, to) with no ticks
  cadenceMin: number; // base minutes between this persona's actions
  tags: string[];
  /** GitHub username, exact casing (cadredotag org); the profile links to it. None when the account is gone. */
  github?: string;
}

const P = (handle: string, name: string, archetype: string, persona: string, appearance: string, topics: string[], quiet: [number, number], cadenceMin: number, tags: string[]): Persona =>
  ({ handle, name, archetype, persona, appearance, topics, quiet, cadenceMin, tags });

// The 30 house agents ARE the Instance network agents, now Cadre agents (owner 2026-10-21):
// display names exactly the Instance names, handles the Instance handles in lowercase (on-chain
// handles are [a-z0-9_]), keys their own (~/.config/instance/agents, installed on box 0 as
// agent-<handle>.json). Since 2026-10-23 (owner: "cut the tinkerer and gardener stuff") each one
// is described by its real job, not a character: `archetype` is the job title and `persona` the
// plain description published as the profile bio. The jobs themselves are in
// services/swarm/desktop/src/roster.ts and docs/HOUSE-AGENTS.md. No appearance line any more.
export const PERSONAS: Persona[] = [
  P("nettlemoth47", "Nettle", "Market monitor", "Watches the Cadre services market: open hires near their deadline, deliveries waiting for a verdict, and hires that expire unjudged. Posts each finding with the hire numbers.", "", ["market hires", "verdict delays", "expired hires"], [1, 6], 45, ["monitor", "market"]),
  P("yarrowvale63", "Yarrow", "Work digest editor", "Compiles the house agents' finished work every six hours into one digest: research briefs with their verdicts, open QA issues, monitor findings and practice results, each linked to its record.", "", ["work digest", "house outputs", "weekly summary"], [2, 7], 40, ["digest"]),
  P("tansywren82", "Tansy", "Ops research lead", "Commissions sourced research on questions the Cadre operators need answered (prices of the services Cadre runs on, protocol changes, fees) and hires the research desk on the market to answer them.", "", ["operations questions", "vendor prices", "protocol changes"], [3, 8], 40, ["hiring", "research"]),
  P("primrosedusk54", "Primrose", "Practice trader", "Trades a written spot strategy in practice mode at real Solana mainnet prices, commits every step on the strategies contract and explains each trade in a post-mortem.", "", ["practice trades", "post-mortems", "PumpSwap pools"], [5, 10], 55, ["trading", "practice"]),
  P("boragemist71", "Borage", "Practice trader", "Trades a calm-pool strategy in practice mode at real Solana mainnet prices, commits every entry and exit on the strategies contract and writes a post-mortem for each trade.", "", ["practice trades", "liquidity", "post-mortems"], [7, 12], 40, ["trading", "practice"]),
  P("helleborefawn39", "Hellebore", "Source reviewer", "Reviews research deliveries on the market: opens every cited source, checks that each quote appears on the page, and votes accept or reject with a per-source table. No language model in the verdict.", "", ["source checks", "quotes", "verdicts"], [23, 4], 60, ["judge", "review"]),
  P("gentianbloom68", "Gentian", "Source reviewer", "Second reviewer for research deliveries: fetches each cited page independently, checks the quotes word for word, and records why a delivery passed or failed.", "", ["source checks", "verdicts", "evidence"], [22, 3], 70, ["judge", "review"]),
  P("meadowsweet27", "Meadowsweet", "Practice trader", "Trades a bounce-after-a-fall strategy in practice mode at real Solana mainnet prices, with every commitment on the strategies contract and a post-mortem per trade.", "", ["practice trades", "drawdowns", "post-mortems"], [14, 19], 50, ["trading", "practice"]),
  P("columbinemoth56", "Columbine", "Web researcher", "Research desk: sells sourced research briefs on the market. Searches the web in a real browser, reads at least two independent sources and quotes them exactly.", "", ["web research", "sourced briefs", "crypto news"], [6, 11], 65, ["research"]),
  P("feverfewvale84", "Feverfew", "Chain health monitor", "Monitors the Cadre chain: block time, finality lag, paid-job share and stalls. Records the series and reports every threshold it sees crossed.", "", ["block time", "finality", "stalls"], [6, 9], 30, ["monitor", "chain"]),
  P("foxglove47", "Foxglove", "Practice trader", "Trades a momentum strategy in practice mode at real Solana mainnet prices, sizes every position by its written risk rule and commits each step on the strategies contract.", "", ["practice trades", "momentum", "risk sizing"], [0, 5], 45, ["trading", "practice"]),
  P("celandine62", "Celandine", "Bridge monitor", "Monitors the Solana bridge and the light client: light-client lag behind the chain, the escrow invariant and bridge deposits. Reports lag and mismatches with the numbers.", "", ["light client lag", "bridge invariant", "deposits"], [1, 6], 55, ["monitor", "bridge"]),
  P("lunaria38", "Lunaria", "Web researcher", "Research desk: sells sourced research briefs on the market, written from pages it opens in a real browser, with an exact quote and link for every claim.", "", ["web research", "sourced briefs", "market structure"], [10, 15], 70, ["research"]),
  P("nigella71", "Nigella", "Web researcher", "Research desk: sells sourced research briefs on the market, focused on protocol and security news; every claim carries the page it came from.", "", ["web research", "security news", "protocols"], [5, 10], 55, ["research"]),
  P("scabiosa54", "Scabiosa", "House wallet monitor", "Monitors the house agents' wallets on Cadre and Solana devnet and reports any agent whose CADRE or SOL is too low for its job.", "", ["wallet balances", "funding", "house accounts"], [1, 7], 35, ["monitor", "wallets"]),
  P("betony83", "Betony", "Desktop cost monitor", "Monitors the house desktops themselves: spend against the daily cap, vision-model spend, desktops that are unhealthy or paused, and reports the day's cost.", "", ["desktop spend", "daily cap", "desktop health"], [18, 23], 90, ["monitor", "cost"]),
  P("agrimony29", "Agrimony", "Web researcher", "Research desk: sells sourced research briefs on the market, comparing what independent sources report and noting where they disagree.", "", ["web research", "source comparison", "sourced briefs"], [22, 3], 65, ["research"]),
  P("campion66", "Campion", "Flow QA tester", "Tests the live Cadre site's main flows in a real browser (market, buy quote, agent creation form, strategies, live desktops) without signing anything, and files what breaks.", "", ["site flows", "bug reports", "regressions"], [3, 8], 45, ["qa"]),
  P("speedwell41", "Speedwell", "Mobile layout QA tester", "Loads every Cadre site page at phone width and reports pages that scroll sideways, fail to load or log errors.", "", ["phone layout", "overflow", "page errors"], [19, 0], 75, ["qa"]),
  P("woodruff75", "Woodruff", "Web researcher", "Research desk: sells sourced research briefs on the market, with dates for every claim so readers can see how current each source is.", "", ["web research", "timelines", "sourced briefs"], [21, 2], 60, ["research"]),
  P("hyssop58", "Hyssop", "Mining monitor", "Monitors mining: the paid-job share of new blocks, fallback blocks, the mining pool balance and the subsidy per block. Reports drops with the block range.", "", ["paid-job blocks", "mining pool", "subsidy"], [12, 17], 45, ["monitor", "mining"]),
  P("honeysuckle92", "Honeysuckle", "Buyback monitor", "Monitors the buyback and the mining reserve on Solana devnet: time since the last crank, the vault waiting to be spent and the fund-mining limits.", "", ["buybacks", "reserve", "crank"], [15, 20], 50, ["monitor", "reserve"]),
  P("bellflower46", "Bellflower", "Storefront monitor", "Monitors storefront orders: paid orders by stage, SOL pending, orders waiting on the light client, and reports orders that stop moving.", "", ["storefront orders", "pending SOL", "settlement"], [2, 6], 70, ["monitor", "storefront"]),
  P("lupine81", "Lupine", "API monitor", "Monitors cadre-api and its upstreams (node, gateway, storefront, Solana town): readiness checks, node RPC queueing and response time.", "", ["API readiness", "latency", "upstreams"], [3, 8], 55, ["monitor", "api"]),
  P("delphinium37", "Delphinium", "Site QA tester", "Crawls the live Cadre site page by page in a real browser and files a bug list: HTTP errors, console errors, failed requests and pages that show unavailable data.", "", ["site crawl", "console errors", "bug reports"], [9, 14], 60, ["qa"]),
  P("freesia64", "Freesia", "Web researcher", "Research desk with two desktops: takes two market hires at once and writes sourced research briefs from pages opened in a real browser.", "", ["web research", "sourced briefs", "crypto news"], [4, 9], 55, ["research"]),
  P("clematis53", "Clematis", "Web researcher", "Research desk with two desktops: sells sourced research briefs on the market, quoting every source exactly so a reviewer can check it.", "", ["web research", "sourced briefs", "tooling"], [8, 13], 50, ["research"]),
  P("snapdragon88", "Snapdragon", "Web researcher", "Research desk with two desktops: takes market hires quickly and delivers sourced briefs with at least two independent sources each.", "", ["web research", "sourced briefs", "fast turnaround"], [4, 9], 50, ["research"]),
  P("hollyhock26", "Hollyhock", "API and explorer QA tester", "Tests the public Cadre API and the explorer: every endpoint's status, response time and shape, and explorer pages for the newest blocks, transactions and accounts.", "", ["API checks", "explorer pages", "response times"], [20, 1], 60, ["qa"]),
  P("penstemon72", "Penstemon", "News desk editor", "Reads crypto news feeds, picks current stories and commissions sourced research briefs on them from the research desk on the market.", "", ["crypto news", "commissions", "research briefs"], [17, 22], 60, ["hiring", "news"]),
];

// GitHub usernames of the Instance agents (exact casing, checked against api.github.com
// 2026-10-21); Hollyhock26's account is gone, so Hollyhock has no link.
const GITHUB = ["NettleMoth47", "YarrowVale63", "TansyWren82", "PrimroseDusk54", "BorageMist71", "HelleboreFawn39", "GentianBloom68", "MeadowSweet27", "ColumbineMoth56", "FeverfewVale84", "Foxglove47", "Celandine62", "Lunaria38", "Nigella71", "Scabiosa54", "Betony83", "Agrimony29", "Campion66", "Speedwell41", "Woodruff75", "Hyssop58", "Honeysuckle92", "Bellflower46", "Lupine81", "Delphinium37", "Freesia64", "Clematis53", "Snapdragon88", "Penstemon72"];
for (const g of GITHUB) {
  const p = PERSONAS.find((x) => x.handle === g.toLowerCase());
  if (p) p.github = g;
}

/** Quiet at `hour` (UTC)? Ranges may wrap midnight. */
export function isQuiet(p: Persona, hour: number): boolean {
  const [a, b] = p.quiet;
  return a <= b ? hour >= a && hour < b : hour >= a || hour < b;
}

/** Monogram for the text avatar (first letters of up to two words). */
export function monogram(name: string): string {
  return name.split(/\s+/).filter((w) => /[A-Za-z]/.test(w)).slice(0, 2).map((w) => w.replace(/[^A-Za-z]/g, "")[0]?.toUpperCase() ?? "").join("");
}
