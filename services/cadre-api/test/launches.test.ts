// Token launches (docs/API.md "Token launches"): the unsigned pump create_v2 builder (account list
// and data bytes against the reserve program's launch CPI encoding, two signers, refusals), launch
// metadata from Cadre blobs, the agent-launched list rebuilt from a Solana create_v2 tx, the halt
// record decode, and self-trade labelling (an agent trading the token it launched is never organic).
// Fake RPC and API on ephemeral ports.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { Api } from "../src/http.ts";
import { registerTrading, type TradingConfig } from "../src/routes/trading.ts";
import { registerLaunches, decodeHaltStatus, createV2In } from "../src/routes/launches.ts";
import { b58decode, b58encode, hex } from "../../../apps/explorer/src/codec.ts";
import { blake3 } from "../../../apps/explorer/src/blake3.ts";
import { walletFromSeed } from "../../swarm/src/wallet.ts";
import { defaultSpecV2, defaultTrade, profileBlob, specMessage, specSha256 } from "../../swarm/agents/src/spec.ts";
import {
  ata, bondingCurvePda, decodeCreateV2, mayhemGlobalParams, mayhemSolVault, mayhemState, MAYHEM, parseWireTx, PUMP, pumpCreateV2Data, pumpEventAuthority,
  pumpGlobalPda, pumpMintAuthority, TOKEN_2022, ATA_PROGRAM, SYSTEM,
} from "../src/sol.ts";

const key = (i: number) => b58encode(Uint8Array.from({ length: 32 }, (_, j) => (j === 0 ? i : 7)));
const seed = (s: string) => createHash("sha256").update(s).digest();
const creator = walletFromSeed(seed("ln-creator"));
const agent = walletFromSeed(seed("ln-agent"));
const CADRE_MINT = key(1), LMINT = key(2), EXISTING = key(3), FRESH = key(4), HALT_C = key(5), BLOCKHASH = key(6);
const SIG_CREATE = "7".repeat(87), SIG_BUY2 = "8".repeat(87), SIG_OTHER = "9".repeat(87);
const META_ID = "ab".repeat(32), NOT_META = "cd".repeat(32);
const putBlobs: string[] = [];

// the agent's spec: the trade skill on CADRE plus a launch section (the agent runtime's opt-in)
const spec: any = defaultSpecV2(agent.address, creator.address, {
  handle: "forge_bot", name: "Forge Bot", persona: "Launches things.", created_ms: 1, skills: ["town", "x402", "trade"],
  actions: { post: true, reply: true, react: true, follow: true, tip: false, trade: true, memo: false, hire: false, mine: false, browse: false }, trade: defaultTrade(CADRE_MINT),
});
spec.launch = { max_dev_buy_lamports: "10000000", max_per_week: 1 };
const sha = specSha256(spec);
const doc = { spec, sha256: sha, sig: Buffer.from(creator.sign(Buffer.from(specMessage(spec, sha), "utf8"))).toString("hex") };
const blob = profileBlob(doc, null);
const uri = `blob:${hex(blake3(blob))}`;

const le64 = (n: bigint) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(n); return b; };
/** A pump Global with the IDL layout fields this API reads (initial reserves at 73/81/89/97, fee 105, create_v2 450, buyback recipients 741). */
function globalBytes(virtualSol: bigint) {
  const b = Buffer.alloc(1088);
  le64(1_073_000_000_000_000n).copy(b, 73);
  le64(virtualSol).copy(b, 81);
  le64(793_100_000_000_000n).copy(b, 89);
  le64(1_000_000_000_000_000n).copy(b, 97);
  le64(95n).copy(b, 105);
  b[450] = 1;
  for (let i = 0; i < 8; i++) Buffer.from(b58decode(key(100 + i))).copy(b, 741 + 32 * i);
  return b;
}
let GLOBAL = globalBytes(30_000_000_000n);
function curveBytes(creatorKey: string) {
  const b = Buffer.alloc(150);
  Buffer.from(b58decode(creatorKey)).copy(b, 49);
  return b;
}
const acct = (data: Buffer, owner: string, lamports = 1_000_000) => ({ data: [data.toString("base64"), "base64"], lamports, owner, executable: false });

