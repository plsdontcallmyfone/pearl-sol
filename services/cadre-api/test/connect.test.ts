// Cadre Connect routes (src/routes/connect.ts) against a fake hosted runtime: the remote MCP
// endpoint's 401 + discovery, the OAuth 2.1 round trip a web client (claude.ai, chatgpt.com) runs
// (protected resource metadata, authorization server metadata, dynamic registration, PKCE S256,
// consent approval with the agent token, code exchange, RFC 9207 iss, refresh, revoke), MCP
// initialize / tools/list / tools/call with the access token, the OpenAI alias with an agent
// token, and the agent-token pass-through.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";

const AGENT = "FiL2yraRKYadA9KL8Po7PnP5Lh7tGsGEcr1h5gJgcge";
const TOKEN = "cadre_at_" + "A".repeat(43);
let access = new Set<string>();
let refresh = new Map<string, string>();
const calls: { tool: string; args: any; auth: string }[] = [];
const urls: string[] = [];

function fakeRuntime(): Server {
  return createServer((req, res) => {
    let b = "";
    req.on("data", (c) => (b += c));
    req.on("end", () => {
      urls.push(String(req.url));
      const send = (s: number, j: unknown) => { res.writeHead(s, { "content-type": "application/json" }); res.end(JSON.stringify(j)); };
      const auth = String(req.headers.authorization ?? "").replace(/^Bearer /, "");
      const ok = auth === TOKEN || access.has(auth);
      const body = b ? JSON.parse(b) : {};
      if (req.url === "/v1/hosted/oauth/grant") {
        if (body.agent_token !== TOKEN) return send(401, { error: { code: "bad_token", message: "bad token" } });
        const a = "cadre_oa_" + randomBytes(24).toString("base64url"), r = "cadre_or_" + randomBytes(24).toString("base64url");
        access.add(a); refresh.set(r, a);
        return send(201, { access_token: a, refresh_token: r, expires_in: 3600, agent: AGENT, handle: "tide" });
      }
      if (req.url === "/v1/hosted/oauth/refresh") {
        if (!refresh.has(body.refresh_token)) return send(400, { error: { code: "invalid_grant", message: "no" } });
        refresh.delete(body.refresh_token);
        const a = "cadre_oa_" + randomBytes(24).toString("base64url"), r = "cadre_or_" + randomBytes(24).toString("base64url");
        access.add(a); refresh.set(r, a);
        return send(200, { access_token: a, refresh_token: r, expires_in: 3600, agent: AGENT });
      }
      if (req.url === "/v1/hosted/oauth/revoke") { access.delete(body.token); return send(200, { revoked: true }); }
      if (req.url === "/v1/hosted/drive") {
        if (!ok) return send(401, { error: { code: "unauthorized", message: "no" } });
        return send(200, { agent: AGENT, handle: "tide", via: "oauth", tools: [{ name: "cadre_whoami", allowed: true }, { name: "cadre_post", allowed: true }, { name: "cadre_think", allowed: true }, { name: "cadre_hire", allowed: false }] });
      }
      const m = /^\/v1\/hosted\/drive\/([a-z_]+)$/.exec(req.url ?? "");
      if (m) {
        if (!ok) return send(401, { error: { code: "unauthorized", message: "no" } });
        calls.push({ tool: m[1], args: body, auth });
        if (m[1] === "cadre_hire") return send(403, { error: { code: "not_allowed", message: "the agent's spec does not allow hire" } });
        if (m[1] === "cadre_think") return send(200, { tool: m[1], ok: true, result: { text: "Paris", request_id: "req_abc123", rows: 9, receipt: { ticket: "aa".repeat(32), charged_lamports: "55395", text_sha256: createHash("sha256").update("Paris").digest("hex"), usage: { prompt_tokens: 8, completion_tokens: 1, total_tokens: 9 } } } });
        return send(200, { tool: m[1], ok: true, result: { agent: AGENT, echo: body } });
      }
      const t = /^\/v1\/hosted\/agents\/([^/]+)\/token(\/rotate)?$/.exec(req.url ?? "");
      if (t) {
        if (req.method === "GET") return send(200, { agent: t[1], has_token: true });
        if (t[2] && auth === TOKEN) return send(200, { agent: t[1], token: "cadre_at_" + "B".repeat(43) });
        if (!t[2] && req.headers["x-cadre-signature"]) return send(201, { agent: t[1], token: TOKEN });
        return send(401, { error: { code: "unauthorized", message: "no" } });
      }
      send(404, { error: { code: "not_found", message: req.url } });
    });
  });
}

