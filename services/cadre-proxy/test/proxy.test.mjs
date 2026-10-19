// cadre-proxy tests against a mock gateway (ports 10696-10699, docs: CLAUDE.md port table).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createHash, createPublicKey, verify as edVerify } from "node:crypto";
import { mkdtempSync, readFileSync, statSync, chmodSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn } from "node:child_process";
import { b58decode, b58encode } from "../src/b58.mjs";
import { loadWallet } from "../src/wallet.mjs";
import { Ledger, parseCaps } from "../src/ledger.mjs";
import { Proxy, estimatePromptTokens } from "../src/proxy.mjs";
import { CLIENTS, configSnippet } from "../src/configs.mjs";
import { nativeTransferTx, paymentHeader } from "../src/pay.mjs";

const GW_PORT = 10696;
const PX_PORT = 10697;
const CLI_PORT = 10698;
const GW = `http://127.0.0.1:${GW_PORT}`;
const PX = `http://127.0.0.1:${PX_PORT}`;
const dir = mkdtempSync(join(tmpdir(), "cadre-proxy-test-"));
const seen = [];
let gwServer;
let price = "10";

function edOk(pub, msg, sig) {
  const key = createPublicKey({ key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), Buffer.from(pub)]), format: "der", type: "spki" });
  return edVerify(null, msg, key, sig);
}

// Independent rebuild of SPEC 5.4's signed bytes (not using src/auth.mjs).
function checkAuth(req, body) {
  if (req.headers.authorization) return { ok: req.headers.authorization === "Bearer cadre_sk_test", kind: "key" };
  const w = req.headers["x-cadre-wallet"], ts = req.headers["x-cadre-timestamp"], sig = req.headers["x-cadre-signature"];
  if (!w || !ts || !sig) return { ok: false };
  const path = req.url.split("?")[0];
  const msg = Buffer.from(`cadre-auth-v1\n${req.method}\n${path}\n${ts}\n${createHash("sha256").update(body).digest("hex")}`);
  return { ok: Math.abs(Number(ts) - Date.now()) < 120_000 && edOk(b58decode(w), msg, Buffer.from(sig, "hex")), kind: "wallet", wallet: w };
}

const STREAM = [
  'data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hel"}}]}\n\n',
  'data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"lo"}}]}\n\n',
  'data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7},"cadre":{"request_id":"req-s","ticket_id":"t1","rows":20,"charged_lamports":"200"}}\n\n',
  "data: [DONE]\n\n",
].join("");

before(async () => {
  gwServer = createServer(async (req, res) => {
    const parts = [];
    for await (const c of req) parts.push(c);
    const body = Buffer.concat(parts);
    const auth = checkAuth(req, body);
    seen.push({ method: req.method, url: req.url, auth, body: body.toString() });
    const json = (s, v, h = {}) => {
      res.writeHead(s, { "content-type": "application/json", ...h });
      res.end(JSON.stringify(v));
    };
    if (req.url === "/v1/models") {
      return json(200, { object: "list", data: [{ id: "llama-3.2-1b-w7a7", object: "model", context_length: 4096, cadre: { model_id: "ab".repeat(32), pricing: { lamports_per_row: price } } }] });
    }
    if (req.url === "/v1/balance/topup") {
      if (req.headers["x-payment"]) return json(201, { ticket_id: "t2", rows: 4096, got_payment: req.headers["x-payment"] });
      return json(402, { x402Version: 1, error: "X-PAYMENT required", accepts: [{ scheme: "exact", network: "solana-localnet", maxAmountRequired: "123456", payTo: b58encode(new Uint8Array(32).fill(7)), asset: "So11111111111111111111111111111111111111112", extra: { feePayer: b58encode(new Uint8Array(32).fill(9)), assetKind: "native", decimals: 9 } }] });
    }
    if (!auth.ok) return json(401, { error: { message: "bad auth", type: "auth", code: "unauthorized" } });
    if (req.url === "/v1/balance") return json(200, { wallet: auth.wallet ?? null, rows_free: 100, tickets: [] });
    if (req.url === "/v1/chat/completions") {
      const b = JSON.parse(body);
      if (b.stream) {
        res.writeHead(200, { "content-type": "text/event-stream", "x-cadre-request": "req-s" });
        // odd split points so the proxy must buffer lines across chunks
        const buf = Buffer.from(STREAM);
        for (let i = 0; i < buf.length; i += 37) res.write(buf.subarray(i, i + 37));
        return res.end();
      }
      return json(200, { id: "c0", object: "chat.completion", choices: [{ index: 0, message: { role: "assistant", content: "hi" }, finish_reason: "stop" }], cadre: { request_id: "req-1", ticket_id: "t1", rows: 30, charged_lamports: "300" } },
        { "x-cadre-request": "req-1", "x-cadre-ticket": "t1", "x-cadre-rows": "30", "x-cadre-charged-lamports": "300" });
    }
    json(404, { error: { message: "no route" } });
  });
  await new Promise((ok) => gwServer.listen(GW_PORT, "127.0.0.1", ok));
});

