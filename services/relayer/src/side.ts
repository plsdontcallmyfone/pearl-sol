// What every relayer side offers its daemon (src/solana-main.ts, src/evm/main.ts): one tick
// at a time and a /health status. The Solana `Relayer` and the RH `EvmRelayer` both fit it;
// test/evm.test.ts checks that at compile time.

export interface RelayerSide<T = unknown> {
  tick(): Promise<T>;
  health(pollMs: number): Record<string, unknown>;
}
