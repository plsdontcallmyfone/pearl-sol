// The hosted runtime end to end against fakes: create (sealed key + signed spec, operator-signed
// request), the agent's bootstrap (waits for funds, publishes the spec in its profile blob,
// registers, waits for the operator's link), paid thoughts posted verbatim with thought_ref,
// allowed actions only, CADRE and SOL caps, pause / resume, spec versions, auth failures,
// key export (sealed to the caller) and withdraw to the operator on both chains with a receipt.
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { mkdtempSync, readdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { CadreApi, NodeRpc, authMessage, latestBlockhash } from "../../src/clients.ts";
import { mulberry32 } from "../../src/swarm.ts";
import { walletFromSeed, type Wallet } from "../../src/wallet.ts";
import { GatewayAt } from "../src/gateway.ts";
import { Runtime } from "../src/runtime.ts";
import { open, seal, x25519FromSeed } from "../src/seal.ts";
import { defaultSpec, profileBlob, specMessage, specSha256, type AgentSpec } from "../src/spec.ts";
import { PORTS, startFakes2, type Fakes2 } from "./fakes.ts";

const seedOf = (s: string) => createHash("sha256").update(s).digest();
const op = walletFromSeed(seedOf("rt-operator"));
const agentSeed = seedOf("rt-agent");
const ag = walletFromSeed(agentSeed);
const other = walletFromSeed(seedOf("rt-stranger"));
const U = (p: string) => `http://127.0.0.1:${PORTS.runtime}${p}`;

let f: Fakes2;
let rt: Runtime;
let server: any;
let dir: string;
const logs: any[] = [];

function spec(over: Partial<AgentSpec> = {}): AgentSpec {
  return defaultSpec(ag.address, op.address, {
    handle: "harbour_cat", name: "Harbour Cat", persona: "A cat who reports on the docks.", appearance: "a grey cat with one white ear",
    topics: ["boats", "fish"], created_ms: 1, schedule: { cadence_min: 5, quiet_utc: null },
    budget: { sol_lamports_per_day: "60000000", cadre_units_per_day: "200000", rows_per_day: 20000 }, ...over,
  });
}
function sign(s: AgentSpec, w: Wallet = op) {
  const sha256 = specSha256(s);
  return { spec: s, sha256, sig: Buffer.from(w.sign(Buffer.from(specMessage(s, sha256)))).toString("hex") };
}
async function call(method: string, path: string, body: unknown, w: Wallet | null = op, tamper = false) {
  const b = body === undefined ? "" : JSON.stringify(body);
  const ts = Date.now();
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (w) {
    headers["x-cadre-wallet"] = w.address;
    headers["x-cadre-timestamp"] = String(ts);
    headers["x-cadre-signature"] = Buffer.from(w.sign(authMessage(method, path, ts, tamper ? b + " " : b))).toString("hex");
  }
  const r = await fetch(U(path), { method, headers, body: method === "GET" ? undefined : b });
  return { status: r.status, body: await r.json(), headers };
}

before(async () => {
  f = await startFakes2((msgs) => {
    const last = msgs.at(-1)!.content;
    if (/Answer yes or no/.test(last)) return "yes";
    return "The tide brought three boats and one very confused gull.";
  });
  dir = mkdtempSync(join(tmpdir(), "agents-rt-"));
  let t = 1_792_500_000_000;
  rt = new Runtime({
    stateDir: dir, masterKeyFile: join(dir, "master.hex"), nodeRpc: `http://127.0.0.1:${PORTS.node}`, gatewayUrl: `http://127.0.0.1:${PORTS.gateway}`,
    apiUrl: `http://127.0.0.1:${PORTS.api}`, publicApiUrl: "https://api.example", solRpc: `http://127.0.0.1:${PORTS.sol}`, network: "test",
    allowHosts: [], maxPerOperator: 2, maxAgents: 10, topupRows: 4096, minFreeRows: 256, tickMs: 10, killFile: join(dir, "STOP"),
  }, (w) => ({
    node: new NodeRpc(`http://127.0.0.1:${PORTS.node}`), gateway: new GatewayAt(`http://127.0.0.1:${PORTS.gateway}`, w), api: new CadreApi(`http://127.0.0.1:${PORTS.api}`),
    latestBlockhash, now: () => (t += 61_000), rand: mulberry32(3), log: (e) => logs.push(e),
  }));
  server = rt.listen(PORTS.runtime);
});
after(async () => {
  server.close();
  await f.close();
});

test("public info carries the seal key, the limits and the trust statement", async () => {
  const r = await fetch(U("/v1/hosted")).then((x) => x.json());
  assert.equal(Buffer.from(r.seal_pub, "base64").length, 32);
  assert.match(r.trust, /custodial/);
});

test("create refuses bad auth, a spec signed by someone else and a key that is not the agent's", async () => {
  const info = await fetch(U("/v1/hosted")).then((x) => x.json());
  const pub = Buffer.from(info.seal_pub, "base64");
  const doc = sign(spec());
  assert.equal((await call("POST", "/v1/hosted/agents", { sealed: seal(agentSeed, pub), spec: doc }, null)).status, 401);
  assert.equal((await call("POST", "/v1/hosted/agents", { sealed: seal(agentSeed, pub), spec: doc }, op, true)).body.error.code, "bad_signature");
  assert.equal((await call("POST", "/v1/hosted/agents", { sealed: seal(agentSeed, pub), spec: sign(spec(), other) }, other)).body.error.code, "bad_spec");
  assert.equal((await call("POST", "/v1/hosted/agents", { sealed: seal(agentSeed, pub), spec: doc }, other)).body.error.code, "not_operator");
  assert.equal((await call("POST", "/v1/hosted/agents", { sealed: seal(randomBytes(32), pub), spec: doc }, op)).body.error.code, "key_mismatch");
  assert.equal((await call("POST", "/v1/hosted/agents", { sealed: seal(agentSeed, x25519FromSeed(randomBytes(32)).pub), spec: doc }, op)).body.error.code, "bad_seal");
});

test("an agent goes live: funds, profile blob with the spec, RegisterAgent, operator link, then paid posts", async () => {
  const info = await fetch(U("/v1/hosted")).then((x) => x.json());
  const doc = sign(spec());
  const r = await call("POST", "/v1/hosted/agents", { sealed: seal(agentSeed, Buffer.from(info.seal_pub, "base64")), spec: doc }, op);
  assert.equal(r.status, 201, JSON.stringify(r.body));
  // replaying the same signed request is refused
  const again = await fetch(U("/v1/hosted/agents"), { method: "POST", headers: r.headers, body: JSON.stringify({ sealed: seal(agentSeed, Buffer.from(info.seal_pub, "base64")), spec: doc }) });
  assert.equal(again.status, 401);
  // the key is on disk only encrypted
  const files = readdirSync(join(dir, "keys"));
  assert.deepEqual(files, [`${ag.address}.key.json`]);
  assert.ok(!readFileSync(join(dir, "keys", files[0]), "utf8").includes(agentSeed.toString("hex")));

  await rt.loopOnce();
  assert.equal(rt.status(ag.address).state, "waiting_funds");
  // the operator funds it: CADRE on the chain and SOL on Solana
  f.node.balances.set(ag.address, 100_000n);
  f.sol.lamports.set(ag.address, 200_000_000n);
  f.gw.rowsFree = 0;
  for (let i = 0; i < 6; i++) await rt.loopOnce();
  const prof = f.node.profiles.get(ag.address);
  assert.ok(prof, "SetProfile sent");
  assert.equal(prof.handle, "harbour_cat");
  assert.equal(prof.operator, op.address);
  // the profile blob is the canonical one carrying the signed spec
  const blob = f.node.blobs.get(prof.profileUri.slice(5))!;
  assert.deepEqual(blob, profileBlob(doc));
  assert.equal(f.node.agents.get(ag.address)?.uri, `https://api.example/v1/useragents/${ag.address}/card`);
  assert.equal(rt.status(ag.address).state, "waiting_link");
  // nothing social happens before the operator's link
  assert.equal(f.node.posts.length, 0);
  f.node.links.add(`${op.address}:${ag.address}`);
  for (let i = 0; i < 8; i++) await rt.loopOnce();
  const st = rt.status(ag.address);
  assert.equal(st.state, "running", JSON.stringify(st));
  const mine = f.node.posts.filter((p) => p.author === ag.address);
  assert.ok(mine.length >= 1, "posted");
  for (const p of mine) {
    assert.match(p.thought_ref, /^req_/);
    const rc = f.gw.receipts.get(p.thought_ref);
    assert.equal(rc.text_sha256, createHash("sha256").update(p.body).digest("hex"), "posted verbatim");
  }
  // the x402 top-up was paid by the agent's own key
  assert.ok(f.gw.topups >= 1);
  // the CADRE cap holds
  assert.ok(BigInt(st.budget_today.cadre_units) <= 200_000n);
});

test("only allowed actions happen; the CADRE cap stops spending", async () => {
  const doc = sign(spec({ version: 2, actions: { ...spec().actions, post: false, reply: false, follow: false, react: true, tip: true }, limits: { tip_units: "50000", trade_lamports: "0" } }));
  const r = await call("POST", `/v1/hosted/agents/${ag.address}/spec`, { spec: doc });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  // an old version is refused
  assert.equal((await call("POST", `/v1/hosted/agents/${ag.address}/spec`, { spec: sign(spec({ version: 2 })) })).body.error.code, "old_version");
  // someone else's post to react to
  f.node.balances.set(other.address, 10_000_000n);
  f.node.posts.push({ post_id: "ab".repeat(32), author: other.address, kind: "post", parent: null, body: "Fresh fish at the pier.", thought_ref: "", height: 1, tx_hash: "00" });
  const before = f.node.posts.filter((p) => p.author === ag.address).length;
  for (let i = 0; i < 12; i++) await rt.loopOnce();
  const sent = f.node.sent.filter((x) => x.from === ag.address).map((x) => x.kind);
  assert.ok(sent.includes("SetProfile"), "the new spec is published");
  assert.equal(f.node.posts.filter((p) => p.author === ag.address && p.kind !== "post").length, 0, "no replies");
  assert.equal(f.node.posts.filter((p) => p.author === ag.address).length, before, "no new posts");
  const st = rt.status(ag.address);
  assert.ok(BigInt(st.budget_today.cadre_units) <= 200_000n, "cap");
  const reacts = f.node.sent.filter((x) => x.from === ag.address && x.kind === "React");
  assert.equal(reacts.length, 1, "one like with a tip (one post to like)");
  assert.equal(reacts[0].body.items[0].tip, 50_000n);
});

test("pause, resume and auth by the operator only", async () => {
  assert.equal((await call("POST", `/v1/hosted/agents/${ag.address}/pause`, {}, other)).status, 403);
  assert.equal((await call("POST", `/v1/hosted/agents/${ag.address}/pause`, {})).body.state, "paused");
  const n = f.node.sent.length;
  await rt.loopOnce();
  assert.equal(f.node.sent.length, n, "a paused agent sends nothing");
  assert.equal((await call("POST", `/v1/hosted/agents/${ag.address}/resume`, {})).body.paused, false);
});

test("export returns the key sealed to the caller's ephemeral key", async () => {
  const eph = x25519FromSeed(randomBytes(32));
  assert.equal((await call("POST", `/v1/hosted/agents/${ag.address}/export`, { eph_pub: eph.pub.toString("base64") }, other)).status, 403);
  const r = await call("POST", `/v1/hosted/agents/${ag.address}/export`, { eph_pub: eph.pub.toString("base64") });
  assert.equal(r.status, 200);
  assert.ok(!JSON.stringify(r.body).includes(agentSeed.toString("hex")));
  assert.deepEqual(open(r.body.sealed, eph), agentSeed);
});

test("withdraw sends SPL CADRE, SOL and chain CADRE to the operator with a receipt on Cadre", async () => {
  f.sol.spl.set(ag.address, 777n);
  const r = await call("POST", `/v1/hosted/agents/${ag.address}/withdraw`, {});
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.equal(r.body.solana.length, 2);
  assert.equal(f.sol.spl.get(op.address), 777n);
  assert.equal(f.sol.lamports.get(ag.address), 0n, "SOL drained to zero (the last fee included)");
  const receipt = f.node.posts.find((p) => p.author === ag.address && p.body.startsWith("receipt sol:"));
  assert.ok(receipt, "receipt post");
  for (const s of r.body.solana) assert.ok(receipt!.body.includes(`sol:${s}`));
  assert.equal(f.node.balances.get(ag.address), 0n, "chain CADRE drained to zero");
  assert.equal(r.body.status.state, "withdrawn");
});

test("a restarted runtime reloads the agent from the encrypted store", () => {
  const rt2 = new Runtime({ ...rt.cfg });
  assert.equal(rt2.status(ag.address).agent, ag.address);
});

test("no key in any log line, status reply or state file", () => {
  assert.equal(ag.leaksIn(JSON.stringify(logs)), false);
  assert.equal(ag.leaksIn(JSON.stringify(rt.status(ag.address))), false);
  for (const f2 of readdirSync(dir)) if (f2.endsWith(".json")) assert.equal(ag.leaksIn(readFileSync(join(dir, f2), "utf8")), false, f2);
});
