// Wallet helpers for wallet connect. The same ed25519 key is the account on both chains
// (base58 address), so one address reads both sides. The API never holds a user key:
// it returns UNSIGNED transactions (Solana bytes, or the JSON form window.pearl takes),
// and relays transactions the user's wallet already signed.

import { processedWithdrawalPda } from "../../../../apps/explorer/src/decode.ts";
import { Api, ApiError, badRequest, notFound } from "../http.ts";
import type { Deps } from "../deps.ts";
import { intParam } from "./stats.ts";
import { minerStats } from "./mining.ts";
import {
  accountDisc,
  ata,
  BRIDGE_ID,
  bridgeDepositIx,
  compileLegacy,
  computeUnitLimit,
  decodeBridgeDeposit,
  isAddress,
  parseWireTx,
  tokenAccountAmount,
  unsignedTx,
} from "../sol.ts";
import { b58encode } from "../../../../apps/explorer/src/codec.ts";

export function addr(v: unknown, name = "address"): string {
  if (!isAddress(v)) throw badRequest(`${name} must be a base58 32-byte address`);
  return v;
}
export function amount(v: unknown, name: string): bigint {
  const t = typeof v === "number" ? String(v) : v;
  if (typeof t !== "string" || !/^\d{1,20}$/.test(t)) throw badRequest(`${name} must be a decimal string of base units`);
  const n = BigInt(t);
  if (n <= 0n || n >= 2n ** 64n) throw badRequest(`${name} must be in [1, 2^64)`);
  return n;
}

async function recentBlockhash(d: Deps): Promise<{ blockhash: string; lastValidBlockHeight: number }> {
  const r = await d.solana<any>("getLatestBlockhash", [{ commitment: "confirmed" }]);
  return r.value;
}

/** SOL, the CADRE SPL balance (all of the owner's token accounts for the mint) and the ATA address. */
export async function solanaBalances(d: Deps, owner: string) {
  const b = await d.bridge();
  const [sol, accs] = await Promise.all([
    d.solana<any>("getBalance", [owner, { commitment: "confirmed" }]),
    d.solana<any>("getTokenAccountsByOwner", [owner, { mint: b.mint }, { encoding: "base64", commitment: "confirmed" }]),
  ]);
  let spl = 0n;
  const accounts = (accs?.value ?? []).map((a: any) => {
    const amt = tokenAccountAmount(Uint8Array.from(Buffer.from(a.account.data[0], "base64")));
    spl += amt;
    return { address: a.pubkey, amount_spl: amt.toString() };
  });
  return { lamports: String(sol?.value ?? 0), mint: b.mint, token_program: b.tokenProgram, spl: spl.toString(), associated_token_account: ata(owner, b.mint, b.tokenProgram), token_accounts: accounts };
}

/** Bridge transfers for an owner: Solana deposits it sent (with the chain unlock) and chain withdrawals it made (with the Solana release). */
export async function transfersModel(d: Deps, owner: string) {
  const [deps, ev] = await Promise.all([
    d.solana<any[]>("getProgramAccounts", [BRIDGE_ID, { encoding: "base64", commitment: "confirmed", filters: [{ memcmp: { offset: 0, bytes: b58encode(accountDisc("Deposit")) } }, { memcmp: { offset: 16, bytes: owner } }] }]),
    d.chain<any>("getBridgeEvents", { from_nonce: 0, limit: 1000 }).catch(() => ({ withdrawals: [] })),
  ]);
  const deposits = [];
  for (const a of deps ?? []) {
    const dep = decodeBridgeDeposit(a.pubkey, Uint8Array.from(Buffer.from(a.account.data[0], "base64")));
    if (!dep) continue;
    const st = await d.chain<any>("getDeposit", { solana_nonce: Number(dep.nonce) }).catch(() => null);
    const stage = !st ? "unknown" : !st.processed ? "submitted" : st.final ? "final" : "unlocked";
    deposits.push({
      direction: "in",
      solana_nonce: dep.nonce.toString(),
      deposit_account: dep.address,
      sender: dep.sender,
      recipient: dep.recipient,
      amount_spl: dep.amount.toString(),
      amount_chain: (dep.amount * 1000n).toString(),
      slot: dep.slot.toString(),
      stage,
      chain: st && st.processed ? { height: st.height, confirmations: st.confirmations, final: st.final, recipient: st.recipient } : null,
    });
  }
  deposits.sort((a, b) => Number(BigInt(b.solana_nonce) - BigInt(a.solana_nonce)));
  const withdrawals = [];
  for (const w of ev.withdrawals ?? []) {
    if (w.from !== owner && w.solana_recipient !== owner) continue;
    const known = d.ix.withdrawals.get(w.nonce);
    const pda = processedWithdrawalPda(w.nonce);
    let released = known?.released ?? false;
    if (!released) {
      const [p] = await d.accounts([pda]);
      released = !!p;
    }
    withdrawals.push({
      direction: "out",
      nonce: w.nonce,
      from: w.from,
      solana_recipient: w.solana_recipient,
      amount_spl: w.amount_spl,
      amount_chain: w.amount ?? (BigInt(w.amount_spl) * 1000n).toString(),
      height: w.height,
      block_hash: w.block_hash,
      confirmations: w.confirmations ?? null,
      stage: released ? "released" : w.final ? "final" : "submitted",
      processed_account: pda,
      release_txs: known?.releaseTxs ?? [],
    });
  }
  withdrawals.sort((a, b) => b.nonce - a.nonce);
  return {
    owner,
    deposits,
    withdrawals,
    stages: {
      in: "submitted (locked on Solana) -> unlocked (relayer BridgeUnlock on the chain) -> final (K blocks deep)",
      out: "submitted (BridgeLock in a block) -> final (K blocks deep) -> released (Solana bridge Processed record exists; by proof or by signers)",
    },
  };
}

