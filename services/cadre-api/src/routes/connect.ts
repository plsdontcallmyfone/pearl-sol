// Cadre Connect (docs/CONNECT.md): how an existing AI starts using Cadre at once.
//
// Hosted mode (an agent token or an OAuth access token drives one hosted agent; the hosted runtime
// services/swarm/agents holds the key and checks the spec's limits before it signs):
//   GET|POST /v1/hosted/agents/{agent}/token, POST .../token/rotate   token status / mint / rotate (runtime pass-through with Authorization)
//   GET /v1/agent, GET /v1/agent/tools, POST /v1/agent/tools/{name}    REST over the runtime's drive tools
//   POST /mcp (alias /v1/mcp)                                          remote MCP, streamable HTTP, JSON replies
//   /.well-known/oauth-protected-resource[/mcp], /.well-known/oauth-authorization-server,
//   /oauth/register, /oauth/authorize (consent page), /oauth/token, /oauth/revoke   OAuth 2.1 for web clients (claude.ai, chatgpt.com)
// Inference: POST /v1/chat/completions, /v1/completions, GET /v1/models (OpenAI base_url <api>/v1):
//   an agent token or OAuth access token pays from that agent's balance through the runtime; any
//   other credential passes through to the gateway exactly like /v1/openai/*.
// A2A: GET /v1/agents/{account}/card and /a2a/{account}/.well-known/agent-card.json, POST /a2a/{account}.
//
// Reads of other cadre-api data go through this API's own HTTP routes (loopback), so internal
// changes in other route files do not break these.
import { ApiError, badRequest, reply, type Api, type Ctx } from "../http.ts";
import type { Deps } from "../deps.ts";
import { handleMessage, INSTRUCTIONS, ToolError, type Backend } from "../../../../packages/cadre-mcp/src/protocol.ts";
import { mcpTools, TOKEN_SCOPES } from "../../../../packages/cadre-mcp/src/catalog.ts";
import { OAuthStore, redirectMatches } from "../connect/oauth.ts";
import { consentPage, errorPage } from "../connect/consent.ts";

const TOKEN_RE = /^cadre_(at|oa)_[A-Za-z0-9_-]{20,}$/;
const MCP_SCOPE = "cadre.agent";

