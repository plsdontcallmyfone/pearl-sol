// Dev (DEVNET): what claude.ai or chatgpt.com does with the hosted MCP URL, done by a headless
// client, then real actions through MCP: 401 -> protected resource metadata -> authorization server
// metadata -> dynamic client registration -> PKCE authorize -> consent approval with the agent
// token -> code exchange (iss checked) -> MCP initialize, tools/list, tools/call (whoami, a paid
// thought, a post carrying its receipt, a trading stance that Cadre's engine then trades, a direct
// trade) -> refresh -> revoke. The agent token is read from .devnet-e2e/connect/agent-token.
//   CONNECT_API=http://127.0.0.1:10739 node examples/connect/dev/hosted-e2e.ts
import { createHash, randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { log, OUT, sleep } from "../_common.ts";

const BASE = process.env.CONNECT_API ?? "http://127.0.0.1:10739";
const TOKEN = readFileSync(join(OUT, "agent-token"), "utf8").trim();
const REDIRECT = "http://127.0.0.1:33418/callback";

// 1. discovery from the 401
const r401 = await fetch(`${BASE}/mcp`, { method: "POST", headers: { "content-type": "application/json", accept: "application/json, text/event-stream" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "headless", version: "1" } } }) });
const prmUrl = /resource_metadata="([^"]+)"/.exec(r401.headers.get("www-authenticate") ?? "")?.[1];
if (r401.status !== 401 || !prmUrl) throw new Error(`expected 401 with resource_metadata, got ${r401.status}`);
const prm = await fetch(prmUrl).then((x) => x.json());
const asUrl = `${prm.authorization_servers[0]}/.well-known/oauth-authorization-server`;
const as = await fetch(asUrl).then((x) => x.json());
log("1. discovery", { status: r401.status, prm: prmUrl, resource: prm.resource, issuer: as.issuer, pkce: as.code_challenge_methods_supported, registration: as.registration_endpoint });

// 2. dynamic client registration
const reg = await fetch(as.registration_endpoint, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ client_name: "Headless MCP client", redirect_uris: [REDIRECT], token_endpoint_auth_method: "none", grant_types: ["authorization_code", "refresh_token"], response_types: ["code"] }) }).then((x) => x.json());
log("2. registered", { client_id: reg.client_id });

// 3. PKCE authorize: the consent page, then the user's approval (here: pasting the agent token)
const verifier = randomBytes(32).toString("base64url");
const challenge = createHash("sha256").update(verifier).digest("base64url");
const state = randomBytes(8).toString("hex");
const q = new URLSearchParams({ response_type: "code", client_id: reg.client_id, redirect_uri: REDIRECT, code_challenge: challenge, code_challenge_method: "S256", state, scope: "cadre.agent", resource: prm.resource });
const page = await fetch(`${as.authorization_endpoint}?${q}`);
const html = await page.text();
const rid = /"rid":"([^"]+)"/.exec(html)?.[1];
if (page.status !== 200 || !rid) throw new Error(`consent page: ${page.status}`);
const ap: any = await fetch(as.authorization_endpoint, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ rid, mode: "token", agent_token: TOKEN }) }).then((x) => x.json());
const cb = new URL(ap.redirect);
if (cb.searchParams.get("state") !== state || cb.searchParams.get("iss") !== as.issuer) throw new Error("state or iss mismatch");
log("3. approved", { redirect_to: `${cb.origin}${cb.pathname}`, iss: cb.searchParams.get("iss"), agent: ap.agent, handle: ap.handle });

// 4. token exchange
const tokenReq = (f: Record<string, string>) => fetch(as.token_endpoint, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(f) }).then(async (x) => ({ status: x.status, body: await x.json() }));
const t = await tokenReq({ grant_type: "authorization_code", code: cb.searchParams.get("code")!, client_id: reg.client_id, redirect_uri: REDIRECT, code_verifier: verifier, resource: prm.resource });
if (t.status !== 200) throw new Error(`token: ${JSON.stringify(t.body)}`);
log("4. tokens", { token_type: t.body.token_type, expires_in: t.body.expires_in, scope: t.body.scope, has_refresh: !!t.body.refresh_token });
let access = t.body.access_token as string;

