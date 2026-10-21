// User-agent routes (SPEC 4.7, docs/API.md "User agents"): tx encoding for Solana-wallet signing
// (checked against the chain's vectors and a real ed25519 signature the fake node verifies),
// builders, agent verification from a profile blob carrying an operator-signed spec, receipt
// history checked against Solana signers, operator agent lists, and the hosted pass-through.
// Every server listens on an ephemeral port.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createHash, createPrivateKey, createPublicKey, sign as edSign, verify as edVerify } from "node:crypto";
import { createServer, type Server } from "node:http";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import { parseWireTx, bridgeConfigPda } from "../src/sol.ts";
import { blake3 } from "../../../apps/explorer/src/blake3.ts";
import { b58decode, b58encode, hex } from "../../../apps/explorer/src/codec.ts";
import { socialSigningBytes } from "../../../apps/explorer/src/social.ts";
import { defaultSpec, profileBlob, specMessage, specSha256 } from "../../swarm/agents/src/spec.ts";
import { bridgeConfigData, mintData, MINT } from "./fake.ts";

const PK8 = Buffer.from("302e020100300506032b657004220420", "hex");
const SPKI = Buffer.from("302a300506032b6570032100", "hex");
function wallet(label: string) {
  const seed = createHash("sha256").update(label).digest();
  const priv = createPrivateKey({ key: Buffer.concat([PK8, seed]), format: "der", type: "pkcs8" });
  const der = createPublicKey(priv).export({ format: "der", type: "spki" }) as Buffer;
  const pub = der.subarray(der.length - 32);
  return { address: b58encode(pub), pub, sign: (m: Uint8Array) => edSign(null, Buffer.from(m), priv) };
}
const verify = (pub: Uint8Array, m: Uint8Array, s: Uint8Array) => edVerify(null, Buffer.from(m), createPublicKey({ key: Buffer.concat([SPKI, Buffer.from(pub)]), format: "der", type: "spki" }), Buffer.from(s));

const USER = wallet("ua-user");
const AGENT = wallet("ua-agent");
const CHAIN_ID = 1347616769;
const VEC = JSON.parse(readFileSync(new URL("../../../chain/agent-types/vectors/tx.json", import.meta.url), "utf8"));

const spec = defaultSpec(AGENT.address, USER.address, { handle: "tide_clerk", name: "Tide Clerk", persona: "Logs the tides.", created_ms: 5 });
const sha = specSha256(spec);
const doc = { spec, sha256: sha, sig: Buffer.from(USER.sign(Buffer.from(specMessage(spec, sha)))).toString("hex") };
const blob = profileBlob(doc);
const blobId = hex(blake3(blob));
const SOL_SIG = b58encode(new Uint8Array(64).fill(9));

let linked = true;
const sent: Buffer[] = [];
const accounts = new Map<string, Buffer>([[bridgeConfigPda(), bridgeConfigData()], [MINT, mintData(1_000_000_000_000_000n)]]);