export function registerConnect(api: Api, d: Deps) {
  const oauth = new OAuthStore(`${d.cfg.stateDir}/connect`);
  const pending = new Map<string, { client_id: string; client_name: string; redirect_uri: string; state: string | null; challenge: string; resource: string | null; scope: string; exp: number }>();

  const runtimeBase = () => {
    const b = process.env.CADRE_AGENTS_URL;
    if (!b) throw new ApiError(501, "upstream_not_configured", "the hosted agent runtime is not configured on this deployment (CADRE_AGENTS_URL)");
    return b.replace(/\/$/, "");
  };
  async function runtime(method: string, path: string, o: { headers?: Record<string, string>; body?: string | Buffer; timeoutMs?: number } = {}): Promise<{ status: number; body: any }> {
    let r: Response;
    try {
      r = await fetch(runtimeBase() + path, { method, headers: { "content-type": "application/json", ...(o.headers ?? {}) }, body: o.body && o.body.length ? o.body : method === "POST" ? "{}" : undefined, signal: AbortSignal.timeout(o.timeoutMs ?? 120_000) });
    } catch (e) {
      throw new ApiError(503, "agents_unavailable", `hosted agent runtime unreachable: ${(e as Error).message.slice(0, 120)}`);
    }
    const t = await r.text();
    let j: any;
    try { j = JSON.parse(t); } catch { j = { raw: t.slice(0, 300) }; }
    return { status: r.status, body: j };
  }
  const must = (r: { status: number; body: any }) => {
    if (r.status >= 400) throw new ApiError(r.status, r.body?.error?.code ?? "agents_error", r.body?.error?.message ?? `runtime answered ${r.status}`);
    return r.body;
  };
  const selfBase = () => process.env.CADRE_SELF_URL ?? `http://127.0.0.1:${d.cfg.port}`;
  async function self(path: string): Promise<any> {
    const r = await fetch(selfBase() + path, { headers: { accept: "application/json" }, signal: AbortSignal.timeout(30_000) });
    if (!r.ok) return null;
    return r.json();
  }

  /** The public base for this request (issuer, resource): the Host it came in on (https behind caddy), else CADRE_PUBLIC_URL. */
  function base(c: Ctx): string {
    const host = String(c.headers["x-forwarded-host"] ?? c.headers.host ?? "").split(",")[0].trim();
    if (host && /^[a-z0-9.:-]+$/i.test(host)) {
      const local = /^(127\.0\.0\.1|localhost)(:\d+)?$/.test(host);
      const proto = String(c.headers["x-forwarded-proto"] ?? "").split(",")[0].trim() || (local ? "http" : "https");
      return `${proto}://${host}`;
    }
    return (d.cfg.publicUrl ?? selfBase()).replace(/\/$/, "");
  }
  const bearerOf = (c: Ctx) => /^Bearer\s+(\S+)$/i.exec(String(c.headers.authorization ?? ""))?.[1] ?? "";
  const authHeader = (tok: string) => ({ authorization: `Bearer ${tok}` });
  const signedHeaders = (c: Ctx) => {
    const h: Record<string, string> = {};
    for (const k of ["x-cadre-wallet", "x-cadre-timestamp", "x-cadre-signature"]) if (typeof c.headers[k] === "string") h[k] = c.headers[k] as string;
    return h;
  };

  // ------------------------------------------------------------ agent tokens (runtime pass-through)

  api.get("/v1/hosted/agents/:agent/token", async (c) => reply({ body: must(await runtime("GET", `/v1/hosted/agents/${encodeURIComponent(c.params.agent)}/token`)), cache: "no-store" }), { cache: "no-store" });
  api.post("/v1/hosted/agents/:agent/token", async (c) => {
    const r = await runtime("POST", `/v1/hosted/agents/${encodeURIComponent(c.params.agent)}/token`, { headers: signedHeaders(c), body: await c.raw() });
    return reply({ status: r.status, body: must(r), cache: "no-store" });
  }, { cost: 10 });
  api.post("/v1/hosted/agents/:agent/token/rotate", async (c) => {
    const tok = bearerOf(c);
    const r = await runtime("POST", `/v1/hosted/agents/${encodeURIComponent(c.params.agent)}/token/rotate`, { headers: tok ? authHeader(tok) : signedHeaders(c), body: await c.raw() });
    return reply({ status: r.status, body: must(r), cache: "no-store" });
  }, { cost: 10 });

  // ------------------------------------------------------------ hosted REST (the same tools as MCP)

  const needBearer = (c: Ctx) => {
    const t = bearerOf(c);
    if (!TOKEN_RE.test(t)) throw new ApiError(401, "unauthorized", "send Authorization: Bearer <agent token (cadre_at_...) or OAuth access token>");
    return t;
  };
  api.get("/v1/agent", async (c) => reply({ body: must(await runtime("POST", "/v1/hosted/drive/cadre_whoami", { headers: authHeader(needBearer(c)) })).result, cache: "no-store" }), { cache: "no-store" });
  api.get("/v1/agent/tools", async (c) => reply({ body: must(await runtime("GET", "/v1/hosted/drive", { headers: authHeader(needBearer(c)) })), cache: "no-store" }), { cache: "no-store" });
  api.post("/v1/agent/tools/:name", async (c) => {
    const tok = needBearer(c);
    if (!/^[a-z_]{3,48}$/.test(c.params.name)) throw badRequest("tool name");
    const r = await runtime("POST", `/v1/hosted/drive/${c.params.name}`, { headers: authHeader(tok), body: await c.raw(), timeoutMs: 900_000 });
    return reply({ status: r.status, body: must(r), cache: "no-store" });
  }, { cost: 5 });

  // ------------------------------------------------------------ remote MCP

  const unauthorized = (c: Ctx, invalid: boolean) => reply({
    status: 401,
    body: { error: { code: invalid ? "invalid_token" : "unauthorized", message: "this MCP server needs an agent token (Authorization: Bearer cadre_at_...) or OAuth sign-in" } },
    headers: { "www-authenticate": `Bearer realm="cadre", resource_metadata="${base(c)}/.well-known/oauth-protected-resource/mcp", scope="${MCP_SCOPE}"${invalid ? ', error="invalid_token"' : ""}` },
    cache: "no-store",
  });

  const mcp = async (c: Ctx) => {
    const tok = bearerOf(c);
    if (!tok) return unauthorized(c, false);
    if (!TOKEN_RE.test(tok)) return unauthorized(c, true);
    // validate the credential once per request (cheap: the runtime holds hashes in memory)
    const who = await runtime("GET", "/v1/hosted/drive", { headers: authHeader(tok) });
    if (who.status === 401) return unauthorized(c, true);
    must(who);
    let msg: unknown;
    try { msg = JSON.parse((await c.raw()).toString("utf8") || "null"); } catch { return reply({ status: 400, body: { jsonrpc: "2.0", id: null, error: { code: -32700, message: "parse error" } }, cache: "no-store" }); }
    const allowed = new Set<string>((who.body.tools ?? []).filter((t: any) => t.allowed).map((t: any) => t.name));
    const backend: Backend = {
      listTools: async () => mcpTools("hosted").filter((t) => allowed.has(t.name)),
      callTool: async (name, args) => {
        const r = await runtime("POST", `/v1/hosted/drive/${encodeURIComponent(name)}`, { headers: authHeader(tok), body: JSON.stringify(args ?? {}), timeoutMs: 900_000 });
        if (r.status >= 400) throw new ToolError(r.body?.error?.code ?? "error", r.body?.error?.message ?? `HTTP ${r.status}`);
        return r.body.result;
      },
    };
    const out = await handleMessage(msg, backend, { name: "cadre", title: `Cadre (@${who.body.handle ?? "agent"})`, version: "1.0.0", instructions: INSTRUCTIONS });
    if (out === null) return reply({ status: 202, text: { type: "text/plain", body: "" }, cache: "no-store" });
    return reply({ body: out, cache: "no-store" });
  };
  const mcpGet = () => reply({ status: 405, body: { error: { code: "method_not_allowed", message: "this MCP server sends no server-initiated stream; POST JSON-RPC to it" } }, headers: { allow: "POST" }, cache: "no-store" });
  for (const p of ["/mcp", "/v1/mcp"]) {
    api.post(p, mcp, { cost: 2 });
    api.get(p, mcpGet, { cache: "no-store" });
    api.delete(p, mcpGet);
  }

  // ------------------------------------------------------------ OAuth 2.1

  const prm = (c: Ctx) => ({
    resource: `${base(c)}/mcp`,
    authorization_servers: [base(c)],
    scopes_supported: [MCP_SCOPE],
    bearer_methods_supported: ["header"],
    resource_name: "Cadre agent (MCP)",
    resource_documentation: "https://github.com/plsdontcallmyfone/pearl-sol/blob/main/docs/CONNECT.md",
  });
  api.get("/.well-known/oauth-protected-resource", (c) => reply({ body: prm(c), cache: 300 }), { cache: 0 });
  api.get("/.well-known/oauth-protected-resource/mcp", (c) => reply({ body: prm(c), cache: 300 }), { cache: 0 });
  api.get("/.well-known/oauth-protected-resource/v1/mcp", (c) => reply({ body: { ...prm(c), resource: `${base(c)}/v1/mcp` }, cache: 300 }), { cache: 0 });
  const asMeta = (c: Ctx) => {
    const b = base(c);
    return {
      issuer: b,
      authorization_endpoint: `${b}/oauth/authorize`,
      token_endpoint: `${b}/oauth/token`,
      registration_endpoint: `${b}/oauth/register`,
      revocation_endpoint: `${b}/oauth/revoke`,
      response_types_supported: ["code"],
      response_modes_supported: ["query"],
      grant_types_supported: ["authorization_code", "refresh_token"],
      code_challenge_methods_supported: ["S256"],
      token_endpoint_auth_methods_supported: ["none"],
      revocation_endpoint_auth_methods_supported: ["none"],
      scopes_supported: [MCP_SCOPE, "offline_access"],
      client_id_metadata_document_supported: true,
      authorization_response_iss_parameter_supported: true,
      service_documentation: "https://github.com/plsdontcallmyfone/pearl-sol/blob/main/docs/CONNECT.md",
    };
  };
  api.get("/.well-known/oauth-authorization-server", (c) => reply({ body: asMeta(c), cache: 300 }), { cache: 0 });
  api.get("/.well-known/openid-configuration", (c) => reply({ body: asMeta(c), cache: 300 }), { cache: 0 });

  const oerr = (status: number, error: string, desc: string) => reply({ status, body: { error, error_description: desc }, cache: "no-store", headers: { pragma: "no-cache" } });

  api.post("/oauth/register", async (c) => {
    const r = oauth.register(await c.json());
    if (r.error) return oerr(400, r.error, r.error_description!);
    const cl = r.client!;
    return reply({ status: 201, body: { client_id: cl.client_id, client_id_issued_at: Math.floor(cl.created_ms / 1000), client_name: cl.client_name, redirect_uris: cl.redirect_uris, token_endpoint_auth_method: "none", grant_types: ["authorization_code", "refresh_token"], response_types: ["code"] }, cache: "no-store" });
  }, { cost: 5 });

  const html = (status: number, body: string) => reply({ status, text: { type: "text/html; charset=utf-8", body }, cache: "no-store", headers: { "x-frame-options": "DENY", "referrer-policy": "no-referrer" } });

  api.get("/oauth/authorize", async (c) => {
    const q = c.query;
    const client_id = q.get("client_id") ?? "";
    const redirect_uri = q.get("redirect_uri") ?? "";
    const { client, error } = await oauth.client(client_id);
    if (!client) return html(400, errorPage("Unknown app", error ?? "unknown client_id"));
    if (!redirect_uri || !redirectMatches(client.redirect_uris, redirect_uri)) return html(400, errorPage("Wrong return address", "redirect_uri is not registered for this app"));
    const back = (e: string, desc: string) => {
      const u = new URL(redirect_uri);
      u.searchParams.set("error", e);
      u.searchParams.set("error_description", desc);
      if (q.get("state")) u.searchParams.set("state", q.get("state")!);
      u.searchParams.set("iss", base(c));
      return reply({ status: 302, headers: { location: u.toString() }, text: { type: "text/plain", body: "" }, cache: "no-store" });
    };
    if (q.get("response_type") !== "code") return back("unsupported_response_type", "only response_type=code");
    const challenge = q.get("code_challenge") ?? "";
    if (q.get("code_challenge_method") !== "S256" || !/^[A-Za-z0-9_-]{43}$/.test(challenge)) return back("invalid_request", "PKCE with code_challenge_method=S256 is required");
    const resource = q.get("resource");
    if (resource && ![`${base(c)}/mcp`, `${base(c)}/v1/mcp`, base(c)].includes(resource.replace(/\/$/, ""))) return back("invalid_target", `resource must be ${base(c)}/mcp`);
    const rid = Buffer.from(crypto.getRandomValues(new Uint8Array(18))).toString("base64url");
    pending.set(rid, { client_id, client_name: client.client_name, redirect_uri, state: q.get("state"), challenge, resource, scope: q.get("scope") ?? MCP_SCOPE, exp: Date.now() + 900_000 });
    for (const [k, v] of pending) if (v.exp < Date.now()) pending.delete(k);
    return html(200, consentPage({ rid, clientName: client.client_name, redirectHost: new URL(redirect_uri).host, network: d.cfg.network, clientId: client_id }));
  }, { cache: "no-store" });

  api.post("/oauth/authorize", async (c) => {
    const b = await c.json();
    const p = pending.get(String(b.rid ?? ""));
    if (!p || p.exp < Date.now()) throw new ApiError(400, "expired", "this sign-in page expired; start the connection again from your app");
    let g: { status: number; body: any };
    if (b.mode === "token") {
      if (!/^cadre_at_/.test(String(b.agent_token ?? ""))) throw badRequest("agent_token starts with cadre_at_");
      g = await runtime("POST", "/v1/hosted/oauth/grant", { body: JSON.stringify({ agent_token: b.agent_token, client_id: p.client_id, client_name: p.client_name }) });
    } else if (b.mode === "wallet") {
      const inner = String(b.body ?? "");
      let j: any;
      try { j = JSON.parse(inner); } catch { throw badRequest("body is the signed JSON text"); }
      if (j.client_id !== p.client_id) throw badRequest("the signed body names another app");
      g = await runtime("POST", "/v1/hosted/oauth/grant", { headers: { "x-cadre-wallet": String(b.wallet ?? ""), "x-cadre-timestamp": String(b.timestamp ?? ""), "x-cadre-signature": String(b.signature ?? "") }, body: inner });
    } else throw badRequest("mode is token or wallet");
    const tokens = must(g);
    pending.delete(String(b.rid));
    const code = oauth.newCode({ client_id: p.client_id, redirect_uri: p.redirect_uri, challenge: p.challenge, resource: p.resource, scope: p.scope, tokens });
    const u = new URL(p.redirect_uri);
    u.searchParams.set("code", code);
    if (p.state !== null) u.searchParams.set("state", p.state);
    u.searchParams.set("iss", base(c));
    return { redirect: u.toString(), agent: tokens.agent, handle: tokens.handle ?? null };
  }, { cost: 10 });

  const formOf = async (c: Ctx): Promise<URLSearchParams> => {
    const raw = (await c.raw()).toString("utf8");
    const ct = String(c.headers["content-type"] ?? "");
    if (ct.includes("application/json")) {
      const j = raw ? JSON.parse(raw) : {};
      return new URLSearchParams(Object.entries(j).map(([k, v]) => [k, String(v)]));
    }
    return new URLSearchParams(raw);
  };

  api.post("/oauth/token", async (c) => {
    let f: URLSearchParams;
    try { f = await formOf(c); } catch { return oerr(400, "invalid_request", "body is form-encoded (or JSON)"); }
    const gt = f.get("grant_type");
    const tokenReply = (t: any, scope: string) => reply({ body: { access_token: t.access_token, token_type: "Bearer", expires_in: t.expires_in, refresh_token: t.refresh_token, scope }, cache: "no-store", headers: { pragma: "no-cache" } });
    if (gt === "authorization_code") {
      const r = oauth.takeCode(f.get("code") ?? "", f.get("client_id") ?? "", f.get("redirect_uri") ?? "", f.get("code_verifier") ?? "", f.get("resource"));
      if (r.error) return oerr(400, r.error, r.error_description!);
      return tokenReply(r.tokens, r.scope!);
    }
    if (gt === "refresh_token") {
      const r = await runtime("POST", "/v1/hosted/oauth/refresh", { body: JSON.stringify({ refresh_token: f.get("refresh_token") ?? "", client_id: f.get("client_id") ?? undefined }) });
      if (r.status >= 500) return oerr(503, "temporarily_unavailable", r.body?.error?.message ?? "runtime unavailable");
      if (r.status >= 400) return oerr(400, "invalid_grant", r.body?.error?.message ?? "invalid refresh token");
      return tokenReply(r.body, f.get("scope") ?? MCP_SCOPE);
    }
    return oerr(400, "unsupported_grant_type", "grant_type is authorization_code or refresh_token");
  }, { cost: 5 });

  api.post("/oauth/revoke", async (c) => {
    const f = await formOf(c).catch(() => new URLSearchParams());
    const t = f.get("token") ?? "";
    if (t) await runtime("POST", "/v1/hosted/oauth/revoke", { body: JSON.stringify({ token: t }) }).catch(() => null);
    return reply({ body: {}, cache: "no-store" });
  }, { cost: 5 });

  // ------------------------------------------------------------ OpenAI-compatible aliases (base_url <api>/v1)

  const toGateway = async (c: Ctx, path: string) => {
    const body = c.method === "POST" ? await c.raw() : undefined;
    const h: Record<string, string> = {};
    for (const k of ["content-type", "authorization", "x-payment", "x-cadre-wallet", "x-cadre-timestamp", "x-cadre-signature", "accept"]) if (typeof c.headers[k] === "string") h[k] = c.headers[k] as string;
    const r = await d.upstream("gateway", path, { method: c.method, headers: h, body: body && body.length ? body : undefined, timeoutMs: 1_800_000, idleMs: 600_000 });
    return reply({ upstream: r });
  };
  const chat = (kind: "chat" | "text") => async (c: Ctx) => {
    const tok = bearerOf(c);
    if (!TOKEN_RE.test(tok)) return toGateway(c, kind === "chat" ? "/v1/chat/completions" : "/v1/completions");
    const b = await c.json();
    const messages = kind === "chat" ? (Array.isArray(b.messages) ? b.messages.map((m: any) => ({ role: String(m.role), content: typeof m.content === "string" ? m.content : Array.isArray(m.content) ? m.content.map((p: any) => p?.text ?? "").join("") : "" })) : []) : [{ role: "user", content: String(b.prompt ?? "") }];
    const max = Number(b.max_completion_tokens ?? b.max_tokens ?? 64);
    const r = await runtime("POST", "/v1/hosted/drive/cadre_think", { headers: authHeader(tok), body: JSON.stringify({ messages, max_tokens: Math.min(256, Math.max(1, max)) }), timeoutMs: 900_000 });
    if (r.status === 401) throw new ApiError(401, "invalid_api_key", "the agent token or access token is not valid");
    const t = must(r).result;
    const usage = t.receipt?.usage ?? null;
    const finish = usage && Number(usage.completion_tokens) >= max ? "length" : "stop";
    const created = Math.floor(Date.now() / 1000);
    const model = String(b.model ?? "llama-3.2-1b-instruct-w7a7");
    const cadre = { request_id: t.request_id, ticket_id: t.receipt?.ticket ?? null, rows: t.rows, charged_lamports: t.receipt?.charged_lamports ?? null, text_sha256: t.receipt?.text_sha256 ?? null, status_url: `${base(c)}/v1/openai/requests/${t.request_id}`, decoding: "greedy", paid_by: "the agent's prepaid balance (agent token)" };
    const headers: Record<string, string> = { "x-cadre-request": t.request_id, "x-cadre-rows": String(t.rows), "x-cadre-receipt": cadre.status_url };
    if (cadre.ticket_id) headers["x-cadre-ticket"] = String(cadre.ticket_id);
    if (cadre.charged_lamports) headers["x-cadre-charged-lamports"] = String(cadre.charged_lamports);
    const id = `chatcmpl-${t.request_id}`;
    if (b.stream === true) {
      const chunk = (delta: any, fin: string | null, extra: any = {}) => `data: ${JSON.stringify({ id, object: kind === "chat" ? "chat.completion.chunk" : "text_completion", created, model, choices: [kind === "chat" ? { index: 0, delta, finish_reason: fin } : { index: 0, text: delta.content ?? "", finish_reason: fin }], ...extra })}\n\n`;
      const sse = chunk({ role: "assistant", content: t.text }, null) + chunk({}, finish, { usage, cadre }) + "data: [DONE]\n\n";
      return reply({ text: { type: "text/event-stream", body: sse }, headers, cache: "no-store" });
    }
    const choice = kind === "chat" ? { index: 0, message: { role: "assistant", content: t.text }, finish_reason: finish } : { index: 0, text: t.text, finish_reason: finish };
    return reply({ body: { id, object: kind === "chat" ? "chat.completion" : "text_completion", created, model, choices: [choice], usage, cadre }, headers, cache: "no-store" });
  };
  api.post("/v1/chat/completions", chat("chat"), { cost: 10 });
  api.post("/v1/completions", chat("text"), { cost: 10 });
  api.get("/v1/models", async (c) => {
    if (d.cfg.gatewayUrl) return toGateway(c, "/v1/models");
    // no gateway on this deployment: the public list of a peer cadre-api (CADRE_SELF_URL), else 501
    const m = process.env.CADRE_SELF_URL ? await self("/v1/openai/models") : null;
    if (!m) throw new ApiError(501, "upstream_not_configured", "gateway is not configured on this deployment");
    return reply({ body: m, cache: "no-store" });
  }, { cache: "no-store" });

  // ------------------------------------------------------------ A2A agent cards

  async function card(c: Ctx, account: string) {
    const [prof, offers, ua] = await Promise.all([
      self(`/v1/profiles/${encodeURIComponent(account)}`),
      self(`/v1/market/offers?provider=${encodeURIComponent(account)}`),
      self(`/v1/useragents/${encodeURIComponent(account)}`).catch(() => null),
    ]);
    if (!prof) throw new ApiError(404, "not_found", "no Cadre profile for that account");
    const b = base(c);
    const a = prof.account ?? account;
    const skills = (offers?.offers ?? []).map((o: any) => ({
      id: `offer-${o.id}`, name: String(o.title), description: `${o.title}. Price ${o.price} CADRE base units (9 decimals) plus the market fee ${o.fee ?? "?"}; turnaround ${o.turnaround_hint ?? `${o.turnaround} blocks`}. Paid into escrow on the Cadre chain, released only when the buyer (or the market judge) accepts.`,
      tags: [String(o.category ?? "service"), "cadre-market"], examples: [], inputModes: ["text/plain"], outputModes: ["text/plain"],
    }));
    return {
      protocolVersion: "0.3.0",
      name: prof.name ?? (prof.handle ? `@${prof.handle}` : a),
      description: String(prof.bio ?? prof.persona ?? "A Cadre agent").slice(0, 1000),
      url: `${b}/a2a/${a}`,
      preferredTransport: "JSONRPC",
      version: "1.0.0",
      provider: { organization: "Cadre (agent chain)", url: b },
      documentationUrl: "https://github.com/plsdontcallmyfone/pearl-sol/blob/main/docs/CONNECT.md",
      capabilities: { streaming: false, pushNotifications: false },
      defaultInputModes: ["text/plain"],
      defaultOutputModes: ["text/plain"],
      skills,
      cadre: {
        account: a, handle: prof.handle ?? null, is_agent: !!prof.is_agent, house: !!prof.house, links: prof.links ?? null, a2a_card: prof.a2a_card ?? null,
        driven_externally: ua?.hosted?.driven_externally ?? null, sovereign: ua?.sovereign ?? null,
        profile: `${b}/v1/profiles/${a}`, offers: `${b}/v1/market/offers?provider=${a}`,
        hire: "Hiring is a Cadre market contract call signed by the buyer's own Cadre key: POST /v1/tx/market {from, action: hire, offer, request_uri, request_hash, deadline_blocks, judge_fee, evaluators, threshold}, or the cadre_hire tool (MCP, SDK). The A2A endpoint answers with the offers and the exact steps; it cannot pay for you.",
      },
    };
  }
  api.get("/v1/agents/:account/card", async (c) => reply({ body: await card(c, c.params.account), cache: 30 }), { cache: 30 });
  api.get("/a2a/:account/.well-known/agent-card.json", async (c) => reply({ body: await card(c, c.params.account), cache: 30 }), { cache: 30 });
  api.get("/a2a/:account/.well-known/agent.json", async (c) => reply({ body: await card(c, c.params.account), cache: 30 }), { cache: 30 });
  api.post("/a2a/:account", async (c) => {
    let m: any;
    try { m = await c.json(); } catch { return { jsonrpc: "2.0", id: null, error: { code: -32700, message: "parse error" } }; }
    const id = m?.id ?? null;
    if (m?.method !== "message/send") return { jsonrpc: "2.0", id, error: { code: -32601, message: "this endpoint serves message/send only" } };
    const cd = await card(c, c.params.account);
    const lines = cd.skills.length
      ? [`${cd.name} sells ${cd.skills.length} service(s) on the Cadre market:`, ...cd.skills.map((s: any) => `- ${s.id}: ${s.description}`), "", cd.cadre.hire]
      : [`${cd.name} has no active offers on the Cadre market right now.`, `Profile: ${cd.cadre.profile}`];
    return { jsonrpc: "2.0", id, result: { kind: "message", role: "agent", messageId: `cadre-${Date.now()}`, parts: [{ kind: "text", text: lines.join("\n") }], contextId: m?.params?.message?.contextId ?? undefined } };
  }, { cost: 5 });

  // the scopes an agent token carries (for docs and clients)
  api.get("/v1/connect", (c) => ({
    mcp_url: `${base(c)}/mcp`, oauth: `${base(c)}/.well-known/oauth-authorization-server`, openai_base_url: `${base(c)}/v1`, rest: `${base(c)}/v1/agent/tools`,
    token_scopes: TOKEN_SCOPES, never: ["withdraw", "export_key", "change_limits", "pause", "delete", "transfer_out"],
    model: "llama-3.2-1b-instruct-w7a7: Llama 3.2 1B Instruct, int7 (W7A7), CPU, greedy decoding, Cadre devnet",
    docs: "docs/CONNECT.md",
  }), { cache: 60 });
}
