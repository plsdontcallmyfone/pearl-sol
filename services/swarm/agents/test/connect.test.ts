// Hosted mode (docs/CONNECT.md): agent tokens, OAuth grants and the token-scoped drive tools,
// against the same fakes as runtime.test.ts. Proves: the token is shown once and stored hashed;
// only the creator mints; rotate kills the old token and its grants; refresh rotates; revoke works;
// tools run only inside the spec (actions, tip limit, trade limit); no tool or route moves funds
// out, exports the key, pauses or deletes.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { CadreApi, NodeRpc, authMessage, latestBlockhash } from "../../src/clients.ts";
import { mulberry32 } from "../../src/swarm.ts";
import { walletFromSeed, type Wallet } from "../../src/wallet.ts";
import { GatewayAt } from "../src/gateway.ts";
import { Runtime } from "../src/runtime.ts";
import { seal } from "../src/seal.ts";
import { defaultSpec, specMessage, specSha256, type AgentSpec } from "../src/spec.ts";
import { PORTS, startFakes2, type Fakes2 } from "./fakes.ts";

const seedOf = (s: string) => createHash("sha256").update(s).digest();
const op = walletFromSeed(seedOf("cn-operator"));
const agentSeed = seedOf("cn-agent");
const ag = walletFromSeed(agentSeed);
const other = walletFromSeed(seedOf("cn-stranger"));
const U = (p: string) => `http://127.0.0.1:${PORTS.runtime}${p}`;

let f: Fakes2;
let rt: Runtime;
let server: any;
let dir: string;
let token = "";

function spec(over: Partial<AgentSpec> = {}): AgentSpec {
  return defaultSpec(ag.address, op.address, {
    handle: "pier_owl", name: "Pier Owl", persona: "An owl who watches the pier.", appearance: "a brown owl", topics: ["boats"], created_ms: 1,
    schedule: { cadence_min: 600, quiet_utc: null }, budget: { sol_lamports_per_day: "60000000", cadre_units_per_day: "500000", rows_per_day: 20000 },
    actions: { post: true, reply: true, react: true, follow: true, tip: false, trade: true, memo: false, hire: false, mine: false, browse: false },
    limits: { tip_units: "0", trade_lamports: "10000000" }, ...over,
  });
}
function sign(s: AgentSpec, w: Wallet = op) {
  const sha256 = specSha256(s);
  return { spec: s, sha256, sig: Buffer.from(w.sign(Buffer.from(specMessage(s, sha256)))).toString("hex") };
}
async function signed(method: string, path: string, body: unknown, w: Wallet = op) {
  const b = body === undefined ? "" : JSON.stringify(body);
  const ts = Date.now();
  const headers = { "content-type": "application/json", "x-cadre-wallet": w.address, "x-cadre-timestamp": String(ts), "x-cadre-signature": Buffer.from(w.sign(authMessage(method, path, ts, b))).toString("hex") };
  const r = await fetch(U(path), { method, headers, body: b });
  return { status: r.status, body: await r.json() };
}
async function bearer(method: string, path: string, tok: string, body?: unknown) {
  const r = await fetch(U(path), { method, headers: { "content-type": "application/json", authorization: `Bearer ${tok}` }, body: body === undefined ? undefined : JSON.stringify(body) });
  return { status: r.status, body: await r.json() };
}
const tool = (name: string, args: unknown, tok = token) => bearer("POST", `/v1/hosted/drive/${name}`, tok, args);

before(async () => {
  f = await startFakes2(() => "The owl counted four boats.");
  dir = mkdtempSync(join(tmpdir(), "agents-cn-"));
  let t = 1_792_500_000_000;
  rt = new Runtime({
    stateDir: dir, masterKeyFile: join(dir, "master.hex"), nodeRpc: `http://127.0.0.1:${PORTS.node}`, gatewayUrl: `http://127.0.0.1:${PORTS.gateway}`,
    apiUrl: `http://127.0.0.1:${PORTS.api}`, publicApiUrl: "https://api.example", solRpc: `http://127.0.0.1:${PORTS.sol}`, network: "test",
    allowHosts: [], maxPerOperator: 2, maxAgents: 10, topupRows: 4096, minFreeRows: 256, tickMs: 10, killFile: join(dir, "STOP"),
  }, (w) => ({
    node: new NodeRpc(`http://127.0.0.1:${PORTS.node}`), gateway: new GatewayAt(`http://127.0.0.1:${PORTS.gateway}`, w), api: new CadreApi(`http://127.0.0.1:${PORTS.api}`),
    latestBlockhash, now: () => (t += 61_000), rand: mulberry32(5), log: () => {},
  }));
  server = rt.listen(PORTS.runtime);
});
after(async () => { server.close(); await f.close(); });