function rpc(m: string, p: any): any {
  switch (m) {
    case "getChainInfo": return { chain_id: CHAIN_ID, height: 10, finalized_height: 8 };
    case "getParams": return { min_fee: "1000", social_min_fee: "1000", social_byte_fee: "0", post_inline_max: 1024 };
    case "getAccount": return { account: p.account, balance: p.account === USER.address || p.account === AGENT.address ? "5000000" : "0", nonce: 3 };
    case "getProfile": return p.account === AGENT.address ? { account: AGENT.address, has_profile: true, handle: "tide_clerk", name: "Tide Clerk", profile_uri: `blob:${blobId}`, profile_hash: blobId, operator: USER.address, operator_linked: linked, set_height: 4 } : null;
    case "getAgent": return p.account === AGENT.address ? { account: AGENT.address, name: "Tide Clerk", uri: "x", registered_height: 5 } : null;
    case "getBlob": return p.uri === `blob:${blobId}` ? { uri: p.uri, data: blob.toString("base64") } : null;
    case "getAccountTxs": {
      if (p.account === AGENT.address) {
        const post = Buffer.from([...socialSigningBytes(CHAIN_ID, AGENT.address, 4, "1000", { kind: "Post", postKind: 0, parent: "0".repeat(64), channel: "0".repeat(64), body: `receipt sol:${SOL_SIG} pumpswap buy: 0.01 SOL`, blobUri: "", blobHash: "0".repeat(64), thoughtRef: "" }), ...new Uint8Array(64)]);
        return { txs: [{ tx_hash: "aa".repeat(32), kind: "Post", from: AGENT.address, role: "sender", height: 9, tx: post.toString("base64"), final: false, confirmations: 1 }], next: null, tip_height: 10 };
      }
      if (p.account === USER.address) {
        const link = Buffer.from([...socialSigningBytes(CHAIN_ID, USER.address, 0, "1000", { kind: "OperatorLink", agent: AGENT.address, add: true }), ...new Uint8Array(64)]);
        return { txs: [{ tx_hash: "bb".repeat(32), kind: "OperatorLink", from: USER.address, role: "sender", height: 3, tx: link.toString("base64") }], next: null, tip_height: 10 };
      }
      return { txs: [], next: null };
    }
    case "sendTx": {
      const t = Buffer.from(p.tx, "base64");
      const signing = t.subarray(0, t.length - 64);
      const from = t.subarray(5, 37);
      if (!verify(from, Buffer.concat([Buffer.from([15]), Buffer.from("pearl-sol:tx:v1"), signing]), t.subarray(t.length - 64))) throw new Error("bad tx signature");
      sent.push(t);
      return { tx_hash: createHash("sha256").update(t).digest("hex") };
    }
    // Solana
    case "getBalance": return { value: p[0] === USER.address || p[0] === AGENT.address ? 2_000_000_000 : 0 };
    case "getLatestBlockhash": return { value: { blockhash: b58encode(new Uint8Array(32).fill(4)), lastValidBlockHeight: 99 } };
    case "getMultipleAccounts": return { value: p[0].map((a: string) => (accounts.has(a) ? { data: [accounts.get(a)!.toString("base64"), "base64"], owner: "x", lamports: 1 } : null)) };
    case "getTokenAccountsByOwner": return { value: p[0] === AGENT.address ? [{ pubkey: "Fake1111111111111111111111111111111111111111", account: { data: [Buffer.alloc(165).fill(0).toString("base64"), "base64"] } }] : [] };
    case "getTransaction": return p[0] === SOL_SIG ? { slot: 77, meta: { err: null }, transaction: { message: { header: { numRequiredSignatures: 1 }, accountKeys: [AGENT.address, "11111111111111111111111111111111"] } } } : null;
  }
  throw new Error(`unknown method ${m}`);
}

let nodeSrv: Server, rtSrv: Server, apiSrv: Server, base = "", dir = "";
const rtSeen: any[] = [];
const port = (s: Server) => (s.address() as any).port;
const up = (s: Server) => new Promise<void>((r) => s.listen(0, "127.0.0.1", () => r()));

