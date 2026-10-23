// GET /v1/wallet/config: serves the signed wallet config file byte for byte, 404 when none is
// published, refuses to serve a file that is not a signed config. No signing key here.
import { test, after } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Server } from "node:http";
import { configFromEnv, Deps } from "../src/deps.ts";
import { build } from "../src/app.ts";

const dir = mkdtempSync(join(tmpdir(), "cadre-api-walletcfg-"));
const servers: Server[] = [];
after(() => {
  for (const s of servers) s.close();
  rmSync(dir, { recursive: true, force: true });
});

async function boot(env: Record<string, string>) {
  const api = build(new Deps(configFromEnv(env, { stateDir: dir })));
  api.o.log = undefined;
  const s = api.listen(0, "127.0.0.1");
  servers.push(s);
  await new Promise((r) => s.on("listening", r));
  return `http://127.0.0.1:${(s.address() as any).port}`;
}

const SIGNED = `{
  "schema": 1,
  "seq": 3,
  "not_before": 1792800000,
  "expires": 1795392000,
  "payload": { "flags": { "swapBuy": false } },
  "signature": { "key_id": "cadre-wallet-config-1", "ed25519": "${"A".repeat(86)}==" }
}
`;

test("serves the published file byte for byte, re-read on every request", async () => {
  const file = join(dir, "wallet-config.json");
  writeFileSync(file, SIGNED);
  const base = await boot({ WALLET_CONFIG_FILE: file });
  const r = await fetch(`${base}/v1/wallet/config`);
  assert.equal(r.status, 200);
  assert.match(r.headers.get("content-type") ?? "", /application\/json/);
  assert.equal(await r.text(), SIGNED);
  const next = SIGNED.replace('"seq": 3', '"seq": 4');
  writeFileSync(file, next);
  assert.equal(await (await fetch(`${base}/v1/wallet/config`)).text(), next);
});

test("404 when no config is published or the file is gone", async () => {
  const none = await boot({});
  assert.equal((await fetch(`${none}/v1/wallet/config`)).status, 404);
  const gone = await boot({ WALLET_CONFIG_FILE: join(dir, "missing.json") });
  assert.equal((await fetch(`${gone}/v1/wallet/config`)).status, 404);
});

test("refuses to serve a file that is not a signed config", async () => {
  for (const [name, body] of [["junk.json", "not json"], ["unsigned.json", '{"schema":1,"seq":1}'], ["big.json", " ".repeat(70 * 1024) + "{}"]]) {
    const file = join(dir, name);
    writeFileSync(file, body);
    const base = await boot({ WALLET_CONFIG_FILE: file });
    const r = await fetch(`${base}/v1/wallet/config`);
    assert.equal(r.status, 500, name);
    const b = await r.json();
    assert.equal(b.error.code, "internal");
  }
});