// the agent's create_v2 + dev buy, as the builder makes it, landed (json encoding)
const createData = pumpCreateV2Data("Forge", "FORGE", "https://api.example/v1/launches/metadata/" + META_ID, agent.address);
const CREATE_TX = {
  slot: 700, blockTime: 1_792_700_000,
  transaction: { message: {
    header: { numRequiredSignatures: 2, numReadonlySignedAccounts: 0, numReadonlyUnsignedAccounts: 1 },
    accountKeys: [agent.address, LMINT, key(40), PUMP],
    instructions: [{ programIdIndex: 3, accounts: [1, 2, 0], data: b58encode(createData) }],
  } },
  meta: { err: null, fee: 10_000, preBalances: [500_000_000, 0, 0, 1], postBalances: [500_000_000 - 5_000_000 - 10_000 - 2_039_280, 1_461_600, 2_039_280, 1],
    preTokenBalances: [], postTokenBalances: [{ accountIndex: 2, mint: LMINT, owner: agent.address, uiTokenAmount: { amount: "170000000000" } }] },
};
// subtract the mint account's rent too: only the token account (2,039,280) and the mint (1,461,600) were created
CREATE_TX.meta.postBalances[0] -= 1_461_600;
const BUY2_TX = {
  slot: 701, blockTime: 1_792_700_100,
  transaction: { message: { header: { numRequiredSignatures: 1 }, accountKeys: [agent.address, key(41), PUMP], instructions: [] } },
  meta: { err: null, fee: 5_000, preBalances: [480_000_000, 2_039_280, 1], postBalances: [480_000_000 - 2_000_000 - 5_000, 2_039_280, 1],
    preTokenBalances: [{ accountIndex: 1, mint: LMINT, owner: agent.address, uiTokenAmount: { amount: "170000000000" } }],
    postTokenBalances: [{ accountIndex: 1, mint: LMINT, owner: agent.address, uiTokenAmount: { amount: "230000000000" } }] },
};
const metaBlob = Buffer.from(JSON.stringify({ format: "cadre-launch-meta-v1", name: "Forge", symbol: "FORGE", description: "Agent-launched by forge_bot on Cadre", agent_launched: true, agent: agent.address, thought_ref: "req_x" }));

function rpc(m: string, p: any): any {
  switch (m) {
    case "listProfiles": return { total: 1, profiles: [{ account: agent.address, has_profile: true, handle: "forge_bot", trades: true, is_agent: true, house: false, operator: creator.address, operator_linked: true, profile_uri: uri }] };
    case "getProfile": return p.account === agent.address ? { has_profile: true, handle: "forge_bot", name: "Forge Bot", operator: creator.address, operator_linked: true, profile_uri: uri, profile_hash: uri.slice(5), set_height: 5, flags: 9 } : { has_profile: false };
    case "getAgent": return p.account === agent.address ? { registered_height: 4, name: "Forge Bot", uri: "" } : null;
    case "getBlob": {
      const id = p.blob_id ?? p.uri;
      if (id === uri) return { data: blob.toString("base64") };
      if (id === META_ID) return { data: metaBlob.toString("base64") };
      if (id === NOT_META) return { data: Buffer.from(JSON.stringify({ format: "other" })).toString("base64") };
      return null;
    }
    case "putBlob": { putBlobs.push(p.data); return { blob_id: hex(blake3(Buffer.from(p.data, "base64"))), new: true }; }
    case "getAccountTxs": return { txs: [] };
    case "getSignaturesForAddress": return p[0] === agent.address ? [
      { signature: SIG_BUY2, slot: 701, err: null, blockTime: 1_792_700_100 },
      { signature: SIG_OTHER, slot: 700, err: null, blockTime: 1_792_700_050 },
      { signature: SIG_CREATE, slot: 700, err: null, blockTime: 1_792_700_000 },
    ] : [];
    case "getTransaction": return p[0] === SIG_CREATE ? CREATE_TX : p[0] === SIG_BUY2 ? BUY2_TX : { slot: 700, blockTime: 1, transaction: { message: { header: { numRequiredSignatures: 1 }, accountKeys: [agent.address, key(31)], instructions: [] } }, meta: { err: null, fee: 5000, preBalances: [1, 0], postBalances: [1, 0], preTokenBalances: [], postTokenBalances: [] } };
    case "getMultipleAccounts": return { value: p[0].map((a: string) =>
      a === pumpGlobalPda() ? acct(GLOBAL, PUMP)
      : a === EXISTING ? acct(Buffer.alloc(82), TOKEN_2022)
      : a === bondingCurvePda(LMINT) ? acct(curveBytes(agent.address), PUMP)
      : null) };
    case "getBalance": return { value: 2_000_000_000 };
    case "getLatestBlockhash": return { value: { blockhash: BLOCKHASH, lastValidBlockHeight: 900 } };
    case "getProgramAccounts": return [];
    case "queryContract": {
      if (p.address !== HALT_C || p.args !== Buffer.concat([Buffer.of(6), Buffer.from("status")]).toString("hex")) return { ok: false, error: "unknown" };
      const reason = Buffer.from("incident drill");
      const r = Buffer.concat([Buffer.of(2 | 4), le64(3n), le64(5123n), Buffer.from(b58decode(creator.address)), (() => { const b = Buffer.alloc(4); b.writeUInt32LE(reason.length); return b; })(), reason]);
      return { ok: true, return: r.toString("hex"), gas_used: 1000 };
    }
    case "getChainInfo": return { chain_id: 1, height: 10 };
    default: throw new Error(`unknown method ${m}`);
  }
}

