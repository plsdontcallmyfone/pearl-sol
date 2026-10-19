// The typed client (client/index.ts) against the server + fake stack: typed reads, ETag
// reuse, error class, the x402 pay-and-complete flow with a Wallet Standard-shaped wallet,
// bridge out through a window.pearl-shaped provider, SSE streaming.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";
import type { Server } from "node:http";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";
import { CadreApi, CadreApiError, bridgeOut, formatUnits, payAndComplete, type PearlProvider, type StandardSolanaWallet } from "../client/index.ts";
import { MODEL, OWNER, SERVER, makeState, startFakeStack } from "./fake.ts";

const web3 = createRequire(new URL("../../storefront-x402/package.json", import.meta.url))("@solana/web3.js");

let stack: Awaited<ReturnType<typeof startFakeStack>>;
let srv: Server;
let api: CadreApi;
let dir = "";
let calls: string[] = [];
const st = makeState();

before(async () => {
  stack = await startFakeStack(st);
  dir = mkdtempSync(join(tmpdir(), "cadre-client-test-"));
  const d = new Deps(configFromEnv({ NODE_RPC: stack.urls.node, SOLANA_RPC: stack.urls.sol, GATEWAY_URL: stack.urls.gateway, STOREFRONT_URL: stack.urls.storefront, POOL_URL: stack.urls.pool }, { stateDir: dir }));
  d.ix.o.log = () => {};
  await d.ix.poll();
  const a = build(d);
  a.o.log = undefined;
  srv = a.listen(0, "127.0.0.1");
  await new Promise((r) => srv.on("listening", r));
  const base = `http://127.0.0.1:${(srv.address() as any).port}`;
  api = new CadreApi({ baseUrl: base, fetch: (u, i) => { calls.push(`${i?.method ?? "GET"} ${String(u).replace(base, "")} ${(i?.headers as any)?.["if-none-match"] ? "inm" : ""}`); return fetch(u, i); } });
});
after(() => {
  srv?.close();
  stack?.close();
  rmSync(dir, { recursive: true, force: true });
});

test("typed reads, ETag reuse, errors", async () => {
  const s = await api.summary();
  assert.equal(s.blocks.total, 12);
  const s2 = await api.summary();
  assert.deepEqual(s2, s);
  assert.match(calls.at(-1)!, /inm$/, "second read sends If-None-Match");
  assert.equal((await api.feed({ filter: "paid" })).rows.length, 2);
  assert.equal((await api.agent(SERVER)).as_server.completed.count, 2);
  assert.equal((await api.inferenceModels()).models[0].model_id, MODEL);
  assert.equal((await api.ready()).ready, true);
  await assert.rejects(api.block(999), (e: unknown) => e instanceof CadreApiError && e.status === 404 && e.code === "not_found");
  assert.equal((await api.prepaidBalance(OWNER)).rows_free, 200);
  assert.equal(formatUnits("1234500000", 9), "1.2345");
  assert.equal(formatUnits("5", 6), "0.000005");
});

test("payAndComplete: 402 -> payment tx -> wallet partial-signs -> X-PAYMENT", async () => {
  const kp = web3.Keypair.generate();
  st.accounts.set(kp.publicKey.toBase58(), { data: Buffer.alloc(0), owner: "11111111111111111111111111111111", lamports: 1e9 });
  const wallet: StandardSolanaWallet = {
    accounts: [{ address: kp.publicKey.toBase58() }],
    features: {
      "solana:signTransaction": {
        async signTransaction({ transaction }: { transaction: Uint8Array }) {
          const vt = web3.VersionedTransaction.deserialize(transaction);
          vt.sign([kp]);
          return [{ signedTransaction: vt.serialize() }];
        },
      },
    },
  };
  const r = await payAndComplete(api, wallet, "solana:localnet", { prompt: "hi", max_tokens: 4 });
  assert.equal(r.status, 201);
  const sent = st.gatewayCalls.filter((c) => c.path === "/v1/completions").at(-1)!;
  const xp = JSON.parse(Buffer.from(sent.headers["x-payment"], "base64").toString());
  assert.equal(xp.scheme, "exact");
  const vt = web3.VersionedTransaction.deserialize(Buffer.from(xp.payload.transaction, "base64"));
  assert.ok(vt.signatures[1].some((b: number) => b !== 0), "payer signed");
  assert.ok(vt.signatures[0].every((b: number) => b === 0), "fee payer slot left for the storefront");
});

test("bridgeOut through window.pearl, chat stream", async () => {
  let got: any = null;
  const pearl = { isPearlWallet: true, address: OWNER, async signAndSendTransaction(i: any) { got = i; return { txHash: "ab".repeat(32) }; } } as unknown as PearlProvider;
  const r = await bridgeOut(api, pearl, "2000000");
  assert.equal(r.txHash, "ab".repeat(32));
  assert.deepEqual(got.transaction, { kind: "BridgeLock", solanaRecipient: OWNER, amount: "2000000", fee: "1000" });
  const chunks: string[] = [];
  for await (const c of api.chatStream({ messages: [{ role: "user", content: "hi" }] })) chunks.push(c);
  assert.equal(chunks.at(-1), "[DONE]");
  assert.equal(chunks.length, 3);
});
