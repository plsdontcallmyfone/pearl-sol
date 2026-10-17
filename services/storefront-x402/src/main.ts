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
//   OVERHEAD_LAMPORTS         12000000   (Order + Deposit rent, tx fees)
//   STATE_FILE                .data/state.json (used payments, for replay protection)

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
    overheadLamports: BigInt(env.OVERHEAD_LAMPORTS ?? 12_000_000),
  },
  stateFile: env.STATE_FILE ?? resolve(here, "../.data/state.json"),
});

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
