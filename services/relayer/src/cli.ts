// Bridge helper CLI (localnet). Uses the same env vars as the relayer (src/config.ts).
//
//   node src/cli.ts escrow-account-json <out.json> [lamports]   --account file for solana-test-validator
//   node src/cli.ts init-bridge <mint> [threshold] [signer,...] admin = TOKEN_PAYER
//   node src/cli.ts init-escrow [threshold] [signer,...]        admin = AGENT_PAYER
//   node src/cli.ts deposit <amount_spl> <agent_recipient> [sender_key]
//   node src/cli.ts lock <amount_lamports> <solana_recipient> [sender_key]
//   node src/cli.ts status                                      snapshot + invariant, no signing
//
// Signers default to the public keys of SIGNER_KEYS.

import { writeFileSync } from "node:fs";
import { PublicKey } from "@solana/web3.js";
import { RpcChain } from "./chain.ts";
import {
  BRIDGE_PROGRAM_ID,
  ESCROW_PROGRAM_ID,
  GENESIS_ESCROW_LAMPORTS,
  bridgeConfigPda,
  bridgeDepositIx,
  bridgeInitializeIx,
  decodeBridgeConfig,
  decodeEscrowConfig,
  escrowConfigPda,
  escrowInitializeIx,
  escrowLockIx,
  escrowPda,
} from "./codec.ts";
import { loadConfig, loadKeypair } from "./config.ts";
import { Relayer } from "./relayer.ts";

const cfg = loadConfig();
const token = new RpcChain("token", cfg.tokenRpc, cfg.commitment);
const agent = new RpcChain("agent", cfg.agentRpc, cfg.commitment);
const [cmd, ...args] = process.argv.slice(2);

function signerSet(arg?: string): PublicKey[] {
  if (arg) return arg.split(",").map((s) => new PublicKey(s));
  return cfg.signerKeyFiles.map((f) => loadKeypair(f).publicKey);
}

async function main(): Promise<void> {
  switch (cmd) {
    case "escrow-account-json": {
      const out = args[0] ?? "escrow-account.json";
      const lamports = BigInt(args[1] ?? GENESIS_ESCROW_LAMPORTS);
      // Written by hand: 1e18 is above 2^53, so it must not pass through a JS number.
      const json = `{
  "pubkey": "${escrowPda().toBase58()}",
  "account": {
    "lamports": ${lamports.toString()},
    "data": ["", "base64"],
    "owner": "${ESCROW_PROGRAM_ID.toBase58()}",
    "executable": false,
    "rentEpoch": 0,
    "space": 0
  }
}
`;
      writeFileSync(out, json);
      console.log(`wrote ${out}: escrow PDA ${escrowPda().toBase58()} with ${lamports} lamports`);
      console.log(`use: solana-test-validator --account ${escrowPda().toBase58()} ${out}`);
      return;
    }
    case "init-bridge": {
      const mint = new PublicKey(args[0]);
      const threshold = Number(args[1] ?? 1);
      const admin = loadKeypair(cfg.tokenPayerFile);
      const sig = await token.send([bridgeInitializeIx(admin.publicKey, mint, signerSet(args[2]), threshold)], admin);
      console.log(`bridge initialized: config ${bridgeConfigPda().toBase58()} tx ${sig}`);
      return;
    }
    case "init-escrow": {
      const threshold = Number(args[0] ?? 1);
      const admin = loadKeypair(cfg.agentPayerFile);
      const sig = await agent.send([escrowInitializeIx(admin.publicKey, signerSet(args[1]), threshold)], admin);
      console.log(`escrow initialized: config ${escrowConfigPda().toBase58()} escrow ${escrowPda().toBase58()} tx ${sig}`);
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
      const lamports = BigInt(args[0]);
      const to = new PublicKey(args[1]);
      const sender = loadKeypair(args[2] ?? cfg.agentPayerFile);
      const [c] = await agent.getAccounts([escrowConfigPda()]);
      if (!c || !c.owner.equals(ESCROW_PROGRAM_ID)) throw new Error("escrow not initialized");
      const ec = decodeEscrowConfig(c.data);
      const ix = escrowLockIx({ sender: sender.publicKey, nonce: ec.withdrawNonce, amountLamports: lamports, solanaRecipient: to });
      const sig = await agent.send([ix], sender);
      console.log(`withdrawal nonce ${ec.withdrawNonce}: ${lamports} lamports -> ${to.toBase58()} tx ${sig}`);
      return;
    }
    case "status": {
      const payer = loadKeypair(cfg.tokenPayerFile);
      const r = new Relayer({ token, agent, signers: [], tokenPayer: payer, agentPayer: payer, cursorFile: cfg.cursorFile,
        genesisEscrowLamports: cfg.genesisEscrowLamports, allowSurplus: cfg.allowSurplus, dryRun: true, log: () => {} });
      const t = await r.tick();
      console.log(JSON.stringify(t, null, 2));
      if (t.refused || t.errors.length) process.exitCode = 1;
      return;
    }
    default:
      console.error("usage: node src/cli.ts escrow-account-json|init-bridge|init-escrow|deposit|lock|status ...");
      process.exitCode = 2;
  }
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
