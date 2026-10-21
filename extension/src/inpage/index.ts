// Runs in the page (MAIN world). Exposes:
//  - a Wallet Standard wallet for Solana (chains solana:localnet etc.)
//  - window.pearl for the agent chain
// Every call goes through the content script to the background, which shows an
// approval window. See extension/README.md for the API.

import { b58, b64, fromB58, fromB64 } from "../core/bytes";
import { APP_NAME } from "../shared/constants";
import { AbiField, encodeAbiFields, encodeCallArgs } from "../chain/abi";
import { SOLANA_CHAINS } from "../shared/networks";

const PAGE = "pearl-wallet:page";
const CONTENT = "pearl-wallet:content";

let seq = 0;
const waiting = new Map<number, { resolve: (v: any) => void; reject: (e: any) => void }>();
const listeners: Record<string, Set<(d: any) => void>> = {};

class ProviderError extends Error {
  constructor(public code: number | undefined, message: string) {
    super(message);
  }
}

function request<T = any>(method: string, params?: unknown): Promise<T> {
  const id = ++seq;
  return new Promise<T>((resolve, reject) => {
    waiting.set(id, { resolve, reject });
    window.postMessage({ source: PAGE, id, method, params }, window.location.origin);
  });
}

window.addEventListener("message", (ev) => {
  if (ev.source !== window || ev.data?.source !== CONTENT) return;
  const d = ev.data;
  if (d.event) {
    for (const f of listeners[d.event] ?? []) f(d.data);
    onEvent(d.event, d.data);
    return;
  }
  const w = waiting.get(d.id);
  if (!w) return;
  waiting.delete(d.id);
  if (d.ok) w.resolve(d.result);
  else w.reject(new ProviderError(d.code, d.error));
});

const toBytes = (m: Uint8Array | string) => (typeof m === "string" ? new TextEncoder().encode(m) : m);

// ---------------------------------------------------------------- window.pearl

let pearlAddress: string | null = null;

const pearl = {
  isPearlWallet: true,
  name: APP_NAME,
  get address() {
    return pearlAddress;
  },
  async connect(opts?: { onlyIfTrusted?: boolean }) {
    const r = await request("connect", { chains: ["pearl"], silent: !!opts?.onlyIfTrusted });
    pearlAddress = r.address;
    if (!r.address) throw new ProviderError(4001, "not connected");
    return { address: r.address as string };
  },
  async disconnect() {
    pearlAddress = null;
    await request("disconnect");
  },
  /** { address, balance (base units, 9 dp, decimal string), nonce, chainId, height } */
  getAccount: () => request("pearl:getAccount"),
  /** tx: Uint8Array | base64 of the unsigned signing bytes, or { transaction: { kind, ... } } */
  signTransaction: (input: Uint8Array | string | { transaction: Record<string, unknown> }) => request("pearl:signTransaction", pearlTxParams(input)),
  signAndSendTransaction: (input: Uint8Array | string | { transaction: Record<string, unknown> }) => request("pearl:signAndSendTransaction", pearlTxParams(input)),
  /** sig = ed25519(varint(20) || "pearl-sol:message:v1" || message) */
  async signMessage(message: Uint8Array | string) {
    const r = await request("pearl:signMessage", { message: b64(toBytes(message)) });
    return { signature: fromB64(r.signature), address: r.address as string, namespace: r.namespace as string };
  },
  /**
   * Contract input helpers (the call ABI, crates/cadre-contract-sdk abi.rs): pass the result as
   * `args` of a Call / Instantiate / Migrate transaction, or pass `method` + `fields` instead.
   * Fields: { u8 | u16 | u32 | u64: number | decimal string }, { bool }, { addr: base58 },
   * { hash: hex }, { bytes: hex }, { str: text }.
   */
  contracts: Object.freeze({
    encodeCallArgs: (method: string, fields: AbiField[] = []) => encodeCallArgs(method, fields),
    encodeFields: (fields: AbiField[] = []) => encodeAbiFields(fields),
  }),
  on(event: "accountChanged" | "disconnect", f: (d: any) => void) {
    (listeners[event] ??= new Set()).add(f);
    return () => listeners[event].delete(f);
  },
};

function pearlTxParams(input: Uint8Array | string | { transaction: Record<string, unknown> }) {
  if (input instanceof Uint8Array) return { tx: b64(input) };
  if (typeof input === "string") return { tx: input };
  return { transaction: jsonSafe(input.transaction) };
}
// bigints as decimal strings, bytes (contract code, args) as hex
const jsonSafe = (o: Record<string, unknown>) =>
  JSON.parse(JSON.stringify(o, (_, v) => (typeof v === "bigint" ? v.toString() : v instanceof Uint8Array ? Array.from(v, (x) => x.toString(16).padStart(2, "0")).join("") : v)));

Object.defineProperty(window, "pearl", { value: Object.freeze(pearl), writable: false, configurable: false });

// ---------------------------------------------------------------- Wallet Standard (Solana)