after(async () => {
  await new Promise((ok) => gwServer.close(ok));
});

async function withProxy(opts, fn) {
  const wallet = loadWallet(join(dir, `w-${opts.name}.json`));
  const ledger = new Ledger(join(dir, `l-${opts.name}.json`), parseCaps(opts.caps ?? {}));
  const p = new Proxy({ gateway: GW, wallet, ledger, token: opts.token ?? null, apiKey: opts.apiKey ?? null, log: () => {} });
  await p.listen(PX_PORT);
  try {
    return await fn({ p, wallet, ledger });
  } finally {
    await p.close();
    await new Promise((r) => setTimeout(r, 50)); // let the client drop pooled sockets to the closed server
  }
}

const chat = (extra = {}, headers = {}) =>
  fetch(`${PX}/v1/chat/completions`, { method: "POST", headers: { "content-type": "application/json", ...headers }, body: JSON.stringify({ model: "llama-3.2-1b-w7a7", messages: [{ role: "user", content: "hello there" }], max_tokens: 16, ...extra }) });

test("wallet-signed forward: signature bytes, receipt headers, ledger booked and persisted", async () => {
  await withProxy({ name: "a" }, async ({ wallet, ledger }) => {
    seen.length = 0;
    const r = await chat();
    assert.equal(r.status, 200);
    assert.equal(r.headers.get("x-cadre-rows"), "30");
    assert.equal(r.headers.get("x-cadre-charged-lamports"), "300");
    assert.equal((await r.json()).choices[0].message.content, "hi");
    const g = seen.find((s) => s.url === "/v1/chat/completions");
    assert.deepEqual([g.auth.ok, g.auth.kind, g.auth.wallet], [true, "wallet", wallet.address]);
    assert.equal(ledger.summary().total_lamports, "300");
    const reread = new Ledger(join(dir, "l-a.json"), parseCaps());
    assert.equal(reread.data.total_lamports, "300");
    assert.equal(reread.data.requests[0].request_id, "req-1");
    // balance is forwarded signed too
    const b = await (await fetch(`${PX}/v1/balance`)).json();
    assert.equal(b.wallet, wallet.address);
  });
});

test("API-key mode sends the bearer key instead of the wallet signature", async () => {
  await withProxy({ name: "k", apiKey: "cadre_sk_test" }, async () => {
    seen.length = 0;
    assert.equal((await chat()).status, 200);
    assert.equal(seen.find((s) => s.url === "/v1/chat/completions").auth.kind, "key");
  });
});

test("SSE streams pass through byte for byte and the last chunk's receipt is booked", async () => {
  await withProxy({ name: "s" }, async ({ ledger }) => {
    const r = await chat({ stream: true });
    assert.equal(r.headers.get("content-type"), "text/event-stream");
    assert.equal(await r.text(), STREAM);
    assert.equal(ledger.summary().total_lamports, "200");
    assert.equal(ledger.summary().total_rows, 20);
  });
});

