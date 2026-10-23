// User agents on the trading routes (docs/TRADING.md 5.6): a sovereign v2 agent with the trade
// skill, found from listProfiles (flag trades, not house) and its verifying spec in the profile
// blob; its own-wallet PumpSwap fills read from its Solana txs' balance changes (fee and new
// token-account rent excluded); receipts linked from its Cadre posts; leaderboard and agent view
// label it user_agent (organic), apart from house. Ports 10782 (fake RPC) and 10783 (api): this
// row's block (CLAUDE.md).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createServer, type Server } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { configFromEnv, Deps } from "../src/deps.ts";
import { Api } from "../src/http.ts";
import { feeBuySigs, realSwapsOn, registerTrading, walletFill, type TradingConfig } from "../src/routes/trading.ts";
import { BRIDGE_ID } from "../src/sol.ts";
import { registerUserAgents } from "../src/routes/useragents.ts";
import { b58decode, b58encode, hex } from "../../../apps/explorer/src/codec.ts";
import { blake3 } from "../../../apps/explorer/src/blake3.ts";
import { socialSigningBytes } from "../../../apps/explorer/src/social.ts";
import { walletFromSeed } from "../../swarm/src/wallet.ts";
import { defaultSpecV2, defaultTrade, profileBlob, specMessage, specSha256 } from "../../swarm/agents/src/spec.ts";

const PORTS = { rpc: 10782, api: 10783 };
const key = (i: number) => b58encode(Uint8Array.from({ length: 32 }, (_, j) => (j === 0 ? i : 9)));
const MINT = key(1), POOL = key(2), BASEV = key(5), QUOTEV = key(6), HOUSE = key(13);
const seed = (s: string) => createHash("sha256").update(s).digest();
const creator = walletFromSeed(seed("ut-creator"));
const agent = walletFromSeed(seed("ut-agent"));
const SIG_BUY = "4".repeat(87), SIG_SELL = "5".repeat(87), SIG_OTHER = "6".repeat(87);

const spec = defaultSpecV2(agent.address, creator.address, {
  handle: "tide_trader", name: "Tide Trader", persona: "Trades with the tide.", created_ms: 1, skills: ["town", "x402", "trade"],
  actions: { post: true, reply: true, react: true, follow: true, tip: false, trade: true, memo: false, hire: false, mine: false, browse: false }, trade: defaultTrade(MINT),
});
const sha = specSha256(spec);
const doc = { spec, sha256: sha, sig: Buffer.from(creator.sign(Buffer.from(specMessage(spec, sha), "utf8"))).toString("hex") };
const self = { format: "cadre-agent-self-v1" as const, strategy: "sell into strength", persona_notes: null, goals: ["compound"], updated_ms: 2, decided_by: "req_self" };
const blob = profileBlob(doc, self);
const uri = `blob:${hex(blake3(blob))}`;

// a practice agent (trade section, real swaps off): its only Solana fill is the runtime's one CADRE buy
// for Cadre fees, bridged out whole right after (agent.ts selfFundCadre); it is not a real-swap trader
const fee = walletFromSeed(seed("ut-fee-agent"));
const feeSpec = defaultSpecV2(fee.address, creator.address, {
  handle: "quick_fee", name: "Quick Fee", persona: "Practices.", created_ms: 1, skills: ["town", "x402", "trade"],
  actions: { post: true, reply: true, react: true, follow: true, tip: false, trade: true, memo: false, hire: false, mine: false, browse: false }, trade: defaultTrade(MINT),
});
const feeSha = specSha256(feeSpec);
const feeDoc = { spec: feeSpec, sha256: feeSha, sig: Buffer.from(creator.sign(Buffer.from(specMessage(feeSpec, feeSha), "utf8"))).toString("hex") };
const feeBlob = profileBlob(feeDoc, self);
const feeUri = `blob:${hex(blake3(feeBlob))}`;
const SIG_FEEBUY = "7".repeat(87), SIG_FEEBRIDGE = "8".repeat(87);
const FEE_BUY = {
  slot: 590, blockTime: 1_792_617_100,
  transaction: { message: { accountKeys: [fee.address, key(32), "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"] } },
  meta: { err: null, fee: 5000, preBalances: [100_000_000, 0, 1], postBalances: [100_000_000 - 20_000_000 - 5_000 - 2_039_280, 2_039_280, 1], preTokenBalances: [], postTokenBalances: [{ accountIndex: 1, mint: MINT, owner: fee.address, uiTokenAmount: { amount: "777000" } }] },
};
const FEE_BRIDGE = {
  slot: 591, blockTime: 1_792_617_110,
  transaction: { message: { accountKeys: [fee.address, key(32), key(33), BRIDGE_ID] } },
  meta: { err: null, fee: 5000, preBalances: [77_955_720, 2_039_280, 0, 1], postBalances: [77_950_720, 2_039_280, 0, 1], preTokenBalances: [{ accountIndex: 1, mint: MINT, owner: fee.address, uiTokenAmount: { amount: "777000" } }], postTokenBalances: [{ accountIndex: 1, mint: MINT, owner: fee.address, uiTokenAmount: { amount: "0" } }] },
};

