// Vault lifecycle: create, import, unlock, lock, accounts, auto-lock. The only
// module that touches seeds. Seeds stay in the background: nothing here returns a
// seed to a caller outside this service worker.

import { b58, fromHex, hex } from "../core/bytes";
import { pubkeyOf } from "../core/crypto";
import { VaultAccount, VaultSecrets, decryptVault, encryptVault, isMnemonic, newMnemonic, normalizeMnemonic, parseSecretKey, seedFromMnemonic } from "../core/vault";
import type { PublicAccount } from "../shared/types";
import { clearSession, getLocal, getSession, setLocal, setSession, settings } from "./store";

const LOCK_ALARM = "autolock";
const rid = () => hex(crypto.getRandomValues(new Uint8Array(8)));

/** A freshly generated phrase waits here (memory only) until the user confirms the backup. */
let draftMnemonic: string | null = null;

function toPublic(a: VaultAccount): PublicAccount {
  return { id: a.id, name: a.name, address: b58(pubkeyOf(fromHex(a.seed))), source: a.source, index: a.index };
}

export async function isInitialized() {
  return !!(await getLocal("vault"));
}

export async function isUnlocked() {
  const s = await getSession("secrets");
  if (!s) return false;
  const lockAt = (await getSession("lockAt")) ?? 0;
  if (lockAt && Date.now() > lockAt) {
    await lock();
    return false;
  }
  return true;
}

async function secrets(): Promise<VaultSecrets> {
  if (!(await isUnlocked())) throw new Error("wallet is locked");
  return (await getSession("secrets"))!;
}

async function persist(s: VaultSecrets, password: string) {
  const vault = await encryptVault(s, password);
  const prev = Object.fromEntries(((await getLocal("accounts")) ?? []).map((a) => [a.id, a.name]));
  await setLocal({ vault, accounts: s.accounts.map((a) => ({ ...toPublic(a), name: prev[a.id] ?? a.name })) });
}

export async function touch() {
  const mins = (await settings()).autoLockMinutes;
  if (!(await getSession("secrets"))) return;
  const lockAt = mins > 0 ? Date.now() + mins * 60_000 : 0;
  await setSession({ lockAt });
  if (mins > 0) await chrome.alarms.create(LOCK_ALARM, { when: lockAt + 500 });
  else await chrome.alarms.clear(LOCK_ALARM);
}

export async function onAlarm(name: string) {
  if (name === LOCK_ALARM) await isUnlocked();
}

export async function lock() {
  await clearSession();
  await chrome.alarms.clear(LOCK_ALARM);
}

/** Generates a phrase for the create flow. Shown once so the user can write it down. */
export function generatePhrase(): string {
  draftMnemonic = newMnemonic();
  return draftMnemonic;
}

export async function create(opts: { password: string; mode: "generate" | "mnemonic" | "secret"; mnemonic?: string; secret?: string; name?: string }) {
  if (await isInitialized()) throw new Error("a vault already exists; reset it in Settings first");
  let s: VaultSecrets;
  if (opts.mode === "generate") {
    if (!draftMnemonic) throw new Error("no phrase generated");
    s = { v: 1, mnemonic: draftMnemonic, nextIndex: 1, accounts: [{ id: rid(), name: opts.name || "Account 1", source: "mnemonic", index: 0, seed: hex(seedFromMnemonic(draftMnemonic, 0)) }] };
    draftMnemonic = null;
  } else if (opts.mode === "mnemonic") {
    const m = normalizeMnemonic(opts.mnemonic ?? "");
    if (!isMnemonic(m)) throw new Error("not a valid 12 or 24 word phrase");
    s = { v: 1, mnemonic: m, nextIndex: 1, accounts: [{ id: rid(), name: opts.name || "Account 1", source: "mnemonic", index: 0, seed: hex(seedFromMnemonic(m, 0)) }] };
  } else {
    const seed = parseSecretKey(opts.secret ?? "");
    s = { v: 1, mnemonic: null, nextIndex: 0, accounts: [{ id: rid(), name: opts.name || "Account 1", source: "imported", seed: hex(seed) }] };
  }
  await persist(s, opts.password);
  await setLocal({ selected: s.accounts[0].id });
  await setSession({ secrets: s });
  await touch();
}

export async function unlock(password: string) {
  const vault = await getLocal("vault");
  if (!vault) throw new Error("no vault");
  const s = await decryptVault(vault, password);
  await setSession({ secrets: s });
  await touch();
}

/** Adds an account; needs the password because the vault is re-encrypted. */
export async function addAccount(opts: { password: string; mode: "derive" | "secret" | "generate"; secret?: string; name?: string }): Promise<PublicAccount> {
  const vault = await getLocal("vault");
  if (!vault) throw new Error("no vault");
  const s = await decryptVault(vault, opts.password);
  let acct: VaultAccount;
  const n = s.accounts.length + 1;
  if (opts.mode === "derive") {
    if (!s.mnemonic) throw new Error("this vault has no recovery phrase; import a key instead");
    const index = s.nextIndex;
    acct = { id: rid(), name: opts.name || `Account ${n}`, source: "mnemonic", index, seed: hex(seedFromMnemonic(s.mnemonic, index)) };
    s.nextIndex = index + 1;
  } else if (opts.mode === "generate") {
    acct = { id: rid(), name: opts.name || `Account ${n}`, source: "generated", seed: hex(crypto.getRandomValues(new Uint8Array(32))) };
  } else {
    const seed = parseSecretKey(opts.secret ?? "");
    acct = { id: rid(), name: opts.name || `Imported ${n}`, source: "imported", seed: hex(seed) };
  }
  const addr = toPublic(acct).address;
  if (s.accounts.some((a) => toPublic(a).address === addr)) throw new Error("that account is already in the wallet");
  s.accounts.push(acct);
  await persist(s, opts.password);
  await setSession({ secrets: s });
  await setLocal({ selected: acct.id });
  await touch();
  return toPublic(acct);
}

export async function renameAccount(id: string, name: string) {
  const accts = (await getLocal("accounts")) ?? [];
  const a = accts.find((x) => x.id === id);
  if (!a) throw new Error("no such account");
  a.name = name.slice(0, 32) || a.name;
  await setLocal({ accounts: accts });
  // the encrypted copy keeps its original label; the public list is what the UI shows
}

export async function removeAccount(id: string, password: string) {
  const vault = await getLocal("vault");
  if (!vault) throw new Error("no vault");
  const s = await decryptVault(vault, password);
  if (s.accounts.length <= 1) throw new Error("cannot remove the last account");
  s.accounts = s.accounts.filter((a) => a.id !== id);
  await persist(s, password);
  await setSession({ secrets: s });
  if ((await getLocal("selected")) === id) await setLocal({ selected: s.accounts[0].id });
}

export async function resetVault() {
  await lock();
  await chrome.storage.local.clear();
}

export async function publicAccounts(): Promise<PublicAccount[]> {
  return (await getLocal("accounts")) ?? [];
}

export async function selectedAccount(): Promise<PublicAccount | null> {
  const accts = await publicAccounts();
  const sel = await getLocal("selected");
  return accts.find((a) => a.id === sel) ?? accts[0] ?? null;
}

/** Seed for an address. Internal to the background. */
export async function seedFor(address: string): Promise<Uint8Array> {
  const s = await secrets();
  for (const a of s.accounts) {
    const seed = fromHex(a.seed);
    if (b58(pubkeyOf(seed)) === address) return seed;
  }
  throw new Error("account not in this vault");
}