let rt: Server, srv: Server, dir: string, U = "";
const up = (s: Server) => new Promise<number>((r) => s.listen(0, "127.0.0.1", () => r((s.address() as any).port)));

before(async () => {
  dir = mkdtempSync(join(tmpdir(), "cadre-connect-"));
  rt = fakeRuntime();
  process.env.CADRE_AGENTS_URL = `http://127.0.0.1:${await up(rt)}`;
  const d = new Deps(configFromEnv({ NODE_RPC: "http://127.0.0.1:9", SOLANA_RPC: "http://127.0.0.1:9", GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off", CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000", CADRE_NETWORK: "devnet" }, { stateDir: dir }));
  const api = build(d);
  srv = api.listen(0, "127.0.0.1");
  await new Promise((r) => srv.once("listening", r));
  U = `http://127.0.0.1:${(srv.address() as any).port}`;
});
after(() => { srv.close(); rt.close(); rmSync(dir, { recursive: true, force: true }); delete process.env.CADRE_AGENTS_URL; });

const rpc = (id: number, method: string, params: unknown = {}) => ({ jsonrpc: "2.0", id, method, params });
async function mcp(auth: string | null, body: unknown) {
  const r = await fetch(`${U}/mcp`, { method: "POST", headers: { "content-type": "application/json", accept: "application/json, text/event-stream", ...(auth ? { authorization: `Bearer ${auth}` } : {}) }, body: JSON.stringify(body) });
  return { status: r.status, headers: r.headers, body: r.status === 202 ? null : await r.json() };
}

test("MCP without a credential: 401 with resource_metadata; metadata documents are consistent", async () => {
  const r = await mcp(null, rpc(1, "initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "t", version: "1" } }));
  assert.equal(r.status, 401);
  const wa = r.headers.get("www-authenticate")!;
  const prmUrl = /resource_metadata="([^"]+)"/.exec(wa)![1];
  assert.equal(prmUrl, `${U}/.well-known/oauth-protected-resource/mcp`);
  const prm = await fetch(prmUrl).then((x) => x.json());
  assert.equal(prm.resource, `${U}/mcp`);
  const as = await fetch(`${prm.authorization_servers[0]}/.well-known/oauth-authorization-server`).then((x) => x.json());
  assert.equal(as.issuer, prm.authorization_servers[0]);
  assert.deepEqual(as.code_challenge_methods_supported, ["S256"]);
  assert.ok(as.token_endpoint_auth_methods_supported.includes("none"));
  assert.equal(as.authorization_response_iss_parameter_supported, true);
  assert.equal(as.client_id_metadata_document_supported, true);
  assert.equal((await mcp("cadre_at_bogusbogusbogusbogusbogus", rpc(1, "ping"))).status, 401);
});

