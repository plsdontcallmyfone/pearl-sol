// The unsigned market txs (pump buy, storefront buyback crank, reserve fund_mining) against
// the builders services/crank already sends live: @pump-fun/pump-sdk / pump-swap-sdk and the
// Anchor programs (token/target/idl) with services/crank's StorefrontKeys / ReserveKeys
// account sets. Each built message must equal, byte for byte, web3.js's compile of the
// crank-side instructions for the same inputs. Also the decoders against the SDK / Anchor
// coders, and every 409 / 422 refusal. Skipped when services/crank has no node_modules.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Server } from "node:http";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import * as S from "../src/sol.ts";
import { BRIDGE_ID } from "../../../apps/explorer/src/decode.ts";
import { b58decode } from "../../../apps/explorer/src/codec.ts";
import { key, makeState, MINT, OWNER, startFakeStack, tokenAccountData, type FakeState } from "./fake.ts";

const CRANK = fileURLToPath(new URL("../../crank/", import.meta.url));
const ROOT = fileURLToPath(new URL("../../../", import.meta.url));
const HAVE = existsSync(join(CRANK, "node_modules/@pump-fun/pump-sdk")) && existsSync(join(ROOT, "token/target/idl/storefront.json"));
const skip = HAVE ? false : "services/crank node_modules or token/target/idl missing";

// crank-side modules (CommonJS builds, one web3.js copy, as services/crank loads them)
const req = createRequire(join(CRANK, "package.json"));
const W = HAVE ? req("@solana/web3.js") : null;
const BN = HAVE ? req("bn.js") : null;
const anchor = HAVE ? req("@coral-xyz/anchor") : null;
const pumpSdk = HAVE ? req("@pump-fun/pump-sdk") : null;
const swapSdk = HAVE ? req("@pump-fun/pump-swap-sdk") : null;
const crankAcc = HAVE ? await import("../../crank/src/accounts.ts") : null;
const crankSf = HAVE ? await import("../../crank/src/storefront.ts") : null;

const NOW = 1_760_000_500;
const idl = (n: string) => JSON.parse(readFileSync(join(ROOT, `token/target/idl/${n}.json`), "utf8"));
const fixture = (n: string) => Buffer.from(JSON.parse(readFileSync(join(ROOT, `fixtures/pump/accounts/${n}.json`), "utf8")).account.data[0], "base64");
const pk = (s: string) => new W.PublicKey(s);

/** Program over an IDL with no connection (instruction builders and coders only). */
function program(i: any) {
  const conn = new W.Connection("http://127.0.0.1:1");
  return new anchor.Program(i, new anchor.AnchorProvider(conn, new anchor.Wallet(W.Keypair.generate()), {}));
}
/** An account struct from the IDL with zero values, then `over` (camelCase keys). */
function zeroed(i: any, typeName: string, over: Record<string, unknown>) {
  const t = i.types.find((x: any) => x.name === typeName);
  const camel = (s: string) => s.replace(/_([a-z0-9])/g, (_m, c) => c.toUpperCase());
  const val = (ty: any): unknown => {
    if (ty === "pubkey") return W.PublicKey.default;
    if (ty === "bool") return false;
    if (typeof ty === "string" && /^[ui](64|128)$/.test(ty)) return new BN(0);
    if (typeof ty === "string") return 0;
    if (ty.vec) return [];
    if (ty.array) return Array.from({ length: ty.array[1] }, () => val(ty.array[0]));
    if (ty.option) return null;
    if (ty.defined) return zeroed(i, ty.defined.name, {});
    throw new Error("type " + JSON.stringify(ty));
  };
  const o: Record<string, unknown> = {};
  if (t.type.kind === "enum") return { [camel(t.type.variants[0].name)]: {} };
  for (const f of t.type.fields) o[camel(f.name)] = val(f.type);
  return { ...o, ...over };
}

const toWeb3 = (ix: S.Ix) => new W.TransactionInstruction({ programId: pk(ix.programId), keys: ix.keys.map((k) => ({ pubkey: pk(k.pubkey), isSigner: k.isSigner, isWritable: k.isWritable })), data: Buffer.from(ix.data) });
/** web3.js compile of `ixs` (crank side) with our fee payer + blockhash. */
const web3Message = (ixs: any[], feePayer: string, bh: string) => {
  const t = new W.Transaction({ feePayer: pk(feePayer), recentBlockhash: bh });
  t.add(...ixs);
  return Buffer.from(t.serializeMessage()).toString("hex");
};
const sameIx = (mine: S.Ix, theirs: any) => {
  assert.equal(mine.programId, theirs.programId.toBase58(), "program id");
  assert.deepEqual(
    mine.keys.map((k) => `${k.pubkey}:${k.isSigner ? "s" : "-"}${k.isWritable ? "w" : "-"}`),
    theirs.keys.map((k: any) => `${k.pubkey.toBase58()}:${k.isSigner ? "s" : "-"}${k.isWritable ? "w" : "-"}`),
    "account order and flags",
  );
  assert.equal(Buffer.from(mine.data).toString("hex"), Buffer.from(theirs.data).toString("hex"), "data");
};

