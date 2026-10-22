// Cadre feed bot (docs/ECOSYSTEM.md decision D9, docs/LISTINGS.md "Bot feed"): reads the public
// event feed of cadre-api (GET /v1/events) and, ONLY when switched on, posts each new event to a
// Telegram channel and/or an X account. The owner approves every account; until then it runs in
// dry-run mode, which prints what it would post and sends nothing.
//
// Switching on needs all three, on purpose:
//   FEED_LIVE=1                      (default 0: dry run)
//   FEED_TARGETS=telegram,x          (which targets; default none)
//   the target's credentials:        FEED_TELEGRAM_TOKEN + FEED_TELEGRAM_CHAT, FEED_X_TOKEN (OAuth 2 user
//                                    token with tweet.write) or a file path in FEED_*_TOKEN_FILE
// Credentials are read from env or 0600 files and never printed or logged.

import { existsSync, mkdirSync, readFileSync, writeFileSync, appendFileSync, statSync } from "node:fs";
import { dirname } from "node:path";

export interface FeedEvent { id: string; type: string; at: number | null; title: string; house: boolean; links: Record<string, string> }

export interface BotConfig {
  api: string;
  live: boolean;
  targets: ("telegram" | "x")[];
  types: string[];
  organicOnly: boolean;
  pollMs: number;
  maxPerHour: number;
  stateFile: string;
  outbox: string | null;
  backfill: number;
  telegram: { token: string | null; chat: string | null; api: string };
  x: { token: string | null; api: string };
  siteUrl: string | null;
}

function secret(env: NodeJS.ProcessEnv, k: string): string | null {
  if (env[k]) return env[k]!.trim();
  const f = env[`${k}_FILE`];
  if (f && existsSync(f)) {
    const mode = statSync(f).mode & 0o077;
    if (mode) throw new Error(`${k}_FILE must not be readable by group or others (chmod 600)`);
    return readFileSync(f, "utf8").trim() || null;
  }
  return null;
}

export function configFromEnv(env: NodeJS.ProcessEnv = process.env): BotConfig {
  const targets = (env.FEED_TARGETS ?? "").split(",").map((s) => s.trim()).filter(Boolean);
  for (const t of targets) if (t !== "telegram" && t !== "x") throw new Error(`FEED_TARGETS: unknown target ${t} (telegram, x)`);
  return {
    api: (env.FEED_API ?? "http://127.0.0.1:10685").replace(/\/$/, ""),
    live: env.FEED_LIVE === "1",
    targets: targets as BotConfig["targets"],
    types: (env.FEED_TYPES ?? "buyback,burn,big_trade,hire_completed,paid_dm_answered,new_agent,milestone").split(",").map((s) => s.trim()).filter(Boolean),
    organicOnly: env.FEED_ORGANIC_ONLY === "1",
    pollMs: Math.max(5, Number(env.FEED_POLL_S ?? 60)) * 1000,
    maxPerHour: Math.max(1, Number(env.FEED_MAX_PER_HOUR ?? 10)),
    stateFile: env.FEED_STATE ?? `${env.HOME ?? "."}/.cadre/feed-bot-state.json`,
    outbox: env.FEED_OUTBOX ?? null,
    backfill: Math.max(0, Number(env.FEED_BACKFILL ?? 0)),
    telegram: { token: secret(env, "FEED_TELEGRAM_TOKEN"), chat: env.FEED_TELEGRAM_CHAT ?? null, api: (env.FEED_TELEGRAM_API ?? "https://api.telegram.org").replace(/\/$/, "") },
    x: { token: secret(env, "FEED_X_TOKEN"), api: (env.FEED_X_API ?? "https://api.x.com").replace(/\/$/, "") },
    siteUrl: env.FEED_SITE_URL ?? null,
  };
}

/** Why the bot will not post (empty when it may). Dry run is the default answer. */
export function blockers(c: BotConfig): string[] {
  const out: string[] = [];
  if (!c.live) out.push("FEED_LIVE is not 1 (dry run)");
  if (!c.targets.length) out.push("FEED_TARGETS is empty");
  if (c.targets.includes("telegram") && (!c.telegram.token || !c.telegram.chat)) out.push("telegram needs FEED_TELEGRAM_TOKEN and FEED_TELEGRAM_CHAT");
  if (c.targets.includes("x") && !c.x.token) out.push("x needs FEED_X_TOKEN");
  return out;
}