let rpcSrv: Server, apiSrv: Server, dir = "", base = "";
before(async () => {
  rpcSrv = createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const q = JSON.parse(body || "{}");
      const one = (x: any) => { try { return { jsonrpc: "2.0", id: x.id, result: rpc(x.method, x.params) }; } catch (e) { return { jsonrpc: "2.0", id: x.id, error: { code: -32601, message: (e as Error).message } }; } };
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(Array.isArray(q) ? q.map(one) : one(q)));
    });
  });
  await new Promise<void>((r) => rpcSrv.listen(0, "127.0.0.1", () => r()));
  const rport = (rpcSrv.address() as any).port;
  dir = mkdtempSync(join(tmpdir(), "cadre-launches-test-"));
  const cfg = configFromEnv({ NODE_RPC: `http://127.0.0.1:${rport}`, SOLANA_RPC: `http://127.0.0.1:${rport}`, GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off", CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000" }, { stateDir: dir });
  const d = new Deps(cfg);
  d.ix.o.log = () => {};
  const tc: TradingConfig = { solana: { program: key(60), config: key(61), mint: CADRE_MINT, pool: key(62), buyback_vault: key(63) }, cadre: null, house: {} };
  const api = new Api({ allowedOrigins: "*", rateLimitPerMin: 0, rateBurst: 1000, trustProxy: false, maxBody: 1 << 20 });
  registerTrading(api, d, tc);
  registerLaunches(api, d);
  api.o.log = undefined;
  apiSrv = api.listen(0, "127.0.0.1");
  await new Promise((r) => apiSrv.on("listening", r));
  base = `http://127.0.0.1:${(apiSrv.address() as any).port}`;
});
after(() => { apiSrv?.close(); rpcSrv?.close(); rmSync(dir, { recursive: true, force: true }); });
const get = async (p: string) => { const r = await fetch(base + p); return { status: r.status, body: await r.json().catch(() => null) }; };
const post = async (p: string, b: unknown) => { const r = await fetch(base + p, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }); return { status: r.status, body: await r.json().catch(() => null) }; };

/** The reserve program's launch CPI encoding (token/programs/reserve/src/lib.rs `launch`), rebuilt here byte by byte. */
function reserveEncoding(name: string, symbol: string, u: string, creatorKey: string) {
  const parts: Buffer[] = [Buffer.from([214, 144, 76, 236, 95, 139, 49, 180])];
  for (const s of [name, symbol, u]) { const l = Buffer.alloc(4); l.writeUInt32LE(Buffer.byteLength(s)); parts.push(l, Buffer.from(s)); }
  parts.push(Buffer.from(b58decode(creatorKey)), Buffer.of(0), Buffer.of(0), Buffer.alloc(8), Buffer.of(0));
  return Buffer.concat(parts);
}