/** Decoded legacy message: instructions with their account keys and data. */
function decodeMessage(wireB64: string) {
  const wire = Uint8Array.from(Buffer.from(wireB64, "base64"));
  const p = S.parseWireTx(wire);
  const m = p.message;
  let o = 3;
  const rd = () => {
    let len = 0, size = 0;
    for (;;) {
      const e = m[o++];
      len |= (e & 0x7f) << (size * 7);
      size++;
      if ((e & 0x80) === 0) return len;
    }
  };
  const nk = rd();
  const keys: string[] = [];
  for (let i = 0; i < nk; i++, o += 32) keys.push(new W.PublicKey(m.subarray(o, o + 32)).toBase58());
  const blockhash = new W.PublicKey(m.subarray(o, o + 32)).toBase58();
  o += 32;
  const nix = rd();
  const ixs = [];
  for (let i = 0; i < nix; i++) {
    const prog = keys[m[o++]];
    const na = rd();
    const accts = Array.from(m.subarray(o, o + na)).map((x) => keys[x]);
    o += na;
    const dl = rd();
    ixs.push({ programId: prog, accounts: accts, data: Buffer.from(m.subarray(o, o + dl)) });
    o += dl;
  }
  return { hex: Buffer.from(m).toString("hex"), keys, blockhash, ixs, signers: p.signers, sigs: p.sigs };
}

// ------------------------------------------------------------ fake chain state

let st: FakeState;
let stack: Awaited<ReturnType<typeof startFakeStack>>;
let srv: Server;
let base = "";
let dir = "";

const SF = HAVE ? program(idl("storefront")) : null;
const RS = HAVE ? program(idl("reserve")) : null;
const ATTESTER = key("attester-1");
const POOL_MINING = key("pool");

function clockData(ts: number) {
  const b = Buffer.alloc(40);
  b.writeBigInt64LE(BigInt(ts), 32);
  return b;
}
async function storeConfig(over: Record<string, unknown> = {}) {
  return Buffer.from(await SF.coder.accounts.encode("config", zeroed(SF.rawIdl, "Config", {
    mint: pk(MINT), bridgeProgram: pk(BRIDGE_ID), attesters: [pk(ATTESTER)], reserveProgram: pk(S.RESERVE_ID), buybackBps: 1000,
    buybackMinBuy: new BN(1000), buybackMinIntervalSecs: new BN(60), bbLastTs: new BN(NOW - 100), bbLocked: new BN(500_000), bbReserveCap: new BN(0), ...over,
  })));
}
async function reserveConfig(over: Record<string, unknown> = {}) {
  return Buffer.from(await RS.coder.accounts.encode("config", zeroed(RS.rawIdl, "Config", {
    admin: pk(key("admin")), mint: pk(MINT), bridgeProgram: pk(BRIDGE_ID), miningPool: pk(POOL_MINING), maxFundPerCall: new BN(50_000_000), solFloor: new BN(2_000_000),
    layout: 1, fundMinIntervalSecs: new BN(3600), fundWindowSecs: new BN(86_400), fundMaxPerWindow: new BN(60_000_000), fundLastTs: new BN(NOW - 4000), fundWindowStart: new BN(NOW - 4000), fundWindowUsed: new BN(20_000_000), fundCalls: new BN(1), ...over,
  })));
}
const put = (a: string, data: Buffer, owner: string, lamports = 1_000_000) => st.accounts.set(a, { data, owner, lamports });

const POOL_BASE_VAULT = key("pool-base-vault");
const POOL_QUOTE_VAULT = key("pool-quote-vault");
async function graduate() {
  const c = st.accounts.get(S.bondingCurvePda(MINT))!;
  c.data[48] = 1;
  const amm = new swapSdk.PumpAmmSdk();
  const ammIdl = amm.offlineProgram.rawIdl;
  const pool = await amm.offlineProgram.coder.accounts.encode("pool", zeroed(ammIdl, "Pool", {
    baseMint: pk(MINT), quoteMint: pk(S.WSOL), poolBaseTokenAccount: pk(POOL_BASE_VAULT), poolQuoteTokenAccount: pk(POOL_QUOTE_VAULT), coinCreator: pk(S.reserveCreatorPda()), lpSupply: new BN(1),
  }));
  put(S.canonicalPool(MINT), Buffer.from(pool), S.AMM);
  put(POOL_BASE_VAULT, tokenAccountData(MINT, S.canonicalPool(MINT), 200_000_000_000_000n), S.TOKEN_2022);
  put(POOL_QUOTE_VAULT, tokenAccountData(S.WSOL, S.canonicalPool(MINT), 85_000_000_000n), S.TOKEN);
}
function ungraduate() {
  st.accounts.get(S.bondingCurvePda(MINT))!.data[48] = 0;
  st.accounts.delete(S.canonicalPool(MINT));
}

