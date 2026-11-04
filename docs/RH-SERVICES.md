# RH side services: relayer (RELAYER_SIDE=rh) and header-oracle poster

Status 2026-11-04: built and tested locally (unit tests on a fake RH behind a viem transport;
scripts/rh-e2e.sh on anvil + a local agent-node). NOT deployed; nothing has been sent to RH
testnet. Plan: docs/RH-TESTNET-PLAN.md 4.3, 4.5, 4.6; chain side docs/BRIDGE-V2.md; contract
layouts token-evm/FORMATS.md.

## 1. What runs

| Process | Entry | What it does |
|---|---|---|
| RH relayer | `RELAYER_SIDE=rh node src/main.ts` (services/relayer) | RH deposit at the finality block -> BridgeUnlockV2 (ed25519 attesters, order deposits with the OrderBody PostJob body); final domain-2 withdrawal -> `release` (EIP-712 signers) or `releaseProof` (header oracle); order job Completed/Expired K deep -> `finalizeOrder`/`expireOrder` + `settleShare`; held-back orders -> `refundDeposit` + `markRefunded` + `settleShare`; invariant `vault x 1000 == outstanding_2 + 1000 x pending` |
| Header-oracle poster | `node src/evm/oracle-main.ts` | posts attester-signed final headers to CadreHeaderOracle every `ORACLE_EVERY` (10, E9) final blocks, and at once when a final withdrawal waits above the oracle's latest header |
| Solana relayer | `node src/main.ts` (no RELAYER_SIDE) | unchanged (src/solana-main.ts is the old main.ts verbatim) |

One process per side: own state file (`RH_STATE_FILE`, refuses a Solana state file), own
health port, own keys. Env is documented at the top of services/relayer/src/evm/config.ts
and src/evm/oracle-main.ts.

## 2. Box 0 against RH testnet (after the deploy in RH-TESTNET-PLAN 4.6 and BRIDGE-V2.md 7)

Preconditions: `getParams.bridge_v2_active` true on the Cadre devnet, `bridge_domains` lists
domain 2 with chain id 46630 and the deployed CadreBridge address (the relayer refuses to
sign otherwise); token-evm/deployments/rh-testnet.json committed.

Keys (files, 600, owner lode, never printed):
- `/etc/lode/keys/attester.json`: the chain attester (ed25519, same as the Solana relayer).
- `/etc/lode/keys/ops-rh.json`: chain account that sends BridgeUnlockV2 (fee 0). Use a key
  the Solana relayer does not send from (two processes on one account race for nonces).
- `/etc/lode/keys/rh-relayer.key`: secp256k1 gas key for the relayer's RH txs; must be a
  storefront attester or the storefront owner (finalize/expire callers). Fund from the faucet.
- `/etc/lode/keys/rh-oracle-sender.key`: secp256k1 gas key for oracle posts (a different EOA).
- `/etc/lode/keys/rh-attester-*.key`: the secp256k1 keys in the bridge signer set, storefront
  attester set and oracle attester set (Deploy.s.sol SIGNERS / STORE_ATTESTERS / ATTESTERS);
  the box holds `threshold` of them.

`/etc/lode/rh.env`:

    RH_RPC=https://rpc.testnet.chain.robinhood.com
    RH_CHAIN_ID=46630
    RH_DOMAIN=2
    RH_BRIDGE=<deployments/rh-testnet.json bridge>
    RH_STOREFRONT=<storefront>
    RH_HEADER_ORACLE=<headerOracle>
    RH_SIGNER_KEY=/etc/lode/keys/rh-relayer.key
    RH_ORACLE_SENDER_KEY=/etc/lode/keys/rh-oracle-sender.key
    RH_ATTESTER_KEYS=/etc/lode/keys/rh-attester-0.key,/etc/lode/keys/rh-attester-1.key
    RH_FINALITY=safe
    RH_RELEASE_MODE=auto
    RH_REFUND_GRACE_MS=3600000

Units: deploy/systemd/cadre-relayer-rh.service (health 127.0.0.1:10666, state
/var/lib/lode/relayer-rh.json, NODE_RPC 127.0.0.1:10630, SIGNER_KEYS attester.json,
AGENT_KEY ops-rh.json, POLL_MS 10000) and deploy/systemd/cadre-rh-oracle.service (health
10669, ORACLE_EVERY 10). Check both ports with `ss -ltn` on the box first (10668 is the swarm).
Start the oracle first, then the relayer; `curl -s 127.0.0.1:10666/health` must show `ok`,
`last.refused` null and `last.invariant.delta` "0".

The public RPC keeps about 10 minutes of state and rate limits: the relayer reads state at the
`safe` block and scans OrderBody logs one block at a time; POLL_MS 10000 keeps it well under
the limit. Use `finalized` on mainnet (E8).

## 3. Local end to end

    scripts/rh-e2e.sh           # anvil :10750, agent-node :10752-10754, relayer :10755, oracle :10756
    scripts/rh-e2e.sh --keep    # leave it up; scripts/rh-e2e.sh stop

Fresh throwaway keys each run (anvil default accounts are never used; EVM keys get ETH by
`anvil_setBalance`). Steps and timings go to `.rh-e2e/transcript.json`.
