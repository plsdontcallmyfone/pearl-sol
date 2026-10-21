import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { CxError, displayCx, isAddress, normalizeToBase58, parseAddress, sameAddress, shortAddress, shortCx, toBase58, toCx } from "../src/index.ts";

const V = JSON.parse(readFileSync(new URL("../../../vectors/cx-address.json", import.meta.url), "utf8"));
const hex = (b: Uint8Array) => Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
const unhex = (s: string) => Uint8Array.from(s.match(/../g)!.map((x) => parseInt(x, 16)));

test("shared vectors: encode, short display, base58", () => {
  for (const v of V.valid) {
    const k = unhex(v.hex);
    assert.equal(toCx(k), v.cx, v.note);
    assert.equal(v.cx.length, 66);
    assert.equal(shortCx(k), v.short, v.note);
    assert.equal(toBase58(k), v.base58, v.note);
  }
});

test("round trip: Cx, lowercase Cx and legacy base58 all parse to the same key", () => {
  for (const v of V.valid) {
    assert.equal(hex(parseAddress(v.cx)), v.hex);
    assert.equal(hex(parseAddress(v.cx_lowercase)), v.hex);
    assert.equal(hex(parseAddress(v.base58)), v.hex);
    assert.equal(hex(parseAddress(`  ${v.cx}\n`)), v.hex, "trims whitespace");
    assert.equal(toCx(v.base58), v.cx);
    assert.equal(displayCx(v.cx_lowercase), v.cx, "lowercase displays with checksum");
    assert.equal(normalizeToBase58(v.cx), v.base58);
    assert.ok(sameAddress(v.cx, v.base58));
    assert.equal(shortAddress(v.base58), v.short);
  }
  for (const v of V.lowercase_accepted) assert.equal(toCx(v.input), v.displays_as);
});

test("checksum and malformed input are refused with a clear error code", () => {
  for (const v of V.invalid) {
    assert.throws(() => parseAddress(v.input), (e: unknown) => e instanceof CxError && e.code === v.error && e.message.length > 10, v.note);
    assert.equal(isAddress(v.input), false, v.note);
  }
  const bad = V.invalid.find((v: { error: string }) => v.error === "checksum");
  assert.throws(() => parseAddress(bad.input), /checksum failed/);
  assert.equal(shortAddress("not an address"), "not an address");
  assert.equal(displayCx("nope"), null);
  assert.throws(() => toCx(new Uint8Array(31)), (e: unknown) => e instanceof CxError && e.code === "not_32_bytes");
});

test("owner example key", () => {
  assert.equal(toCx("C1Epvwo91BCyeYr9o9dC2m7t4o5wuLUNSUeZxDQ4ePh7"), "Cxa38030920a564589bDFe976BDB50f37CdeaECBBb34b3934e65b3f1a717bB7d06");
  assert.equal(shortCx("C1Epvwo91BCyeYr9o9dC2m7t4o5wuLUNSUeZxDQ4ePh7"), "Cxa380...7d06");
});