test("create_v2 data is byte for byte the reserve program's launch encoding, and decodes back", () => {
  const d = pumpCreateV2Data("Cadre", "CADRE", "https://x.example/m.json", creator.address);
  assert.deepEqual(Buffer.from(d), reserveEncoding("Cadre", "CADRE", "https://x.example/m.json", creator.address));
  assert.deepEqual(decodeCreateV2(d), { name: "Cadre", symbol: "CADRE", uri: "https://x.example/m.json", creator: creator.address });
  assert.equal(decodeCreateV2(Uint8Array.from([1, 2, 3])), null);
});

test("POST /v1/tx/pump-create: 16-account create_v2 + ATA + dev buy, two signers [creator, mint]", async () => {
  const r = await post("/v1/tx/pump-create", { creator: creator.address, mint: FRESH, name: "Forge", symbol: "FORGE", uri: "https://api.example/v1/launches/metadata/" + META_ID, dev_buy_lamports: "10000000" });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.deepEqual(r.body.signers, [creator.address, FRESH]);
  assert.equal(r.body.summary.bonding_curve, bondingCurvePda(FRESH));
  assert.equal(r.body.summary.token_account, ata(creator.address, FRESH, TOKEN_2022));
  assert.equal(r.body.summary.instruction, "create_v2");
  // quote on Global's initial reserves: net = 10M x 10000 / 10125, out = vt x net / (vs + net)
  const net = (10_000_000n * 10_000n) / 10_125n;
  const out = (1_073_000_000_000_000n * net) / (30_000_000_000n + net);
  assert.equal(r.body.summary.quote_tokens_out_spl, out.toString());
  assert.equal(r.body.summary.min_tokens_out_spl, ((out * 9_900n) / 10_000n).toString());
  assert.match(r.body.label, /GET \/v1\/launches/);
  const w = parseWireTx(Buffer.from(r.body.transaction, "base64"));
  assert.equal(w.sigs.length, 2);
  assert.deepEqual(w.signers, [creator.address, FRESH]);
  // decode the message: find the create_v2 instruction and check its 16 accounts in order
  const msg = Buffer.from(w.message);
  const nKeys = msg[3];
  const keys: string[] = [];
  for (let i = 0; i < nKeys; i++) keys.push(b58encode(msg.subarray(4 + 32 * i, 36 + 32 * i)));
  let o = 4 + 32 * nKeys + 32;
  const nIx = msg[o++];
  const ixs: { program: string; accounts: string[]; data: Buffer }[] = [];
  for (let i = 0; i < nIx; i++) {
    const program = keys[msg[o++]];
    const na = msg[o++];
    const accounts = [...msg.subarray(o, o + na)].map((x) => keys[x]); o += na;
    let dl = msg[o++];
    if (dl & 0x80) { dl = (dl & 0x7f) | (msg[o++] << 7); }
    const data = msg.subarray(o, o + dl); o += dl;
    ixs.push({ program, accounts, data: Buffer.from(data) });
  }
  assert.equal(ixs.length, 3); // create_v2, ATA, buy (no compute-budget ix: the 1,232-byte limit is tight)
  const c = ixs[0];
  assert.equal(c.program, PUMP);
  assert.deepEqual(c.accounts, [
    FRESH, pumpMintAuthority(), bondingCurvePda(FRESH), ata(bondingCurvePda(FRESH), FRESH, TOKEN_2022), pumpGlobalPda(), creator.address, SYSTEM, TOKEN_2022, ATA_PROGRAM,
    MAYHEM, mayhemGlobalParams(), mayhemSolVault(), mayhemState(FRESH), ata(mayhemSolVault(), FRESH, TOKEN_2022), pumpEventAuthority(), PUMP,
  ]);
  assert.deepEqual(c.data, reserveEncoding("Forge", "FORGE", "https://api.example/v1/launches/metadata/" + META_ID, creator.address));
  assert.equal(ixs[1].program, ATA_PROGRAM);
  assert.equal(ixs[2].program, PUMP);
  assert.deepEqual([...ixs[2].data.subarray(0, 8)], [225, 247, 80, 30, 213, 179, 132, 136]);
  assert.equal(ixs[2].data.readBigUInt64LE(8), 10_000_000n);
  assert.ok(r.body.tx_bytes <= 1232, `tx is ${r.body.tx_bytes} bytes`);
});

