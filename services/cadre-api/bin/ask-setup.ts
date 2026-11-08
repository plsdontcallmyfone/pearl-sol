// Demo balance for the site's "Ask a house agent" box (cadre-api /v1/ask, src/routes/ask.ts).
// Keys are files and never printed; only addresses and counts are.
//   node bin/ask-setup.ts wallet --key <demo.json>                         create the demo wallet (0600) if missing
//   node bin/ask-setup.ts topup  --key <demo.json> [--rows 8192] [--times 1] pay prepaid rows from the wallet's SOL (x402)
//   node bin/ask-setup.ts apikey --key <demo.json> --out <api.key>         mint a gateway API key for the wallet (0600)
//   node bin/ask-setup.ts status --key <demo.json>                         the wallet's prepaid balance
// Env: GATEWAY_URL (default http://127.0.0.1:10664), SOLANA_RPC (devnet; default the public endpoint).
import { existsSync, writeFileSync, chmodSync, renameSync } from "node:fs";
import { Gateway, latestBlockhash } from "../../swarm/src/clients.ts";
import { paymentHeader, type Requirement } from "../../swarm/src/pay.ts";
import { createKeyFile, loadWallet } from "../../swarm/src/wallet.ts";

const [cmd, ...rest] = process.argv.slice(2);
const arg = (k: string, d?: string) => {
  const i = rest.indexOf(`--${k}`);
  if (i >= 0 && rest[i + 1]) return rest[i + 1];
  if (d !== undefined) return d;
  throw new Error(`missing --${k}`);
};
const log = (e: Record<string, unknown>) => console.log(JSON.stringify({ t: new Date().toISOString(), ...e }));
const gwUrl = process.env.GATEWAY_URL ?? "http://127.0.0.1:10664";
const solRpc = process.env.SOLANA_RPC ?? "https://api.devnet.solana.com";

class DemoGateway extends Gateway {
  async createKey() {
    const body = "{}";
    const r = await fetch(this.url + "/v1/keys", { method: "POST", headers: await this.signedHeaders("POST", "/v1/keys", body), body });
    return { status: r.status, body: await r.json().catch(() => null) };
  }
}

async function retry<T>(what: string, f: () => Promise<T>, tries = 8): Promise<T> {
  for (let i = 1; ; i++) {
    try { return await f(); } catch (e) {
      if (i >= tries) throw e;
      const ms = Math.min(60_000, 5_000 * i);
      log({ event: "retry", what, attempt: i, ms, error: (e as Error).message.slice(0, 160) });
      await new Promise((r) => setTimeout(r, ms));
    }
  }
}

async function main() {
  if (cmd === "wallet") {
    const r = createKeyFile(arg("key"));
    log({ event: "wallet", address: r.address, created: r.created });
    return;
  }
  const w = loadWallet(arg("key"));
  const gw = new DemoGateway(gwUrl, w);
  if (cmd === "topup") {
    const rows = Number(arg("rows", "8192"));
    const times = Number(arg("times", "1"));
    for (let n = 0; n < times; n++) {
      const q = await retry("quote", async () => {
        const r = await gw.topup({ rows });
        if (r.status !== 402) throw new Error(`quote: expected 402, got ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
        const req: Requirement | undefined = r.body?.accepts?.[0];
        if (!req) throw new Error("quote has no payment requirements");
        return req;
      });
      const paid = await retry("pay", async () => {
        const x = paymentHeader(w, q, await latestBlockhash(solRpc));
        const r = await gw.topup({ rows }, x);
        if (r.status !== 201) throw new Error(`top-up: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
        return r.body;
      });
      log({ event: "topup", wallet: w.address, rows: paid?.rows ?? rows, ticket: paid?.ticket_id, order: paid?.order, price_lamports: q.maxAmountRequired });
    }
    return;
  }
  if (cmd === "apikey") {
    const out = arg("out");
    if (existsSync(out)) { log({ event: "apikey", exists: true, out }); return; }
    const r = await gw.createKey();
    if (r.status !== 201 || typeof r.body?.api_key !== "string") throw new Error(`create key: ${r.status} ${JSON.stringify(r.body?.error ?? "").slice(0, 200)}`);
    const tmp = `${out}.tmp-${process.pid}`;
    writeFileSync(tmp, r.body.api_key + "\n", { mode: 0o600 });
    chmodSync(tmp, 0o600);
    renameSync(tmp, out);
    log({ event: "apikey", wallet: w.address, key_id: r.body.key_id, out });
    return;
  }
  if (cmd === "status") {
    const r = await gw.balance();
    log({ event: "status", wallet: w.address, status: r.status, balance: r.body });
    return;
  }
  throw new Error("usage: ask-setup.ts wallet|topup|apikey|status --key <file> ...");
}

main().catch((e) => { console.error(JSON.stringify({ event: "fatal", error: (e as Error).message })); process.exit(1); });
