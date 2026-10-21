// dApp provider backend: per-origin permissions, approval windows, and the
// Solana (Wallet Standard) and agent-chain (window.pearl) request handlers.
// The origin always comes from chrome's MessageSender, never from the page.

import { isSocialKind, socialBodyFromJson } from "../chain/social";
import { contractBodyFromJson, dappContractWarnings, isContractKind } from "../chain/contracts";
import { dmBodyFromJson, isDmKind } from "../chain/dm";
import { b58, b64, fromB64, hex, key32 } from "../core/bytes";
import { decodeEither, TxBody, UnsignedTx } from "../chain/tx";
import { signPearlMessage, signSolanaMessage } from "../core/sign";
import { signRaw } from "../core/crypto";
import { insertSignature, parseTransaction, pk } from "../solana/tx";
import { summarizeChainTx, summarizeMessage, summarizeSolanaTx, Summary } from "../shared/summary";
import { MESSAGE_NAMESPACE, NATIVE_DECIMALS, TOKEN_SYMBOL, formatUnits } from "../shared/constants";
import type { ApprovalKind, ApprovalRequest, ChainSide, Permission } from "../shared/types";
import { isUnlocked, seedFor, selectedAccount, touch } from "./keyring";
import { activeNetwork, getLocal, setLocal } from "./store";
import { bridgeConfig, contractSummaryContext, ctx, prepareChainTx, recordActivity, sendChainTx, signChainTx } from "./wallet";

interface Pending {
  req: ApprovalRequest;
  resolve: (ok: boolean) => void;
  windowId?: number;
}
const approvals = new Map<string, Pending>();
const rid = () => hex(crypto.getRandomValues(new Uint8Array(8)));

export const pendingCount = () => approvals.size;
export const getApproval = (id: string) => approvals.get(id)?.req ?? null;
export function listApprovals() {
  return [...approvals.values()].map((p) => p.req);
}

export function resolveApproval(id: string, ok: boolean) {
  const p = approvals.get(id);
  if (!p) return false;
  approvals.delete(id);
  p.resolve(ok);
  if (p.windowId !== undefined) chrome.windows.remove(p.windowId).catch(() => {});
  return true;
}

chrome.windows?.onRemoved.addListener((wid) => {
  for (const [id, p] of approvals) if (p.windowId === wid) {
    approvals.delete(id);
    p.resolve(false);
  }
});

async function ask(kind: ApprovalKind, origin: string, account: string, summary: Summary | null, extra: Partial<ApprovalRequest> = {}): Promise<void> {
  const net = await activeNetwork();
  const req: ApprovalRequest = { id: rid(), kind, origin, account, network: net.name, summary, createdAt: Date.now(), ...extra };
  const ok = await new Promise<boolean>((resolve) => {
    const p: Pending = { req, resolve };
    approvals.set(req.id, p);
    chrome.windows
      .create({ url: chrome.runtime.getURL(`approval.html#${req.id}`), type: "popup", width: 376, height: 640, focused: true })
      .then((w) => {
        p.windowId = w?.id;
      })
      .catch(() => {});
  });
  if (!ok) throw new ProviderError(4001, "user rejected the request");
}

export class ProviderError extends Error {
  constructor(public code: number, message: string) {
    super(message);
  }
}

// ---------------------------------------------------------------- permissions

export async function permissions(): Promise<Permission[]> {
  return Object.values((await getLocal("permissions")) ?? {}).sort((a, b) => b.lastUsed - a.lastUsed);
}
export async function revoke(origin: string) {
  const all = (await getLocal("permissions")) ?? {};
  delete all[origin];
  await setLocal({ permissions: all });
  await broadcast({ event: "disconnect" }, origin);
}
async function permissionFor(origin: string, side: ChainSide): Promise<Permission | null> {
  const all = (await getLocal("permissions")) ?? {};
  const p = all[origin];
  const sel = await selectedAccount();
  if (!p || !sel || p.account !== sel.address || !p[side]) return null;
  p.lastUsed = Date.now();
  await setLocal({ permissions: all });
  return p;
}
async function grant(origin: string, account: string, sides: ChainSide[]) {
  const all = (await getLocal("permissions")) ?? {};
  const prev = all[origin];
  const keep = prev && prev.account === account ? prev : null;
  all[origin] = {
    origin, account, grantedAt: keep?.grantedAt ?? Date.now(), lastUsed: Date.now(),
    solana: sides.includes("solana") || !!keep?.solana, pearl: sides.includes("pearl") || !!keep?.pearl,
  };
  await setLocal({ permissions: all });
}

