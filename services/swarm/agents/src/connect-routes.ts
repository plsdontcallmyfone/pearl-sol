// Cadre Connect routes of the hosted runtime (docs/CONNECT.md "Hosted mode"), kept out of
// runtime.ts so other rows' runtime edits merge easily: agent token mint / rotate / status,
// OAuth grant / refresh / revoke (called by cadre-api's OAuth server), and the token-scoped
// drive tools. runtime.ts calls connectRoute() once from handle() and mintToken() from create.
import { existsSync } from "node:fs";
import type { IncomingMessage } from "node:http";
import { NEVER_SCOPES, TOKEN_SCOPES } from "../../../../packages/cadre-mcp/src/catalog.ts";
import { Drive, DriveError } from "./drive.ts";
import { cadreAddr, HttpError, type Runtime } from "./runtime.ts";
import type { AgentRunner } from "./agent.ts";

const drives = new WeakMap<AgentRunner, Drive>();

/** The agent's creator (the spec's operator), for token minting. Works for sovereign agents too:
 * a token carries no withdraw, export, limit, pause or delete scope. */
export function creatorCheck(rt: Runtime, agent: string, signer: string) {
  const e = rt.entries.get(agent);
  if (!e) throw new HttpError(404, "not_found", "this agent is not hosted here");
  if (e.operator !== signer) throw new HttpError(403, "not_creator", "only the agent's creator wallet can do this");
  return e;
}

export function mintToken(rt: Runtime, agent: string, by: string, rotate: boolean) {
  let t;
  try { t = rt.tokens.issue(agent, by, rotate); } catch (e) {
    if ((e as Error).message === "exists") throw new HttpError(409, "token_exists", "this agent already has a token; rotate it with POST /v1/hosted/agents/{agent}/token/rotate");
    throw e;
  }
  const pub = rt.cfg.publicApiUrl.replace(/\/$/, "");
  rt.runners.get(agent)?.note("token", rotate ? "agent token rotated (the old token and its OAuth grants no longer work)" : "agent token minted: an external AI may now drive this agent inside its fixed limits", { ok: true });
  return {
    agent, token: t.token, token_id: t.token_id, scopes: TOKEN_SCOPES, never: [...NEVER_SCOPES], created_ms: t.created_ms, shown_once: true,
    mcp_url: `${pub}/mcp`, api_base: `${pub}/v1`,
    note: "Whoever holds this token steers the agent's decisions (posts, DMs, hires, trades inside its fixed limits, paid inference). It can never withdraw the agent's money, export its key, change its limits, pause or delete it. Keep it secret; rotate it if it leaks.",
  };
}

function bearer(h: IncomingMessage["headers"]): string {
  const a = String(h["authorization"] ?? "");
  const m = /^Bearer\s+(\S+)$/i.exec(a);
  return m ? m[1] : "";
}

function driveOf(rt: Runtime, agent: string): Drive {
  const r = rt.runners.get(agent);
  if (!r) throw new HttpError(404, "not_found", "this agent is not hosted here");
  let d = drives.get(r);
  if (!d) { d = new Drive(r); drives.set(r, d); }
  return d;
}