test("full OAuth round trip as claude.ai does it, then MCP tools with the access token", async () => {
  const redirect = "https://claude.ai/api/mcp/auth_callback";
  const reg = await fetch(`${U}/oauth/register`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ client_name: "Claude", redirect_uris: [redirect], token_endpoint_auth_method: "none", grant_types: ["authorization_code", "refresh_token"] }) });
  assert.equal(reg.status, 201);
  const { client_id } = await reg.json();
  const verifier = randomBytes(32).toString("base64url");
  const challenge = createHash("sha256").update(verifier).digest("base64url");
  const q = new URLSearchParams({ response_type: "code", client_id, redirect_uri: redirect, code_challenge: challenge, code_challenge_method: "S256", state: "st-1", scope: "cadre.agent", resource: `${U}/mcp` });
  // a bad redirect never redirects
  assert.equal((await fetch(`${U}/oauth/authorize?${new URLSearchParams({ ...Object.fromEntries(q), redirect_uri: "https://evil.example/cb" })}`)).status, 400);
  // no PKCE: redirected back with an error
  const noPkce = await fetch(`${U}/oauth/authorize?${new URLSearchParams({ ...Object.fromEntries(q), code_challenge_method: "plain" })}`, { redirect: "manual" });
  assert.equal(noPkce.status, 302);
  assert.match(noPkce.headers.get("location")!, /error=invalid_request/);
  const page = await fetch(`${U}/oauth/authorize?${q}`);
  assert.equal(page.status, 200);
  const htmlText = await page.text();
  assert.match(htmlText, /Let Claude drive one of your Cadre agents/);
  assert.match(htmlText, /It can never/);
  const rid = /"rid":"([^"]+)"/.exec(htmlText)![1];
  // a wrong token is refused, the right one approves
  assert.equal((await fetch(`${U}/oauth/authorize`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rid, mode: "token", agent_token: "cadre_at_" + "Z".repeat(43) }) })).status, 401);
  const ap = await fetch(`${U}/oauth/authorize`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rid, mode: "token", agent_token: TOKEN }) }).then((x) => x.json());
  const back = new URL(ap.redirect);
  assert.equal(back.origin + back.pathname, redirect);
  assert.equal(back.searchParams.get("state"), "st-1");
  assert.equal(back.searchParams.get("iss"), U);
  const code = back.searchParams.get("code")!;
  const tok = async (f: Record<string, string>) => { const r = await fetch(`${U}/oauth/token`, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(f) }); return { status: r.status, body: await r.json() }; };
  assert.equal((await tok({ grant_type: "authorization_code", code, client_id, redirect_uri: redirect, code_verifier: "x".repeat(43), resource: `${U}/mcp` })).body.error, "invalid_grant", "wrong verifier");
  // the failed attempt burned the code (one use); run the consent again
  const page2 = await fetch(`${U}/oauth/authorize?${q}`).then((x) => x.text());
  const rid2 = /"rid":"([^"]+)"/.exec(page2)![1];
  const ap2 = await fetch(`${U}/oauth/authorize`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rid: rid2, mode: "token", agent_token: TOKEN }) }).then((x) => x.json());
  const code2 = new URL(ap2.redirect).searchParams.get("code")!;
  const t = await tok({ grant_type: "authorization_code", code: code2, client_id, redirect_uri: redirect, code_verifier: verifier, resource: `${U}/mcp` });
  assert.equal(t.status, 200, JSON.stringify(t.body));
  assert.equal(t.body.token_type, "Bearer");
  assert.match(t.body.access_token, /^cadre_oa_/);
  assert.equal((await tok({ grant_type: "authorization_code", code: code2, client_id, redirect_uri: redirect, code_verifier: verifier })).body.error, "invalid_grant", "code reuse");

  // MCP with the access token
  const init = await mcp(t.body.access_token, rpc(1, "initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "claude-ai", version: "0.1" } }));
  assert.equal(init.status, 200);
  assert.equal(init.body.result.protocolVersion, "2025-06-18");
  assert.equal((await mcp(t.body.access_token, { jsonrpc: "2.0", method: "notifications/initialized" })).status, 202);
  const list = await mcp(t.body.access_token, rpc(2, "tools/list"));
  const names = list.body.result.tools.map((x: any) => x.name);
  assert.deepEqual(names.sort(), ["cadre_post", "cadre_think", "cadre_whoami"], "only what the spec allows");
  const post = list.body.result.tools.find((x: any) => x.name === "cadre_post");
  assert.equal(post.annotations.readOnlyHint, false);
  assert.equal(list.body.result.tools.find((x: any) => x.name === "cadre_whoami").annotations.readOnlyHint, true);
  const call = await mcp(t.body.access_token, rpc(3, "tools/call", { name: "cadre_post", arguments: { body: "hello" } }));
  assert.equal(call.body.result.isError, false);
  assert.deepEqual(call.body.result.structuredContent.echo, { body: "hello" });
  assert.equal(calls.at(-1)!.auth, t.body.access_token);
  const refused = await mcp(t.body.access_token, rpc(4, "tools/call", { name: "cadre_hire", arguments: { offer: 1 } }));
  assert.equal(refused.body.result.isError, true);
  assert.match(refused.body.result.content[0].text, /not_allowed/);

  // refresh rotates; revoke
  const r1 = await tok({ grant_type: "refresh_token", refresh_token: t.body.refresh_token, client_id });
  assert.equal(r1.status, 200);
  assert.equal((await tok({ grant_type: "refresh_token", refresh_token: t.body.refresh_token, client_id })).body.error, "invalid_grant");
  await fetch(`${U}/oauth/revoke`, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ token: r1.body.access_token }) });
  assert.equal((await mcp(r1.body.access_token, rpc(5, "ping"))).status, 401);
});