test("create with agent_token: true returns the token once; it is stored hashed", async () => {
  const info = await fetch(U("/v1/hosted")).then((x) => x.json());
  const r = await signed("POST", "/v1/hosted/agents", { sealed: seal(agentSeed, Buffer.from(info.seal_pub, "base64")), spec: sign(spec()), agent_token: true });
  assert.equal(r.status, 201, JSON.stringify(r.body));
  const at = r.body.agent_token;
  assert.match(at.token, /^cadre_at_[A-Za-z0-9_-]{43}$/);
  assert.equal(at.agent, ag.address);
  assert.equal(at.mcp_url, "https://api.example/mcp");
  assert.ok(at.never.includes("withdraw") && at.never.includes("export_key") && at.never.includes("change_limits"));
  assert.ok(!at.scopes.includes("withdraw"));
  token = at.token;
  const disk = readFileSync(join(dir, "tokens.json"), "utf8");
  assert.ok(!disk.includes(token), "plaintext token never on disk");
  assert.ok(disk.includes(createHash("sha256").update(token).digest("hex")));
  // public status, no secret
  const st = await fetch(U(`/v1/hosted/agents/${ag.address}/token`)).then((x) => x.json());
  assert.equal(st.has_token, true);
  assert.ok(!JSON.stringify(st).includes(token));
  assert.equal(rt.status(ag.address).driven_externally, true);
  // a second mint is refused; a stranger cannot mint
  assert.equal((await signed("POST", `/v1/hosted/agents/${ag.address}/token`, {})).body.error.code, "token_exists");
  assert.equal((await signed("POST", `/v1/hosted/agents/${ag.address}/token`, {}, other)).body.error.code, "not_creator");
});

test("bring the agent up (funds, profile, link) so the drive tools sign real txs", async () => {
  f.node.balances.set(ag.address, 400_000n);
  f.sol.lamports.set(ag.address, 200_000_000n);
  f.gw.rowsFree = 0;
  for (let i = 0; i < 8; i++) await rt.loopOnce();
  f.node.links.add(`${op.address}:${ag.address}`);
  for (let i = 0; i < 8; i++) await rt.loopOnce();
  assert.equal(rt.status(ag.address).state, "running", JSON.stringify(rt.status(ag.address)));
});

test("drive: tools list says what the spec allows; raw and money-out tools are absent", async () => {
  assert.equal((await bearer("GET", "/v1/hosted/drive", "cadre_at_nope")).status, 401);
  const r = await bearer("GET", "/v1/hosted/drive", token);
  assert.equal(r.status, 200);
  const names = r.body.tools.map((t: any) => t.name);
  for (const n of ["cadre_transfer", "cadre_send_cadre_tx", "cadre_send_solana_tx"]) assert.ok(!names.includes(n), `${n} is not a hosted tool`);
  assert.equal(r.body.tools.find((t: any) => t.name === "cadre_hire").allowed, false);
  assert.equal(r.body.tools.find((t: any) => t.name === "cadre_post").allowed, true);
  assert.equal((await tool("cadre_transfer", { to: other.address, amount: "1" })).body.error.code, "not_allowed");
  assert.equal((await tool("cadre_strategy_commit", { text: "x" })).status, 501);
});

test("drive: think, then post with that thought_ref; tips and hires refused by the spec", async () => {
  const th = await tool("cadre_think", { prompt: "Say something about boats.", max_tokens: 16 });
  assert.equal(th.status, 200, JSON.stringify(th.body));
  assert.match(th.body.result.request_id, /^req_/);
  const p = await tool("cadre_post", { body: th.body.result.text, thought_ref: th.body.result.request_id });
  assert.equal(p.status, 200, JSON.stringify(p.body));
  const mine = f.node.posts.filter((x) => x.author === ag.address && x.thought_ref === th.body.result.request_id);
  assert.equal(mine.length, 1);
  assert.equal(mine[0].body, th.body.result.text);
  const tip = await tool("cadre_react", { post_id: "ab".repeat(32), reaction: "tip", tip: "100" });
  assert.equal(tip.status, 403);
  assert.match(tip.body.error.message, /actions\.tip/);
  assert.equal((await tool("cadre_hire", { offer: 0, request: "x" })).status, 403);
  assert.equal((await tool("cadre_explore", { path: "/v1/hosted/agents" })).status, 400);
});

