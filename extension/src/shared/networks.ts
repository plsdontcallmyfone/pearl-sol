// Network presets. Every field is editable in Settings; program ids are configurable.
// The token mint is read from the bridge Config unless overridden.

export interface Network {
  id: string;
  name: string;
  /** Wallet Standard chain id for the Solana side */
  solanaChain: "solana:localnet" | "solana:devnet" | "solana:testnet" | "solana:mainnet";
  solanaRpc: string;
  chainRpc: string;
  programs: {
    bridge: string;
    storefront: string;
  };
  /** optional override; empty = read bridge Config.mint */
  mint: string;
  /** blocks scanned back for the mining rewards view */
  rewardScanDepth: number;
  builtin?: boolean;
}

export const LOCALNET: Network = {
  id: "localnet",
  name: "Localnet",
  solanaChain: "solana:localnet",
  solanaRpc: "http://127.0.0.1:10600",
  chainRpc: "http://127.0.0.1:10630",
  programs: {
    bridge: "7LgyoRiKjNXD1AkMQUePb7TzTvRewEXKKLVzXB5vEjc8",
    storefront: "DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr",
  },
  mint: "",
  rewardScanDepth: 400,
  builtin: true,
};

export const PRESETS: Network[] = [LOCALNET];

export const SOLANA_CHAINS = ["solana:localnet", "solana:devnet", "solana:testnet", "solana:mainnet"] as const;
