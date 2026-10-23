// OAuth 2.1 authorization server for the remote MCP endpoint (docs/CONNECT.md "OAuth"):
// RFC 8414 metadata, RFC 9728 protected resource metadata, RFC 7591 dynamic client registration,
// Client ID Metadata Documents (an https client_id whose JSON lists its redirect_uris), PKCE S256
// only, RFC 9207 `iss` on the authorization response, authorization codes for 5 minutes, one use.
// Tokens themselves (access 1 h, refresh 30 days rotating) are issued and checked by the hosted
// runtime (services/swarm/agents/src/tokens.ts), which holds the agent keys; this file never sees
// a key and keeps only registered clients (public data) on disk.
import { createHash, randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export interface Client { client_id: string; client_name: string; redirect_uris: string[]; created_ms: number; source: "dcr" | "cimd" }
export interface CodeRec { client_id: string; redirect_uri: string; challenge: string; resource: string | null; scope: string; exp: number; tokens: { access_token: string; refresh_token: string; expires_in: number; agent: string; handle?: string } }

const CODE_TTL_MS = 300_000;

export const b64url = (b: Buffer) => b.toString("base64url");
export const s256 = (verifier: string) => b64url(createHash("sha256").update(verifier, "ascii").digest());

/** A redirect URI we accept at all: https anywhere, or http on a loopback host (any port). */
export function redirectShapeOk(u: string): boolean {
  let x: URL;
  try { x = new URL(u); } catch { return false; }
  if (x.hash) return false;
  if (x.protocol === "https:") return true;
  return x.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(x.hostname);
}

/** Registered URI matches the requested one; loopback URIs match without the port (RFC 8252 7.3). */
export function redirectMatches(registered: string[], asked: string): boolean {
  if (registered.includes(asked)) return true;
  let a: URL;
  try { a = new URL(asked); } catch { return false; }
  if (a.protocol !== "http:" || !["localhost", "127.0.0.1", "[::1]"].includes(a.hostname)) return false;
  return registered.some((r) => {
    try {
      const x = new URL(r);
      return x.protocol === "http:" && x.hostname === a.hostname && x.pathname === a.pathname && x.search === a.search;
    } catch { return false; }
  });
}

export class OAuthStore {
  file: string;
  dir: string;
  clients = new Map<string, Client>();
  codes = new Map<string, CodeRec>();
  cimd = new Map<string, { at: number; client: Client | null; error?: string }>();
  fetcher: typeof fetch;

  constructor(stateDir: string, fetcher: typeof fetch = fetch) {
    this.dir = stateDir;
    this.file = join(stateDir, "oauth-clients.json");
    this.fetcher = fetcher;
    if (existsSync(this.file)) for (const c of JSON.parse(readFileSync(this.file, "utf8")).clients ?? []) this.clients.set(c.client_id, c);
  }

  private save() {
    mkdirSync(this.dir, { recursive: true });
    const tmp = `${this.file}.tmp-${process.pid}`;
    writeFileSync(tmp, JSON.stringify({ clients: [...this.clients.values()].slice(-20_000) }), { mode: 0o600 });
    renameSync(tmp, this.file);
  }

  /** RFC 7591 registration of a public client. Returns the client or an OAuth error. */
  register(b: any): { client?: Client; error?: string; error_description?: string } {
    const uris: unknown = b?.redirect_uris;
    if (!Array.isArray(uris) || !uris.length || uris.length > 10 || uris.some((u) => typeof u !== "string" || !redirectShapeOk(u))) return { error: "invalid_redirect_uri", error_description: "redirect_uris: 1..10 https URIs, or http on localhost / 127.0.0.1" };
    const method = b?.token_endpoint_auth_method ?? "none";
    if (method !== "none") return { error: "invalid_client_metadata", error_description: "only public clients (token_endpoint_auth_method none, PKCE) are supported" };
    if (this.clients.size > 50_000) return { error: "temporarily_unavailable", error_description: "registration store full" };
    const c: Client = { client_id: `cadre_client_${randomBytes(12).toString("hex")}`, client_name: String(b?.client_name ?? "MCP client").slice(0, 100), redirect_uris: uris as string[], created_ms: Date.now(), source: "dcr" };
    this.clients.set(c.client_id, c);
    this.save();
    return { client: c };
  }

  /** The client for an id: a registered one, or a Client ID Metadata Document fetched from the https id. */
  async client(client_id: string): Promise<{ client: Client | null; error?: string }> {
    const c = this.clients.get(client_id);
    if (c) return { client: c };
    if (!/^https:\/\//.test(client_id) || client_id.length > 400) return { client: null, error: "unknown client_id (register at /oauth/register, or use an https client metadata document URL)" };
    const hit = this.cimd.get(client_id);
    if (hit && Date.now() - hit.at < 3_600_000) return { client: hit.client, error: hit.error };
    let client: Client | null = null;
    let error: string | undefined;
    try {
      const r = await this.fetcher(client_id, { headers: { accept: "application/json" }, redirect: "error", signal: AbortSignal.timeout(5000) });
      const t = await r.text();
      if (!r.ok || t.length > 65_536) throw new Error(`HTTP ${r.status}`);
      const j = JSON.parse(t);
      if (j.client_id !== client_id) throw new Error("the document's client_id is not its URL");
      const uris = Array.isArray(j.redirect_uris) ? j.redirect_uris.filter((u: unknown) => typeof u === "string" && redirectShapeOk(u)) : [];
      if (!uris.length) throw new Error("the document lists no usable redirect_uris");
      client = { client_id, client_name: String(j.client_name ?? new URL(client_id).hostname).slice(0, 100), redirect_uris: uris, created_ms: Date.now(), source: "cimd" };
    } catch (e) {
      error = `client metadata document ${client_id}: ${(e as Error).message}`;
    }
    this.cimd.set(client_id, { at: Date.now(), client, error });
    if (this.cimd.size > 2000) this.cimd.delete(this.cimd.keys().next().value!);
    return { client, error };
  }

  newCode(rec: Omit<CodeRec, "exp">): string {
    const code = b64url(randomBytes(32));
    this.codes.set(code, { ...rec, exp: Date.now() + CODE_TTL_MS });
    for (const [k, v] of this.codes) if (v.exp < Date.now()) this.codes.delete(k);
    return code;
  }

  /** One-use code exchange with PKCE; returns the tokens or an OAuth error. */
  takeCode(code: string, client_id: string, redirect_uri: string, verifier: string, resource: string | null) {
    const c = this.codes.get(code);
    if (!c) return { error: "invalid_grant", error_description: "unknown, used or expired authorization code" };
    this.codes.delete(code);
    if (c.exp < Date.now()) return { error: "invalid_grant", error_description: "expired authorization code" };
    if (c.client_id !== client_id) return { error: "invalid_grant", error_description: "the code was issued to another client" };
    if (c.redirect_uri !== redirect_uri) return { error: "invalid_grant", error_description: "redirect_uri differs from the authorization request" };
    if (!/^[A-Za-z0-9._~-]{43,128}$/.test(verifier) || s256(verifier) !== c.challenge) return { error: "invalid_grant", error_description: "PKCE code_verifier does not match" };
    if (resource && c.resource && resource !== c.resource) return { error: "invalid_target", error_description: "resource differs from the authorization request" };
    return { tokens: c.tokens, scope: c.scope };
  }
}