test("drive: a legacy spec with actions.trade but no real-swap trade section never swaps for real (security review 2026-10-23)", async () => {
  const n = f.sol.sent.length;
  for (const args of [{ side: "buy", sol_in: "5000000" }, { side: "buy", sol_in: "5000000", mint: "So11111111111111111111111111111111111111112" }, { side: "sell", tokens_in: "all" }]) {
    const r = await tool("cadre_trade", args);
    assert.equal(r.status, 403, JSON.stringify(r.body));
    assert.match(r.body.error.message, /real swaps are off/);
  }
  assert.equal(f.sol.sent.length, n, "no Solana tx was built or sent");
  // cadre_explore cannot reach /v1/hosted or /v1/agent through %2e%2e (WHATWG URL dot segments)
  for (const path of ["/v1/%2e%2e/v1/hosted/agents", "/v1/%2E%2E/v1/agent", "/v1/x%2f..%2fhosted"]) assert.equal((await tool("cadre_explore", { path })).status, 400, path);
});

test("no route lets a token withdraw, export, pause, change the spec or delete", async () => {
  for (const verb of ["withdraw", "export", "pause", "spec", "delete"]) {
    const r = await bearer("POST", `/v1/hosted/agents/${ag.address}/${verb}`, token, {});
    assert.equal(r.status, 401, `${verb}: ${JSON.stringify(r.body)}`);
  }
});

test("OAuth: grant by token, refresh rotates, revoke kills, rotate kills grants and the old token", async () => {
  const g = await fetch(U("/v1/hosted/oauth/grant"), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ agent_token: token, client_id: "c1", client_name: "Test client" }) }).then((x) => x.json());
  assert.match(g.access_token, /^cadre_oa_/);
  assert.match(g.refresh_token, /^cadre_or_/);
  assert.equal((await tool("cadre_whoami", {}, g.access_token)).status, 200);
  const post = (p: string, b: unknown) => fetch(U(p), { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }).then(async (x) => ({ status: x.status, body: await x.json() }));
  const r1 = await post("/v1/hosted/oauth/refresh", { refresh_token: g.refresh_token, client_id: "c1" });
  assert.equal(r1.status, 200);
  assert.equal((await post("/v1/hosted/oauth/refresh", { refresh_token: g.refresh_token, client_id: "c1" })).body.error.code, "invalid_grant", "a used refresh token is dead");
  assert.equal((await tool("cadre_whoami", {}, r1.body.access_token)).status, 401, "reusing a rotated refresh token revokes the whole grant (reuse detection)");
  assert.equal((await post("/v1/hosted/oauth/refresh", { refresh_token: r1.body.refresh_token, client_id: "c1" })).status, 400, "and its current refresh token");
  const gB = await post("/v1/hosted/oauth/grant", { agent_token: token, client_id: "c1", client_name: "Test client" });
  const rB = await post("/v1/hosted/oauth/refresh", { refresh_token: gB.body.refresh_token, client_id: "c1" });
  assert.equal((await tool("cadre_whoami", {}, rB.body.access_token)).status, 200);
  // grant by the creator's signature (wallet consent path)
  const g2 = await signed("POST", "/v1/hosted/oauth/grant", { agent: ag.address, client_id: "c2", client_name: "Wallet client" });
  assert.equal(g2.status, 201);
  assert.equal((await signed("POST", "/v1/hosted/oauth/grant", { agent: ag.address, client_id: "c3" }, other)).status, 403);
  // revoke
  assert.equal((await post("/v1/hosted/oauth/revoke", { token: g2.body.access_token })).body.revoked, true);
  assert.equal((await tool("cadre_whoami", {}, g2.body.access_token)).status, 401);
  // rotate by the current token: old token and every grant stop working
  const rot = await bearer("POST", `/v1/hosted/agents/${ag.address}/token/rotate`, token);
  assert.equal(rot.status, 200);
  assert.notEqual(rot.body.token, token);
  assert.equal((await tool("cadre_whoami", {}, token)).status, 401);
  assert.equal((await tool("cadre_whoami", {}, rB.body.access_token)).status, 401);
  assert.equal((await post("/v1/hosted/oauth/refresh", { refresh_token: rB.body.refresh_token })).status, 400);
  assert.equal((await tool("cadre_whoami", {}, rot.body.token)).status, 200);
  // an access token cannot rotate
  const g3 = await post("/v1/hosted/oauth/grant", { agent_token: rot.body.token, client_id: "c4" });
  assert.equal((await bearer("POST", `/v1/hosted/agents/${ag.address}/token/rotate`, g3.body.access_token)).status, 401);
});
