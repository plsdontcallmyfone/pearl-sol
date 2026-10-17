import { Keypair } from "@solana/web3.js";
import { describe, expect, it } from "vitest";
import { b58, fromB64, fromHex, hex } from "../src/core/bytes";
import { pubkeyOf } from "../src/core/crypto";
import {
  KDF_ITERATIONS, VaultSecrets, decryptVault, encryptVault, isMnemonic, newMnemonic, parseSecretKey, seedFromMnemonic, slip10Derive, slip10Master,
} from "../src/core/vault";

const secrets: VaultSecrets = { v: 1, mnemonic: null, nextIndex: 0, accounts: [{ id: "a", name: "Main", source: "imported", seed: "11".repeat(32) }] };

describe("vault encryption", () => {
  it("round trips with the default KDF and refuses a wrong password", async () => {
    const enc = await encryptVault(secrets, "correct horse battery");
    expect(enc.iterations).toBe(KDF_ITERATIONS);
    expect(fromB64(enc.salt).length).toBe(16);
    expect(fromB64(enc.iv).length).toBe(12);
    expect(JSON.stringify(enc)).not.toContain("1111");
    expect(await decryptVault(enc, "correct horse battery")).toEqual(secrets);
    await expect(decryptVault(enc, "wrong password!")).rejects.toThrow(/wrong password/);
  });
  it("fresh salt and iv per encryption; tampering is detected", async () => {
    const a = await encryptVault(secrets, "password123", 1000);
    const b = await encryptVault(secrets, "password123", 1000);
    expect(a.salt).not.toBe(b.salt);
    expect(a.iv).not.toBe(b.iv);
    const ct = fromB64(a.ct);
    ct[0] ^= 1;
    const { b64 } = await import("../src/core/bytes");
    await expect(decryptVault({ ...a, ct: b64(ct) }, "password123")).rejects.toThrow(/wrong password/);
  });
  it("rejects short passwords", async () => {
    await expect(encryptVault(secrets, "short")).rejects.toThrow(/at least 8/);
  });
});

describe("key import", () => {
  it("base58 64-byte secret key (Solana CLI / Phantom export)", () => {
    const kp = Keypair.generate();
    const seed = parseSecretKey(b58(kp.secretKey));
    expect(b58(pubkeyOf(seed))).toBe(kp.publicKey.toBase58());
  });
  it("base58 32-byte seed and JSON byte array", () => {
    const kp = Keypair.generate();
    expect(b58(pubkeyOf(parseSecretKey(b58(kp.secretKey.slice(0, 32)))))).toBe(kp.publicKey.toBase58());
    expect(b58(pubkeyOf(parseSecretKey(JSON.stringify(Array.from(kp.secretKey)))))).toBe(kp.publicKey.toBase58());
  });
  it("rejects mismatched halves and junk", () => {
    const kp = Keypair.generate();
    const bad = kp.secretKey.slice();
    bad[40] ^= 1;
    expect(() => parseSecretKey(b58(bad))).toThrow(/do not match/);
    expect(() => parseSecretKey("0OIl")).toThrow();
    expect(() => parseSecretKey(b58(new Uint8Array(20)))).toThrow(/32 or 64/);
  });
});

describe("SLIP-0010 ed25519 (spec test vector 1)", () => {
  const seed = fromHex("000102030405060708090a0b0c0d0e0f");
  it("master", () => {
    const m = slip10Master(seed);
    expect(hex(m.key)).toBe("2b4be7f19ee27bbf30c667b642d5f4aa69fd169872f8fc3059c08ebae2eb19e7");
    expect(hex(m.chain)).toBe("90046a93de5380a72b5e45010748567d5ea02bbf6522f979e05c0d8d8ca9fffb");
  });
  it("m/0'", () => {
    const d = slip10Derive(seed, [0x80000000]);
    expect(hex(d.key)).toBe("68e0fe46dfb67e368c75379acec591dad19df3cde26e63b93a8e704f1dade7a3");
    expect(hex(d.chain)).toBe("8b59aa11380b624e81507a27fedda59fea6d0b779a778918a2fd3590e16e9c69");
  });
  it("mnemonic accounts are deterministic and distinct per index", () => {
    const m = newMnemonic();
    expect(isMnemonic(m)).toBe(true);
    expect(hex(seedFromMnemonic(m, 0))).toBe(hex(seedFromMnemonic(`  ${m.toUpperCase()} `, 0)));
    expect(hex(seedFromMnemonic(m, 0))).not.toBe(hex(seedFromMnemonic(m, 1)));
    expect(() => seedFromMnemonic("abandon abandon abandon", 0)).toThrow(/BIP-39/);
  });
});