before(async () => {
  nodeSrv = createServer((req, res) => {
    let b = "";
    req.on("data", (c) => (b += c));
    req.on("end", () => {
      const j = JSON.parse(b);
      res.setHeader("content-type", "application/json");
      try { res.end(JSON.stringify({ jsonrpc: "2.0", id: j.id, result: rpc(j.method, j.params) })); } catch (e) { res.end(JSON.stringify({ jsonrpc: "2.0", id: j.id, error: { code: -32000, message: (e as Error).message } })); }
    });
  });
  rtSrv = createServer((req, res) => {
    let b = "";
    req.on("data", (c) => (b += c));
    req.on("end", () => {
      rtSeen.push({ method: req.method, url: req.url, sig: req.headers["x-cadre-signature"], body: b });
      res.setHeader("content-type", "application/json");
      if (req.url === "/v1/hosted") return res.end(JSON.stringify({ runtime: "cadre-agents", seal_pub: "x" }));
      if (req.url?.startsWith("/v1/hosted/agents/") && req.method === "GET") return res.end(JSON.stringify({ agent: AGENT.address, state: "running" }));
      if (req.url === "/v1/hosted/agents" && req.method === "POST") { res.statusCode = 401; return res.end(JSON.stringify({ error: { code: "bad_signature", message: "nope" } })); }
      res.end(JSON.stringify({ agents: [], total: 0 }));
    });
  });
  await up(nodeSrv);
  await up(rtSrv);
  process.env.CADRE_AGENTS_URL = `http://127.0.0.1:${port(rtSrv)}`;
  dir = mkdtempSync(join(tmpdir(), "cadre-ua-test-"));
  const url = `http://127.0.0.1:${port(nodeSrv)}`;
  const d = new Deps(configFromEnv({ NODE_RPC: url, SOLANA_RPC: url, GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off", CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000" }, { stateDir: dir }));
  const api = build(d);
  api.o.log = undefined;
  apiSrv = api.listen(0, "127.0.0.1");
  await new Promise((r) => apiSrv.on("listening", r));
  base = `http://127.0.0.1:${port(apiSrv)}`;
});
after(() => {
  delete process.env.CADRE_AGENTS_URL;
  apiSrv?.close();
  nodeSrv?.close();
  rtSrv?.close();
  rmSync(dir, { recursive: true, force: true });
});

const post = async (p: string, b: unknown) => { const r = await fetch(base + p, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }); return { status: r.status, body: await r.json() }; };
const get = async (p: string) => { const r = await fetch(base + p); return { status: r.status, body: await r.json() }; };

test("encode: Transfer and RegisterAgent bytes match the chain vectors", async () => {
  for (const name of ["Transfer", "RegisterAgent"]) {
    const v = VEC.find((x: any) => x.kind_name === name);
    // the vectors use chain id 1347616769, the fake chain's id
    const tx = name === "Transfer" ? { kind: "Transfer", to: v.body.to, amount: v.body.amount, fee: v.fee } : { kind: "RegisterAgent", name: v.body.name, uri: v.body.uri, fee: v.fee };
    const r = await post("/v1/tx/encode", { from: v.from, transaction: tx, nonce: Number(v.nonce) });
    assert.equal(r.status, 200, JSON.stringify(r.body));
    assert.equal(Buffer.from(r.body.signing_bytes, "base64").toString("hex"), v.signing_bytes, name);
    assert.equal(r.body.sign_message_hex, v.framed_message, name);
  }
});

test("encode an OperatorLink, sign the framed message like a Solana wallet, send: the node verifies it", async () => {
  const b = await post("/v1/tx/operator-link", { from: USER.address, agent: AGENT.address, add: true });
  assert.equal(b.status, 200);
  const e = await post("/v1/tx/encode", { from: USER.address, transaction: b.body.window_pearl.params.transaction });
  assert.equal(e.status, 200, JSON.stringify(e.body));
  assert.match(e.body.plain, /^Link the agent .* you become its operator/);
  const msg = Buffer.from(e.body.sign_message, "base64");
  assert.equal(msg[0], 15);
  assert.equal(msg.subarray(1, 16).toString(), "pearl-sol:tx:v1");
  const sb = Buffer.from(e.body.signing_bytes, "base64");
  assert.deepEqual(sb, Buffer.from(socialSigningBytes(CHAIN_ID, USER.address, 3, b.body.fee, { kind: "OperatorLink", agent: AGENT.address, add: true })));
  const tx = Buffer.concat([sb, USER.sign(msg)]).toString("base64");
  const s = await post("/v1/tx/chain", { tx });
  assert.equal(s.status, 200, JSON.stringify(s.body));
  assert.equal(sent.length, 1);
  // a fee under the rule is refused
  assert.equal((await post("/v1/tx/encode", { from: USER.address, transaction: { ...b.body.window_pearl.params.transaction, fee: "1" } })).status, 400);
});

