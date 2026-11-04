// Bridge helper CLI (localnet). Uses the same env vars as the relayer (src/config.ts).
//
//   node src/cli.ts init-bridge <mint> [threshold] [signer,...]   admin = TOKEN_PAYER
//   node src/cli.ts deposit <amount_spl> <chain_recipient> [sender_key]
//   node src/cli.ts lock <amount_chain_units> <solana_recipient> [sender_key]   BridgeLock via the node
//   node src/cli.ts status                                         snapshot + invariant + orders held for refund, no signing
//   node src/cli.ts refund-old <below_nonce> [--send]              one-off (devnet rebuild 2026-11, D9): refund every order
//                                                                  deposit below the relayer cursor whose Order is still Pending
//                                                                  or Expired and has no refund record; dry run without --send
//
// Signers default to the public keys of SIGNER_KEYS.

import { PublicKey } from "@solana/web3.js";
import { KIND, bridgeLockBody, signTx } from "./agent-tx.ts";
import { RpcChain } from "./chain.ts";
import {
  BRIDGE_PROGRAM_ID, ORDER_STATUS_EXPIRED, ORDER_STATUS_PENDING, ata, bridgeConfigPda, bridgeDepositIx, bridgeInitializeIx,
  bridgeRefundIx, bridgeRefundPda, createAtaIdempotentIx, decodeBridgeConfig, decodeDeposit, decodeOrder, depositPda,
  ed25519Instruction, markRefundedIx, orderPda, refundMessage, settleShareIx, sharePda, signEntries,
} from "./codec.ts";
import { loadConfig, loadKeypair } from "./config.ts";
import { NodeRpc } from "./node.ts";
import { Relayer } from "./relayer.ts";

const cfg = loadConfig();
const token = new RpcChain("token", cfg.tokenRpc, cfg.commitment);
const node = NodeRpc.http(cfg.nodeRpc);
const [cmd, ...args] = process.argv.slice(2);

function signerSet(arg?: string): PublicKey[] {
  if (arg) return arg.split(",").map((s) => new PublicKey(s));
  return cfg.signerKeyFiles.map((f) => loadKeypair(f).publicKey);
}