/** Tell connected tabs that the account changed (or that they lost access). */
export async function broadcast(msg: Record<string, unknown>, onlyOrigin?: string) {
  const tabs = await chrome.tabs.query({});
  const all = (await getLocal("permissions")) ?? {};
  const sel = await selectedAccount();
  for (const t of tabs) {
    if (!t.id || !t.url) continue;
    let origin: string;
    try {
      origin = new URL(t.url).origin;
    } catch {
      continue;
    }
    if (onlyOrigin && origin !== onlyOrigin) continue;
    const p = all[origin];
    let payload = msg;
    if (msg.event === "accountChanged") payload = p && sel && p.account === sel.address ? { event: "accountChanged", address: sel.address, solana: p.solana, pearl: p.pearl } : { event: "disconnect" };
    else if (!onlyOrigin && !p) continue;
    chrome.tabs.sendMessage(t.id, { channel: "pearl-wallet:event", ...payload }).catch(() => {});
  }
}

// ---------------------------------------------------------------- requests

async function requireUnlockedAccount(origin: string, side: ChainSide) {
  const p = await permissionFor(origin, side);
  if (!p) throw new ProviderError(4100, `not connected: call connect() for ${side === "solana" ? "Solana" : "the agent chain"} first`);
  return p.account;
}

async function ensureUnlockedOrAsk(origin: string, account: string, kind: ApprovalKind, summary: Summary | null) {
  // the approval window shows the unlock form first when locked
  await ask(kind, origin, account, summary);
  if (!(await isUnlocked())) throw new ProviderError(4100, "wallet is locked");
  await touch();
}

function pearlTxFromParams(params: any, account: string): Promise<UnsignedTx> | UnsignedTx {
  if (typeof params?.tx === "string") {
    const { tx, signed } = decodeEither(fromB64(params.tx));
    if (signed) throw new ProviderError(-32602, "pass the unsigned signing bytes, not a signed tx");
    return tx;
  }
  const t = params?.transaction;
  if (!t || typeof t.kind !== "string") throw new ProviderError(-32602, "expected { tx: base64 signing bytes } or { transaction: { kind, ... } }");
  let body: TxBody;
  switch (t.kind) {
    case "Transfer":
      body = { kind: "Transfer", to: key32(t.to), amount: BigInt(t.amount) };
      break;
    case "RegisterAgent":
      body = { kind: "RegisterAgent", name: String(t.name), uri: String(t.uri ?? "") };
      break;
    case "BridgeLock":
      body = { kind: "BridgeLock", solanaRecipient: key32(t.solanaRecipient ?? account), amount: BigInt(t.amount) };
      break;
    case "SubmitResult":
      body = { kind: "SubmitResult", jobId: key32(t.jobId), resultHash: key32(t.resultHash), uri: String(t.uri ?? "") };
      break;
    case "ExpireJob":
      body = { kind: "ExpireJob", jobId: key32(t.jobId) };
      break;
    default:
      if (isContractKind(t.kind)) {
        try {
          body = contractBodyFromJson(t);
        } catch (e) {
          throw new ProviderError(-32602, `transaction ${t.kind}: ${(e as Error).message}`);
        }
        break;
      }
      if (isSocialKind(t.kind)) {
        try {
          body = socialBodyFromJson(t);
        } catch (e) {
          throw new ProviderError(-32602, `transaction ${t.kind}: ${(e as Error).message}`);
        }
        break;
      }
      if (isDmKind(t.kind)) {
        try {
          body = dmBodyFromJson(t);
        } catch (e) {
          throw new ProviderError(-32602, `transaction ${t.kind}: ${(e as Error).message}`);
        }
        break;
      }
      throw new ProviderError(-32602, `transaction.kind ${t.kind} is not supported in JSON form; pass { tx } bytes`);
  }
  return prepareChainTx(account, body, t.fee !== undefined ? BigInt(t.fee) : undefined);
}

