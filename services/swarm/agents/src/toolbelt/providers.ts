// Outside providers the toolbelt calls, and whether each has a key on this host (docs/TOOLBELT.md 8).
// Keys are read from files named by env (never from the repo, never printed, never logged, never
// put in a prompt or a receipt). A provider without a key reports status "needs_key" and every tool
// that needs it answers ToolRefused("needs_key") before anything is reserved or paid.
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { loadSecrets, parseEnvFile } from "../../../desktop/src/config.ts";

export interface ProviderKeys {
  firecrawl: string | null;
  brave: string | null;
  e2b: string | null;
  openrouter: string | null;
  agentmail: string | null;
}

const expand = (p: string) => p.replace(/^~(?=\/)/, homedir());

function fromFile(path: string | undefined, key: string): string | null {
  if (!path) return null;
  const p = expand(path);
  if (!existsSync(p)) return null;
  if (p.endsWith(".json")) {
    try { const j = JSON.parse(readFileSync(p, "utf8")); const v = j[key] ?? j.apiKey ?? j.api_key; return typeof v === "string" && v.trim() ? v.trim() : null; } catch { return null; }
  }
  const e = parseEnvFile(p);
  return e[key]?.trim() || null;
}

/** TOOLBELT_ENV_FILE (an env file of KEY=value lines) or per-provider *_KEY_FILE variables. */
export function loadProviderKeys(env: Record<string, string | undefined> = process.env): ProviderKeys {
  const base = env.TOOLBELT_ENV_FILE ? parseEnvFile(expand(env.TOOLBELT_ENV_FILE)) : {};
  const desk = loadSecrets(env as NodeJS.ProcessEnv);
  const pick = (...v: (string | null | undefined)[]) => v.find((x) => x && x.trim())?.trim() ?? null;
  return {
    firecrawl: pick(env.FIRECRAWL_API_KEY, base.FIRECRAWL_API_KEY, fromFile(env.FIRECRAWL_KEY_FILE, "apiKey")),
    brave: pick(env.BRAVE_API_KEY, base.BRAVE_API_KEY, fromFile(env.BRAVE_KEY_FILE, "BRAVE_API_KEY")),
    e2b: pick(base.E2B_API_KEY, desk.e2bKey),
    openrouter: pick(base.OPENROUTER_API_KEY, desk.openrouterKey),
    agentmail: pick(env.AGENTMAIL_API_KEY, base.AGENTMAIL_API_KEY, fromFile(env.AGENTMAIL_KEY_FILE, "AGENTMAIL_API_KEY")),
  };
}

/** Which tools work on this host and which wait for a key (no key values, ever). */
export function providerStatus(k: ProviderKeys, githubApp = false) {
  const s = (ok: boolean, need: string) => (ok ? "ready" : `needs_key: ${need}`);
  return {
    "web.search": s(!!(k.firecrawl || k.brave), "FIRECRAWL_API_KEY or BRAVE_API_KEY"),
    "web.fetch": "ready",
    "computer.task": s(!!(k.e2b && k.openrouter), "E2B_API_KEY and OPENROUTER_API_KEY"),
    "code.run": s(!!k.e2b, "E2B_API_KEY"),
    "code.github_*": githubApp ? "ready (the Cadre GitHub App)" : "needs_github_app: TOOLBELT_GITHUB_APP_ID and the app's private key (docs/TOOLBELT.md 3.3)",
    "files.put": "ready (Cadre blob store)",
    "email.*": s(!!k.agentmail, "AGENTMAIL_API_KEY"),
    "http.call": "ready",
  };
}

// ---------------------------------------------------------------- search

export interface SearchHit { url: string; title: string; snippet: string }
export interface SearchOut { provider: string; hits: SearchHit[]; units: number; unit: string }

export async function firecrawlSearch(key: string, query: string, limit: number, base = "https://api.firecrawl.dev"): Promise<SearchOut> {
  const r = await fetch(`${base}/v2/search`, { method: "POST", headers: { authorization: `Bearer ${key}`, "content-type": "application/json" }, body: JSON.stringify({ query, limit }), signal: AbortSignal.timeout(45_000) });
  const j: any = await r.json().catch(() => null);
  if (!r.ok || !j?.success) throw new Error(`search provider answered ${r.status}: ${String(j?.error ?? "").slice(0, 160)}`);
  const web: any[] = Array.isArray(j.data) ? j.data : j.data?.web ?? [];
  return {
    provider: "firecrawl", unit: "credits", units: Number(j.creditsUsed ?? 2 * Math.max(1, Math.ceil(limit / 10))),
    hits: web.slice(0, limit).map((x) => ({ url: String(x.url ?? ""), title: String(x.title ?? "").slice(0, 200), snippet: String(x.description ?? "").replace(/\s+/g, " ").slice(0, 400) })).filter((x) => /^https?:\/\//.test(x.url)),
  };
}

export async function braveSearch(key: string, query: string, limit: number, base = "https://api.search.brave.com"): Promise<SearchOut> {
  const r = await fetch(`${base}/res/v1/web/search?q=${encodeURIComponent(query)}&count=${limit}`, { headers: { "x-subscription-token": key, accept: "application/json" }, signal: AbortSignal.timeout(30_000) });
  const j: any = await r.json().catch(() => null);
  if (!r.ok) throw new Error(`search provider answered ${r.status}`);
  return { provider: "brave", unit: "requests", units: 1, hits: (j?.web?.results ?? []).slice(0, limit).map((x: any) => ({ url: String(x.url), title: String(x.title ?? "").slice(0, 200), snippet: String(x.description ?? "").replace(/<[^>]+>/g, "").slice(0, 400) })) };
}

// ---------------------------------------------------------------- email (AgentMail, docs.agentmail.to)

export class AgentMail {
  key: string;
  base: string;
  constructor(key: string, base = "https://api.agentmail.to") { this.key = key; this.base = base; }
  private async req(method: string, path: string, body?: unknown) {
    const r = await fetch(`${this.base}${path}`, { method, headers: { authorization: `Bearer ${this.key}`, "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(30_000) });
    const j: any = await r.json().catch(() => null);
    if (!r.ok) throw new Error(`email provider answered ${r.status}: ${String(j?.message ?? "").slice(0, 160)}`);
    return j;
  }
  createInbox(username: string, displayName: string) { return this.req("POST", "/v0/inboxes", { username, display_name: displayName }); }
  list(inbox: string, limit = 20) { return this.req("GET", `/v0/inboxes/${encodeURIComponent(inbox)}/messages?limit=${limit}`); }
  get(inbox: string, id: string) { return this.req("GET", `/v0/inboxes/${encodeURIComponent(inbox)}/messages/${encodeURIComponent(id)}`); }
  send(inbox: string, to: string, subject: string, text: string) { return this.req("POST", `/v0/inboxes/${encodeURIComponent(inbox)}/messages/send`, { to, subject, text }); }
  reply(inbox: string, id: string, text: string) { return this.req("POST", `/v0/inboxes/${encodeURIComponent(inbox)}/messages/${encodeURIComponent(id)}/reply`, { text }); }
}
