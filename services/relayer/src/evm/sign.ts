// EIP-712 messages the RH contracts check (token-evm/FORMATS.md 3.3, 3.4, 5, 6) and the
// signature bundles they take: 65-byte r || s || v, strictly ascending by signer address.
//
// Digests are computed locally from the contract's chain id and address (the same
// solady EIP712 domain the contracts use: name, version, chainId, verifyingContract);
// test/evm.test.ts checks them against the Solidity digests in token-evm.

import { hashTypedData } from "viem";
import type { Address, Hex, TypedDataDomain } from "viem";
import { privateKeyToAccount, sign, serializeSignature } from "viem/accounts";
import type { PrivateKeyAccount } from "viem/accounts";

export interface EvmKey {
  address: Address;
  account: PrivateKeyAccount;
  /** Raw key, kept only to sign digests. Never logged. */
  readonly pk: Hex;
}

export function evmKey(pk: Hex): EvmKey {
  const account = privateKeyToAccount(pk);
  return { address: account.address, account, pk };
}

const domain = (name: string, version: string, chainId: bigint, verifyingContract: Address): TypedDataDomain =>
  ({ name, version, chainId, verifyingContract });

export const releaseDigest = (chainId: bigint, bridge: Address, nonce: bigint, recipient: Address, amount: bigint): Hex =>
  hashTypedData({
    domain: domain("CadreBridge", "2", chainId, bridge),
    types: { Release: [{ name: "nonce", type: "uint64" }, { name: "recipient", type: "address" }, { name: "amount", type: "uint64" }] },
    primaryType: "Release",
    message: { nonce, recipient, amount },
  });

export const refundDigest = (chainId: bigint, bridge: Address, nonce: bigint, buyer: Address, amount: bigint): Hex =>
  hashTypedData({
    domain: domain("CadreBridge", "2", chainId, bridge),
    types: { Refund: [{ name: "nonce", type: "uint64" }, { name: "buyer", type: "address" }, { name: "amount", type: "uint64" }] },
    primaryType: "Refund",
    message: { nonce, buyer, amount },
  });

export const resultDigest = (chainId: bigint, store: Address, jobId: Hex, deliverer: Hex, resultHash: Hex): Hex =>
  hashTypedData({
    domain: domain("CadreStorefront", "1", chainId, store),
    types: { Result: [{ name: "jobId", type: "bytes32" }, { name: "deliverer", type: "bytes32" }, { name: "resultHash", type: "bytes32" }] },
    primaryType: "Result",
    message: { jobId, deliverer, resultHash },
  });

export const expiredDigest = (chainId: bigint, store: Address, jobId: Hex): Hex =>
  hashTypedData({
    domain: domain("CadreStorefront", "1", chainId, store),
    types: { Expired: [{ name: "jobId", type: "bytes32" }] },
    primaryType: "Expired",
    message: { jobId },
  });

export interface FinalHeaderFields {
  blockHash: Hex;
  height: bigint;
  txRoot: Hex;
  stateRoot: Hex;
  withdrawRoot: Hex;
}

export const finalHeaderDigest = (chainId: bigint, oracle: Address, cadreChainId: bigint, h: FinalHeaderFields): Hex =>
  hashTypedData({
    domain: domain("CadreHeaderOracle", "1", chainId, oracle),
    types: {
      FinalHeader: [
        { name: "cadreChainId", type: "uint64" }, { name: "blockHash", type: "bytes32" }, { name: "height", type: "uint64" },
        { name: "txRoot", type: "bytes32" }, { name: "stateRoot", type: "bytes32" }, { name: "withdrawRoot", type: "bytes32" },
      ],
    },
    primaryType: "FinalHeader",
    message: { cadreChainId, ...h },
  });

/**
 * Local keys that are members of `set`, ascending by address, or null when they cannot
 * reach `threshold`. Exactly `threshold` of them are used.
 */
export function pickSigners(keys: EvmKey[], set: readonly Address[], threshold: number): EvmKey[] | null {
  const members = new Set(set.map((a) => a.toLowerCase()));
  const seen = new Set<string>();
  const mine = keys.filter((k) => {
    const a = k.address.toLowerCase();
    if (!members.has(a) || seen.has(a)) return false;
    seen.add(a);
    return true;
  });
  if (threshold < 1 || mine.length < threshold) return null;
  return sortKeys(mine).slice(0, threshold);
}

export const sortKeys = (keys: EvmKey[]): EvmKey[] =>
  [...keys].sort((a, b) => (BigInt(a.address) < BigInt(b.address) ? -1 : BigInt(a.address) > BigInt(b.address) ? 1 : 0));

/** 65-byte signatures over `digest` (no EIP-191 prefix), in ascending signer order. */
export async function signBundle(keys: EvmKey[], digest: Hex): Promise<Hex[]> {
  const out: Hex[] = [];
  for (const k of sortKeys(keys)) out.push(serializeSignature(await sign({ hash: digest, privateKey: k.pk })));
  return out;
}
