// Local shim of DefiLlama/dimension-adapters `helpers/chains.ts` (an enum there; a const
// object here because Node's type stripping has no enums; `CHAIN.SOLANA` reads the same).
export const CHAIN = {
  SOLANA: "solana",
} as const;
