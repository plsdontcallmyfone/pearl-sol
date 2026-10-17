// Key vault crypto: password -> PBKDF2-SHA256 -> AES-256-GCM over the JSON secret
// set. Runs only in the background service worker (and in tests). WebCrypto only.
//
// Mnemonic accounts use the Solana wallet path m/44'/501'/i'/0' (SLIP-0010 ed25519),
// so a Phantom / Solflare phrase gives the same addresses here.

import { hmac } from "@noble/hashes/hmac";
import { sha512 } from "@noble/hashes/sha512";
import { generateMnemonic, mnemonicToSeedSync, validateMnemonic } from "@scure/bip39";
import { wordlist } from "@scure/bip39/wordlists/english";
import { b64, concat, fromB58, fromB64, u32, utf8, fromUtf8 } from "./bytes";
import { seedFromSecret } from "./crypto";

export const KDF_ITERATIONS = 600_000;

export interface VaultAccount {
  id: string;
  name: string;
  source: "mnemonic" | "imported" | "generated";
  /** mnemonic derivation index when source = mnemonic */
  index?: number;
  /** 32-byte ed25519 seed, hex */
  seed: string;
}

export interface VaultSecrets {
  v: 1;
  mnemonic: string | null;
  nextIndex: number;
  accounts: VaultAccount[];
}

export interface EncryptedVault {
  v: 1;
  kdf: "PBKDF2-SHA256";
  iterations: number;
  salt: string;
  iv: string;
  ct: string;
}

const subtle = () => globalThis.crypto.subtle;
const rand = (n: number) => globalThis.crypto.getRandomValues(new Uint8Array(n));

async function deriveKey(password: string, salt: Uint8Array, iterations: number): Promise<CryptoKey> {
  const base = await subtle().importKey("raw", utf8(password), "PBKDF2", false, ["deriveKey"]);
  return subtle().deriveKey({ name: "PBKDF2", hash: "SHA-256", salt, iterations }, base, { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
}

export async function encryptVault(secrets: VaultSecrets, password: string, iterations = KDF_ITERATIONS): Promise<EncryptedVault> {
  if (password.length < 8) throw new Error("password must be at least 8 characters");
  const salt = rand(16);
  const iv = rand(12);
  const key = await deriveKey(password, salt, iterations);
  const ct = new Uint8Array(await subtle().encrypt({ name: "AES-GCM", iv }, key, utf8(JSON.stringify(secrets))));
  return { v: 1, kdf: "PBKDF2-SHA256", iterations, salt: b64(salt), iv: b64(iv), ct: b64(ct) };
}

export async function decryptVault(enc: EncryptedVault, password: string): Promise<VaultSecrets> {
  const key = await deriveKey(password, fromB64(enc.salt), enc.iterations);
  let pt: ArrayBuffer;
  try {
    pt = await subtle().decrypt({ name: "AES-GCM", iv: fromB64(enc.iv) }, key, fromB64(enc.ct));
  } catch {
    throw new Error("wrong password");
  }
  const s = JSON.parse(fromUtf8(new Uint8Array(pt))) as VaultSecrets;
  if (s.v !== 1) throw new Error("unknown vault version");
  return s;
}

// ---------------------------------------------------------------- SLIP-0010 ed25519

const HARDENED = 0x80000000;

export function slip10Master(seed: Uint8Array) {
  const I = hmac(sha512, utf8("ed25519 seed"), seed);
  return { key: I.slice(0, 32), chain: I.slice(32) };
}

export function slip10Derive(seed: Uint8Array, path: number[]) {
  let { key, chain } = slip10Master(seed);
  for (const idx of path) {
    const I = hmac(sha512, chain, concat(Uint8Array.of(0), key, be32(idx | 0) ));
    key = I.slice(0, 32);
    chain = I.slice(32);
  }
  return { key, chain };
}
function be32(n: number) {
  const b = u32(n >>> 0);
  return b.reverse();
}

export const solanaPath = (i: number) => [44 + HARDENED, 501 + HARDENED, i + HARDENED, 0 + HARDENED].map((x) => x >>> 0);

export function seedFromMnemonic(mnemonic: string, index: number): Uint8Array {
  const phrase = normalizeMnemonic(mnemonic);
  if (!validateMnemonic(phrase, wordlist)) throw new Error("not a valid BIP-39 phrase");
  return slip10Derive(mnemonicToSeedSync(phrase), solanaPath(index)).key;
}

export const normalizeMnemonic = (m: string) => m.trim().toLowerCase().split(/\s+/).join(" ");
export const isMnemonic = (m: string) => validateMnemonic(normalizeMnemonic(m), wordlist);
export const newMnemonic = () => generateMnemonic(wordlist, 128);

/** Base58 secret key (64-byte Solana keypair or 32-byte seed), or a JSON byte array like id.json. */
export function parseSecretKey(input: string): Uint8Array {
  const t = input.trim();
  if (t.startsWith("[")) {
    const arr = JSON.parse(t);
    if (!Array.isArray(arr)) throw new Error("expected a JSON byte array");
    return seedFromSecret(Uint8Array.from(arr));
  }
  let raw: Uint8Array;
  try {
    raw = fromB58(t);
  } catch {
    throw new Error("not a base58 string");
  }
  return seedFromSecret(raw);
}