test("caps: per-request, daily and total refusals happen before the gateway is called", async () => {
  // estimate: ceil(11/3) + 8 + 16 = 28 rows x 10 lamports = 280
  assert.equal(estimatePromptTokens({ messages: [{ role: "user", content: "hello there" }] }, true) + 16, 28);
  await withProxy({ name: "c1", caps: { requestLamports: "279" } }, async () => {
    seen.length = 0;
    const r = await chat();
    assert.equal(r.status, 429);
    const e = await r.json();
    assert.equal(e.error.code, "cadre_spend_cap");
    assert.equal(e.error.type, "insufficient_quota");
    assert.ok(!seen.some((s) => s.url === "/v1/chat/completions"), "gateway not called");
  });
  await withProxy({ name: "c2", caps: { dayLamports: "500" } }, async () => {
    assert.equal((await chat()).status, 200); // books 300
    assert.equal((await chat()).status, 429); // 300 + 280 > 500
  });
  // ledger persisted across restarts: same file, total cap counts earlier spend
  await withProxy({ name: "c2", caps: { totalLamports: "500" } }, async () => {
    assert.equal((await chat()).status, 429);
  });
  await withProxy({ name: "c3", caps: { requestRows: 10 } }, async () => {
    assert.equal((await chat()).status, 429);
  });
  price = null;
  try {
    await withProxy({ name: "c4", caps: { dayLamports: "100000" } }, async ({ p }) => {
      p.models = null;
      const r = await chat();
      assert.equal(r.status, 429, "no published price + lamport cap fails closed");
    });
  } finally {
    price = "10";
  }
});

test("a configured local token is required", async () => {
  await withProxy({ name: "t", token: "local-secret" }, async () => {
    assert.equal((await chat()).status, 401);
    assert.equal((await chat({}, { authorization: "Bearer wrong" })).status, 401);
    assert.equal((await chat({}, { authorization: "Bearer local-secret" })).status, 200);
  });
  const w = loadWallet(join(dir, "w-t.json"));
  const p = new Proxy({ gateway: GW, wallet: w, ledger: new Ledger(null, parseCaps()), log: () => {} });
  assert.throws(() => p.listen(PX_PORT, "0.0.0.0"), /without --token/);
});

test("wallet file is 0600 in a 0700 dir, refused when world-readable, address matches its public key", () => {
  const sub = join(dir, "wallet-dir", "nested");
  const path = join(sub, "w.json");
  const w = loadWallet(path);
  assert.equal(statSync(path).mode & 0o777, 0o600);
  assert.equal(statSync(sub).mode & 0o777, 0o700);
  const arr = JSON.parse(readFileSync(path, "utf8"));
  assert.equal(arr.length, 64);
  assert.equal(b58encode(Uint8Array.from(arr.slice(32))), w.address);
  assert.equal(JSON.stringify(w), JSON.stringify({ address: w.address }), "serializing the wallet never includes the secret");
  chmodSync(path, 0o644);
  assert.throws(() => loadWallet(path), /readable by others/);
});

test("the secret never appears in CLI stdout, stderr, logs or the ledger", async () => {
  const home = join(dir, "cli-home");
  const env = { ...process.env, CADRE_PROXY_HOME: home, CADRE_GATEWAY: GW };
  const run = (args) =>
    new Promise((ok) => {
      const c = spawn(process.execPath, ["src/cli.mjs", ...args], { env, cwd: new URL("..", import.meta.url).pathname });
      let so = "", se = "";
      c.stdout.on("data", (d) => (so += d));
      c.stderr.on("data", (d) => (se += d));
      c.on("close", () => ok(so + se));
    });
  let text = await run(["wallet"]);
  text += await run(["balance"]);
  text += await run(["topup"]);
  for (const c of CLIENTS) text += await run(["config", c]);
  const srv = spawn(process.execPath, ["src/cli.mjs", "serve", "--port", String(CLI_PORT)], { env, cwd: new URL("..", import.meta.url).pathname });
  let srvOut = "";
  srv.stdout.on("data", (d) => (srvOut += d));
  srv.stderr.on("data", (d) => (srvOut += d));
  for (let i = 0; i < 50 && !srvOut.includes("cadre-proxy "); i++) await new Promise((r) => setTimeout(r, 100));
  const r = await fetch(`http://127.0.0.1:${CLI_PORT}/v1/chat/completions`, { method: "POST", body: JSON.stringify({ messages: [{ role: "user", content: "x" }], max_tokens: 4, stream: true }) });
  await r.text();
  await new Promise((r) => setTimeout(r, 100));
  srv.kill("SIGTERM"); // our own child, by handle
  await new Promise((ok) => srv.on("close", ok));
  text += srvOut;
  const arr = JSON.parse(readFileSync(join(home, "wallet.json"), "utf8"));
  const seed = Buffer.from(arr.slice(0, 32));
  const ledgerText = existsSync(join(home, "ledger.json")) ? readFileSync(join(home, "ledger.json"), "utf8") : "";
  for (const needle of [seed.toString("hex"), seed.toString("base64"), b58encode(seed), b58encode(Uint8Array.from(arr)), JSON.stringify(arr), arr.slice(0, 32).join(",")]) {
    assert.ok(!text.includes(needle), "secret leaked to output");
    assert.ok(!ledgerText.includes(needle), "secret leaked to the ledger");
  }
  assert.ok(text.includes(b58encode(Uint8Array.from(arr.slice(32)))), "the public address is shown");
  assert.ok(srvOut.includes('"event":"served"'), srvOut);
});