async function main(): Promise<void> {
  switch (cmd) {
    case "init-bridge": {
      const mint = new PublicKey(args[0]);
      const threshold = Number(args[1] ?? 1);
      const admin = loadKeypair(cfg.tokenPayerFile);
      const sig = await token.send([bridgeInitializeIx(admin.publicKey, mint, signerSet(args[2]), threshold)], admin);
      console.log(`bridge initialized: config ${bridgeConfigPda().toBase58()} tx ${sig}`);
      return;
    }
    case "deposit": {
      const amount = BigInt(args[0]);
      const to = new PublicKey(args[1]);
      const sender = loadKeypair(args[2] ?? cfg.tokenPayerFile);
      const [c] = await token.getAccounts([bridgeConfigPda()]);
      if (!c || !c.owner.equals(BRIDGE_PROGRAM_ID)) throw new Error("bridge not initialized");
      const bc = decodeBridgeConfig(c.data);
      const ix = bridgeDepositIx({ payer: sender.publicKey, sender: sender.publicKey, mint: bc.mint, nonce: bc.depositNonce,
        amount, agentRecipient: to, tokenProgram: bc.tokenProgram });
      const sig = await token.send([ix], sender);
      console.log(`deposit nonce ${bc.depositNonce}: ${amount} SPL -> ${to.toBase58()} tx ${sig}`);
      return;
    }
    case "lock": {
      const amount = BigInt(args[0]);
      const to = new PublicKey(args[1]);
      const sender = loadKeypair(args[2] ?? cfg.agentKeyFile);
      const info = await node.getChainInfo();
      const acct = await node.getAccount(sender.publicKey.toBase58());
      const minFee = BigInt(((await node.call("getParams", {})) as { min_fee: string }).min_fee ?? "0");
      const tx = signTx(sender, { chainId: info.chainId, from: sender.publicKey, nonce: acct.nonce, fee: minFee, kind: KIND.BridgeLock },
        bridgeLockBody(to, amount));
      console.log(`BridgeLock ${amount} -> ${to.toBase58()} tx ${await node.sendTx(tx)}`);
      return;
    }
    case "status": {
      const payer = loadKeypair(cfg.tokenPayerFile);
      const r = new Relayer({ token, node, signers: [], tokenPayer: payer, agentKey: payer, stateFile: cfg.stateFile,
        storefrontProgram: cfg.storefrontProgram, totalSupply: cfg.totalSupply, allowSurplus: cfg.allowSurplus, dryRun: true,
        log: () => {} });
      const t = await r.tick();
      console.log(JSON.stringify({ ...t, needsRefundDetail: r.state.needsRefund, refunds: r.state.refunds }, null, 2));
      if (t.refused || t.errors.length) process.exitCode = 1;
      return;
    }
    case "refund-old": {
      // The old chain is gone (devnet rebuild 2026-11), so these deposits can never be unlocked; the relayer's
      // cursor starts above them (D7) and never sees them. Same transaction as Relayer.refundDeposits.
      const below = BigInt(args[0] ?? "NaN");
      const send = args.includes("--send");
      const program = cfg.storefrontProgram;
      const [c] = await token.getAccounts([bridgeConfigPda()]);
      if (!c || !c.owner.equals(BRIDGE_PROGRAM_ID)) throw new Error("bridge not initialized");
      const bc = decodeBridgeConfig(c.data);
      const keys = cfg.signerKeyFiles.map(loadKeypair).filter((k) => bc.signers.some((s) => s.equals(k.publicKey)))
        .slice(0, Math.max(bc.threshold, 1));
      if (keys.length < bc.threshold) throw new Error(`hold ${keys.length} of ${bc.threshold} bridge signer keys`);
      const payer = loadKeypair(cfg.tokenPayerFile);
      const done: string[] = [], skipped: Record<string, string> = {};
      for (let n = 0n; n < below && n < bc.depositNonce; n++) {
        const k = n.toString();
        const [da, oa, ra, sa] = await token.getAccounts([depositPda(n), orderPda(n, program), bridgeRefundPda(n), sharePda(n, program)]);
        if (!da || !da.owner.equals(BRIDGE_PROGRAM_ID)) { skipped[k] = "no deposit"; continue; }
        if (!oa || !oa.owner.equals(program)) { skipped[k] = "plain deposit, no order"; continue; }
        if (ra && ra.owner.equals(BRIDGE_PROGRAM_ID)) { skipped[k] = "already refunded"; continue; }
        const d = decodeDeposit(da.data), order = decodeOrder(oa.data);
        if (order.status !== ORDER_STATUS_PENDING && order.status !== ORDER_STATUS_EXPIRED) { skipped[k] = `order status ${order.status}`; continue; }
        if (order.depositNonce !== n || order.tokens !== d.amount) { skipped[k] = "order does not match deposit"; continue; }
        if (!send) { done.push(`${k} ${order.buyer.toBase58()} ${d.amount}`); continue; }
        const ixs = [
          createAtaIdempotentIx(payer.publicKey, order.buyer, bc.mint, bc.tokenProgram),
          ed25519Instruction(signEntries(keys, refundMessage(n, order.buyer, d.amount))),
          bridgeRefundIx({ payer: payer.publicKey, mint: bc.mint, nonce: n, buyer: order.buyer,
            buyerToken: ata(order.buyer, bc.mint, bc.tokenProgram), tokenProgram: bc.tokenProgram, order: orderPda(n, program) }),
          markRefundedIx(n, program),
        ];
        if (sa && sa.owner.equals(program)) ixs.push(settleShareIx(n, order.buyer, program));
        try { done.push(`${k} ${await token.send(ixs, payer)}`); } catch (e) { skipped[k] = `send failed: ${e instanceof Error ? e.message : e}`; }
      }
      console.log(JSON.stringify({ mode: send ? "sent" : "dry run", count: done.length, refunds: done, skipped }, null, 2));
      return;
    }
    default:
      console.error("usage: node src/cli.ts init-bridge|deposit|lock|status|refund-old ...");
      process.exitCode = 2;
  }
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
