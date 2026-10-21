// Types shared by the background, popup and approval window.

import type { Network } from "./networks";
import type { Summary } from "./summary";

export interface PublicAccount {
  id: string;
  name: string;
  address: string;
  source: "mnemonic" | "imported" | "generated";
  index?: number;
}

export interface Settings {
  networkId: string;
  autoLockMinutes: number;
  theme: "system" | "light" | "dark";
}

export interface Permission {
  origin: string;
  solana: boolean;
  pearl: boolean;
  account: string;
  grantedAt: number;
  lastUsed: number;
}

export type ChainSide = "solana" | "pearl";

export interface ActivityItem {
  id: string;
  account: string;
  network: string;
  chain: ChainSide;
  kind: string;
  title: string;
  detail?: string;
  /** Solana signature or agent-chain tx hash */
  ref: string;
  /** agent chain: pending, preconfirmed (weak blocks, docs/SPEED.md), confirmed (= included), final */
  status: "pending" | "preconfirmed" | "confirmed" | "final" | "failed";
  error?: string;
  amount?: string;
  at: number;
  origin?: string;
}

export interface PendingTransfer {
  id: string;
  account: string;
  network: string;
  direction: "in" | "out";
  /** SPL base units (6 dp) */
  amountSpl: string;
  createdAt: number;
  updatedAt: number;
  stage: "submitted" | "source-confirmed" | "waiting-final" | "relaying" | "done" | "failed";
  /** in: Solana deposit signature; out: agent-chain tx hash */
  sourceRef: string;
  depositNonce?: number;
  withdrawalNonce?: number;
  sourceHeight?: number;
  confirmations?: number;
  finalityK?: number;
  destRef?: string;
  error?: string;
}

export interface WalletState {
  initialized: boolean;
  locked: boolean;
  accounts: PublicAccount[];
  selected: PublicAccount | null;
  network: Network;
  networks: Network[];
  settings: Settings;
  pendingApprovals: number;
}

export interface Balances {
  sol: string | null;
  token: string | null;
  tokenDecimals: number;
  native: string | null;
  nonce: number | null;
  mint: string | null;
  errors: { solana?: string; chain?: string; bridge?: string };
  chain: { height: number; finalizedHeight: number; finalityK: number; chainId: number } | null;
  agent: { name: string; uri: string; registered_height: number } | null;
}

export type ApprovalKind = "connect" | "solana-sign" | "solana-sign-send" | "solana-message" | "pearl-sign" | "pearl-sign-send" | "pearl-message";

export interface ApprovalRequest {
  id: string;
  kind: ApprovalKind;
  origin: string;
  favicon?: string;
  account: string;
  network: string;
  summary: Summary | null;
  /** for connect */
  chains?: ChainSide[];
  createdAt: number;
}
