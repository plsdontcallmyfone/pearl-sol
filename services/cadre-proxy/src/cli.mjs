#!/usr/bin/env node
// cadre-proxy CLI. See README.md.
//   cadre-proxy serve [--port 10695] [--host 127.0.0.1] [--token T] [caps...]
//   cadre-proxy wallet            show the proxy wallet's address (creates it on first run)
//   cadre-proxy key               make a gateway API key (POST /v1/keys), saved to a 0600 file
//   cadre-proxy balance           GET /v1/balance (wallet-signed)
//   cadre-proxy topup [--rows N]  show what a top-up needs (the 402 requirements)
//       --pay                     pay it (native SOL x402), only against a loopback --sol-rpc
//   cadre-proxy spend             the local spend ledger and caps
//   cadre-proxy config <hermes|openclaw|kilo|cline> [--model id]
// Env: CADRE_GATEWAY (default http://127.0.0.1:10664), CADRE_PROXY_HOME (default ~/.config/cadre-proxy).
import { chmodSync, existsSync, readFileSync, renameSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { fetchAudience, gatewayHeaders } from "./auth.mjs";
import { CLIENTS, configSnippet } from "./configs.mjs";
import { Ledger, parseCaps } from "./ledger.mjs";
import { isLoopbackUrl, latestBlockhash, paymentHeader } from "./pay.mjs";
import { Proxy } from "./proxy.mjs";
import { loadWallet } from "./wallet.mjs";

const HOME = process.env.CADRE_PROXY_HOME ?? join(homedir(), ".config", "cadre-proxy");

const { values: o, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    gateway: { type: "string", default: process.env.CADRE_GATEWAY ?? "http://127.0.0.1:10664" },
    port: { type: "string", default: "10695" },
    host: { type: "string", default: "127.0.0.1" },
    token: { type: "string", default: process.env.CADRE_PROXY_TOKEN },
    wallet: { type: "string", default: join(HOME, "wallet.json") },
    ledger: { type: "string", default: join(HOME, "ledger.json") },
    "api-key-file": { type: "string", default: join(HOME, "api-key.json") },
    "use-api-key": { type: "boolean", default: false },
    "max-request-lamports": { type: "string" },
    "max-day-lamports": { type: "string" },
    "max-total-lamports": { type: "string" },
    "max-request-rows": { type: "string" },
    "max-day-rows": { type: "string" },
    "default-max-tokens": { type: "string", default: "256" },
    rows: { type: "string" },
    pay: { type: "boolean", default: false },
    "sol-rpc": { type: "string", default: "http://127.0.0.1:10600" },
    model: { type: "string" },
    "base-url": { type: "string" },
    help: { type: "boolean", short: "h", default: false },
  },
});

const cmd = positionals[0] ?? "serve";
const out = (v) => process.stdout.write((typeof v === "string" ? v : JSON.stringify(v, null, 2)) + "\n");

function readApiKey() {
  const p = o["api-key-file"];
  if (!existsSync(p)) return null;
  if (statSync(p).mode & 0o077) throw new Error(`${p} is readable by others; chmod 600 it`);
  return JSON.parse(readFileSync(p, "utf8"));
}

async function gw(wallet, method, path, body, { auth = true, apiKey, extra = {} } = {}) {
  const buf = body ? Buffer.from(JSON.stringify(body)) : Buffer.alloc(0);
  const headers = { ...extra };
  if (buf.length) headers["content-type"] = "application/json";
  if (auth) Object.assign(headers, gatewayHeaders({ wallet, apiKey, audience: apiKey ? null : await fetchAudience(o.gateway) }, method, path, buf));
  const r = await fetch(o.gateway.replace(/\/+$/, "") + path, { method, headers, body: method === "GET" ? undefined : buf });
  const text = await r.text();
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    json = { raw: text };
  }
  return { status: r.status, json, headers: r.headers };
}