before(async () => {
  if (!HAVE) return;
  st = makeState();
  put(S.pumpGlobalPda(), fixture("pump_global"), S.PUMP);
  put(S.ammGlobalConfigPda(), fixture("amm_global_config"), S.AMM);
  put(S.CLOCK, clockData(NOW), "Sysvar1111111111111111111111111111111111111");
  put(S.reserveConfigPda(), await reserveConfig(), S.RESERVE_ID);
  put(S.storeConfigAddress(), await storeConfig(), S.STOREFRONT_ID);
  stack = await startFakeStack(st);
  dir = mkdtempSync(join(tmpdir(), "cadre-api-mtx-"));
  const cfg = configFromEnv({ NODE_RPC: stack.urls.node, SOLANA_RPC: stack.urls.sol, GATEWAY_URL: stack.urls.gateway, STOREFRONT_URL: stack.urls.storefront, POOL_URL: stack.urls.pool, CADRE_RATE_LIMIT_PER_MIN: "100000", CADRE_RATE_BURST: "100000" }, { stateDir: dir });
  const d = new Deps(cfg);
  d.ix.o.log = () => {};
  const api = build(d);
  api.o.log = undefined;
  srv = api.listen(0, "127.0.0.1");
  await new Promise((r) => srv.on("listening", r));
  base = `http://127.0.0.1:${(srv.address() as any).port}`;
});
after(() => {
  srv?.close();
  stack?.close();
  if (dir) rmSync(dir, { recursive: true, force: true });
});

