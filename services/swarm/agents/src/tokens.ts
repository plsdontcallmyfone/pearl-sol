// Agent tokens and OAuth grants for hosted agents (docs/CONNECT.md "Hosted mode").
//
// An agent token (cadre_at_...) is a bearer credential bound to one hosted agent. Whoever holds it
// steers the agent's decisions through the tools in packages/cadre-mcp/src/catalog.ts; the runtime
// signs with the agent key it holds, after checking the agent spec's fixed limits. No token, grant
// or scope can withdraw, export the key, change limits, pause or delete (there is no code path).
//
// Storage: only sha256(token) is kept (stateDir/tokens.json, mode 0600). A token is shown once.
// One live agent token per agent; rotating issues a new one, kills the old and every OAuth grant
// made from it (grants carry the token generation they were made under).
//
// OAuth grants (for remote MCP clients that cannot send a static header: claude.ai, chatgpt.com):
// an access token (cadre_oa_..., 1 hour) and a refresh token (cadre_or_..., 30 days, rotated on every
// use), both hashed, both bound to the agent and the token generation. Revoking either revokes the grant.
import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";

export const ACCESS_TTL_MS = 3_600_000;
export const REFRESH_TTL_MS = 30 * 86_400_000;
const PREFIX = { agent: "cadre_at_", access: "cadre_oa_", refresh: "cadre_or_" } as const;

const sha = (s: string) => createHash("sha256").update(s, "utf8").digest("hex");
const fresh = (p: string) => p + randomBytes(32).toString("base64url");
const eqHex = (a: string, b: string) => a.length === b.length && timingSafeEqual(Buffer.from(a, "hex"), Buffer.from(b, "hex"));

interface TokenRec { agent: string; token_id: string; hash: string; gen: number; created_ms: number; rotated_ms: number | null; last_used_ms: number | null; by: string }
interface GrantRec { grant_id: string; agent: string; gen: number; client_id: string; client_name: string; access_hash: string; access_exp: number; refresh_hash: string; refresh_exp: number; created_ms: number; last_used_ms: number | null; revoked: boolean }
interface File { tokens: Record<string, TokenRec>; grants: GrantRec[] }

export interface Resolved { agent: string; kind: "agent_token" | "oauth"; token_id: string; grant_id?: string }

export class TokenStore {
  path: string;
  f: File = { tokens: {}, grants: [] };
  now: () => number;

  constructor(path: string, now: () => number = Date.now) {
    this.path = path;
    this.now = now;
    if (existsSync(path)) this.f = JSON.parse(readFileSync(path, "utf8"));
    this.f.tokens ??= {};
    this.f.grants ??= [];
  }

  private save() {
    const tmp = `${this.path}.tmp-${process.pid}`;
    writeFileSync(tmp, JSON.stringify(this.f), { mode: 0o600 });
    renameSync(tmp, this.path);
  }

  status(agent: string) {
    const t = this.f.tokens[agent];
    const live = this.f.grants.filter((g) => g.agent === agent && !g.revoked && g.gen === t?.gen && g.refresh_exp > this.now());
    return {
      agent, has_token: !!t, token_id: t?.token_id ?? null, created_ms: t?.created_ms ?? null, rotated_ms: t?.rotated_ms ?? null,
      last_used_ms: t ? Math.max(t.last_used_ms ?? 0, ...live.map((g) => g.last_used_ms ?? 0)) || null : null,
      driven_externally: !!t, oauth_grants: live.map((g) => ({ grant_id: g.grant_id, client_name: g.client_name, created_ms: g.created_ms, last_used_ms: g.last_used_ms })),
    };
  }

  has(agent: string) { return !!this.f.tokens[agent]; }

  /** A new agent token for `agent`; throws "exists" when one is live and `rotate` is false. Returns the plaintext once. */
  issue(agent: string, by: string, rotate = false): { token: string; token_id: string; created_ms: number } {
    const old = this.f.tokens[agent];
    if (old && !rotate) throw new Error("exists");
    const token = fresh(PREFIX.agent);
    const rec: TokenRec = { agent, token_id: `tok_${randomBytes(6).toString("hex")}`, hash: sha(token), gen: (old?.gen ?? 0) + 1, created_ms: this.now(), rotated_ms: old ? this.now() : null, last_used_ms: null, by };
    this.f.tokens[agent] = rec;
    // grants made under the old generation die with it
    for (const g of this.f.grants) if (g.agent === agent && g.gen !== rec.gen) g.revoked = true;
    this.prune();
    this.save();
    return { token, token_id: rec.token_id, created_ms: rec.created_ms };
  }