async function main() {
  if (o.help) return out(readFileSync(new URL(import.meta.url), "utf8").split("\n").slice(1, 14).map((l) => l.replace(/^\/\/ ?/, "")).join("\n"));
  if (cmd === "config") {
    const client = positionals[1];
    if (!CLIENTS.includes(client)) throw new Error(`usage: cadre-proxy config <${CLIENTS.join("|")}>`);
    let model = o.model;
    let contextLength = 4096;
    try {
      const r = await fetch(o.gateway.replace(/\/+$/, "") + "/v1/models");
      const m = (await r.json()).data ?? [];
      const pick = m.find((x) => x.id === model) ?? m[0];
      if (pick) {
        model = model ?? pick.id;
        contextLength = pick.context_length ?? contextLength;
      }
    } catch {}
    if (!model) {
      model = "llama-3.2-1b-w7a7";
      process.stderr.write(`gateway ${o.gateway} not reachable; using the model id ${model} from docs/SPEC.md 5.4 (pass --model to override)\n`);
    }
    const baseUrl = o["base-url"] ?? `http://${o.host === "0.0.0.0" ? "127.0.0.1" : o.host}:${o.port}`;
    return out(configSnippet(client, { baseUrl, model, apiKey: o.token ?? "cadre-local", contextLength }));
  }
  const wallet = loadWallet(resolve(o.wallet));
  if (wallet.created) process.stderr.write(`created a new proxy wallet ${wallet.address} (key file ${wallet.path}, mode 600)\n`);
  const caps = parseCaps({
    requestLamports: o["max-request-lamports"], dayLamports: o["max-day-lamports"], totalLamports: o["max-total-lamports"],
    requestRows: o["max-request-rows"], dayRows: o["max-day-rows"],
  });
  const keyRec = o["use-api-key"] ? readApiKey() : null;
  if (o["use-api-key"] && !keyRec) throw new Error(`no API key at ${o["api-key-file"]} (run: cadre-proxy key)`);
  switch (cmd) {
    case "wallet":
      return out({ address: wallet.address, key_file: wallet.path });
    case "spend":
      return out(new Ledger(resolve(o.ledger), caps).summary());
    case "balance": {
      const r = await gw(wallet, "GET", "/v1/balance", null, { apiKey: keyRec?.api_key });
      return out({ status: r.status, ...r.json });
    }
    case "key": {
      const r = await gw(wallet, "POST", "/v1/keys", {});
      if (r.status !== 200 && r.status !== 201) return out({ status: r.status, ...r.json });
      const p = o["api-key-file"];
      const tmp = `${p}.tmp-${process.pid}`;
      writeFileSync(tmp, JSON.stringify({ key_id: r.json.key_id, api_key: r.json.api_key, wallet: wallet.address }), { mode: 0o600 });
      chmodSync(tmp, 0o600);
      renameSync(tmp, p);
      return out({ key_id: r.json.key_id, saved_to: p, note: "the key itself is not printed; start with --use-api-key to send it" });
    }
    case "topup": {
      const body = o.rows ? { rows: Number(o.rows) } : {};
      const first = await gw(wallet, "POST", "/v1/balance/topup", body, { auth: false });
      if (first.status !== 402) return out({ status: first.status, ...first.json });
      const accepts = first.json.accepts ?? [];
      if (!o.pay) return out({ status: 402, payer: wallet.address, requirements: accepts, cadre: first.json.cadre ?? null, how: "rerun with --pay to pay from the proxy wallet (loopback --sol-rpc only)" });
      if (!isLoopbackUrl(o["sol-rpc"])) throw new Error(`--pay only runs against a local Solana RPC (got ${o["sol-rpc"]}); public chains are refused`);
      const req = accepts.find((a) => a.extra?.assetKind === "native");
      if (!req) throw new Error("no native-SOL requirement in the 402");
      const header = paymentHeader(wallet, req, await latestBlockhash(o["sol-rpc"]));
      const paid = await gw(wallet, "POST", "/v1/balance/topup", body, { auth: false, extra: { "x-payment": header } });
      return out({ status: paid.status, ...paid.json });
    }
    case "serve": {
      const ledger = new Ledger(resolve(o.ledger), caps);
      const proxy = new Proxy({ gateway: o.gateway, wallet, apiKey: keyRec?.api_key ?? null, ledger, token: o.token ?? null, defaultMaxTokens: Number(o["default-max-tokens"]), allowOrigins: (process.env.CADRE_PROXY_ALLOW_ORIGINS ?? "").split(",").map((s) => s.trim()).filter(Boolean) });
      await proxy.listen(Number(o.port), o.host);
      process.stderr.write(`cadre-proxy ${wallet.address} on http://${o.host}:${o.port}/v1 -> ${o.gateway} (auth ${keyRec ? "api-key" : "wallet"}, pid ${process.pid})\n`);
      return;
    }
    default:
      throw new Error(`unknown command ${cmd} (serve, wallet, key, balance, topup, spend, config)`);
  }
}

main().catch((e) => {
  process.stderr.write(`cadre-proxy: ${e?.message ?? e}\n`);
  process.exit(1);
});