const post = async (p: string, body: unknown) => {
  const r = await fetch(base + p, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  return { status: r.status, body: (await r.json()) as any };
};

// ------------------------------------------------------------ decoders

test("decoders match the pump SDKs and the Anchor coders", { skip }, async () => {
  const g = fixture("pump_global");
  assert.deepEqual(S.pumpBuybackRecipients(g), pumpSdk.PUMP_SDK.decodeGlobal({ data: g }).buybackFeeRecipients.map((k: any) => k.toBase58()));
  const gc = fixture("amm_global_config");
  const amm = new swapSdk.PumpAmmSdk();
  assert.deepEqual(S.ammBuybackRecipients(gc), amm.decodeGlobalConfig({ data: gc }).buybackFeeRecipients.map((k: any) => k.toBase58()));
  // PDAs as services/crank derives them
  assert.equal(S.pumpGlobalPda(), crankAcc!.pda(["global"], crankAcc!.PUMP).toBase58());
  assert.equal(S.pumpFeeConfig(), crankAcc!.pda(["fee_config", crankAcc!.PUMP], crankAcc!.FEES).toBase58());
  assert.equal(S.ammFeeConfig(), crankAcc!.pda(["fee_config", crankAcc!.AMM], crankAcc!.FEES).toBase58());
  assert.equal(S.canonicalPool(MINT), crankAcc!.canonicalPool(pk(MINT)).toBase58());
  // pool keys from an encoded pump_amm Pool
  await graduate();
  const pl = S.decodePoolKeys(st.accounts.get(S.canonicalPool(MINT))!.data);
  const dec = amm.decodePool({ data: st.accounts.get(S.canonicalPool(MINT))!.data });
  assert.deepEqual(pl, { baseMint: dec.baseMint.toBase58(), quoteMint: dec.quoteMint.toBase58(), baseVault: dec.poolBaseTokenAccount.toBase58(), quoteVault: dec.poolQuoteTokenAccount.toBase58(), coinCreator: dec.coinCreator.toBase58() });
  ungraduate();
  // storefront Config buyback fields behind a non-empty attesters vec
  const sc = S.decodeStoreCrankConfig(await storeConfig({ buybackCranker: pk(key("c")), buybackMaxPerCall: new BN(777), bbReserveCap: new BN(999) }))!;
  assert.deepEqual(sc, { mint: MINT, reserveProgram: S.RESERVE_ID, cranker: key("c"), maxPerCall: 777n, minBuy: 1000n, minIntervalSecs: 60n, lastTs: BigInt(NOW - 100), locked: 500_000n, reserveCap: 999n });
  // reserve fund limits
  const rc = await reserveConfig({ fundPaused: true });
  assert.deepEqual(S.decodeReserveFundLimits(rc), { layout: 1, paused: true, minIntervalSecs: 3600n, windowSecs: 86_400n, maxPerWindow: 60_000_000n, lastTs: BigInt(NOW - 4000), windowStart: BigInt(NOW - 4000), windowUsed: 20_000_000n, calls: 1n });
  assert.equal(S.decodeReserveConfig(rc)!.maxFundPerCall, 50_000_000n);
});

// ------------------------------------------------------------ builders vs crank-side builders

test("instruction builders equal the SDK / Anchor + services/crank account sets", { skip }, async () => {
  const user = key("u1"), recip = key("bb");
  sameIx(S.pumpBuyExactQuoteInIx(user, MINT, 123_456n, 789n, recip),
    await pumpSdk.PUMP_SDK.getBuyExactQuoteInV3InstructionRaw({ user: pk(user), mint: pk(MINT), spendableQuoteIn: new BN(123_456), minTokensOut: new BN(789), tokenProgram: pk(S.TOKEN_2022), buybackFeeRecipient: pk(recip) }));

  const amm = new swapSdk.PumpAmmSdk();
  const p = { pool: key("pool-k"), baseMint: MINT, quoteMint: S.WSOL, baseVault: key("bv"), quoteVault: key("qv") };
  const state = {
    globalConfig: { buybackFeeRecipients: [pk(recip)] }, poolKey: pk(p.pool), pool: { baseMint: pk(MINT), quoteMint: pk(S.WSOL), poolBaseTokenAccount: pk(p.baseVault), poolQuoteTokenAccount: pk(p.quoteVault) },
    baseTokenProgram: pk(S.TOKEN_2022), quoteTokenProgram: pk(S.TOKEN), user: pk(user), userBaseTokenAccount: pk(S.ata(user, MINT, S.TOKEN_2022)), userQuoteTokenAccount: pk(S.ata(user, S.WSOL, S.TOKEN)),
    userBaseAccountInfo: null, userQuoteAccountInfo: null,
  };
  const sdkIxs = await amm.buyExactQuoteInV2Instructions(state, new BN(1000), new BN(7));
  const wsol = S.ata(user, S.WSOL, S.TOKEN);
  const mine = [S.createAtaIdempotent(user, user, S.WSOL, S.TOKEN), S.systemTransfer(user, wsol, 1000n), S.syncNative(wsol), S.createAtaIdempotent(user, user, MINT, S.TOKEN_2022), S.ammBuyExactQuoteInIx(user, p, 1000n, 7n, recip), S.closeAccount(wsol, user, user)];
  assert.equal(mine.length, sdkIxs.length);
  mine.forEach((ix, i) => sameIx(ix, sdkIxs[i]));

  // storefront crank_buyback / crank_buyback_amm with services/crank StorefrontKeys
  const sk = new crankSf!.StorefrontKeys(pk(S.STOREFRONT_ID));
  const cranker = key("cranker");
  const sc = S.decodeStoreCrankConfig(await storeConfig())!;
  sameIx(S.storeCrankBuybackIx(cranker, sc, 1_000_000_000n, 55n, recip),
    await SF.methods.crankBuyback(new BN(1_000_000_000), new BN(55)).accountsStrict(sk.curve(pk(cranker), pk(MINT), pk(S.RESERVE_ID), pk(recip))).instruction());
  sameIx(S.storeCrankBuybackAmmIx(cranker, sc, { ...p, pool: S.canonicalPool(MINT) }, 1_000_000_000n, 55n, recip),
    await SF.methods.crankBuybackAmm(new BN(1_000_000_000), new BN(55)).accountsStrict(sk.amm(pk(cranker), pk(MINT), pk(S.RESERVE_ID), pk(p.baseVault), pk(p.quoteVault), pk(recip))).instruction());

  // reserve fund_mining with services/crank ReserveKeys.fund
  const rk = new crankAcc!.ReserveKeys(pk(S.RESERVE_ID));
  sameIx(S.reserveFundMiningIx(cranker, MINT, BRIDGE_ID, 7n, 40_000_000n),
    await RS.methods.fundMining(new BN(40_000_000)).accountsStrict(rk.fund(pk(cranker), pk(MINT), pk(BRIDGE_ID), 7n)).instruction());
});

// ------------------------------------------------------------ routes

test("POST /v1/tx/pump-buy on the curve: buy_exact_quote_in_v3 as pump-sdk builds it, slippage on the fee-adjusted quote", { skip }, async () => {
  const r = await post("/v1/tx/pump-buy", { buyer: OWNER, sol_in: "1000000000" });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  const b = r.body;
  assert.equal(b.version, "legacy");
  assert.equal(b.fee_payer, OWNER);
  assert.deepEqual(b.signers, [OWNER]);
  assert.equal(b.summary.venue, "curve");
  assert.ok(S.pumpBuybackRecipients(fixture("pump_global")).includes(b.summary.buyback_fee_recipient));
  // fake curve: virtual token 1e15, virtual sol 3e10; net = 1e9 x 10000 / 10125
  const net = (1_000_000_000n * 10_000n) / 10_125n;
  const quoted = (10n ** 15n * net) / (30_000_000_000n + net);
  assert.equal(b.summary.quote_tokens_out_spl, quoted.toString());
  assert.equal(b.summary.min_tokens_out_spl, ((quoted * 9_900n) / 10_000n).toString());
  const m = decodeMessage(b.transaction);
  assert.equal(m.sigs.length, 1);
  assert.ok(m.sigs[0].every((x) => x === 0), "unsigned");
  assert.deepEqual(m.ixs.map((x) => x.programId), [S.COMPUTE_BUDGET, S.ATA_PROGRAM, S.PUMP]);
  const pumpIx = await pumpSdk.PUMP_SDK.getBuyExactQuoteInV3InstructionRaw({ user: pk(OWNER), mint: pk(MINT), spendableQuoteIn: new BN("1000000000"), minTokensOut: new BN(b.summary.min_tokens_out_spl), tokenProgram: pk(S.TOKEN_2022), buybackFeeRecipient: pk(b.summary.buyback_fee_recipient) });
  const expect = [W.ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }), toWeb3(S.createAtaIdempotent(OWNER, OWNER, MINT, S.TOKEN_2022)), pumpIx];
  assert.equal(m.hex, web3Message(expect, OWNER, m.blockhash));
  // pump-sdk's own ATA create is the same instruction
  sameIx(S.createAtaIdempotent(OWNER, OWNER, MINT, S.TOKEN_2022), pumpSdk.PUMP_SDK.baseAtaInstructions(null, pk(MINT), pk(OWNER), pk(S.TOKEN_2022))[0]);
  // data layout: disc, spendable u64, min_out u64, partial_fill 0
  const data = m.ixs[2].data;
  assert.equal(data.length, 25);
  assert.equal(data.readBigUInt64LE(8), 1_000_000_000n);
  assert.equal(data.readBigUInt64LE(16).toString(), b.summary.min_tokens_out_spl);
  assert.equal(data[24], 0);

  // explicit min_tokens_out and slippage, input errors, balance
  const r2 = await post("/v1/tx/pump-buy", { buyer: OWNER, sol_in: "1000000", min_tokens_out: "5" });
  assert.equal(r2.body.summary.min_tokens_out_spl, "5");
  assert.equal((await post("/v1/tx/pump-buy", { buyer: OWNER, sol_in: "1000000", slippage_bps: 9999 })).status, 400);
  assert.equal((await post("/v1/tx/pump-buy", { buyer: "nope", sol_in: "1" })).status, 400);
  const poor = await post("/v1/tx/pump-buy", { buyer: key("poor"), sol_in: "1000" });
  assert.equal(poor.status, 422);
  assert.equal(poor.body.error.code, "insufficient_funds");
});