export function registerWallet(api: Api, d: Deps) {
  api.get("/v1/accounts/:addr", async (c) => {
    const a = addr(c.params.addr);
    const [acct, agent, route, txs, sol, sigs] = await Promise.all([
      d.chain<any>("getAccount", { account: a }),
      d.chain<any>("getAgent", { account: a }).catch(() => null),
      d.chain<any>("getRoute", { account: a }).catch(() => null),
      d.chain<any>("getAccountTxs", { account: a, limit: intParam(c.query, "tx_limit", 20, 1, 200) }).catch((e) => ({ error: (e as Error).message })),
      solanaBalances(d, a).catch((e) => ({ error: (e as Error).message })),
      d.solana<any[]>("getSignaturesForAddress", [a, { limit: intParam(c.query, "sig_limit", 20, 1, 100), commitment: "confirmed" }]).catch(() => []),
    ]);
    return {
      address: a,
      chain: { balance: acct.balance, nonce: acct.nonce, route: acct.route ?? route ?? null, chain_id: d.ix.chainInfo?.chain_id ?? null },
      agent,
      solana: sol,
      mining: minerStats(d, a),
      profile: `/v1/agents/${a}`,
      history: { chain: txs, solana: sigs.map((s) => ({ signature: s.signature, slot: s.slot, time: s.blockTime ?? null, ok: !s.err })) },
      notes: { units: "chain balance 9 dp; spl 6 dp; lamports 9 dp. 1 SPL base unit = 1000 chain units." },
    };
  }, { cache: 2 });

  api.get("/v1/accounts/:addr/txs", async (c) => {
    const a = addr(c.params.addr);
    const before = c.query.get("before");
    if (before && !/^\d+:\d+$/.test(before)) throw badRequest("before is the `next` value of the previous page (height:index)");
    return d.chain("getAccountTxs", { account: a, limit: intParam(c.query, "limit", 50, 1, 200), ...(before ? { before } : {}) });
  }, { cache: 2 });

  api.get("/v1/accounts/:addr/balances", async (c) => {
    const a = addr(c.params.addr);
    const [acct, sol] = await Promise.all([d.chain<any>("getAccount", { account: a }), solanaBalances(d, a)]);
    return { address: a, chain: { balance: acct.balance, nonce: acct.nonce }, solana: sol };
  }, { cache: 2 });

  // ---------------------------------------------------------------- bridge

  api.get("/v1/bridge", async () => {
    const b = await d.bridge();
    const [vault] = await d.accounts([b.vault]);
    const esc = await d.chain<any>("getEscrow", {});
    const vaultSpl = vault ? tokenAccountAmount(vault.data) : 0n;
    const outside = BigInt(esc.outside_escrow_spl);
    return {
      program: BRIDGE_ID,
      mint: b.mint,
      vault: b.vault,
      vault_spl: vaultSpl.toString(),
      deposit_nonce: b.depositNonce.toString(),
      signer_threshold: b.threshold,
      chain_escrow: esc,
      invariant: { vault_spl: vaultSpl.toString(), chain_outside_escrow_spl: outside.toString(), delta_spl: (vaultSpl - outside).toString(), note: "a non-zero delta is normal while transfers are in flight (locked on one side, not yet unlocked/released on the other)" },
    };
  }, { cache: 5 });

  api.get("/v1/bridge/transfers", async (c) => transfersModel(d, addr(c.query.get("owner"), "owner")), { cache: 3 });

  api.get("/v1/bridge/quote", async (c) => {
    const dir = c.query.get("direction");
    if (dir !== "in" && dir !== "out") throw badRequest("direction must be in (Solana -> chain) or out (chain -> Solana)");
    const params = await d.chain<any>("getParams", {});
    const info = await d.chain<any>("getChainInfo", {});
    const K = info.finality_k ?? params.finality_k;
    if (dir === "in") {
      const amt = amount(c.query.get("amount_spl"), "amount_spl");
      return {
        direction: "in",
        amount_spl: amt.toString(),
        receive_chain: (amt * 1000n).toString(),
        fees: { solana: "the deposit tx fee (signature fee + any priority fee) paid by the owner", chain: "0 (BridgeUnlock needs no fee)" },
        timing: `the relayer unlocks after the Solana deposit confirms; the unlock is final after ${K} agent-chain blocks (target ${params.target_block_time_ms} ms each)`,
        build: "POST /v1/tx/bridge-deposit {owner, amount_spl, recipient?}",
      };
    }
    const amt = amount(c.query.get("amount"), "amount");
    if (amt % 1000n !== 0n) throw badRequest("amount (chain units, 9 dp) must be a positive multiple of 1000 (1 SPL base unit)");
    return {
      direction: "out",
      amount: amt.toString(),
      receive_spl: (amt / 1000n).toString(),
      fees: { chain: `min_fee ${params.min_fee} chain units (the BridgeLock tx fee)`, solana: "the release is submitted by the relayer or header-relay" },
      timing: `released on Solana after the BridgeLock is ${K} blocks deep (by signers) or once the Solana light client's final height passes it (by proof)`,
      build: "POST /v1/tx/bridge-lock {from, amount, solana_recipient?}",
    };
  }, { cache: 10 });

  api.post("/v1/tx/bridge-deposit", async (c) => {
    const b0 = await c.json();
    const owner = addr(b0.owner, "owner");
    const recipient = b0.recipient === undefined ? owner : addr(b0.recipient, "recipient");
    const amt = amount(b0.amount_spl, "amount_spl");
    const cfg = await d.bridge();
    const bal = await solanaBalances(d, owner);
    const ataAddr = ata(owner, cfg.mint, cfg.tokenProgram);
    const [ataAcc] = await d.accounts([ataAddr]);
    const have = ataAcc ? tokenAccountAmount(ataAcc.data) : 0n;
    if (have < amt) throw new ApiError(422, "insufficient_funds", `the owner's token account holds ${have} SPL base units, the deposit needs ${amt}`, { associated_token_account: ataAddr, spl_total: bal.spl });
    const { blockhash, lastValidBlockHeight } = await recentBlockhash(d);
    const ixs = [computeUnitLimit(100_000), bridgeDepositIx(cfg, owner, amt, recipient)];
    const msg = compileLegacy(ixs, owner, blockhash);
    return {
      transaction: Buffer.from(unsignedTx(msg)).toString("base64"),
      encoding: "base64",
      version: "legacy",
      fee_payer: owner,
      signers: msg.accountKeys.slice(0, msg.numRequiredSignatures),
      recent_blockhash: blockhash,
      last_valid_block_height: lastValidBlockHeight,
      deposit_nonce: cfg.depositNonce.toString(),
      summary: { owner, recipient, amount_spl: amt.toString(), receive_chain: (amt * 1000n).toString(), bridge: BRIDGE_ID, vault: cfg.vault, mint: cfg.mint },
      sign_with: "Wallet Standard solana:signAndSendTransaction (or signTransaction, then POST /v1/tx/solana)",
      note: "the deposit PDA is keyed by the bridge's deposit nonce at build time; if another deposit lands first the tx fails without moving funds: build it again",
    };
  });

  api.post("/v1/tx/bridge-lock", async (c) => {
    const b0 = await c.json();
    const from = addr(b0.from, "from");
    const to = b0.solana_recipient === undefined ? from : addr(b0.solana_recipient, "solana_recipient");
    const amt = amount(b0.amount, "amount");
    if (amt % 1000n !== 0n) throw badRequest("amount (chain units, 9 dp) must be a positive multiple of 1000");
    const [acct, params, info] = await Promise.all([d.chain<any>("getAccount", { account: from }), d.chain<any>("getParams", {}), d.chain<any>("getChainInfo", {})]);
    const fee = BigInt(params.min_fee);
    if (params.bridge_paused) throw new ApiError(409, "bridge_paused", "the bridge is paused on the agent chain");
    if (BigInt(acct.balance) < amt + fee) throw new ApiError(422, "insufficient_funds", `balance ${acct.balance} < amount ${amt} + fee ${fee}`);
    return {
      window_pearl: { method: "signAndSendTransaction", params: { transaction: { kind: "BridgeLock", solanaRecipient: to, amount: amt.toString(), fee: fee.toString() } } },
      chain_id: info.chain_id,
      nonce: acct.nonce,
      fee: fee.toString(),
      summary: { from, solana_recipient: to, amount: amt.toString(), receive_spl: (amt / 1000n).toString() },
      sign_with: "window.pearl.signAndSendTransaction({ transaction: window_pearl.params.transaction }) -> { txHash }; then GET /v1/tx/chain/{txHash}",
    };
  });

  // ---------------------------------------------------------------- tx status + broadcast

  api.get("/v1/tx/chain/:hash", async (c) => {
    const h = c.params.hash;
    if (!/^[0-9a-fA-F]{64}$/.test(h)) throw badRequest("hash is 64 hex");
    const t = await d.chain<any>("getTx", { tx_hash: h.toLowerCase() });
    if (!t) throw notFound("tx not known to the node (not in the mempool or the canonical chain)");
    return t;
  }, { cache: 1 });

  api.get("/v1/tx/solana/:sig", async (c) => {
    const sig = c.params.sig;
    if (!/^[1-9A-HJ-NP-Za-km-z]{64,90}$/.test(sig)) throw badRequest("sig is a base58 Solana signature");
    const st = await d.solana<any>("getSignatureStatuses", [[sig], { searchTransactionHistory: true }]);
    const v = st?.value?.[0] ?? null;
    if (!v) throw notFound("signature not found (not landed yet, expired, or pruned from the RPC's history)");
    return { signature: sig, slot: v.slot, confirmations: v.confirmations, status: v.confirmationStatus ?? null, ok: v.err === null, err: v.err };
  }, { cache: 1 });

  api.post("/v1/tx/chain", async (c) => {
    const b0 = await c.json();
    if (typeof b0.tx !== "string" || !/^[A-Za-z0-9+/]+={0,2}$/.test(b0.tx)) throw badRequest("tx must be the signed transaction, base64");
    const bytes = Buffer.from(b0.tx, "base64");
    if (bytes.length < 1 + 4 + 32 + 8 + 8 + 1 + 64) throw badRequest("tx is too short to be a signed agent-chain tx");
    if (bytes.subarray(bytes.length - 64).every((x) => x === 0)) throw badRequest("tx is not signed (the signature is zero)");
    const r = await d.chain<any>("sendTx", { tx: b0.tx });
    return { tx_hash: r.tx_hash, status: `/v1/tx/chain/${r.tx_hash}` };
  });

  api.post("/v1/tx/solana", async (c) => {
    const b0 = await c.json();
    if (typeof b0.tx !== "string" || !/^[A-Za-z0-9+/]+={0,2}$/.test(b0.tx)) throw badRequest("tx must be the signed transaction, base64");
    let parsed;
    try {
      parsed = parseWireTx(Uint8Array.from(Buffer.from(b0.tx, "base64")));
    } catch (e) {
      throw badRequest(`not a Solana transaction: ${(e as Error).message}`);
    }
    const missing = parsed.signers.filter((_, i) => parsed.sigs[i].every((x) => x === 0));
    if (missing.length) throw badRequest("tx is not fully signed", { unsigned: missing });
    const sig = await d.solana<string>("sendTransaction", [b0.tx, { encoding: "base64", skipPreflight: b0.skip_preflight === true, preflightCommitment: "confirmed", maxRetries: 5 }]);
    return { signature: sig, status: `/v1/tx/solana/${sig}` };
  });

}