test("pump-create refuses a tx over Solana's 1,232-byte limit; the short metadata route serves base58 ids", async () => {
  const long = "https://api.example/v1/launches/metadata/" + META_ID + "?" + "x".repeat(200 - 41 - 64 - 1);
  const r = await post("/v1/tx/pump-create", { creator: creator.address, mint: FRESH, name: "N".repeat(32), symbol: "ABCDEFGHIJ", uri: long, dev_buy_lamports: "10000000" });
  assert.equal(r.status, 422, JSON.stringify(r.body));
  assert.equal(r.body.error.code, "tx_too_large");
  const short = await get(`/v1/l/${b58encode(Buffer.from(META_ID, "hex"))}`);
  assert.equal(short.status, 200, JSON.stringify(short.body));
  assert.equal(short.body.format, "cadre-launch-meta-v1");
});

test("pump-create with no dev buy: create_v2 only; refusals", async () => {
  const z = await post("/v1/tx/pump-create", { creator: creator.address, mint: FRESH, name: "Zero", symbol: "Z0", uri: "https://a.example/z" });
  assert.equal(z.status, 200, JSON.stringify(z.body));
  assert.equal(z.body.summary.dev_buy_lamports, "0");
  const bad = async (b: Record<string, unknown>, status: number, code?: string) => {
    const r = await post("/v1/tx/pump-create", { creator: creator.address, mint: FRESH, name: "Ok", symbol: "OK", uri: "https://a.example/x", ...b });
    assert.equal(r.status, status, JSON.stringify(r.body));
    if (code) assert.equal(r.body.error?.code ?? r.body.code, code, JSON.stringify(r.body));
  };
  await bad({ symbol: "low" }, 400);
  await bad({ symbol: "TOOLONGSYMB" }, 400);
  await bad({ name: "x".repeat(33) }, 400);
  await bad({ uri: "https://a.example/" + "x".repeat(200) }, 400);
  await bad({ uri: "ftp://a.example/x" }, 400);
  await bad({ mint: EXISTING }, 409, "mint_exists");
  await bad({ mint: creator.address }, 400);
  // a devnet-like Global (0.3 SOL virtual): a 1 SOL dev buy would complete the curve
  GLOBAL = globalBytes(300_000_000n);
  try {
    await bad({ dev_buy_lamports: "1000000000" }, 422, "dev_buy_completes_curve");
    const ok = await post("/v1/tx/pump-create", { creator: creator.address, mint: FRESH, name: "Ok", symbol: "OK", uri: "https://a.example/x", dev_buy_lamports: "10000000" });
    assert.equal(ok.status, 200, JSON.stringify(ok.body));
    assert.equal(ok.body.summary.curve_start.initial_virtual_sol_lamports, "300000000");
  } finally {
    GLOBAL = globalBytes(30_000_000_000n);
  }
});

test("launch metadata is served only for cadre-launch-meta-v1 blobs", async () => {
  const ok = await get(`/v1/launches/metadata/${META_ID}`);
  assert.equal(ok.status, 200);
  assert.equal(ok.body.agent_launched, true);
  assert.equal((await get(`/v1/launches/metadata/${NOT_META}`)).status, 404);
  assert.equal((await get(`/v1/launches/metadata/${"ee".repeat(32)}`)).status, 404);
  assert.equal((await get(`/v1/launches/metadata/xyz`)).status, 400);
});

test("GET /v1/launches lists the agent's own create_v2 (signed by it, creator = it), labelled agent-launched", async () => {
  assert.deepEqual(createV2In(CREATE_TX, agent.address)?.mint, LMINT);
  assert.equal(createV2In(CREATE_TX, creator.address), null); // not a signer
  const r = await get("/v1/launches");
  assert.equal(r.status, 200, JSON.stringify(r.body));
  assert.equal(r.body.total, 1);
  const x = r.body.launches[0];
  assert.equal(x.mint, LMINT);
  assert.equal(x.symbol, "FORGE");
  assert.equal(x.agent, agent.address);
  assert.equal(x.agent_creator, creator.address);
  assert.equal(x.label, "agent-launched");
  assert.equal(x.signature, SIG_CREATE);
  assert.equal(x.dev_buy_lamports, "5000000"); // SOL out minus the fee and the two new accounts' rent
  assert.equal(x.dev_buy_tokens_spl, "170000000000");
  assert.equal((await get(`/v1/launches/${LMINT}`)).status, 200);
  assert.equal((await get(`/v1/launches/${CADRE_MINT}`)).status, 404);
  assert.equal((await get(`/v1/launches?agent=${creator.address}`)).body.total, 0);
});