test("POST /v1/tx/pump-buy after graduation: PumpSwap buy_exact_quote_in_v2 with the wSOL wrap, as pump-swap-sdk builds it", { skip }, async () => {
  await graduate();
  try {
    const r = await post("/v1/tx/pump-buy", { buyer: OWNER, sol_in: "1000000000", slippage_bps: 200 });
    assert.equal(r.status, 200, JSON.stringify(r.body));
    const b = r.body;
    assert.equal(b.summary.venue, "pumpswap");
    assert.equal(b.summary.pool, S.canonicalPool(MINT));
    const net = (1_000_000_000n * 10_000n) / 10_125n;
    assert.equal(b.summary.quote_tokens_out_spl, ((200_000_000_000_000n * net) / (85_000_000_000n + net)).toString());
    const owners = S.ammBuybackRecipients(fixture("amm_global_config"));
    const owner = owners.find((o) => S.ata(o, S.WSOL, S.TOKEN) === b.summary.buyback_fee_recipient);
    assert.ok(owner, "a listed GlobalConfig buyback recipient's wSOL ATA");
    const amm = new swapSdk.PumpAmmSdk();
    const state = {
      globalConfig: { buybackFeeRecipients: [pk(owner!)] }, poolKey: pk(S.canonicalPool(MINT)), pool: { baseMint: pk(MINT), quoteMint: pk(S.WSOL), poolBaseTokenAccount: pk(POOL_BASE_VAULT), poolQuoteTokenAccount: pk(POOL_QUOTE_VAULT) },
      baseTokenProgram: pk(S.TOKEN_2022), quoteTokenProgram: pk(S.TOKEN), user: pk(OWNER), userBaseTokenAccount: pk(S.ata(OWNER, MINT, S.TOKEN_2022)), userQuoteTokenAccount: pk(S.ata(OWNER, S.WSOL, S.TOKEN)),
      userBaseAccountInfo: null, userQuoteAccountInfo: null,
    };
    const sdkIxs = await amm.buyExactQuoteInV2Instructions(state, new BN("1000000000"), new BN(b.summary.min_tokens_out_spl));
    const m = decodeMessage(b.transaction);
    assert.equal(m.hex, web3Message([W.ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }), ...sdkIxs], OWNER, m.blockhash));
  } finally {
    ungraduate();
  }
});

