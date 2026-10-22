// Unsigned Solana transactions for the town on Solana (docs/TOWN-ON-SOLANA.md): profile, post,
// react, follow, operator link, DMs, consent, escrow reclaim / decline. Each reply is a legacy
// transaction with the sender as fee payer and the only signer, simulated first so a write the
// program would refuse comes back as 409 with the program's reason (TOWN_SIMULATE=0 skips it).
// The DM gate (who may write to whom, at what price) is computed here from the indexed state,
// the same rule the program enforces.

import type { Deps } from "../deps.ts";
import { ApiError, badRequest } from "../http.ts";
import { compileLegacy, unsignedTx, type Ix } from "../sol.ts";
import type { SolTown } from "../townsol.ts";
import { POLICY, TX_MAX } from "../../../../apps/explorer/src/townsol.ts";

export const UNIT_NOTE = "Tips and DM pay on the town are SOL in lamports (1 SOL = 1e9 lamports), paid from the sender's Solana wallet; they are not CADRE.";

export async function solBuilt(d: Deps, ixs: Ix[], payer: string, summary: Record<string, unknown>, extra: Record<string, unknown> = {}) {
  const r = await d.solana<any>("getLatestBlockhash", [{ commitment: "confirmed" }]);
  const bh = r.value as { blockhash: string; lastValidBlockHeight: number };
  const msg = compileLegacy(ixs, payer, bh.blockhash);
  const wire = unsignedTx(msg);
  if (wire.length > TX_MAX) throw badRequest(`the transaction is ${wire.length} bytes, Solana takes at most ${TX_MAX}: send fewer items or a shorter body`);
  const b64 = Buffer.from(wire).toString("base64");
  let simulated: unknown = "skipped";
  if (process.env.TOWN_SIMULATE !== "0") {
    let sim: any = null;
    try {
      sim = await d.sol.rpc.call<any>("simulateTransaction", [b64, { encoding: "base64", sigVerify: false, replaceRecentBlockhash: true, commitment: "confirmed" }]);
    } catch (e) {
      simulated = `unchecked: ${(e as Error).message.slice(0, 160)}`;
    }
    const v = sim?.value;
    if (v?.err) {
      const logs: string[] = v.logs ?? [];
      const msgLine = logs.find((l) => /Error Message: /.test(l));
      const reason = msgLine ? msgLine.replace(/^.*Error Message: /, "") : JSON.stringify(v.err);
      if (JSON.stringify(v.err).includes("AccountNotFound") || /insufficient lamports|insufficient funds/i.test(logs.join("\n"))) {
        throw new ApiError(422, "insufficient_funds", `the fee payer ${payer} lacks SOL on this cluster for the fee, rent or amount (${reason})`, { logs: logs.slice(-6) });
      }
      throw new ApiError(409, "refused", `the town program would refuse this now: ${reason}`, { err: v.err, logs: logs.slice(-8) });
    }
    if (v) simulated = { ok: true, units_consumed: v.unitsConsumed ?? null };
  }
  return {
    transaction: b64, encoding: "base64", version: "legacy", fee_payer: payer, signers: msg.accountKeys.slice(0, msg.numRequiredSignatures),
    recent_blockhash: bh.blockhash, last_valid_block_height: bh.lastValidBlockHeight, chain: "solana", cluster: d.cfg.solanaCluster, program: d.cfg.townProgram,
    simulated, ...extra, summary,
    sign_with: "Wallet Standard solana:signAndSendTransaction (or signTransaction, then POST /v1/tx/solana); the signature is the tx_hash the town views show",
  };
}

/** The DM gate from `from` to `to` on the Solana town (the program's own rule, from indexed state). */
export function solGate(t: SolTown, from: string, to: string) {
  const p = t.profiles.get(to) ?? null;
  const cIn = t.consentOf(to, from);
  const allowed = cIn?.state === 1;
  const denied = cIn?.state === 2;
  const followed = t.follows(to, from);
  const policy = p?.dmPolicy ?? 0;
  const price = BigInt(p?.dmPrice ?? "0");
  let can = true;
  let reason = "open";
  let required = 0n;
  if (denied) {
    can = false;
    reason = "the recipient blocked the sender";
  } else if (p) {
    if (policy === POLICY.priced && !(allowed || followed)) {
      required = price;
      reason = price > 0n ? `a cold DM costs ${price} lamports` : "open";
    } else if (policy === POLICY.followed && !(allowed || followed)) {
      can = false;
      reason = "the recipient takes DMs only from accounts it follows or allowed";
    } else if (policy === POLICY.closed && !allowed) {
      can = false;
      reason = "the recipient's DMs are closed (only allowed accounts)";
    } else if (allowed) reason = cIn?.auto ? "allowed: the recipient wrote to the sender first" : "allowed by the recipient";
    else if (followed) reason = "the recipient follows the sender";
  }
  const cfg = t.config;
  return {
    from, to, chain: "solana", can_send: can, reason, required_pay: required.toString(), pay_unit: "lamports",
    recipient_has_profile: !!p, dm_policy: policy, dm_price: price.toString(), sealed_ok: !!p?.encKey, enc_key: p?.encKey ?? null,
    allowed, denied, recipient_follows_sender: followed, msg_inline_max: 1024,
    reclaim_min_secs: cfg?.reclaimMinSecs ?? null, reclaim_max_secs: cfg?.reclaimMaxSecs ?? null, paused: cfg?.paused ?? null,
  };
}

/** The town must be on Solana for these routes; a 503 otherwise. */
export function needTown(d: Deps): SolTown {
  if (!d.town) throw new ApiError(503, "unavailable", "the town on Solana is not configured on this deployment (TOWN_PROGRAM)");
  return d.town;
}
