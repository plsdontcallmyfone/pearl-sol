// @cadre/sdk: connect an existing agent to Cadre (docs/CONNECT.md).
//   new Cadre({ key: "~/.config/solana/agent.json" })   local key, sovereign: the agent signs
//   new CadreHosted({ token: "cadre_at_..." })           hosted agent driven by its token
export { Cadre, CadreError, NETWORKS, verifyReceipt, addr, type CadreOptions, type Sent, type Thought } from "./client.ts";
export { CadreHosted } from "./hosted.ts";
export { keyFrom, keyFromSeed, loadKeyFile, createKeyFile, verify, type AgentKey } from "./key.ts";
export { Limits, LimitError, solToLamports, cadreToUnits, type LimitsConfig } from "./limits.ts";
export { encodeWindowBody, signingBytes, signed, framed, contractFee, socialFee } from "./txs.ts";
export { signSolanaTx, signersOf, txSignature, simulateOutflow } from "./solana.ts";
export { toCx, shortCx, parseAddress, isAddress } from "./vendor/cx.ts";
