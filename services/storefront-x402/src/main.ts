// HTTP wrapper: node src/main.ts. Binds 127.0.0.1:STOREFRONT_PORT (10662).
//
// Env (defaults are localnet):
//   TOKEN_RPC                 http://127.0.0.1:10600
//   STOREFRONT_PORT           10662
//   STOREFRONT_WALLET         ../../keys/localnet/payer-keypair.json (service wallet = order buyer, x402 payTo + fee payer)
//   X402_NETWORK              solana-localnet
//   PUBLIC_URL                http://127.0.0.1:<port>
//   PRICE_BASE_LAMPORTS       10000000   (config defaults, not a market price; set per deployment)
//   PRICE_LAMPORTS_PER_GFLOP  100000
//   OVERHEAD_LAMPORTS         14000000   (Order + Deposit + buyback ShareRecord rent, tx fees; the record rent comes back at settle_share)
//   STATE_FILE                .data/state.json (used payments, for replay protection)
//   AGENT_RPC                 http://127.0.0.1:10630 (agent-chain node: prices inference tickets by the registered model)
//   RECONCILE_MS              60000 (refund payments left without an Order by a crash or a failed refund; 0 = off)
//   RECONCILE_AFTER_MS        120000 (only payments at least this old)
//
// Routes: GET /health, GET /quote, POST /jobs (x402), GET /jobs/:id, GET /buyback (vault + totals, chain reads),
// GET /payments/unserved (payments taken without an Order or a confirmed refund).

import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { Keypair } from "@solana/web3.js";
import { RpcChain } from "./chain.ts";
import { Service, toJson } from "./service.ts";

const here = dirname(fileURLToPath(import.meta.url));
const env = process.env;
const port = Number(env.STOREFRONT_PORT ?? 10662);
const walletFile = env.STOREFRONT_WALLET ?? resolve(here, "../../../keys/localnet/payer-keypair.json");
const wallet = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(walletFile, "utf8"))));

const service = new Service({
  chain: new RpcChain(env.TOKEN_RPC ?? "http://127.0.0.1:10600"),
  wallet,
  network: env.X402_NETWORK ?? "solana-localnet",
  publicUrl: env.PUBLIC_URL ?? `http://127.0.0.1:${port}`,
  pricing: {
    baseLamports: BigInt(env.PRICE_BASE_LAMPORTS ?? 10_000_000),
    lamportsPerGflop: BigInt(env.PRICE_LAMPORTS_PER_GFLOP ?? 100_000),
    overheadLamports: BigInt(env.OVERHEAD_LAMPORTS ?? 14_000_000),
  },
  stateFile: env.STATE_FILE ?? resolve(here, "../.data/state.json"),
  agentRpc: env.AGENT_RPC ?? "http://127.0.0.1:10630",
  reconcileAfterMs: Number(env.RECONCILE_AFTER_MS ?? 120_000),
});

// @solana/web3.js can reject outside any awaited promise (a 429 from the public RPC inside
// its confirmation polling). Unhandled, node exits, and a payment already taken was left with
// no Order and no refund (devnet 2026-10-20/21: 6 such exits, each right after a payment).
// Log and keep serving; reconcile() below settles anything a real crash leaves behind.
process.on("unhandledRejection", (e) => console.error(JSON.stringify({ msg: "unhandled rejection (kept running)", error: String((e as Error)?.message ?? e).slice(0, 300) })));
// The same 429 can surface as a throw from web3.js's RPC client callback; only that one is survived.
process.on("uncaughtException", (e) => {
  if (/429|Too Many Requests|rate limit/i.test(String(e?.message))) {
    console.error(JSON.stringify({ msg: "uncaught RPC 429 (kept running)", error: e.message.slice(0, 300) }));
    return;
  }
  console.error(e);
  process.exit(1);
});

const reconcileMs = Number(env.RECONCILE_MS ?? 60_000);
const reconcile = () =>
  service.reconcile().then((r) => r.length && console.log(JSON.stringify({ msg: "reconcile", done: r })), (e) => console.error(JSON.stringify({ msg: "reconcile failed", error: (e as Error).message.slice(0, 300) })));
if (reconcileMs > 0) {
  reconcile();
  setInterval(reconcile, reconcileMs).unref();
}

const server = createServer((req, res) => {
  const chunks: Buffer[] = [];
  let size = 0;
  req.on("data", (c: Buffer) => {
    size += c.length;
    if (size > 64 * 1024) req.destroy();
    else chunks.push(c);
  });
  req.on("end", async () => {
    const h: Record<string, string | undefined> = {};
    for (const [k, v] of Object.entries(req.headers)) h[k.toLowerCase()] = Array.isArray(v) ? v[0] : v;
    const out = await service.handle({ method: req.method ?? "GET", url: req.url ?? "/", headers: h, body: Buffer.concat(chunks).toString("utf8") });
    res.writeHead(out.status, { ...out.headers, "access-control-expose-headers": "x-payment-response" });
    res.end(toJson(out.body));
  });
});
server.on("error", (e) => {
  console.error(`storefront-x402 on :${port} failed: ${e.message}`);
  process.exit(1);
});
server.listen(port, "127.0.0.1", () => console.log(JSON.stringify({ msg: "storefront-x402 started", url: `http://127.0.0.1:${port}`, wallet: wallet.publicKey.toBase58() })));
for (const sig of ["SIGINT", "SIGTERM"] as const) process.on(sig, () => server.close(() => process.exit(0)));