test("sell builders equal pump-sdk getSellV3InstructionRaw and pump-swap-sdk sellV2Instructions", { skip }, async () => {
  const user = key("u2"), recip = key("bb2");
  sameIx(S.pumpSellV3Ix(user, MINT, 123_456n, 789n, recip),
    await pumpSdk.PUMP_SDK.getSellV3InstructionRaw({ user: pk(user), mint: pk(MINT), amount: new BN(123_456), quoteAmount: new BN(789), tokenProgram: pk(S.TOKEN_2022), buybackFeeRecipient: pk(recip) }));
  const amm = new swapSdk.PumpAmmSdk();
  const p = { pool: key("pool-k2"), baseMint: MINT, quoteMint: S.WSOL, baseVault: key("bv2"), quoteVault: key("qv2") };
  const state = {
    globalConfig: { buybackFeeRecipients: [pk(recip)] }, poolKey: pk(p.pool), pool: { baseMint: pk(MINT), quoteMint: pk(S.WSOL), poolBaseTokenAccount: pk(p.baseVault), poolQuoteTokenAccount: pk(p.quoteVault) },
    baseTokenProgram: pk(S.TOKEN_2022), quoteTokenProgram: pk(S.TOKEN), user: pk(user), userBaseTokenAccount: pk(S.ata(user, MINT, S.TOKEN_2022)), userQuoteTokenAccount: pk(S.ata(user, S.WSOL, S.TOKEN)),
    userBaseAccountInfo: null, userQuoteAccountInfo: null,
  };
  const sdkIxs = await amm.sellV2Instructions(state, new BN(1000), new BN(7));
  const wsol = S.ata(user, S.WSOL, S.TOKEN);
  const mine = [S.createAtaIdempotent(user, user, S.WSOL, S.TOKEN), S.ammSellV2Ix(user, p, 1000n, 7n, recip), S.closeAccount(wsol, user, user)];
  assert.equal(mine.length, sdkIxs.length);
  mine.forEach((ix, i) => sameIx(ix, sdkIxs[i]));
});

function setRealSol(lamports: bigint) {
  const c = st.accounts.get(S.bondingCurvePda(MINT))!;
  c.data.writeBigUInt64LE(lamports, 32);
}

test("POST /v1/tx/pump-sell on the curve: sell_v3 as pump-sdk builds it, min SOL out net of the fee allowance and slippage", { skip }, async () => {
  setRealSol(5_000_000_000n);
  try {
    const r = await post("/v1/tx/pump-sell", { seller: OWNER, amount_spl: "9000000" });
    assert.equal(r.status, 200, JSON.stringify(r.body));
    const b = r.body;
    assert.equal(b.fee_payer, OWNER);
    assert.deepEqual(b.signers, [OWNER]);
    assert.equal(b.summary.venue, "curve");
    assert.equal(b.summary.instruction, "sell_v3");
    // fake curve: virtual token 1e15, virtual sol 3e10
    const gross = (30_000_000_000n * 9_000_000n) / (10n ** 15n + 9_000_000n);
    assert.equal(b.summary.quote_sol_out_lamports, gross.toString());
    const minOut = (((gross * 9_875n) / 10_000n) * 9_900n) / 10_000n;
    assert.equal(b.summary.min_sol_out_lamports, minOut.toString());
    const m = decodeMessage(b.transaction);
    assert.ok(m.sigs[0].every((x) => x === 0), "unsigned");
    assert.deepEqual(m.ixs.map((x) => x.programId), [S.COMPUTE_BUDGET, S.PUMP]);
    const sdkIx = await pumpSdk.PUMP_SDK.getSellV3InstructionRaw({ user: pk(OWNER), mint: pk(MINT), amount: new BN("9000000"), quoteAmount: new BN(minOut.toString()), tokenProgram: pk(S.TOKEN_2022), buybackFeeRecipient: pk(b.summary.buyback_fee_recipient) });
    assert.equal(m.hex, web3Message([W.ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }), sdkIx], OWNER, m.blockhash));
    const data = m.ixs[1].data;
    assert.equal(data.length, 24);
    assert.equal(data.readBigUInt64LE(8), 9_000_000n);
    assert.equal(data.readBigUInt64LE(16), minOut);
    // the quote endpoint agrees
    const q = await (await fetch(base + "/v1/pump/quote?side=sell&tokens_in=9000000")).json() as any;
    assert.equal(q.side, "sell");
    assert.equal(q.sol_out_lamports, gross.toString());
    assert.equal((await fetch(base + "/v1/pump/quote?side=sideways&sol_in=1")).status, 400);
    // explicit min_sol_out, refusals
    assert.equal((await post("/v1/tx/pump-sell", { seller: OWNER, amount_spl: "1000", min_sol_out: "3" })).body.summary.min_sol_out_lamports, "3");
    const over = await post("/v1/tx/pump-sell", { seller: OWNER, amount_spl: "9000001" });
    assert.equal(over.status, 422);
    assert.equal(over.body.error.code, "insufficient_tokens");
    assert.equal(over.body.error.details.balance_spl, "9000000");
    assert.equal((await post("/v1/tx/pump-sell", { seller: key("nobody"), amount_spl: "1" })).body.error.code, "insufficient_tokens");
    assert.equal((await post("/v1/tx/pump-sell", { seller: OWNER, amount_spl: "0" })).status, 400);
    assert.equal((await post("/v1/tx/pump-sell", { seller: OWNER, amount_spl: "1000", slippage_bps: 6000 })).status, 400);
    // the real SOL on the curve caps the quote
    setRealSol(100n);
    const capped = await post("/v1/tx/pump-sell", { seller: OWNER, amount_spl: "9000000" });
    assert.equal(capped.body.summary.quote_sol_out_lamports, "100");
    assert.equal(capped.body.summary.capped_at_real_reserve, true);
  } finally {
    setRealSol(0n);
  }
});

