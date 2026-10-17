// MV3 service worker: routes popup / approval / content-script messages.

import * as K from "./keyring";
import * as W from "./wallet";
import * as D from "./dapp";
import { getLocal, networks, setLocal, settings, activeNetwork } from "./store";
import type { WalletState } from "../shared/types";
import type { Network } from "../shared/networks";

async function state(): Promise<WalletState> {
  const [initialized, unlocked, accounts, selected, network, nets, s] = await Promise.all([
    K.isInitialized(), K.isUnlocked(), K.publicAccounts(), K.selectedAccount(), activeNetwork(), networks(), settings(),
  ]);
  return { initialized, locked: !unlocked, accounts, selected, network, networks: nets, settings: s, pendingApprovals: D.pendingCount() };
}

async function needUnlocked() {
  if (!(await K.isUnlocked())) throw new Error("wallet is locked");
  await K.touch();
  const a = await K.selectedAccount();
  if (!a) throw new Error("no account");
  return a.address;
}

type Handler = (m: any) => Promise<unknown>;
const ui: Record<string, Handler> = {
  state: () => state(),
  generatePhrase: async () => K.generatePhrase(),
  create: async (m) => (await K.create(m), state()),
  unlock: async (m) => (await K.unlock(m.password), state()),
  lock: async () => (await K.lock(), state()),
  addAccount: async (m) => (await K.addAccount(m), await D.broadcast({ event: "accountChanged" }), state()),
  renameAccount: async (m) => (await K.renameAccount(m.id, m.name), state()),
  removeAccount: async (m) => (await K.removeAccount(m.id, m.password), state()),
  selectAccount: async (m) => {
    await setLocal({ selected: m.id });
    await D.broadcast({ event: "accountChanged" });
    return state();
  },
  reset: async () => (await K.resetVault(), state()),

  saveSettings: async (m) => {
    await setLocal({ settings: { ...(await settings()), ...m.settings } });
    await K.touch();
    return state();
  },
  saveNetwork: async (m) => {
    const n = m.network as Network;
    for (const u of [n.solanaRpc, n.chainRpc]) new URL(u);
    const saved = ((await getLocal("networks")) ?? []).filter((x) => x.id !== n.id);
    saved.push({ ...n, builtin: false });
    await setLocal({ networks: saved });
    return state();
  },
  deleteNetwork: async (m) => {
    await setLocal({ networks: ((await getLocal("networks")) ?? []).filter((x) => x.id !== m.id) });
    const s = await settings();
    if (s.networkId === m.id) await setLocal({ settings: { ...s, networkId: "localnet" } });
    return state();
  },
  selectNetwork: async (m) => {
    await setLocal({ settings: { ...(await settings()), networkId: m.id } });
    return state();
  },

  balances: async () => W.balances(await needUnlocked()),
  activity: async () => W.activity(await needUnlocked()),
  pending: async () => W.refreshPending(await needUnlocked()),
  sendSol: async (m) => W.sendSol(await needUnlocked(), m.to, BigInt(m.amount)),
  sendToken: async (m) => W.sendToken(await needUnlocked(), m.to, BigInt(m.amount)),
  sendNative: async (m) => W.sendNative(await needUnlocked(), m.to, BigInt(m.amount)),
  bridgeIn: async (m) => W.bridgeIn(await needUnlocked(), BigInt(m.amountSpl), m.recipient),
  bridgeOut: async (m) => W.bridgeOut(await needUnlocked(), BigInt(m.amount), m.recipient),
  registerAgent: async (m) => W.registerAgent(await needUnlocked(), m.name, m.uri),
  listJobs: async (m) => W.listJobs(m.status, m.offset ?? 0, m.limit ?? 50),
  getJob: async (m) => W.getJob(m.jobId),
  preparePostJob: async (m) => {
    const p = await W.preparePostJob(await needUnlocked(), m.input);
    return { jobId: p.jobId, jobKey: p.jobKey, hashA: p.hashA, hashB: p.hashB, termsHash: p.termsHash, nonce: p.tx.nonce.toString(), fee: p.tx.fee.toString() };
  },
  postJob: async (m) => W.postJob(await needUnlocked(), m.input),
  storefrontQuote: async (m) => W.storefrontQuote(BigInt(m.solIn)),
  storefrontBuy: async (m) => W.storefrontBuy(await needUnlocked(), m.input),
  storefrontOrder: async (m) => W.storefrontOrder(m.nonce),
  miningRewards: async (m) => W.miningRewards(await needUnlocked(), !!m.force),

  permissions: async () => D.permissions(),
  revoke: async (m) => (await D.revoke(m.origin), D.permissions()),
  getApproval: async (m) => D.getApproval(m.id),
  listApprovals: async () => D.listApprovals(),
  resolveApproval: async (m) => {
    if (m.approve && !(await K.isUnlocked())) throw new Error("unlock first");
    return D.resolveApproval(m.id, !!m.approve);
  },
};

const isExtensionPage = (s: chrome.runtime.MessageSender) => s.id === chrome.runtime.id && !!s.url?.startsWith(chrome.runtime.getURL(""));

chrome.runtime.onMessage.addListener((msg, sender, reply) => {
  (async () => {
    if (msg?.channel === "pearl-wallet:dapp") {
      // from our content script: origin is taken from the sender, not the page
      if (sender.id !== chrome.runtime.id || !sender.tab) throw new Error("bad sender");
      const origin = sender.origin ?? (sender.url ? new URL(sender.url).origin : "");
      return D.handleDapp(origin, String(msg.method), msg.params);
    }
    if (msg?.channel === "pearl-wallet:ui") {
      if (!isExtensionPage(sender)) throw new Error("ui messages only from extension pages");
      const h = ui[msg.type];
      if (!h) throw new Error(`unknown ui call ${msg.type}`);
      return h(msg);
    }
    throw new Error("unknown message");
  })().then(
    (result) => reply({ ok: true, result }),
    (e) => reply({ ok: false, error: e instanceof Error ? e.message : String(e), code: (e as { code?: number }).code }),
  );
  return true;
});

chrome.alarms.onAlarm.addListener((a) => {
  void K.onAlarm(a.name);
  if (a.name === "pending") void W.refreshPending().catch(() => {});
});
chrome.runtime.onInstalled.addListener(() => {
  void chrome.alarms.create("pending", { periodInMinutes: 1 });
});