/** The text posted for one event: its title and the link that proves it (X limit 280 chars). */
export function format(e: FeedEvent, c: Pick<BotConfig, "siteUrl">, max = 280): string {
  const link = e.links.explorer ?? e.links.solana ?? (c.siteUrl && e.links.api ? `${c.siteUrl.replace(/\/$/, "")}${e.links.api}` : "");
  const tail = link ? `\n${link}` : "";
  let t = e.title + (e.house && !/house/i.test(e.title) ? " [house]" : "");
  if (t.length + tail.length > max) t = t.slice(0, Math.max(0, max - tail.length - 3)) + "...";
  return t + tail;
}

interface State { since: number; seen: string[]; sent: number[] }
function load(f: string): State | null {
  try { return JSON.parse(readFileSync(f, "utf8")); } catch { return null; }
}
function save(f: string, s: State) {
  mkdirSync(dirname(f), { recursive: true });
  writeFileSync(f + ".tmp", JSON.stringify(s));
  writeFileSync(f, JSON.stringify(s));
}

export interface Poster { (target: "telegram" | "x", text: string): Promise<void> }

export function realPoster(c: BotConfig, fetchFn: typeof fetch = fetch): Poster {
  return async (target, text) => {
    if (target === "telegram") {
      const r = await fetchFn(`${c.telegram.api}/bot${c.telegram.token}/sendMessage`, {
        method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ chat_id: c.telegram.chat, text, disable_web_page_preview: true }),
      });
      if (!r.ok) throw new Error(`telegram HTTP ${r.status}`);
    } else {
      const r = await fetchFn(`${c.x.api}/2/tweets`, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${c.x.token}` }, body: JSON.stringify({ text }) });
      if (!r.ok) throw new Error(`x HTTP ${r.status}`);
    }
  };
}

export class FeedBot {
  c: BotConfig;
  post: Poster;
  log: (m: string) => void;
  fetchFn: typeof fetch;
  s: State;
  constructor(c: BotConfig, opts: { post?: Poster; log?: (m: string) => void; fetchFn?: typeof fetch } = {}) {
    this.c = c;
    this.fetchFn = opts.fetchFn ?? fetch;
    this.post = opts.post ?? realPoster(c, this.fetchFn);
    this.log = opts.log ?? ((m) => console.log(m));
    this.s = load(c.stateFile) ?? { since: -1, seen: [], sent: [] };
  }

  async events(): Promise<FeedEvent[]> {
    const q = new URLSearchParams({ limit: "200", types: this.c.types.join(",") });
    if (this.c.organicOnly) q.set("organic", "1");
    const r = await this.fetchFn(`${this.c.api}/v1/events?${q}`, { signal: AbortSignal.timeout(60_000) });
    if (!r.ok) throw new Error(`cadre-api /v1/events HTTP ${r.status}`);
    const j: any = await r.json();
    return (j.events ?? []) as FeedEvent[];
  }

  /** One poll: new events oldest first; on the first run only the newest FEED_BACKFILL are taken. */
  async tick(now = Date.now()): Promise<{ new: number; posted: number; dry: number; held: number }> {
    const all = await this.events();
    const seen = new Set(this.s.seen);
    let fresh = all.filter((e) => !seen.has(e.id));
    if (this.s.since === -1) {
      fresh = fresh.slice(0, this.c.backfill);
      for (const e of all) seen.add(e.id);
    }
    fresh.reverse();
    const block = blockers(this.c);
    let posted = 0, dry = 0, held = 0;
    this.s.sent = this.s.sent.filter((t) => now - t < 3_600_000);
    for (const e of fresh) {
      const text = format(e, this.c);
      if (block.length) {
        dry++;
        this.log(`[dry-run] ${e.type} ${e.id}: ${text.replace(/\n/g, " | ")}`);
        if (this.c.outbox) appendFileSync(this.c.outbox, JSON.stringify({ at: now, id: e.id, type: e.type, text, would_post_to: this.c.targets, blocked_by: block }) + "\n");
      } else if (this.s.sent.length >= this.c.maxPerHour) {
        held++;
        continue; // not marked seen: posted on a later tick, under the hourly cap
      } else {
        for (const t of this.c.targets) await this.post(t, text);
        this.s.sent.push(now);
        posted++;
        this.log(`[posted] ${e.type} ${e.id} -> ${this.c.targets.join(",")}`);
      }
      seen.add(e.id);
    }
    this.s.seen = [...seen].slice(-5000);
    this.s.since = now;
    save(this.c.stateFile, this.s);
    return { new: fresh.length, posted, dry, held };
  }
}