const le64 = (n: bigint) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(n); return b; };
function tokenAcct(amount: bigint) { const b = Buffer.alloc(165); le64(amount).copy(b, 64); return b; }
function poolBytes() {
  const b = Buffer.alloc(300);
  Buffer.from(createHash("sha256").update("account:Pool").digest().subarray(0, 8)).copy(b, 0);
  Buffer.from(b58decode(MINT)).copy(b, 43);
  Buffer.from(b58decode(BASEV)).copy(b, 139);
  Buffer.from(b58decode(QUOTEV)).copy(b, 171);
  return b;
}
const acct = (data: Buffer, lamports = 2_039_280, owner = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb") => ({ data: [data.toString("base64"), "base64"], lamports, owner, executable: false });

/** A landed PumpSwap tx moving SOL and `mint` for the agent (json encoding, accountKeys strings). */
function solTx(sol: [number, number], tok: [number | null, number], fee = 5000, newAtaRent = 0) {
  const pre = tok[0] === null ? [] : [{ accountIndex: 1, mint: MINT, owner: agent.address, uiTokenAmount: { amount: String(tok[0]) } }];
  return {
    slot: 600, blockTime: 1_792_617_200,
    transaction: { message: { accountKeys: [agent.address, key(30), "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"] } },
    meta: { err: null, fee, preBalances: [sol[0], tok[0] === null ? 0 : 2_039_280, 1], postBalances: [sol[1], newAtaRent || 2_039_280, 1], preTokenBalances: pre, postTokenBalances: [{ accountIndex: 1, mint: MINT, owner: agent.address, uiTokenAmount: { amount: String(tok[1]) } }] },
  };
}
// buy: 10,000,000 lamports + 5,000 fee + 2,039,280 new token account rent for 400,000 tokens
const BUY = solTx([100_000_000, 100_000_000 - 10_000_000 - 5_000 - 2_039_280], [null, 400_000], 5_000, 2_039_280);
// sell 200,000 tokens for 6,000,000 lamports (fee 5,000 paid from it)
const SELL = solTx([87_955_720, 87_955_720 + 6_000_000 - 5_000], [400_000, 200_000]);

function receiptTx(body: string, nonce: number) {
  const sb = socialSigningBytes(1, agent.address, nonce, 1000n, { kind: "Post", postKind: 0, parent: "0".repeat(64), channel: "0".repeat(64), body, blobUri: "", blobHash: "0".repeat(64), thoughtRef: "" } as any);
  return Buffer.concat([Buffer.from(sb), Buffer.alloc(64)]).toString("base64");
}

function rpc(m: string, p: any): any {
  switch (m) {
    case "listProfiles": return { total: 2, profiles: [
      { account: agent.address, has_profile: true, handle: "tide_trader", trades: true, is_agent: true, house: false, operator: creator.address, operator_linked: true, profile_uri: uri },
      { account: HOUSE, has_profile: true, handle: "house_one", trades: true, is_agent: true, house: true },
      { account: fee.address, has_profile: true, handle: "quick_fee", trades: true, is_agent: true, house: false, operator: creator.address, operator_linked: true, profile_uri: feeUri },
    ] };
    case "getProfile": if (p.account === fee.address) return { has_profile: true, handle: "quick_fee", name: "Quick Fee", operator: creator.address, operator_linked: true, profile_uri: feeUri, profile_hash: feeUri.slice(5), set_height: 5, flags: 9 };
      return p.account === agent.address ? { has_profile: true, handle: "tide_trader", name: "Tide Trader", operator: creator.address, operator_linked: true, profile_uri: uri, profile_hash: uri.slice(5), set_height: 5, flags: 9 } : { has_profile: false };
    case "getAgent": return p.account === agent.address || p.account === fee.address ? { registered_height: 4, name: "Tide Trader", uri: "" } : null;
    case "getBlob": return p.uri === uri ? { data: blob.toString("base64") } : p.uri === feeUri ? { data: feeBlob.toString("base64") } : null;
    case "getAccountTxs": return { txs: [
      { kind: "Post", height: 7, tx_hash: "cc".repeat(32), from: agent.address, tx: receiptTx(`receipt sol:${SIG_BUY} pumpswap buy: 0.01 SOL for 0.4 CADRE; own wallet; stance buy 3 decided by req_buy1`, 3) },
    ] };
    case "getSignaturesForAddress": return p[0] === agent.address ? [
      { signature: SIG_SELL, slot: 601, err: null, blockTime: 1_792_617_300 },
      { signature: SIG_OTHER, slot: 600, err: null, blockTime: 1_792_617_250 },
      { signature: SIG_BUY, slot: 600, err: null, blockTime: 1_792_617_200 },
    ] : p[0] === fee.address ? [
      { signature: SIG_FEEBRIDGE, slot: 591, err: null, blockTime: 1_792_617_110 },
      { signature: SIG_FEEBUY, slot: 590, err: null, blockTime: 1_792_617_100 },
    ] : [];
    case "getTransaction": if (p[0] === SIG_FEEBUY) return FEE_BUY; if (p[0] === SIG_FEEBRIDGE) return FEE_BRIDGE;
      return p[0] === SIG_BUY ? BUY : p[0] === SIG_SELL ? SELL : { slot: 600, blockTime: 1, transaction: { message: { accountKeys: [agent.address, key(31)] } }, meta: { err: null, fee: 5000, preBalances: [1, 0], postBalances: [1, 0], preTokenBalances: [], postTokenBalances: [] } };
    case "getMultipleAccounts": return { value: p[0].map((a: string) => a === POOL ? acct(poolBytes(), 1, "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA") : a === BASEV ? acct(tokenAcct(80_000_000n)) : a === QUOTEV ? acct(tokenAcct(2_000_000_000n)) : null) };
    case "getProgramAccounts": return [];
    case "getChainInfo": return { chain_id: 1, height: 10 };
    case "getParams": return { min_fee: "1000" };
    default: throw new Error(`unknown method ${m}`);
  }
}

let rpcSrv: Server, apiSrv: Server, dir = "", base = "";
const listen = (s: Server, port: number) => new Promise<void>((r, j) => s.once("error", j).listen(port, "127.0.0.1", () => r()));
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
  await listen(rpcSrv, PORTS.rpc);
  dir = mkdtempSync(join(tmpdir(), "cadre-usertrading-test-"));
  const cfg = configFromEnv({ NODE_RPC: `http://127.0.0.1:${PORTS.rpc}`, SOLANA_RPC: `http://127.0.0.1:${PORTS.rpc}`, GATEWAY_URL: "off", STOREFRONT_URL: "off", POOL_URL: "off", CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000" }, { stateDir: dir });
  const d = new Deps(cfg);
  d.ix.o.log = () => {};
  const tc: TradingConfig = { solana: { program: key(40), config: key(20), mint: MINT, pool: POOL, buyback_vault: key(21) }, cadre: null, house: { [HOUSE]: "house_one" } };
  const api = new Api({ allowedOrigins: "*", rateLimitPerMin: 0, rateBurst: 1000, trustProxy: false, maxBody: 1 << 20 });
  registerTrading(api, d, tc);
  registerUserAgents(api, d);
  api.o.log = undefined;
  apiSrv = api.listen(PORTS.api, "127.0.0.1");
  await new Promise((r) => apiSrv.on("listening", r));
  base = `http://127.0.0.1:${PORTS.api}`;
});
after(() => { apiSrv?.close(); rpcSrv?.close(); rmSync(dir, { recursive: true, force: true }); });
const get = async (p: string) => { const r = await fetch(base + p); return { status: r.status, body: await r.json().catch(() => null) }; };

test("walletFill: buy excludes the fee and the new token account's rent; sell adds the fee back; other txs are not fills", () => {
  assert.deepEqual(walletFill(BUY, agent.address, MINT), { side: "buy", amount_in: 10_000_000n, amount_out: 400_000n });
  assert.deepEqual(walletFill(SELL, agent.address, MINT), { side: "sell", amount_in: 200_000n, amount_out: 6_000_000n });
  assert.equal(walletFill(BUY, agent.address, key(77)), null);
  assert.equal(walletFill({ ...BUY, meta: { ...BUY.meta, err: { x: 1 } } }, agent.address, MINT), null);
});

test("a sovereign user agent trading from its own wallet: trades, agent view, leaderboard (organic, apart from house)", async () => {
  const t = await get("/v1/trading/trades?limit=10");
  assert.equal(t.status, 200, JSON.stringify(t.body));
  const mine = t.body.trades.filter((x: any) => x.agent === agent.address);
  assert.equal(mine.length, 2);
  assert.deepEqual(mine.map((x: any) => [x.side, x.via, x.kind, x.house]), [["sell", "wallet", "user_agent", false], ["buy", "wallet", "user_agent", false]]);
  const buy = mine.find((x: any) => x.side === "buy");
  assert.equal(buy.decided_by, "req_buy1");
  assert.equal(buy.receipt_post, "cc".repeat(32));
  const a = await get(`/v1/trading/agents/${agent.address}`);
  assert.equal(a.status, 200, JSON.stringify(a.body));
  assert.equal(a.body.kind, "user_agent");
  assert.equal(a.body.sovereign, true);
  assert.equal(a.body.creator, creator.address);
  const w = a.body.positions.solana_wallet[0];
  // bought 400,000 for 10,000,000; sold 200,000 for 6,000,000: basis 5,000,000, realized +1,000,000
  assert.equal(w.position_spl, "200000");
  assert.equal(w.cost_basis_lamports, "5000000");
  assert.equal(w.realized_pnl_lamports, "1000000");
  assert.equal(w.trades, 2);
  assert.equal(w.marked_at_mid_lamports, ((200_000n * 2_000_000_000n) / 80_000_000n).toString());
  const l = await get("/v1/trading/leaderboard");
  assert.equal(l.body.user_agents.length, 1);
  assert.equal(l.body.user_agents[0].kind, "user_agent");
  assert.equal(l.body.user_agents.some((x: any) => x.agent === HOUSE), false);
  const s = await get("/v1/trading");
  assert.equal(s.body.user_agents, 1);
  assert.equal(s.body.volume.solana_lamports.organic, "16000000"); // buy spent 10M + sell received 6M
});

test("useragents view says sovereign, custody and the agent's own rewrite", async () => {
  const u = await get(`/v1/useragents/${agent.address}`);
  assert.equal(u.status, 200, JSON.stringify(u.body));
  assert.equal(u.body.sovereign, true);
  assert.equal(u.body.legacy, false);
  assert.equal(u.body.self.strategy, "sell into strength");
  assert.match(u.body.custody, /hosted: the Cadre runtime holds the agent key/);
  assert.match(u.body.trust, /nobody can withdraw its funds, export its key, pause, resume, delete it or edit its spec/);
  assert.match(u.body.controls.stop, /kill switch/);
});

test("a practice agent's one CADRE buy for Cadre fees (bridged out whole) is not a fill: off the real-swaps board, counted as fee_buys", async () => {
  assert.equal(realSwapsOn(feeSpec), false);
  assert.equal(realSwapsOn({ ...feeSpec, trade: { ...feeSpec.trade!, real_swaps: true } }), true);
  const t = await get("/v1/trading/trades?limit=50");
  assert.equal(t.body.trades.some((x: any) => x.agent === fee.address), false);
  const l = await get("/v1/trading/leaderboard");
  assert.deepEqual(l.body.user_agents.map((x: any) => x.handle), ["tide_trader"]);
  const a = await get(`/v1/trading/agents/${fee.address}`);
  assert.equal(a.status, 200, JSON.stringify(a.body));
  const w = a.body.positions.solana_wallet[0];
  assert.equal(w.trades, 0);
  assert.equal(w.fee_buys, 1);
  assert.equal(w.real_swaps, false);
  assert.equal(w.position_spl, "0");
  assert.equal((await get("/v1/trading")).body.user_agents, 1);
});

test("feeBuySigs: a buy matches one later bridge deposit of the same amount; other buys stay fills", () => {
  const f = (amount: bigint) => [{ mint: MINT, side: "buy", amount_out: amount }];
  const m = new Map<string, any>([
    ["a", { slot: 1, blockTime: 1, fills: f(100n) }],
    ["b", { slot: 2, blockTime: 2, fills: f(100n) }],
    ["c", { slot: 3, blockTime: 3, fills: [], bridged: [{ mint: MINT, amount: 100n }] }],
    ["d", { slot: 4, blockTime: 4, fills: f(50n) }],
    ["e", { slot: 0, blockTime: 0, fills: [], bridged: [{ mint: MINT, amount: 50n }] }],
  ]);
  assert.deepEqual([...feeBuySigs(m)], ["a"]);
});