test("x402 native payment: legacy transfer of exactly the amount, storefront as fee payer", async () => {
  const w = loadWallet(join(dir, "w-pay.json"));
  const req = { scheme: "exact", network: "solana-localnet", maxAmountRequired: "123456", payTo: b58encode(new Uint8Array(32).fill(7)), extra: { feePayer: b58encode(new Uint8Array(32).fill(9)), assetKind: "native" } };
  const bh = b58encode(new Uint8Array(32).fill(3));
  const tx = nativeTransferTx(w, req, bh);
  assert.equal(tx[0], 2);
  assert.deepEqual([...tx.subarray(1, 65)], new Array(64).fill(0), "fee payer signature left empty");
  const msg = tx.subarray(129);
  assert.ok(edOk(w.publicKey, msg, tx.subarray(65, 129)), "payer signature over the message");
  assert.deepEqual([...msg.subarray(0, 4)], [2, 0, 1, 4]);
  assert.equal(Buffer.from(msg.subarray(msg.length - 8)).readBigUInt64LE(), 123456n);
  // cross-check with the storefront's own verifier when its deps are installed
  let x402;
  try {
    x402 = await import("../../storefront-x402/src/x402.ts");
  } catch {}
  if (x402?.verifyExact) {
    const hdr = paymentHeader(w, req, bh);
    const v = x402.verifyExact(x402.decodePaymentHeader(hdr), { ...req, asset: "So11111111111111111111111111111111111111112", resource: "", description: "", mimeType: "", maxTimeoutSeconds: 60, extra: { ...req.extra, decimals: 9 } });
    assert.equal(v.amount, 123456n);
    assert.equal(v.payer.toBase58(), w.address);
    assert.equal(v.needsFeePayerSig, true);
  }
  // CLI topup without --pay only shows the requirements; with --pay refuses a public RPC
  const env = { ...process.env, CADRE_PROXY_HOME: join(dir, "pay-home"), CADRE_GATEWAY: GW };
  const out = await new Promise((ok) => {
    const c = spawn(process.execPath, ["src/cli.mjs", "topup", "--pay", "--sol-rpc", "https://api.devnet.solana.com"], { env, cwd: new URL("..", import.meta.url).pathname });
    let s = "";
    c.stdout.on("data", (d) => (s += d));
    c.stderr.on("data", (d) => (s += d));
    c.on("close", (code) => ok({ s, code }));
  });
  assert.equal(out.code, 1);
  assert.match(out.s, /public chains are refused/);
});

test("config snippets for every client point at the proxy base URL and model", () => {
  for (const c of CLIENTS) {
    const s = configSnippet(c, { baseUrl: "http://127.0.0.1:10695", model: "llama-3.2-1b-w7a7", apiKey: "cadre-local", contextLength: 4096 });
    assert.ok(s.includes("http://127.0.0.1:10695/v1"), c);
    assert.ok(s.includes("llama-3.2-1b-w7a7"), c);
    assert.ok(!s.includes(String.fromCharCode(0x2014)), "no em dashes");
  }
  assert.match(configSnippet("openclaw", { baseUrl: "http://h:1", model: "m", apiKey: "k" }), /"api": "openai-completions"/);
  assert.match(configSnippet("kilo", { baseUrl: "http://h:1", model: "m", apiKey: "k" }), /"baseURL": "http:\/\/h:1\/v1"/);
  assert.throws(() => configSnippet("nope", { baseUrl: "x", model: "m", apiKey: "k" }));
});
