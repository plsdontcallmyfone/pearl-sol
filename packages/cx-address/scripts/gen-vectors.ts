// Writes vectors/cx-address.json (repo root) from this implementation:
//   node packages/cx-address/scripts/gen-vectors.ts
// chain/agent-types (Rust) and every TS copy check against the file, so a change here that
// alters any output fails their tests until the file is regenerated on purpose.
import { writeFileSync } from "node:fs";
import { base58Decode, base58Encode, shortCx, toCx } from "../src/index.ts";

const hex = (b: Uint8Array) => Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
const keys: { note: string; key: Uint8Array }[] = [
  { note: "owner example (base58 C1Epvwo9...)", key: base58Decode("C1Epvwo91BCyeYr9o9dC2m7t4o5wuLUNSUeZxDQ4ePh7") },
  { note: "all zero key", key: new Uint8Array(32) },
  { note: "all 0xff", key: new Uint8Array(32).fill(0xff) },
  { note: "bytes 0x00..0x1f", key: Uint8Array.from({ length: 32 }, (_, i) => i) },
  { note: "bytes 0xe0..0xff", key: Uint8Array.from({ length: 32 }, (_, i) => 0xe0 + i) },
  { note: "0xab repeated", key: new Uint8Array(32).fill(0xab) },
  { note: "leading zero byte then 0xcd", key: Uint8Array.from({ length: 32 }, (_, i) => (i === 0 ? 0 : 0xcd)) },
];
const valid = keys.map(({ note, key }) => ({
  note,
  hex: hex(key),
  base58: base58Encode(key),
  cx: toCx(key),
  cx_lowercase: "Cx" + hex(key),
  short: shortCx(key),
}));

const ex = valid[0];
const flip = (s: string, at: number) => s.slice(0, at) + (s[at] === s[at].toUpperCase() ? s[at].toLowerCase() : s[at].toUpperCase()) + s.slice(at + 1);
const firstLetter = (s: string) => [...s].findIndex((c, i) => i >= 2 && /[a-fA-F]/.test(c));
const invalid = [
  { note: "one letter's case flipped (checksum failure)", input: flip(ex.cx, firstLetter(ex.cx)), error: "checksum" },
  { note: "all uppercase hex is not the checksum casing", input: "Cx" + ex.hex.toUpperCase(), error: "checksum" },
  { note: "one hex digit changed, casing kept", input: ex.cx.slice(0, 65) + (ex.cx[65] === "6" ? "7" : "6"), error: "checksum" },
  { note: "too short (63 hex)", input: ex.cx.slice(0, 65), error: "bad_length" },
  { note: "too long (65 hex)", input: ex.cx + "0", error: "bad_length" },
  { note: "non-hex character", input: "Cx" + "g" + ex.hex.slice(1), error: "bad_hex" },
  { note: "lowercase prefix cx is not a Cx address (and not base58: x is fine but 0 is not)", input: "cx" + ex.hex, error: "bad_base58" },
  { note: "0x prefix is not accepted as an address", input: "0x" + ex.hex, error: "bad_base58" },
  { note: "base58 that is not 32 bytes", input: "3mJr7AoUXx2Wqd", error: "not_32_bytes" },
  { note: "empty", input: "", error: "empty" },
];
const lowercase_accepted = valid.map((v) => ({ input: v.cx_lowercase, hex: v.hex, displays_as: v.cx }));

const out = {
  format: "Cx + 64 hex of the 32-byte ed25519 key; checksum casing: h = blake3(ascii(lowercase hex)), uppercase hex letter i iff nibble i of h >= 8 (nibble 2j = h[j] >> 4, 2j+1 = h[j] & 15). docs/SPEC.md 2.1",
  short: "Cx + first 4 + '...' + last 4 checksummed hex characters",
  valid,
  lowercase_accepted,
  invalid,
};
writeFileSync(new URL("../../../vectors/cx-address.json", import.meta.url), JSON.stringify(out, null, 2) + "\n");
console.log(`wrote ${valid.length} valid, ${invalid.length} invalid vectors`);