test("POST /v1/tx/pump-sell after graduation: PumpSwap sell_v2 with the wSOL account and unwrap, as pump-swap-sdk builds it", { skip }, async () => {
  await graduate();
  try {
    const r = await post("/v1/tx/pump-sell", { seller: OWNER, amount_spl: "9000000", slippage_bps: 200 });
    assert.equal(r.status, 200, JSON.stringify(r.body));
    const b = r.body;
    assert.equal(b.summary.venue, "pumpswap");
    assert.equal(b.summary.instruction, "sell_v2");
    const gross = (85_000_000_000n * 9_000_000n) / (200_000_000_000_000n + 9_000_000n);
    assert.equal(b.summary.quote_sol_out_lamports, gross.toString());
    const minOut = (((gross * 9_875n) / 10_000n) * 9_800n) / 10_000n;
    assert.equal(b.summary.min_sol_out_lamports, minOut.toString());
    const owners = S.ammBuybackRecipients(fixture("amm_global_config"));
    const owner = owners.find((o) => S.ata(o, S.WSOL, S.TOKEN) === b.summary.buyback_fee_recipient);
    assert.ok(owner);
    const amm = new swapSdk.PumpAmmSdk();
    const state = {
      globalConfig: { buybackFeeRecipients: [pk(owner!)] }, poolKey: pk(S.canonicalPool(MINT)), pool: { baseMint: pk(MINT), quoteMint: pk(S.WSOL), poolBaseTokenAccount: pk(POOL_BASE_VAULT), poolQuoteTokenAccount: pk(POOL_QUOTE_VAULT) },
      baseTokenProgram: pk(S.TOKEN_2022), quoteTokenProgram: pk(S.TOKEN), user: pk(OWNER), userBaseTokenAccount: pk(S.ata(OWNER, MINT, S.TOKEN_2022)), userQuoteTokenAccount: pk(S.ata(OWNER, S.WSOL, S.TOKEN)),
      userBaseAccountInfo: null, userQuoteAccountInfo: null,
    };
    const sdkIxs = await amm.sellV2Instructions(state, new BN("9000000"), new BN(minOut.toString()));
    const m = decodeMessage(b.transaction);
    assert.equal(m.hex, web3Message([W.ComputeBudgetProgram.setComputeUnitLimit({ units: 400_000 }), ...sdkIxs], OWNER, m.blockhash));
    const q = await (await fetch(base + "/v1/pump/quote?side=sell&tokens_in=8000000")).json() as any; // a new URL: quotes are cached 3 s
    assert.equal(q.venue, "pumpswap");
    assert.equal(q.sol_out_lamports, ((85_000_000_000n * 8_000_000n) / (200_000_000_000_000n + 8_000_000n)).toString());
  } finally {
    ungraduate();
  }
});

test("POST /v1/tx/crank-buyback: storefront crank_buyback as services/crank builds it; refusals the program would make", { skip }, async () => {
  const cranker = OWNER;
  const r = await post("/v1/tx/crank-buyback", { cranker });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  const b = r.body;
  // vault 3_000_000 - rent 890_880 - held 500_000
  const spend = 3_000_000n - 890_880n - 500_000n;
  assert.equal(b.summary.spend_lamports, spend.toString());
  const q = (10n ** 15n * spend) / (30_000_000_000n + spend);
  assert.equal(b.summary.quote_tokens_out_spl, q.toString());
  const minOut = (q * 9_700n) / 10_000n; // services/crank MIN_OUT_BPS default 300
  assert.equal(b.summary.min_tokens_out_spl, minOut.toString());
  assert.equal(b.summary.destination, "reserve");
  const sk = new crankSf!.StorefrontKeys(pk(S.STOREFRONT_ID));
  const crankIx = await SF.methods.crankBuyback(new BN("1000000000"), new BN(minOut.toString())).accountsStrict(sk.curve(pk(cranker), pk(MINT), pk(S.RESERVE_ID), pk(b.summary.buyback_fee_recipient))).instruction();
  const m = decodeMessage(b.transaction);
  assert.equal(m.hex, web3Message([W.ComputeBudgetProgram.setComputeUnitLimit({ units: 600_000 }), crankIx], cranker, m.blockhash));
  assert.deepEqual(m.ixs[1].accounts.slice(0, 3), [cranker, S.storeConfigAddress(), S.storeBuybackVault()]);

  // graduated: crank_buyback_amm with the first GlobalConfig buyback recipient (as the crank)
  await graduate();
  try {
    const ra = await post("/v1/tx/crank-buyback", { cranker });
    assert.equal(ra.status, 200, JSON.stringify(ra.body));
    const owner0 = S.ammBuybackRecipients(fixture("amm_global_config"))[0];
    const qa = (200_000_000_000_000n * spend) / (85_000_000_000n + spend);
    const ammIx = await SF.methods.crankBuybackAmm(new BN("1000000000"), new BN(((qa * 9_700n) / 10_000n).toString())).accountsStrict(sk.amm(pk(cranker), pk(MINT), pk(S.RESERVE_ID), pk(POOL_BASE_VAULT), pk(POOL_QUOTE_VAULT), pk(owner0))).instruction();
    const ma = decodeMessage(ra.body.transaction);
    assert.equal(ma.hex, web3Message([W.ComputeBudgetProgram.setComputeUnitLimit({ units: 600_000 }), ammIx], cranker, ma.blockhash));
  } finally {
    ungraduate();
  }

  // too soon (bb_last_ts + 60 > now)
  put(S.storeConfigAddress(), await storeConfig({ bbLastTs: new BN(NOW - 10) }), S.STOREFRONT_ID);
  const soon = await post("/v1/tx/crank-buyback", { cranker });
  assert.equal(soon.status, 409);
  assert.equal(soon.body.error.code, "too_soon");
  assert.equal(soon.body.error.details.next_allowed_ts, NOW + 50);
  // restricted cranker
  put(S.storeConfigAddress(), await storeConfig({ buybackCranker: pk(key("only")) }), S.STOREFRONT_ID);
  assert.equal((await post("/v1/tx/crank-buyback", { cranker })).body.error.code, "cranker_restricted");
  // nothing crankable (everything held)
  put(S.storeConfigAddress(), await storeConfig({ bbLocked: new BN(2_200_000) }), S.STOREFRONT_ID);
  const none = await post("/v1/tx/crank-buyback", { cranker });
  assert.equal(none.status, 409);
  assert.equal(none.body.error.code, "nothing_to_buy");
  put(S.storeConfigAddress(), await storeConfig(), S.STOREFRONT_ID);
});

