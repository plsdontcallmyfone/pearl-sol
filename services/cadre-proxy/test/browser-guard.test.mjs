// cadre-proxy browser guard (security pass 2026-10-21, finding V-6). Port 10699 (CLAUDE.md
// port table, cadre-proxy tests 10696-10699).
//
// Exploit before the fix: on loopback with no --token, any web page the user visits could
// spend the proxy's wallet with a "simple" cross-origin POST (no CORS preflight), e.g.
//   fetch("http://127.0.0.1:10695/v1/chat/completions", {method: "POST", mode: "no-cors",
//         body: JSON.stringify({messages: [...], max_tokens: 4096})})
// and a DNS-rebinding page could also read the replies.
import { test } from "node:test";
import assert from "node:assert/strict";
import { request } from "node:http";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadWallet } from "../src/wallet.mjs";
import { Ledger, parseCaps } from "../src/ledger.mjs";
import { Proxy } from "../src/proxy.mjs";

const PORT = 10699;
const dir = mkdtempSync(join(tmpdir(), "cadre-proxy-guard-"));

function post(headers) {
  return new Promise((ok, fail) => {
    const r = request({ host: "127.0.0.1", port: PORT, method: "POST", path: "/v1/chat/completions", headers: { "content-type": "text/plain", ...headers } }, (res) => {
      let b = "";
      res.on("data", (c) => (b += c));
      res.on("end", () => ok({ status: res.statusCode, body: b }));
    });
    r.on("error", fail);
    r.end(JSON.stringify({ messages: [{ role: "user", content: "x" }], max_tokens: 4 }));
  });
}

test("a web page cannot drive the loopback proxy", async () => {
  const p = new Proxy({ gateway: "http://127.0.0.1:1", wallet: loadWallet(join(dir, "w.json")), ledger: new Ledger(join(dir, "l.json"), parseCaps({})), allowOrigins: ["https://my-app.example"], log: () => {} });
  await p.listen(PORT);
  try {
    const cross = await post({ origin: "https://evil.example" });
    assert.equal(cross.status, 403);
    assert.match(cross.body, /forbidden_origin/);
    const rebound = await post({ host: `evil.example:${PORT}` });
    assert.equal(rebound.status, 403);
    assert.match(rebound.body, /DNS rebinding/);
    // a local client (no Origin, loopback Host) and an allowed origin pass the guard (502: no gateway here)
    assert.equal((await post({})).status, 502);
    assert.equal((await post({ origin: "https://my-app.example" })).status, 502);
  } finally {
    await p.close();
  }
});
