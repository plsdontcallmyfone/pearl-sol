// Operator tool for payments the service took without placing an Order (crashes before the
// v 2 records, refunds that failed).
//
//   node src/reconcile.ts                      list unserved payments + the wallet's OrderDrafts (read only)
//   node src/reconcile.ts --refund ID[,ID..]   refund those payments (SOL transfer to the payer),
//                                              each only if no unattributed Order names its payer.
//                                              Needs the storefront STOPPED: it rewrites the state file.
//   node src/reconcile.ts --check ID[,ID..]    the --refund safety check only (read only)
//   node src/reconcile.ts --cancel-drafts      close every ORPHANED OrderDraft of the wallet (rent back to it).
//                                              Safe while the storefront runs: it does not write the state
//                                              file and skips every draft an unserved payment names (see
//                                              orphanDrafts in service.ts). The running service also does
//                                              this itself every DRAFT_SWEEP_MS.
//
// The tool always exits: every RPC call and confirmation is bounded (src/chain.ts RpcLimits) and a
// watchdog ends it after TOOL_TIMEOUT_MS (default 600000) with exit code 2. (It used to hang after its
// work on web3.js's confirmation websocket; devnet 2026-10-23 the storefront stayed stopped meanwhile.)
//
// Env as src/main.ts: TOKEN_RPC, STOREFRONT_WALLET, STATE_FILE, STOREFRONT_PORT (--refund refuses
// while /health answers there, unless --force), plus TOOL_TIMEOUT_MS, CONFIRM_TIMEOUT_MS (90000), RPC_REQUEST_TIMEOUT_MS (30000).

import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { bs58, RpcChain } from "./chain.ts";
import * as sf from "./storefront.ts";
import { orphanDrafts, unserved } from "./service.ts";
import type { PaymentRecord } from "./service.ts";

const env = process.env;
const watchdog = setTimeout(() => {
  console.error(`reconcile: still running after ${env.TOOL_TIMEOUT_MS ?? 600_000} ms, exiting (check the last lines above for what was done)`);
  process.exit(2);
}, Number(env.TOOL_TIMEOUT_MS ?? 600_000));
watchdog.unref();

const args = process.argv.slice(2);
const flag = (n: string) => args.includes(n);
const val = (n: string) => (args.indexOf(n) >= 0 ? args[args.indexOf(n) + 1] : undefined);
const stateFile = env.STATE_FILE ?? ".data/state.json";
const wallet = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(env.STOREFRONT_WALLET ?? "../../keys/localnet/payer-keypair.json", "utf8"))));
const chain = new RpcChain(env.TOKEN_RPC ?? "http://127.0.0.1:10600", "confirmed", { confirmMs: Number(env.CONFIRM_TIMEOUT_MS ?? 90_000), requestMs: Number(env.RPC_REQUEST_TIMEOUT_MS ?? 30_000) });
const A = sf.addresses();
const ORDER_DISC = sf.sha256(Buffer.from("account:Order")).subarray(0, 8);

type State = { payments: Record<string, PaymentRecord> };
const load = (): State => (existsSync(stateFile) ? JSON.parse(readFileSync(stateFile, "utf8")) : { payments: {} });
let state = load();
const save = () => {
  writeFileSync(stateFile + ".tmp", JSON.stringify(state, null, 1));
  renameSync(stateFile + ".tmp", stateFile);
};
const out = (v: unknown) => console.log(JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? x.toString() : x), 1));
const drafts = () => chain.listDrafts(wallet.publicKey, A.storefront);

/** Orders naming `payer` as recipient that no payment record claims. */
async function unattributedOrders(payer: string) {
  const claimed = new Set(Object.values(state.payments).map((p) => p.nonce).filter(Boolean));
  const r = await chain.conn.getProgramAccounts(A.storefront, {
    filters: [{ memcmp: { offset: 0, bytes: bs58(ORDER_DISC) } }, { memcmp: { offset: 84, bytes: payer } }],
  });
  return r.map((a) => sf.decodeOrder(a.account.data)).filter((o) => o.buyer === wallet.publicKey.toBase58() && !claimed.has(o.nonce.toString()));
}

async function serviceUp() {
  const port = Number(env.STOREFRONT_PORT ?? 10662);
  return fetch(`http://127.0.0.1:${port}/health`, { signal: AbortSignal.timeout(5_000) }).then(() => true, () => false);
}

async function main() {
  if (!flag("--force") && flag("--refund") && (await serviceUp())) {
    throw new Error(`storefront answers on :${env.STOREFRONT_PORT ?? 10662}; stop it first (--refund rewrites ${stateFile}) or pass --force`);
  }
  for (const id of val("--check")?.split(",").filter(Boolean) ?? []) {
    const r = state.payments[id];
    if (!r) throw new Error(`no payment ${id}`);
    out({ id, payer: r.payer, unserved: unserved(r), unattributed_orders: (await unattributedOrders(r.payer)).map((o) => o.nonce) });
  }
  if (flag("--check")) return;
  const ids = val("--refund")?.split(",").filter(Boolean) ?? [];
  for (const id of ids) {
    const r = state.payments[id];
    if (!r) throw new Error(`no payment ${id}`);
    if (!unserved(r)) {
      out({ id, skipped: "already served or refunded", nonce: r.nonce, refund: r.refund });
      continue;
    }
    const orders = await unattributedOrders(r.payer);
    if (orders.length) {
      out({ id, skipped: "an Order for this payer is not claimed by any payment: check it first", orders: orders.map((o) => o.nonce) });
      continue;
    }
    r.refund = await chain.send([SystemProgram.transfer({ fromPubkey: wallet.publicKey, toPubkey: new PublicKey(r.payer), lamports: BigInt(r.amount) })], wallet);
    r.review = undefined;
    save();
    out({ id, payer: r.payer, lamports: r.amount, refund: r.refund });
  }
  if (flag("--cancel-drafts")) {
    const listed = await drafts();
    state = load(); // after listing: a draft seen on chain has its record saved already
    const orphans = orphanDrafts(listed, state.payments);
    out({ drafts: listed.length, orphans: orphans.length, kept: listed.filter((d) => !orphans.includes(d)).map((d) => d.address) });
    for (const d of orphans) {
      try {
        const sig = await chain.send([sf.cancelDraftIx(wallet.publicKey, d.draftId, A)], wallet);
        out({ cancelled: d.address, lamports: d.lamports, sig });
      } catch (e) {
        out({ draft: d.address, error: (e as Error).message.slice(0, 300) });
      }
    }
  }
  if (!ids.length && !flag("--cancel-drafts")) {
    const list = Object.entries(state.payments).filter(([, r]) => unserved(r)).map(([id, r]) => ({ id, ...r, at_iso: new Date(r.at).toISOString() }));
    const ds = await drafts();
    out({ wallet: wallet.publicKey.toBase58(), unserved: list, total_lamports: list.reduce((s, r) => s + BigInt(r.amount), 0n), drafts: ds, orphan_drafts: orphanDrafts(ds, state.payments).map((d) => d.address) });
  }
}

main().then(
  () => process.exit(0),
  (e) => {
    console.error((e as Error).message);
    process.exit(1);
  },
);
