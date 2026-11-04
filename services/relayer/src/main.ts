// Relayer entry point. One process serves ONE side (docs/RH-TESTNET-PLAN.md 4.3):
//
//   RELAYER_SIDE unset or "solana"  the Solana relayer, exactly as before (src/solana-main.ts, env in src/config.ts)
//   RELAYER_SIDE=rh                 the Robinhood Chain (EVM) side (src/evm/main.ts, env in src/evm/config.ts)
//
// Each side has its own state file, health port and keys; run two processes for two sides.

const side = (process.env.RELAYER_SIDE ?? "solana").trim().toLowerCase();
if (side === "solana") await import("./solana-main.ts");
else if (side === "rh" || side === "evm") await import("./evm/main.ts");
else {
  console.error(`RELAYER_SIDE must be solana or rh (got ${JSON.stringify(side)})`);
  process.exit(2);
}