export async function handleDapp(origin: string, method: string, params: any): Promise<unknown> {
  if (!origin || origin === "null") throw new ProviderError(4100, "requests need a real origin");
  const net = await activeNetwork();
  switch (method) {
    case "connect": {
      const sides: ChainSide[] = (params?.chains ?? ["solana", "pearl"]).filter((s: string) => s === "solana" || s === "pearl");
      const sel = await selectedAccount();
      if (!sel) throw new ProviderError(4100, "the wallet has no account yet");
      const all = (await getLocal("permissions")) ?? {};
      const p = all[origin];
      const have = p && p.account === sel.address && sides.every((s) => p[s]);
      if (have) return { address: sel.address, network: net.id, solanaChain: net.solanaChain };
      if (params?.silent) return { address: null };
      await ask("connect", origin, sel.address, null, { chains: sides });
      await grant(origin, sel.address, sides);
      return { address: sel.address, network: net.id, solanaChain: net.solanaChain };
    }
    case "disconnect": {
      return true;
    }
    case "accounts": {
      const sel = await selectedAccount();
      const p = ((await getLocal("permissions")) ?? {})[origin];
      return p && sel && p.account === sel.address ? { address: sel.address, solana: p.solana, pearl: p.pearl, solanaChain: net.solanaChain } : { address: null };
    }

    // ------------------------------------------------ Solana
    case "solana:signTransaction":
    case "solana:signAndSendTransaction": {
      const account = await requireUnlockedAccount(origin, "solana");
      if (params?.chain && params.chain !== net.solanaChain) throw new ProviderError(4901, `wallet is on ${net.solanaChain}, request is for ${params.chain}`);
      const bytes = fromB64(params.tx);
      const { parsed, message } = parseTransaction(bytes);
      let decimals: number | undefined;
      try {
        decimals = (await bridgeConfig(await ctx())).decimals;
      } catch {
        decimals = undefined;
      }
      const summary = summarizeSolanaTx(parsed, { me: account, bridge: net.programs.bridge, storefront: net.programs.storefront, mintDecimals: decimals });
      const send = method === "solana:signAndSendTransaction";
      await ensureUnlockedOrAsk(origin, account, send ? "solana-sign-send" : "solana-sign", summary);
      const seed = await seedFor(account);
      const signed = insertSignature(bytes, pk(account), signRaw(seed, message));
      if (!send) return { signedTx: b64(signed) };
      const c = await ctx();
      const sig = await c.sol.send(signed);
      await recordActivity({ account, network: net.id, chain: "solana", kind: "dapp", title: summary.headline, ref: sig, status: "pending", origin });
      return { signature: sig };
    }
    case "solana:signMessage": {
      const account = await requireUnlockedAccount(origin, "solana");
      const msg = fromB64(params.message);
      await ensureUnlockedOrAsk(origin, account, "solana-message", summarizeMessage(msg));
      return { signature: b64(signSolanaMessage(await seedFor(account), msg)), address: account };
    }

    // ------------------------------------------------ agent chain
    case "pearl:getAccount": {
      const account = await requireUnlockedAccount(origin, "pearl");
      const c = await ctx();
      const [a, info] = await Promise.all([c.chain.account(account), c.chain.chainInfo()]);
      return { address: account, balance: a.balance, nonce: a.nonce, chainId: info.chain_id, height: info.height };
    }
    case "pearl:signTransaction":
    case "pearl:signAndSendTransaction": {
      const account = await requireUnlockedAccount(origin, "pearl");
      const t = await pearlTxFromParams(params, account);
      if (b58(t.from) !== account) throw new ProviderError(4100, "transaction.from is not the connected account");
      const c = await ctx();
      const info = await c.chain.chainInfo();
      const summary = summarizeChainTx(t, account, info.chain_id, isContractKind(t.body.kind) ? await contractSummaryContext(account, t) : undefined);
      if (t.chainId !== info.chain_id) throw new ProviderError(4901, summary.warnings.join(" "));
      // a site asking to move value into a contract or for an admin action gets a warning up front
      summary.warnings.unshift(...dappContractWarnings(t.body, (v) => `${formatUnits(v, NATIVE_DECIMALS, 9)} ${TOKEN_SYMBOL}`));
      const send = method === "pearl:signAndSendTransaction";
      await ensureUnlockedOrAsk(origin, account, send ? "pearl-sign-send" : "pearl-sign", summary);
      if (!send) {
        const { bytes, hash } = await signChainTx(account, t);
        return { signedTx: b64(bytes), txHash: hex(hash) };
      }
      const txHash = await sendChainTx(account, t, summary.headline, "dapp", origin);
      return { txHash };
    }
    case "pearl:signMessage": {
      const account = await requireUnlockedAccount(origin, "pearl");
      const msg = fromB64(params.message);
      await ensureUnlockedOrAsk(origin, account, "pearl-message", summarizeMessage(msg));
      return { signature: b64(signPearlMessage(await seedFor(account), msg)), address: account, namespace: MESSAGE_NAMESPACE };
    }
  }
  throw new ProviderError(-32601, `unknown method ${method}`);
}
