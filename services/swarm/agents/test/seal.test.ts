import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { mkdtempSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { KeyStore, open, seal, x25519FromSeed } from "../src/seal.ts";

test("seal / open round trip; tampering and the wrong key fail", () => {
  const r = x25519FromSeed(randomBytes(32));
  const msg = randomBytes(32);
  const s = seal(msg, r.pub);
  assert.deepEqual(open(s, r), msg);
  const ct = Buffer.from(s.ct, "base64");
  ct[0] ^= 1;
  assert.throws(() => open({ ...s, ct: ct.toString("base64") }, r));
  assert.throws(() => open(s, x25519FromSeed(randomBytes(32))));
  assert.throws(() => open({ ...s, iv: "AA==" }, r), /malformed/);
});

test("key store: 0600 files, encrypted at rest, bound to the agent address, wipe", () => {
  const dir = mkdtempSync(join(tmpdir(), "agents-ks-"));
  const ks = new KeyStore(join(dir, "keys"), join(dir, "master.hex"));
  assert.equal(statSync(join(dir, "master.hex")).mode & 0o777, 0o600);
  const seed = randomBytes(32);
  const a = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
  ks.put(a, seed);
  const f = join(dir, "keys", `${a}.key.json`);
  assert.equal(statSync(f).mode & 0o777, 0o600);
  const raw = readFileSync(f, "utf8");
  assert.ok(!raw.includes(seed.toString("hex")) && !raw.includes(seed.toString("base64")));
  assert.deepEqual(ks.get(a), seed);
  // the file cannot be moved to another agent's name
  const b = "9xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
  writeFileSync(join(dir, "keys", `${b}.key.json`), raw.replace(a, b));
  assert.throws(() => ks.get(b));
  // a second store with the same master file reads it; the seal key is stable
  const ks2 = new KeyStore(join(dir, "keys"), join(dir, "master.hex"));
  assert.deepEqual(ks2.get(a), seed);
  assert.equal(ks2.publicKey, ks.publicKey);
  ks.wipe(a);
  assert.equal(ks.has(a), false);
});