test("the agent's trades in its own token are self_trade: labelled, never organic", async () => {
  const t = await get("/v1/trading/trades?limit=20");
  assert.equal(t.status, 200, JSON.stringify(t.body));
  const mine = t.body.trades.filter((x: any) => x.agent === agent.address);
  assert.equal(mine.length, 2);
  for (const x of mine) {
    assert.equal(x.self_trade, true);
    assert.equal(x.token, LMINT);
    assert.ok(x.flags.includes("own token: launched by this agent"));
  }
  assert.equal((await get("/v1/trading/trades?organic=1")).body.total, 0);
  const s = await get("/v1/trading");
  assert.equal(s.body.volume.solana_lamports.organic, "0");
  assert.equal(s.body.volume.solana_lamports.self_trade, String(5_000_000 + 2_000_000));
  const l = await get("/v1/trading/leaderboard");
  assert.equal(l.body.user_agents.some((x: any) => x.mint === LMINT), false);
  const own = l.body.user_agents_own_token.find((x: any) => x.mint === LMINT);
  assert.ok(own, JSON.stringify(l.body));
  assert.equal(own.own_token, true);
  assert.equal(own.position_spl, "230000000000");
});

test("GET /v1/halt decodes the halt contract's status", async () => {
  const prev = process.env.CADRE_HALT_CONTRACT;
  process.env.CADRE_HALT_CONTRACT = HALT_C;
  try {
    const r = await get("/v1/halt");
    assert.equal(r.status, 200, JSON.stringify(r.body));
    assert.deepEqual(r.body.halted, { all: false, trade: true, launch: true });
    assert.equal(r.body.scope, 6);
    assert.equal(r.body.seq, "3");
    assert.equal(r.body.height, "5123");
    assert.equal(r.body.authority, creator.address);
    assert.equal(r.body.reason, "incident drill");
    assert.match(r.body.note, /advisory/);
  } finally {
    if (prev === undefined) delete process.env.CADRE_HALT_CONTRACT; else process.env.CADRE_HALT_CONTRACT = prev;
  }
  assert.throws(() => decodeHaltStatus(Buffer.alloc(10)));
});

test("launch metadata is kept on this API's disk: it still serves after the node pruned the blob", async () => {
  const meta = Buffer.from(JSON.stringify({ format: "cadre-launch-meta-v1", name: "Keep Me", symbol: "KEEP", description: "Launched by the AI agent @forge_bot on Cadre. Kept for good.", image: null, agent_launched: true, agent: agent.address, thought_ref: "req_k" }));
  const r = await fetch(base + "/v1/launches/metadata", { method: "POST", headers: { "content-type": "application/json" }, body: meta });
  const j: any = await r.json();
  assert.equal(r.status, 201, JSON.stringify(j));
  assert.equal(j.id, hex(blake3(meta)), "the id is blake3 of the exact bytes, the same id as the Cadre blob");
  assert.equal(j.path, `/v1/l/${b58encode(Buffer.from(j.id, "hex"))}`);
  assert.equal(j.node_blob, "stored");
  assert.equal(putBlobs.at(-1), meta.toString("base64"));
  // the fake node's getBlob returns null for this id: as if the blob lifecycle pruned it
  for (const path of [j.path, `/v1/launches/metadata/${j.id}`]) {
    const g = await get(path);
    assert.equal(g.status, 200, path);
    assert.equal(g.body.symbol, "KEEP");
  }
  const bad = await fetch(base + "/v1/launches/metadata", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ format: "other" }) });
  assert.equal(bad.status, 400);
  const big = await fetch(base + "/v1/launches/metadata", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ format: "cadre-launch-meta-v1", name: "x", symbol: "X", description: "y".repeat(3000) }) });
  assert.equal(big.status, 413);
  const missing = await get(`/v1/l/${b58encode(Buffer.alloc(32, 9))}`);
  assert.equal(missing.status, 404);
});