  /** Resolve a bearer credential (agent token or OAuth access token) to its agent; null when invalid or expired. */
  resolve(bearer: string): Resolved | null {
    if (typeof bearer !== "string") return null;
    const h = sha(bearer);
    if (bearer.startsWith(PREFIX.agent)) {
      for (const t of Object.values(this.f.tokens)) if (eqHex(t.hash, h)) { t.last_used_ms = this.now(); this.saveSoon(); return { agent: t.agent, kind: "agent_token", token_id: t.token_id }; }
      return null;
    }
    if (bearer.startsWith(PREFIX.access)) {
      const g = this.f.grants.find((x) => eqHex(x.access_hash, h));
      if (!g || g.revoked || g.access_exp <= this.now()) return null;
      const t = this.f.tokens[g.agent];
      if (!t || t.gen !== g.gen) return null;
      g.last_used_ms = this.now();
      this.saveSoon();
      return { agent: g.agent, kind: "oauth", token_id: t.token_id, grant_id: g.grant_id };
    }
    return null;
  }

  /** The agent of a live agent token (for rotate by token); null otherwise. */
  agentOfToken(token: string): string | null {
    const r = token.startsWith(PREFIX.agent) ? this.resolve(token) : null;
    return r?.agent ?? null;
  }

  /** A new OAuth grant for `agent` (the caller proved control: the agent token, or the creator's signature). */
  grant(agent: string, client_id: string, client_name: string) {
    const t = this.f.tokens[agent];
    if (!t) throw new Error("no_token");
    const access = fresh(PREFIX.access);
    const refresh = fresh(PREFIX.refresh);
    const g: GrantRec = { grant_id: `grant_${randomBytes(6).toString("hex")}`, agent, gen: t.gen, client_id: String(client_id).slice(0, 200), client_name: String(client_name || "MCP client").slice(0, 100), access_hash: sha(access), access_exp: this.now() + ACCESS_TTL_MS, refresh_hash: sha(refresh), refresh_exp: this.now() + REFRESH_TTL_MS, created_ms: this.now(), last_used_ms: null, revoked: false };
    this.f.grants.push(g);
    this.prune();
    this.save();
    return { access_token: access, refresh_token: refresh, expires_in: ACCESS_TTL_MS / 1000, grant_id: g.grant_id, agent };
  }

  /** Refresh: a new access token and a new refresh token (the old refresh token dies). */
  refresh(refreshToken: string, client_id?: string) {
    if (typeof refreshToken !== "string" || !refreshToken.startsWith(PREFIX.refresh)) return null;
    const h = sha(refreshToken);
    const g = this.f.grants.find((x) => eqHex(x.refresh_hash, h));
    if (!g || g.revoked || g.refresh_exp <= this.now()) return null;
    if (client_id && g.client_id && client_id !== g.client_id) return null;
    const t = this.f.tokens[g.agent];
    if (!t || t.gen !== g.gen) return null;
    const access = fresh(PREFIX.access);
    const refresh = fresh(PREFIX.refresh);
    g.access_hash = sha(access);
    g.access_exp = this.now() + ACCESS_TTL_MS;
    g.refresh_hash = sha(refresh);
    g.refresh_exp = this.now() + REFRESH_TTL_MS;
    this.save();
    return { access_token: access, refresh_token: refresh, expires_in: ACCESS_TTL_MS / 1000, grant_id: g.grant_id, agent: g.agent };
  }

  /** RFC 7009: revoke the grant an access or refresh token belongs to. Unknown tokens are fine. */
  revoke(token: string): boolean {
    if (typeof token !== "string") return false;
    const h = sha(token);
    const g = this.f.grants.find((x) => eqHex(x.access_hash, h) || eqHex(x.refresh_hash, h));
    if (!g) return false;
    g.revoked = true;
    this.save();
    return true;
  }

  private prune() {
    const now = this.now();
    this.f.grants = this.f.grants.filter((g) => !g.revoked && g.refresh_exp > now).slice(-5000);
  }

  private timer: ReturnType<typeof setTimeout> | null = null;
  private saveSoon() {
    if (this.timer) return;
    this.timer = setTimeout(() => { this.timer = null; try { this.save(); } catch { /* next save */ } }, 5_000);
    this.timer.unref?.();
  }
}

export const isBearerShape = (s: string) => s.startsWith(PREFIX.agent) || s.startsWith(PREFIX.access);
