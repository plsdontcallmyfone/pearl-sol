// Local shim of DefiLlama/dimension-adapters `helpers/solana.ts` getConnection(): a web3.js
// Connection on SOLANA_RPC (the DefiLlama helper reads the same env name).
import { Connection } from "@solana/web3.js";

let conn: Connection | null = null;
export function getConnection(): Connection {
  return (conn ??= new Connection(process.env.SOLANA_RPC ?? "https://api.mainnet-beta.solana.com", "confirmed"));
}
export function setConnection(c: any) {
  conn = c;
}