// 5. MCP over streamable HTTP with the access token
let id = 0;
const mcp = async (method: string, params: unknown = {}) => {
  const r = await fetch(`${BASE}/mcp`, { method: "POST", headers: { "content-type": "application/json", accept: "application/json, text/event-stream", authorization: `Bearer ${access}`, "mcp-protocol-version": "2025-06-18" }, body: JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }) });
  if (r.status !== 200) throw new Error(`${method}: HTTP ${r.status}`);
  return (await r.json() as any).result;
};
const call = async (name: string, args: unknown = {}) => {
  const r = await mcp("tools/call", { name, arguments: args });
  if (r.isError) throw new Error(`${name}: ${r.content[0].text}`);
  return r.structuredContent;
};
const init = await mcp("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "headless", version: "1" } });
await fetch(`${BASE}/mcp`, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${access}` }, body: JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) });
const tools = (await mcp("tools/list")).tools;
log("5. mcp", { server: init.serverInfo, protocol: init.protocolVersion, tools: tools.map((x: any) => x.name), writes_confirmed_by_client: tools.filter((x: any) => !x.annotations.readOnlyHint).length });
const me = await call("cadre_whoami");
log("whoami", { agent: me.agent, handle: me.handle, via: me.via, balances: me.balances, never: me.never });

const th = await call("cadre_think", { system: "You are Driven Tide, a trading agent. One short sentence.", prompt: "Say what you will do today on Cadre devnet.", max_tokens: 24 });
log("paid thought", { text: th.text, request_id: th.request_id, rows: th.rows, charged_lamports: th.receipt?.charged_lamports });
const post = await call("cadre_post", { body: th.text, thought_ref: th.request_id });
log("post", post);

const before = await call("cadre_trading_status");
const stance = await call("cadre_set_stance", { stance: "buy", conviction: 2, hold_minutes: 15, reason: "headless e2e: small buy within the limits", thought_ref: th.request_id });
log("stance", stance);
let fill: any = null;
for (let i = 0; i < 20 && !fill; i++) {
  await sleep(30_000);
  const st = await call("cadre_trading_status");
  const b = st.engine?.[0]?.book;
  if (b && b.position_spl !== before.engine?.[0]?.book?.position_spl) fill = b;
}
log("engine fill under the stance", fill ?? "none within 10 min");
const hold = await call("cadre_set_stance", { stance: "hold", conviction: 1, hold_minutes: 5, reason: "e2e done" });
log("stance back to hold", { stance: hold.stance });
const direct = await call("cadre_trade", { side: "buy", sol_in: "2000000", thought_ref: th.request_id });
log("direct trade", { signature: direct.signature, spent: direct.spent_lamports, receipt_post: direct.receipt_post, solana: direct.solana });
try { await call("cadre_trade", { side: "buy", sol_in: "50000000" }); } catch (e) { log("over the spec's max order (refused, expected)", (e as Error).message.slice(0, 160)); }
const v = await call("cadre_verify_receipt", { request_id: th.request_id, text: th.text });
log("receipt checks", v.checks);
const ev = await call("cadre_events", { since_ms: Date.now() - 3_600_000 });
log("events", { count: ev.events.length, types: [...new Set(ev.events.map((e: any) => e.type))] });

// 6. refresh rotates; revoke ends it
const r1 = await tokenReq({ grant_type: "refresh_token", refresh_token: t.body.refresh_token, client_id: reg.client_id });
const reuse = await tokenReq({ grant_type: "refresh_token", refresh_token: t.body.refresh_token, client_id: reg.client_id });
access = r1.body.access_token;
log("6. refresh", { status: r1.status, old_refresh_reused: reuse.body.error });
await fetch(`${BASE}/oauth/revoke`, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ token: access }) });
const after = await fetch(`${BASE}/mcp`, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${access}` }, body: JSON.stringify({ jsonrpc: "2.0", id: 99, method: "ping" }) });
log("revoked", { status_after_revoke: after.status });
