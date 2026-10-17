// chrome.storage wrappers. `local` persists (encrypted vault, public account list,
// settings, permissions, activity). `session` is memory-only and cleared when the
// browser closes; the unlocked secrets live there so a service-worker restart does
// not lock the wallet, and they never touch disk. Session storage is restricted to
// trusted extension contexts (the default access level), so content scripts cannot read it.

import { EncryptedVault, VaultSecrets } from "../core/vault";
import { Network, PRESETS } from "../shared/networks";
import { DEFAULT_AUTOLOCK_MIN } from "../shared/constants";
import type { ActivityItem, PendingTransfer, Permission, PublicAccount, Settings } from "../shared/types";

export interface LocalState {
  vault?: EncryptedVault;
  accounts?: PublicAccount[];
  selected?: string;
  settings?: Settings;
  networks?: Network[];
  permissions?: Record<string, Permission>;
  activity?: ActivityItem[];
  pending?: PendingTransfer[];
  rewardCache?: Record<string, { network: string; scannedTo: number; scannedFrom: number; blocks: { height: number; hash: string; fees: string; subsidy?: string; reward?: string; jobBlock: boolean }[] }>;
  chainNonces?: Record<string, { nonce: number; at: number }>;
}

export interface SessionState {
  secrets?: VaultSecrets;
  lockAt?: number;
}

export async function getLocal<K extends keyof LocalState>(k: K): Promise<LocalState[K]> {
  const r = await chrome.storage.local.get(k);
  return r[k] as LocalState[K];
}
export async function setLocal(patch: Partial<LocalState>) {
  await chrome.storage.local.set(patch);
}
export async function getSession<K extends keyof SessionState>(k: K): Promise<SessionState[K]> {
  const r = await chrome.storage.session.get(k);
  return r[k] as SessionState[K];
}
export async function setSession(patch: Partial<SessionState>) {
  await chrome.storage.session.set(patch);
}
export async function clearSession() {
  await chrome.storage.session.clear();
}

export const defaultSettings = (): Settings => ({ networkId: "localnet", autoLockMinutes: DEFAULT_AUTOLOCK_MIN, theme: "system" });

export async function settings(): Promise<Settings> {
  return { ...defaultSettings(), ...((await getLocal("settings")) ?? {}) };
}
export async function networks(): Promise<Network[]> {
  const saved = (await getLocal("networks")) ?? [];
  const ids = new Set(saved.map((n) => n.id));
  return [...PRESETS.filter((p) => !ids.has(p.id)), ...saved];
}
export async function activeNetwork(): Promise<Network> {
  const [s, all] = await Promise.all([settings(), networks()]);
  return all.find((n) => n.id === s.networkId) ?? all[0];
}