/** Neutral generic glyph (no brand art): an outlined rounded rectangle. */
const ICON = `data:image/svg+xml;base64,${btoa('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="8" fill="#e8e8ea"/><rect x="7" y="10" width="18" height="13" rx="3" fill="none" stroke="#55565c" stroke-width="2"/><circle cx="21" cy="16.5" r="1.6" fill="#55565c"/></svg>')}` as const;

type Account = { address: string; publicKey: Uint8Array; chains: readonly string[]; features: readonly string[]; label?: string };
let solAccount: Account | null = null;
let solChain = "solana:localnet";
const changeListeners = new Set<(p: { accounts?: Account[] }) => void>();

const FEATURES = ["solana:signTransaction", "solana:signAndSendTransaction", "solana:signMessage"] as const;
const makeAccount = (address: string): Account => ({ address, publicKey: fromB58(address), chains: [solChain], features: FEATURES, label: APP_NAME });

function setSol(address: string | null) {
  const prev = solAccount?.address ?? null;
  solAccount = address ? makeAccount(address) : null;
  if (prev !== address) for (const f of changeListeners) f({ accounts: wallet.accounts });
}

function onEvent(event: string, d: any) {
  if (event === "disconnect") {
    setSol(null);
    pearlAddress = null;
  } else if (event === "accountChanged") {
    if (d.solana && solAccount) setSol(d.address);
    if (d.pearl && pearlAddress) pearlAddress = d.address;
  }
}

function check(input: { account: Account; chain?: string }) {
  if (!solAccount || input.account.address !== solAccount.address) throw new ProviderError(4100, "account not connected");
}

const wallet = {
  version: "1.0.0" as const,
  name: APP_NAME,
  icon: ICON,
  get chains() {
    return SOLANA_CHAINS as unknown as string[];
  },
  get accounts() {
    return solAccount ? [solAccount] : [];
  },
  features: {
    "standard:connect": {
      version: "1.0.0",
      async connect(input?: { silent?: boolean }) {
        const r = await request("connect", { chains: ["solana"], silent: !!input?.silent });
        if (r.solanaChain) solChain = r.solanaChain;
        setSol(r.address ?? null);
        return { accounts: wallet.accounts };
      },
    },
    "standard:disconnect": {
      version: "1.0.0",
      async disconnect() {
        setSol(null);
        await request("disconnect");
      },
    },
    "standard:events": {
      version: "1.0.0",
      on(event: "change", f: (p: { accounts?: Account[] }) => void) {
        if (event !== "change") return () => {};
        changeListeners.add(f);
        return () => changeListeners.delete(f);
      },
    },
    "solana:signTransaction": {
      version: "1.0.0",
      supportedTransactionVersions: ["legacy", 0],
      async signTransaction(...inputs: { account: Account; transaction: Uint8Array; chain?: string }[]) {
        const out = [];
        for (const i of inputs) {
          check(i);
          const r = await request("solana:signTransaction", { tx: b64(i.transaction), chain: i.chain });
          out.push({ signedTransaction: fromB64(r.signedTx) });
        }
        return out;
      },
    },
    "solana:signAndSendTransaction": {
      version: "1.0.0",
      supportedTransactionVersions: ["legacy", 0],
      async signAndSendTransaction(...inputs: { account: Account; transaction: Uint8Array; chain: string }[]) {
        const out = [];
        for (const i of inputs) {
          check(i);
          const r = await request("solana:signAndSendTransaction", { tx: b64(i.transaction), chain: i.chain });
          out.push({ signature: fromB58(r.signature) });
        }
        return out;
      },
    },
    "solana:signMessage": {
      version: "1.0.0",
      async signMessage(...inputs: { account: Account; message: Uint8Array }[]) {
        const out = [];
        for (const i of inputs) {
          check(i);
          const r = await request("solana:signMessage", { message: b64(i.message) });
          out.push({ signedMessage: i.message, signature: fromB64(r.signature) });
        }
        return out;
      },
    },
  },
};

// Wallet Standard registration protocol (same as @wallet-standard/wallet registerWallet).
function register() {
  const cb = ({ register }: { register: (w: unknown) => void }) => register(wallet);
  try {
    window.dispatchEvent(new (class extends Event {
      detail = cb;
      constructor() {
        super("wallet-standard:register-wallet", { bubbles: false, cancelable: false, composed: false });
      }
    })());
  } catch (e) {
    console.error("wallet-standard:register-wallet event could not be dispatched", e);
  }
  try {
    window.addEventListener("wallet-standard:app-ready", ({ detail: api }: any) => cb(api));
  } catch (e) {
    console.error("wallet-standard:app-ready listener could not be added", e);
  }
}
register();

// restore a trusted session silently
void request("accounts").then((r) => {
  if (r?.address) {
    if (r.solanaChain) solChain = r.solanaChain;
    if (r.solana) setSol(r.address);
    if (r.pearl) pearlAddress = r.address;
  }
}).catch(() => {});

export { b58 };