test("static bearer: the agent token works on MCP and REST directly", async () => {
  const r = await mcp(TOKEN, [rpc(1, "ping"), rpc(2, "tools/list")]);
  assert.equal(r.status, 200);
  assert.equal(r.body.length, 2);
  const rest = await fetch(`${U}/v1/agent/tools/cadre_whoami`, { method: "POST", headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" }, body: "{}" }).then((x) => x.json());
  assert.equal(rest.result.agent, AGENT);
  assert.equal((await fetch(`${U}/v1/agent`)).status, 401);
});

test("OpenAI alias: an agent token pays from the agent's balance; receipt in headers and body", async () => {
  const r = await fetch(`${U}/v1/chat/completions`, { method: "POST", headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" }, body: JSON.stringify({ model: "llama-3.2-1b-instruct-w7a7", messages: [{ role: "user", content: "Capital of France?" }], max_tokens: 4 }) });
  assert.equal(r.status, 200);
  assert.equal(r.headers.get("x-cadre-request"), "req_abc123");
  const j = await r.json();
  assert.equal(j.object, "chat.completion");
  assert.equal(j.choices[0].message.content, "Paris");
  assert.equal(j.cadre.request_id, "req_abc123");
  assert.equal(j.cadre.text_sha256, createHash("sha256").update("Paris").digest("hex"));
  assert.deepEqual(calls.at(-1)!.args.messages, [{ role: "user", content: "Capital of France?" }]);
  const s = await fetch(`${U}/v1/chat/completions`, { method: "POST", headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" }, body: JSON.stringify({ messages: [{ role: "user", content: "x" }], stream: true }) }).then((x) => x.text());
  assert.match(s, /chat\.completion\.chunk/);
  assert.match(s, /data: \[DONE\]/);
  // any other credential goes to the gateway (off here: 501)
  assert.equal((await fetch(`${U}/v1/chat/completions`, { method: "POST", headers: { authorization: "Bearer cadre_sk_abc", "content-type": "application/json" }, body: "{}" })).status, 501);
});

test("token routes pass through with Authorization (rotate) and signatures (mint)", async () => {
  const rot = await fetch(`${U}/v1/hosted/agents/${AGENT}/token/rotate`, { method: "POST", headers: { authorization: `Bearer ${TOKEN}` } });
  assert.equal(rot.status, 200);
  const mint = await fetch(`${U}/v1/hosted/agents/${AGENT}/token`, { method: "POST", headers: { "x-cadre-wallet": AGENT, "x-cadre-timestamp": "1", "x-cadre-signature": "00".repeat(64) }, body: "{}" });
  assert.equal(mint.status, 201);
  assert.equal((await fetch(`${U}/v1/hosted/agents/${AGENT}/token`).then((x) => x.json())).has_token, true);
});

// security review 2026-10-23: the agent param was decoded and pasted into the runtime path, so
// %2F..%2F reached any other runtime route (with the caller's signed headers or body)
test("token routes: an encoded agent param cannot leave /v1/hosted/agents/<agent>/token", async () => {
  urls.length = 0;
  await fetch(`${U}/v1/hosted/agents/..%2F..%2Fdrive%2Fcadre_post%3F/token`);
  await fetch(`${U}/v1/hosted/agents/..%2F..%2Foauth%2Fgrant%3F/token`, { method: "POST", headers: { "content-type": "application/json" }, body: "{}" });
  assert.ok(urls.length >= 1);
  for (const u of urls) assert.match(u, /^\/v1\/hosted\/agents\/[^/]+\/token$/, u);
});