test("POST /v1/tx/fund-mining: reserve fund_mining as services/crank builds it; 409 on pause / rate limit, GET /v1/crank agrees", { skip }, async () => {
  const cranker = OWNER;
  const r = await post("/v1/tx/fund-mining", { cranker });
  assert.equal(r.status, 200, JSON.stringify(r.body));
  // min(reserve 77e6, max_fund_per_call 50e6, window room 60e6 - 20e6)
  assert.equal(r.body.summary.amount_spl, "40000000");
  assert.equal(r.body.deposit_nonce, "7");
  const rk = new crankAcc!.ReserveKeys(pk(S.RESERVE_ID));
  const crankIx = await RS.methods.fundMining(new BN(40_000_000)).accountsStrict(rk.fund(pk(cranker), pk(MINT), pk(BRIDGE_ID), 7n)).instruction();
  const m = decodeMessage(r.body.transaction);
  assert.equal(m.hex, web3Message([W.ComputeBudgetProgram.setComputeUnitLimit({ units: 600_000 }), crankIx], cranker, m.blockhash));
  assert.equal(m.ixs[1].programId, S.RESERVE_ID);
  assert.equal(m.ixs[1].data.length, 16);
  assert.equal(m.ixs[1].data.readBigUInt64LE(8), 40_000_000n);
  assert.equal(m.ixs[1].accounts[7], S.depositPda(7n));
  assert.equal(Buffer.from(b58decode(m.ixs[1].accounts[10])).length, 32);

  const c = await (await fetch(base + "/v1/crank")).json() as any;
  assert.equal(c.fund_mining.ok, true);
  assert.equal(c.fund_mining.next_allowed_ts, NOW);
  assert.equal(c.fund_mining.clock, "clock_sysvar");
  assert.equal(c.txs.fund_mining.includes("pending"), false);

  // explicit amount above the window room -> 409 window_full; above the reserve -> 422
  const big = await post("/v1/tx/fund-mining", { cranker, amount_spl: "45000000" });
  assert.equal(big.status, 409);
  assert.equal(big.body.error.code, "window_full");
  // rate limited: last fund 100 s ago, interval 3600
  put(S.reserveConfigPda(), await reserveConfig({ fundLastTs: new BN(NOW - 100) }), S.RESERVE_ID);
  const rl = await post("/v1/tx/fund-mining", { cranker });
  assert.equal(rl.status, 409);
  assert.equal(rl.body.error.code, "rate_limited");
  assert.equal(rl.body.error.details.next_allowed_ts, NOW + 3500);
  // paused
  put(S.reserveConfigPda(), await reserveConfig({ fundPaused: true }), S.RESERVE_ID);
  const pz = await post("/v1/tx/fund-mining", { cranker });
  assert.equal(pz.status, 409);
  assert.equal(pz.body.error.code, "fund_paused");
  // an unmigrated config (layout 0)
  put(S.reserveConfigPda(), await reserveConfig({ layout: 0 }), S.RESERVE_ID);
  assert.equal((await post("/v1/tx/fund-mining", { cranker })).body.error.code, "needs_migration");
  put(S.reserveConfigPda(), await reserveConfig(), S.RESERVE_ID);
});