test("builders: register-agent, transfer (funds checked), Solana transfer, SPL transfer and memo", async () => {
  const r = await post("/v1/tx/register-agent", { from: AGENT.address, name: "Tide Clerk", uri: "https://api.example/v1/useragents/x/card" });
  assert.equal(r.status, 200);
  assert.equal(r.body.window_pearl.params.transaction.kind, "RegisterAgent");
  assert.equal((await post("/v1/tx/transfer", { from: USER.address, to: AGENT.address, amount: "9000000" })).status, 422);
  assert.equal((await post("/v1/tx/transfer", { from: USER.address, to: AGENT.address, amount: "1000" })).status, 200);
  const s = await post("/v1/tx/sol-transfer", { from: USER.address, to: AGENT.address, lamports: "100000000" });
  assert.equal(s.status, 200, JSON.stringify(s.body));
  const w = parseWireTx(Uint8Array.from(Buffer.from(s.body.transaction, "base64")));
  assert.deepEqual(w.signers, [USER.address]);
  const m = await post("/v1/tx/sol-memo", { from: AGENT.address, memo: "cadre-agent checkpoint" });
  assert.equal(m.status, 200);
  assert.equal(parseWireTx(Uint8Array.from(Buffer.from(m.body.transaction, "base64"))).signers[0], AGENT.address);
  assert.equal((await post("/v1/tx/spl-transfer", { from: AGENT.address, to: USER.address, amount_spl: "5" })).status, 422, "holds 0 SPL");
});

test("a user agent verifies from the chain: blob, spec signature, operator named and linked, registered", async () => {
  const r = await get(`/v1/useragents/${AGENT.address}`);
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.equal(r.body.verified, true, JSON.stringify(r.body.checks));
  assert.equal(r.body.spec.handle, "tide_clerk");
  assert.equal(r.body.hosted.state, "running");
  linked = false;
  try {
    const r2 = await get(`/v1/useragents/${AGENT.address}?fresh=1`); // a new cache key (responses are cached 3 s)
    assert.equal(r2.body.verified, false);
    assert.equal(r2.body.checks.find((c: any) => c.name === "operator_linked").ok, false);
  } finally {
    linked = true;
  }
  const card = await get(`/v1/useragents/${AGENT.address}/card`);
  assert.equal(card.body.cadre.verified, true);
  assert.ok(card.body.skills.some((s: any) => s.id === "post"));
});

test("history: a receipt's Solana signature is checked against the same key", async () => {
  const r = await get(`/v1/useragents/${AGENT.address}/history`);
  assert.equal(r.status, 200);
  const rc = r.body.txs[0].receipt.solana[0];
  assert.equal(rc.signature, SOL_SIG);
  assert.equal(rc.signed_by_agent, true);
  assert.equal(rc.ok, true);
});

test("operator agents: OperatorLink txs folded", async () => {
  const r = await get(`/v1/operators/${USER.address}/agents`);
  assert.equal(r.status, 200);
  assert.equal(r.body.agents.length, 1);
  assert.equal(r.body.agents[0].account, AGENT.address);
  assert.equal(r.body.agents[0].operator_linked, true);
});

test("hosted pass-through keeps the signed headers and the runtime's errors", async () => {
  assert.equal((await get("/v1/hosted")).body.runtime, "cadre-agents");
  const r = await fetch(base + "/v1/hosted/agents", { method: "POST", headers: { "content-type": "application/json", "x-cadre-wallet": USER.address, "x-cadre-timestamp": "1", "x-cadre-signature": "ab".repeat(64) }, body: "{}" });
  assert.equal(r.status, 401);
  assert.equal((await r.json()).error.code, "bad_signature");
  assert.equal(rtSeen.at(-1).sig, "ab".repeat(64));
  assert.equal(b58decode(USER.address).length, 32);
});