export async function connectRoute(rt: Runtime, method: string, path: string, h: IncomingMessage["headers"], body: string, json: () => any): Promise<{ status: number; body: unknown } | null> {
  const tm = /^\/v1\/hosted\/agents\/([1-9A-HJ-NP-Za-km-z]{32,44}|Cx[0-9a-fA-F]{64})\/token(\/rotate)?$/.exec(path);
  if (tm) {
    const agent = cadreAddr(tm[1], "agent")!;
    if (!tm[2] && method === "GET") { if (!rt.entries.has(agent)) throw new HttpError(404, "not_found", "this agent is not hosted here"); return { status: 200, body: rt.tokens.status(agent) }; }
    if (method !== "POST") throw new HttpError(405, "method_not_allowed", `${method} not allowed`);
    if (!tm[2]) {
      const op = rt.verifyAuth(method, path, h, body);
      creatorCheck(rt, agent, op);
      return { status: 201, body: mintToken(rt, agent, op, false) };
    }
    const b = bearer(h);
    if (b) {
      if (rt.tokens.agentOfToken(b) !== agent) throw new HttpError(401, "bad_token", "the bearer is not this agent's current token");
      return { status: 200, body: mintToken(rt, agent, "token", true) };
    }
    const op = rt.verifyAuth(method, path, h, body);
    creatorCheck(rt, agent, op);
    return { status: 200, body: mintToken(rt, agent, op, true) };
  }
  if (path === "/v1/hosted/oauth/grant" && method === "POST") {
    // the consent page's approval: proof is the agent token, or the creator's signature (then a token is minted behind the scenes if none exists, never shown)
    const b = json();
    let agent: string;
    if (typeof b.agent_token === "string" && b.agent_token) {
      const a = rt.tokens.agentOfToken(b.agent_token);
      if (!a) throw new HttpError(401, "bad_token", "that agent token is not valid (rotated, or mistyped)");
      agent = a;
    } else {
      const op = rt.verifyAuth(method, path, h, body);
      agent = cadreAddr(String(b.agent ?? ""), "agent")!;
      creatorCheck(rt, agent, op);
      if (!rt.tokens.has(agent)) rt.tokens.issue(agent, op, false);
    }
    if (!rt.entries.has(agent)) throw new HttpError(404, "not_found", "this agent is not hosted here");
    const g = rt.tokens.grant(agent, String(b.client_id ?? ""), String(b.client_name ?? ""));
    rt.runners.get(agent)?.note("token", `OAuth grant to ${String(b.client_name ?? "an MCP client").slice(0, 60)}`, { ok: true });
    return { status: 201, body: { ...g, scopes: TOKEN_SCOPES, handle: rt.entries.get(agent)!.doc.spec.handle } };
  }
  if (path === "/v1/hosted/oauth/refresh" && method === "POST") {
    const b = json();
    const g = rt.tokens.refresh(String(b.refresh_token ?? ""), b.client_id ? String(b.client_id) : undefined);
    if (!g) throw new HttpError(400, "invalid_grant", "the refresh token is not valid (revoked, expired, rotated or used already)");
    return { status: 200, body: { ...g, scopes: TOKEN_SCOPES } };
  }
  if (path === "/v1/hosted/oauth/revoke" && method === "POST") {
    const b = json();
    return { status: 200, body: { revoked: rt.tokens.revoke(String(b.token ?? "")) } };
  }
  const dm = /^\/v1\/hosted\/drive(?:\/([a-z_]{3,48}))?$/.exec(path);
  if (dm) {
    const who = rt.tokens.resolve(bearer(h));
    if (!who) throw new HttpError(401, "unauthorized", "send Authorization: Bearer <agent token or OAuth access token>");
    if (!rt.entries.has(who.agent)) throw new HttpError(404, "not_found", "this agent is no longer hosted here");
    const d = driveOf(rt, who.agent);
    if (!dm[1] && method === "GET") return { status: 200, body: { agent: who.agent, handle: d.spec.handle, via: who.kind, tools: d.tools() } };
    if (dm[1] && method === "POST") {
      if (existsSync(rt.cfg.killFile)) throw new HttpError(409, "halted", "the hosted runtime's kill switch is on");
      try {
        const result = await d.call(dm[1], json(), { agent: who.agent, via: who.kind, token_id: who.token_id, grant_id: who.grant_id });
        return { status: 200, body: { tool: dm[1], ok: true, result } };
      } catch (e) {
        if (e instanceof DriveError) throw new HttpError(e.status, e.code, e.message);
        throw new HttpError(502, "tool_failed", String((e as Error).message ?? e).slice(0, 400));
      }
    }
    throw new HttpError(405, "method_not_allowed", `${method} not allowed`);
  }
  return null;
}

