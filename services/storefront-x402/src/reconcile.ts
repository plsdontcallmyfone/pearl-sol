// Operator tool for payments the service took without placing an Order (crashes before the
// v 2 records, refunds that failed). Run it with the storefront STOPPED: it rewrites the
// state file the service also writes.
//
//   node src/reconcile.ts                      list unserved payments + the wallet's OrderDrafts (read only)
//   node src/reconcile.ts --refund ID[,ID..]   refund those payments (SOL transfer to the payer),
//                                              each only if no unattributed Order names its payer
//   node src/reconcile.ts --check ID[,ID..]    the --refund safety check only (read only)
//   node src/reconcile.ts --cancel-drafts      close every OrderDraft of the wallet (rent back to it)
//
// Env as src/main.ts: TOKEN_RPC, STOREFRONT_WALLET, STATE_FILE, STOREFRONT_PORT (the tool refuses
// while /health answers there, unless --force).

import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { Keypair, PublicKey, SystemProgram } from "@solana/web3.js";
import { RpcChain } from "./chain.ts";
import * as sf from "./storefront.ts";
import { unserved } from "./service.ts";
import type { PaymentRecord } from "./service.ts";

const env = process.env;
const args = process.argv.slice(2);
const flag = (n: string) => args.includes(n);
const val = (n: string) => (args.indexOf(n) >= 0 ? args[args.indexOf(n) + 1] : undefined);
const stateFile = env.STATE_FILE ?? ".data/state.json";
const wallet = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(env.STOREFRONT_WALLET ?? "../../keys/localnet/payer-keypair.json", "utf8"))));
const chain = new RpcChain(env.TOKEN_RPC ?? "http://127.0.0.1:10600");
const A = sf.addresses();
const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const bs58 = { encode(b: Uint8Array) {
  let n = BigInt("0x" + (Buffer.from(b).toString("hex") || "0"));
  let s = "";
  while (n > 0n) { s = B58[Number(n % 58n)] + s; n /= 58n; }
  for (const x of b) { if (x !== 0) break; s = "1" + s; }
  return s;
} };
const DRAFT_DISC = sf.sha256(Buffer.from("account:OrderDraft")).subarray(0, 8);
const ORDER_DISC = sf.sha256(Buffer.from("account:Order")).subarray(0, 8);

const state: { payments: Record<string, PaymentRecord> } = existsSync(stateFile) ? JSON.parse(readFileSync(stateFile, "utf8")) : { payments: {} };
const save = () => {
  writeFileSync(stateFile + ".tmp", JSON.stringify(state, null, 1));
  renameSync(stateFile + ".tmp", stateFile);
};
const out = (v: unknown) => console.log(JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? x.toString() : x), 1));

async function drafts() {
  const r = await chain.conn.getProgramAccounts(A.storefront, {
    filters: [{ memcmp: { offset: 0, bytes: bs58.encode(DRAFT_DISC) } }, { memcmp: { offset: 8, bytes: wallet.publicKey.toBase58() } }],
  });
  return r.map((a) => ({ address: a.pubkey.toBase58(), draftId: Buffer.from(a.account.data).readBigUInt64LE(40), recipient: new PublicKey(a.account.data.subarray(48, 80)).toBase58(), lamports: a.account.lamports }));
}

/** Orders naming `payer` as recipient that no payment record claims. */
async function unattributedOrders(payer: string) {
  const claimed = new Set(Object.values(state.payments).map((p) => p.nonce).filter(Boolean));
  const r = await chain.conn.getProgramAccounts(A.storefront, {
    filters: [{ memcmp: { offset: 0, bytes: bs58.encode(ORDER_DISC) } }, { memcmp: { offset: 84, bytes: payer } }],
  });
  return r.map((a) => sf.decodeOrder(a.account.data)).filter((o) => o.buyer === wallet.publicKey.toBase58() && !claimed.has(o.nonce.toString()));
}

async function main() {
  if (!flag("--force") && (flag("--refund") || flag("--cancel-drafts"))) {
    const port = Number(env.STOREFRONT_PORT ?? 10662);
    const up = await fetch(`http://127.0.0.1:${port}/health`).then(() => true, () => false);
    if (up) throw new Error(`storefront answers on :${port}; stop it first (it rewrites ${stateFile}) or pass --force`);
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
    for (const d of await drafts()) {
      const sig = await chain.send([sf.cancelDraftIx(wallet.publicKey, d.draftId, A)], wallet);
      out({ cancelled: d.address, lamports: d.lamports, sig });
    }
  }
  if (!ids.length && !flag("--cancel-drafts")) {
    const list = Object.entries(state.payments).filter(([, r]) => unserved(r)).map(([id, r]) => ({ id, ...r, at_iso: new Date(r.at).toISOString() }));
    out({ wallet: wallet.publicKey.toBase58(), unserved: list, total_lamports: list.reduce((s, r) => s + BigInt(r.amount), 0n), drafts: await drafts() });
  }
}

main().catch((e) => {
  console.error((e as Error).message);
  process.exit(1);
});
