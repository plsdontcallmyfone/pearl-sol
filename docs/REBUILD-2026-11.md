# Devnet rebuild, November 2026

The three BitLaunch devnet boxes were deleted about 2026-11-01 (negative balance). This is the read-only research result (9 agents, 2026-11-03) and the plan the owner approved.


Source of truth: `plsdontcallmyfone/pearl-sol` main `5faea10`, `cadre-web` main `c67dc53`, `danishseal/cadre-extension` `cadre-chain` `9d4db2646`. Deploy from a fresh clone or a dedicated worktree. `~/pearl-sol` is shared and dirty: `CLAUDE.md` is modified and `deploy/ops/` is untracked. `sync` and `site` refuse to run unless local main equals origin/main.

Your 7-step plan leaves out four things the maps found:
- **The light client can't be re-initialized.** `initialize` uses `init` on fixed-seed PDAs and there's no reset instruction (`token/programs/lightclient/src/lib.rs:1150-1162`). It needs a new program id or an upgrade.
- **The model checkpoint is public.** `unsloth/Llama-3.2-1B-Instruct` is ungated, so no HF token is needed.
- **The Cadre-chain contracts must be redeployed.** That covers halt, market, toolbelt, strategies, swarms and trading.
- **Some lost keys and state were not on your list.** The gateway key, the agents `master.hex`, both relay payers, and 12 to 18 hosted user agents are gone.

## 1. Lost vs survived vs rebuildable

| Item | Status | Where / how |
|---|---|---|
| Chain history, balances, registrations, posts, sales | Lost | Can't be restored. New genesis starts at height 0 |
| cadre-api and explorer indexes | Rebuildable | They re-index the new chain by themselves |
| relayer.json, storefront-state.json, gateway ledger.json, swarm, desktops, agents and practice state, `/var/backups/cadre` | Lost | Start empty. `deploy/backup.sh` output was never copied off the box |
| Box-generated keys: relay.json box 1 `XQUUCs5t` and box 2 `E66ekRjf`, gateway.json `5wHXuYu1`, agents `master.hex`, desktops-stream.key, header-relay work keys | Lost | Regenerate with `devnet-deploy.sh relay-key 1`, `relay-key 2`, `svc-key 0 gateway`, and `devnet-desktops.sh secrets`. cadre-agents creates a new `master.hex` on first start |
| 12 to 18 hosted user agents (custodial keys, the maps disagree on the count) | Lost | Their keys were sealed with the lost `master.hex`. Owners must re-create them |
| Five hand-written drop-ins and TOWN_PROGRAM in services.env | Lost | Re-create by hand (Phase F). The drop-ins are cadre-api `agents.conf`, cadre-agents `toolbelt.conf`, cadre-practice `desk-traders.conf` and cadre-worker `10-serial.conf` |
| int7 model `llama-3.2-1b-instruct-w7a7-k2048` (1.7 GB, 144 matrices) | Rebuildable | `tools/cadre/quantize.py` from the HF checkpoint. It gets a new model_id |
| Cadre-chain contracts and `deploy/{halt,market,toolbelt,strategies,swarms,trading}-devnet.json` | Rebuildable | Redeploy with the bins under `services/swarm/**/bin` |
| Light client accounts on Solana (tip 8023 of the old chain) | Rebuildable | New program id (recommended) or an upgrade that adds a reset |
| Hardcoded IPs and sslip names | Rebuildable | 44 to 61 pearl-sol files, plus cadre-web (12), cadre-extension (5), cadre-docs (6), the wallet zip 6.73.4 and cadre-cli v0.1.0 |
| Code on GitHub, all branches (only `wallet-config-route` d245571 is unmerged and optional) | Survived | GitHub |
| `deploy/` (genesis, devnet.env, node-config.py, provision.sh, toolchain.sh, build.sh, systemd units, Caddy templates, hardening), `scripts/devnet-*.sh` | Survived | origin/main |
| All keys in `~/pearl-sol/keys/devnet` (92 files) and `~/.config/instance/agents` (30) | Survived | Mac only. The keys-devnet-backup copy is missing 11 entries |
| deployer `Fvqtw8` (26.74 SOL, upgrade authority of all 6 programs), ops `Bpxwem` (9.71 SOL), attester, storefront-wallet, miner0-2, node0-2 p2p, house operator `Cu4bS99S` (4.25 SOL) | Survived | keys/devnet |
| Solana programs bridge, storefront, reserve, lightclient, town, mandate; CADRE mint `BwJqbs` and its PumpSwap pool; town (55 profiles); mandate (3) | Survived | Solana devnet. Town and mandate don't depend on the Cadre chain |
| SSH keys `~/.ssh/lode_devnet{0,1,2}` and BitLaunch key ids `6ad58095/96/97...` | Survived | Can be attached to the new servers |
| Third-party creds: Helius (`keys/devnet/solana-rpc-url`), OpenRouter and E2B (`~/.config/lineage/`), GitHub agent-dump token (`~/.config/cadre/github-org-owner.token`) | Survived (external) | Check that each is still valid and has credit. No Firecrawl key was found |
| Agent-dump snapshot (98 agents, up to block 29293) | Survived | `github.com/cadredotag/agent-dump` `5f1f7b9e`. This is the only public record of the old chain's agents |

## 2. Owner decisions before starting (recommended default in bold)

| # | Decision | Options | Default |
|---|---|---|---|
| D1 | Hosting | BitLaunch Dallas (nibble-8192 + 2x nibble-2048 = $156/mo, prepaid in crypto, servers destroyed 24 h after the balance goes negative; it has happened twice), DigitalOcean (same sizes $84/mo, card billing), Hetzner US (~€99/mo, currently "unavailable") | **BitLaunch, same sizes, as you asked.** It is a hard blocker: the balance is $26.16, which is about 103 h of runway. **Top up $350-400 first**, set `billingAlert` to 30 days, and add the Mac watcher (Phase J). DO is the cheaper fallback if you want card billing |
| D2 | chain_id | Keep 1128350802 (the nine scheduled heights replay: no contracts until 4700, 60 s blocks until 6000, about 3-4 days) or a new id | **New id 1128350803.** Every rule is active from genesis. Speed values must go in genesis params: `{"target_block_time_ms":30000,"finality_k":60,"emission_ppb":3500,"max_balance_rows":8192,"price_mlamports_per_gflop":8767}`. Side effects: the town sunset never applies (harmless, town lives on Solana); the mock-verifier guard at `agent-node/src/main.rs:187` no longer covers devnet (harmless, node-config uses `verifier pearl`); `deploy/*-devnet.json` chain_id fields get regenerated anyway |
| D3 | Genesis target_bits | Reuse 0x1e0f1b3f (DEVNET.md:44 says about 1000x too easy: a fast first ~1000 blocks until WTEMA settles) or derive a harder one | **Derive from the measured rate** (3 boxes x 2 threads x ~3,085 nonces/s at 30 s). A fast first hour is acceptable if unsure |
| D4 | Light client | New program id (~3.9 SOL rent, touches 4 files, old 47mavK left orphaned) or an upgrade adding an admin reset (keeps the id, but needs a reviewed program change) | **New id** (`keys/devnet/lightclient2-keypair.json`). No new program code; built from main, so it also picks up the HANDOFF-56 fork-stall fix |
| D5 | Hostnames | Keep `<svc>.<IP>.sslip.io` (every URL changes again next time) or `cadre.ag` subdomains on Porkbun (`api.cadre.ag` already exists and points at the dead IP) | **cadre.ag subdomains**, e.g. `rpc`, `rpc1`, `rpc2`, `api`, `explorer`, `storefront`, `relayer`, `mcp`, `app` under `devnet.cadre.ag`. Add `DEVNET_DOMAIN` to devnet.env and an `@HOST@` placeholder in the Caddy templates. The 44+ files have to be edited this time anyway; with a domain it is the last time. Fallback: a sed pass to the new sslip names |
| D6 | Model checkpoint | `unsloth/Llama-3.2-1B-Instruct` (ungated, HANDOFF.md:671) or `meta-llama/...` (gated, and no HF token on the Mac) | **unsloth, pinned at revision `5a8abab4a5d6f164389b1079fb721cfab8d7126c`.** Record the pin in DEVNET.md. Quantize on box 0 |
| D7 | Old bridge deposits (219, 67.19M CADRE in the vault) | Replay them (relayer cursor 0) or skip them | **Skip.** Write `/var/lib/lode/relayer.json` with `deposit:"219"` before the relayer first starts. The vault surplus is accepted (ALLOW_SURPLUS) |
| D8 | Withdrawal nonces 0-8 already marked Processed on Solana | Burn 9 dust withdrawals, or upgrade the bridge with a nonce epoch | **Burn.** ops sends 9 dust BridgeLock withdrawals on the new chain before any user or e2e withdrawal |
| D9 | 141 Pending storefront orders | Refund them or leave them | **Refund** through the relayer refund path: `refund_deposit` + `mark_refunded`, signed by the attester |
| D10 | Mapping agent-N to handle for the 30 Instance keys | Owner supplies it, or derive it | **Derive it:** read the public keys in `~/.config/instance/agents/agent-N.json` and match them against the agent dirs in cadredotag/agent-dump. Ask the owner only if any remain unmatched |
| D11 | Third-party keys | OpenRouter, E2B (present), Helius (present), GitHub token (present), Firecrawl/Brave/AgentMail (optional, none found), Telegram/X feed-bot | **Use what is present; skip Firecrawl, feed-bot and cadre-trading.** Check E2B and OpenRouter credit, and look for old E2B sandboxes still billing |
| D12 | Off-box backup of new box-generated keys | Yes or no | **Yes.** Encrypted copies of relay.json x2, gateway.json, `master.hex`, the stream key and the quantized model's sha256 list, plus `keys/devnet` |
| D13 | Desktops Phase A budget (30 agents, $64/day E2B + OpenRouter) | Run or defer | **Run after e2e passes** |
| D14 | agent-dump repo | Tag the old snapshot and continue, or leave the writer off | **Tag `5f1f7b9e` as `old-chain-29293`, then re-enable the writer** |

## Hostnames (D5, set in Phase B)

`DEVNET_DOMAIN=devnet.cadre.ag` in `deploy/devnet.env`. Every public URL in the repo (scripts, units, services, CLI, SDKs, current docs) is a name under it, so a later rebuild only changes the DNS records and `BOX0_IP`..`BOX2_IP`. `scripts/devnet-deploy.sh install N` fills the Caddy templates: `@DOMAIN@` everywhere, and `@RPC_HOST@` (`rpc`, `rpc1`, `rpc2`) in `deploy/Caddyfile.tmpl`.

| Name | Box | Serves (local port behind caddy) | First devnet name |
|---|---|---|---|
| `rpc.devnet.cadre.ag` | 0 | agent-chain node 0 JSON-RPC :10630, WebSocket `/ws` :10631 | `rpc.64-95-12-116.sslip.io` |
| `rpc1.devnet.cadre.ag` | 1 | node 1 JSON-RPC and `/ws` | `rpc.64-95-13-213.sslip.io` |
| `rpc2.devnet.cadre.ag` | 2 | node 2 JSON-RPC and `/ws` | `rpc.64-95-12-22.sslip.io` |
| `api.devnet.cadre.ag` | 0 | cadre-api :10685 (`/v1`, `/v1/openai`, `/mcp`) | `api.64-95-12-116.sslip.io` |
| `mcp.devnet.cadre.ag` | 0 | the same cadre-api, short name for Cadre Connect (`/mcp`) | `mcp.64-95-12-116.sslip.io` |
| `explorer.devnet.cadre.ag` | 0 | explorer :10670, plus `/cadre-token.json` | `explorer.64-95-12-116.sslip.io` |
| `storefront.devnet.cadre.ag` | 0 | storefront-x402 :10662 | `storefront.64-95-12-116.sslip.io` |
| `relayer.devnet.cadre.ag` | 0 | relayer health :10660 (GET only) | `relayer.64-95-12-116.sslip.io` |
| `devnet.cadre.ag` | 0 | the website, cadre-web :10710 (`/install-miner.sh`, `/explorer`, `/live`) | `cadre.64-95-12-116.sslip.io` |
| `app.devnet.cadre.ag` | 0 | permanent redirect to `https://devnet.cadre.ag` | none |

Allow-host defaults (swarm, hosted agents, cadre-agent) are `devnet.cadre.ag`; the filter matches the name and every subdomain. p2p seeds stay IPs (`<BOXn_IP>:10640`): a seed is a socket address, not a name. The CADRE mint's metadata URI is the one exception: it was written into the mint as `explorer.64-95-12-116.sslip.io/cadre-token.json` and can't change.

**DNS records the owner creates at Porkbun** (zone `cadre.ag`, TTL 600, as soon as the IPs are known so they propagate during the build). Either 10 A records:

| Type | Host (in zone cadre.ag) | Answer |
|---|---|---|
| A | `devnet` | BOX0_IP |
| A | `app.devnet` | BOX0_IP |
| A | `rpc.devnet` | BOX0_IP |
| A | `api.devnet` | BOX0_IP |
| A | `mcp.devnet` | BOX0_IP |
| A | `explorer.devnet` | BOX0_IP |
| A | `storefront.devnet` | BOX0_IP |
| A | `relayer.devnet` | BOX0_IP |
| A | `rpc1.devnet` | BOX1_IP |
| A | `rpc2.devnet` | BOX2_IP |

or 4 records: A `devnet` = BOX0_IP, A `*.devnet` = BOX0_IP, A `rpc1.devnet` = BOX1_IP, A `rpc2.devnet` = BOX2_IP (an explicit record wins over the wildcard; the wildcard does not cover the bare `devnet` name). No AAAA records (the boxes are set up IPv4 only). The existing `api.cadre.ag` record (dead IP) is not used by the devnet; point it or delete it as you like. Caddy gets a Let's Encrypt certificate per name on first request, so each name must resolve to its box before `install N` reloads caddy, or caddy retries until it does.

Check after the records exist: `for h in devnet app.devnet rpc.devnet api.devnet mcp.devnet explorer.devnet storefront.devnet relayer.devnet rpc1.devnet rpc2.devnet; do echo "$h $(dig +short $h.cadre.ag)"; done`.

## 3. Ordered rebuild

Run everything from the Mac with `scripts/devnet-deploy.sh` unless noted. N is 0, 1 or 2.

### Phase A. Account and boxes (owner spend), about 30 min
1. Owner tops up BitLaunch (D1) and raises `billingAlert` to 30 days.
2. Check capacity and prices: GET `/api/user` (limit 5, used 1) and `/api/hosts-create-options/4`.
3. Buy the boxes with POST `/api/servers`: `{"server":{"name":"lode-devnet-0","hostID":4,"hostImageID":"10006","sizeID":"nibble-8192","regionID":"dal1","sshKeys":["6ad58095b64e983f5dbc9136"]}}`. Then `lode-devnet-1` and `-2`, each nibble-2048, with `...8096...9138` and `...8097...913a`. Poll until each has an ipv4.
4. Run `ssh-keygen -R` for the 3 old IPs. If D5 = domain, set the Porkbun A records now so DNS is propagating during the build.

### Phase B. Code changes on a branch, then merge to main (about 1.5 h, runs in parallel with Phase C)
1. `deploy/devnet.env:2-4` gets the new BOX0/1/2_IP. Add `DEVNET_DOMAIN` if D5 = domain.
2. Replace the old hostnames. List the files with `git grep -lE '64[.-]95[.-](12[.-]116|13[.-]213|12[.-]22)' -- ':!docs'`. They include:
   - `crates/cadre-cli/src/{endpoints.rs:45,72-77,solana.rs:52,agent.rs:26}`
   - `packages/cadre-sdk/src/client.ts:15`, `packages/cadre-py/cadre_connect/client.py:26`
   - `scripts/devnet-health.sh:13-17`. The RELAY_PAYERS at line 16 can only be filled in after Phase D6.
   - `scripts/install-miner.sh:43-48`
   - `deploy/systemd/cadre-agents.service:10`, `cadre-agent-dump.service:10`
   - `services/attester/src/bin/pearl-evaluator.rs:64`, `services/agent-dump/src/links.ts:13-16`
   - The allow-host defaults `services/swarm/src/filter.ts:9`, `src/main.ts:45`, `agents/src/main.ts:36`, `agents/bin/cadre-agent.ts:76`
   - `market/bin/market.ts:207` (MARKET_BLOB_NODES), plus the other swarm, desktop, trading and strategies defaults
3. Genesis (D2, D3): in `deploy/genesis.json` set the new chain_id, `timestamp_ms` = launch time, the same admin ops and attester `[9U5wdELZ]`, threshold 1, and the params from D2.
4. Light client (D4):
   - Generate a new keypair.
   - Set the devnet `declare_id` in `token/programs/lightclient/src/lib.rs:64`.
   - Update `deploy/devnet.env:15` LIGHTCLIENT_PROGRAM, `deploy/devnet-idl/lightclient.json:2`, `crates/cadre-cli/src/solana.rs:58` and the extension `networks.ts:59-63`.
5. Script fixes:
   - Make `devnet-deploy.sh install` write TOWN_PROGRAM into services.env.
   - Optionally add cadre-gateway to `deploy/build.sh`.
   - Add a CPU venv step for the worker. Today no script builds it; the pip lines are in `scripts/gpu/runpod-bootstrap.sh:16-17`.
6. Merge to main and push. `sync` ships pushed main only.

### Phase C. Provision, about 45 min (all 3 boxes in parallel)
1. Box 0: `ssh -i ~/.ssh/lode_devnet0 root@BOX0 'bash -s' < deploy/provision.sh build`. Boxes 1 and 2: the same without `build`.
2. Box 0 as lode: `ssh lode@BOX0 'bash -s' < deploy/toolchain.sh`. This installs rust 1.94.1 and 1.95.0, solana 3.1.12, anchor 0.31.1 and node 22.22.1.
3. `devnet-deploy.sh os N` for each box.

### Phase D. Build, install, chain start, about 1.5 h (needs B and C)
1. `devnet-deploy.sh sync`, then `devnet-deploy.sh build`. Builds run one at a time on the 8 GB box.
2. Build the gateway separately: `cd ~/pearl-sol/crates && cargo build --release -p cadre-gateway -j 2`, then install it to `/opt/lode/bin`.
3. `devnet-deploy.sh ship`.
4. `devnet-deploy.sh install 0/1/2`, then `rpc-url`, `keys 0/1/2` and `harden 0` (before any unit starts).
5. Check that `/etc/lode/node.json` carries BOXn_BLOBS on every box. Without it the model upload fails with 413.
6. `relay-key 1` and `relay-key 2`. Fund each with about 6 SOL: `solana transfer -u devnet -k keys/devnet/deployer-keypair.json --allow-unfunded-recipient <pub> 6`. Back up both keys (D12).
7. Start node 0: `ctl 0 enable --now cadre-node`. When `getChainInfo` answers, record the new genesis_hash.
8. Start nodes and miners on boxes 1 and 2: `ctl 1 enable --now cadre-node cadre-miner`, and the same for box 2. Hold the box 0 miner until Phase E3 has finished (memory).
9. `devnet-deploy.sh status`: expect peers 2 on each box and equal heights.
10. From ops, send an AdminConfig with `house_operators=[Cu4bS99S...]`. It can't go in genesis because `config.rs:76` reads it through `num()`. Check with `getParams` on all 3 nodes.
11. Set `scripts/install-miner.sh:47` DEVNET_GENESIS to the new hash and push. `site` copies it later.

### Phase E. Inference model, about 1 h (box 0, after D1 build is done; runs alongside D8-D11)
1. Create the venv as lode:
   ```
   python3 -m venv ~/cadre-venv
   ~/cadre-venv/bin/pip install torch==2.5.1 --index-url https://download.pytorch.org/whl/cpu
   ~/cadre-venv/bin/pip install transformers==4.46.3 datasets==3.1.0 'numpy>=2' safetensors huggingface_hub
   ```
2. Download the checkpoint (2.47 GB) with `snapshot_download('unsloth/Llama-3.2-1B-Instruct', revision='5a8abab4...', local_dir='/home/lode/hf/llama-3.2-1b-instruct')`.
3. Quantize, which takes about 20-30 min and about 5 GB RAM:
   ```
   python tools/cadre/quantize.py --model /home/lode/hf/llama-3.2-1b-instruct \
     --out ~/cadre-models/llama-3.2-1b-instruct-w7a7-k2048 \
     --name llama-3.2-1b-instruct-w7a7 --base-model unsloth/Llama-3.2-1B-Instruct
   ```
   Expect 144 matrices, 329 files and about 1.7 GB.
4. Run `worker.py --model <out> --ppl 2`. Save the sha256 list off-box.
5. Start the box 0 miner: `ctl 0 enable --now cadre-miner`.
6. Move the model into place with `cp -al <out> /var/lib/lode/cadre-model`. Run `svc-key 0 gateway` and back up the key. Fund it with 1000 CADRE from miner0 on the new chain.
7. Register the model (836 blobs, up to 600 s inclusion wait):
   ```
   /opt/lode/bin/cadre --rpc http://127.0.0.1:10630 --key /etc/lode/keys/gateway.json \
     --json --wait model register /var/lib/lode/cadre-model --store /var/lib/lode/models --parallel 4
   ```
   `getModel` on nodes 1 and 2 should show `manifest_present` true. Record the new model_id.

### Phase F. Solana side, about 1 h (needs D9 chain finalizing)
1. Preflight: `scripts/devnet-solana.sh balances`.
2. Deploy the new light client (~3.9 SOL): `scripts/devnet-solana.sh deploy lightclient`, with `pub lightclient` pointed at the new keypair. Rebuild header-relay with `--features devnet` against the new id.
3. Write `/var/lib/lode/relayer.json` with `deposit:"219"` (D7).
4. Move `~/pearl-sol/.devnet-e2e/init-done.json` aside. If it stays, init skips the gas deposit and fund_mining.
5. Run `scripts/devnet-solana.sh init` (`scripts/devnet-init/init.ts`). It does the following:
   - Runs `header-relay init` at the finalized height plus `init_accumulator`.
   - Calls `set_release_modes` and `set_finalize_modes(2, new LC)`.
   - Does the chain gas deposit for ops and miner0.
   - Calls fund_mining (10M; the limit is 20M per 24 h).
   - Enables cadre-relayer, cadre-header-relay, cadre-storefront and cadre-crank.
6. No `set-speed` is needed with a new chain_id, because the speed values are in genesis.
7. `ctl 1` and `ctl 2 enable --now cadre-header-relay`. `ctl 0 enable --now cadre-attester cadre-settle`.
8. ops sends the 9 dust withdrawals (D8).
9. Let the refund path handle the 141 Pending orders (D9, REFUND_GRACE_MS 1 h).
   **Open (found in Phase B):** D7 and D9 conflict as written. The relayer only looks at deposits at or above its cursor (`services/relayer/src/relayer.ts` `checkOrderFees` / `unlockDeposits` take the pending deposits from `state.deposit` on), and every one of the 141 order deposits has a nonce below 219. With the cursor at 219 the relayer never sees them, so it neither unlocks nor refunds them. They need a one-off refund (bridge `refund_deposit` + storefront `mark_refunded`, attester-signed) by nonce, or the owner accepts leaving them Pending. Not built in Phase B.

### Phase G. Box 0 services, in start order (about 45 min)
Order so far: node, miner, attester, storefront, relayer, settle, crank (Phases D to F). Then:
1. `cadre-worker`, then `cadre-gateway`. Check `127.0.0.1:10665/health` and `:10664/v1/models`.
2. `cadre-explorer`, then `cadre-api`. Check `:10685/v1/ready` and `/v1/summary` (`light_client_lag`).
3. Web: `devnet-deploy.sh site`. It builds `NEXT_PUBLIC_CADRE_API` and `NEXT_PUBLIC_CADRE_EXPLORER` and copies `install-miner.sh`.
4. Redeploy the contracts and regenerate the `deploy/*-devnet.json` files. Use `services/swarm/agents/bin/{halt,swarms-deploy,toolbelt-devnet}.ts`, `services/swarm/market/bin/market.ts`, `strategies/bin/practice.ts` and optionally `trading/bin/deploy-cadre.ts`. Fund the deployers with CADRE from miner0. Commit, push, `sync`, and restart cadre-api.

### Phase H. Agents, about 1.5 h (needs G)
1. House swarm:
   - Convert the 30 Instance keys to 64-byte array files named `keys/devnet/swarm/agent-<handle>.json` (D10). Do it in the scratchpad and never print them.
   - Write `/etc/lode/swarm.env`: `SWARM_AGENTS=0`, `SWARM_TOPUP_ROWS=8192`, `SWARM_TOPUP_ROUND_SECS=10800`, `TOWN_PROGRAM`, and the allow-hosts if the defaults were not changed.
   - Fund the operator with CADRE (miner0) and devnet SOL.
   - Run `devnet-deploy.sh swarm keys`, then `swarm unit`, then `ctl 0 enable --now cadre-swarm`. On its first run it re-does the fundings, OperatorLinks and profiles. That is the "re-register the 30 agents" step.
2. Desktops: `scripts/devnet-desktops.sh code`, then `secrets`, `wallet` and `unit`, then `ctl 0 enable --now cadre-desktops`, then `devnet-desktops.sh status`. Set DESK_PUBLIC_API (it isn't set by the script).
3. Hosted agents (start empty):
   - Install `cadre-agents.service`. It creates a new `master.hex`; back it up.
   - Add the drop-in `cadre-agents.service.d/toolbelt.conf` (TOOLBELT_PAYEE, toolbelt env).
   - Add the drop-in `cadre-api.service.d/agents.conf` (`CADRE_AGENTS_URL=http://127.0.0.1:10730`).
   - daemon-reload, then restart.
4. Practice traders:
   - Copy `keys/devnet/strategies` to `/etc/lode/keys/strategies`.
   - Install the unit and the `desk-traders.conf` drop-in (`STRAT_DESK_TRADERS=foxglove47,meadowsweet27,primrosedusk54,boragemist71`, `STRAT_DESK_KEYS=/etc/lode/keys/swarm`).
   - Start it.
5. Agent dump: tag the old snapshot (D14), install the token per `docs/AGENT-DUMP.md:136-145`, then install the unit and start it.
6. Run `devnet-deploy.sh harden 0` again so the units added in this phase get the drop-ins, then restart them.

### Phase I. Clients and docs, about 1.5 h (parallel with E to H once the names are fixed)
1. cadre-web: `public/install-miner.sh:41-46` and the 11 `e2e/*.mjs` files.
2. cadre-extension `cadre-chain`: `src/shared/cadre/networks.ts:58,68,70` (and chain_id plus the LC id), `scripts/cadre/{devnet-check,live-check}.mjs`, then a new wallet zip release to replace `cadre-devnet-6.73.4`.
3. cadre-cli v0.1.1 release. Republish cadre-sdk and cadre-py.
4. Docs: `~/cadre-docs` (6 mdx files) and `pearl-sol docs/*.md` (DEVNET.md: new IPs, genesis hash, model_id, relay payers, LC id, checkpoint pin; also correct DEVNET.md:640-641, which wrongly says the miner keys were box-only).
5. Verify each repo has 0 hits: `git grep -nE '64[-.]95[-.](12[-.]116|13[-.]213|12[-.]22)'`.

### Phase J. Health, e2e, monitoring, about 3 h, mostly waiting
1. `devnet-deploy.sh health`. Update RELAY_PAYERS and NODES/API/SITES first.
2. `INF_RUN=rebuild scripts/devnet-inference.sh`. The buyer needs 0.2 SOL from ops. Settlement takes about 27 min, and mining all 144 segments took 138 min last time.
3. `scripts/devnet-e2e.sh`.
4. Mac launchd watcher at `~/Library/LaunchAgents/com.cadre.bitlaunch-watch.plist`, running hourly:
   - Balance runway from `/api/user`: WARN under 14 days, FAIL under 5 days.
   - Any `low_balance` or `*_for_negative_balance` event in `/api/security/history`.
   - All 4 servers present in `/api/servers`.
   - Public GETs on rpc, api `/v1/summary` and the site.
   - Alerts via osascript plus ntfy.sh or Telegram, plus a dead-man ping.
5. As root on box 0, run `bash deploy/backup.sh` and copy the output off the box.

## 4. Parallelism

- **While the boxes boot:** do Phase B (IP and genesis edits, LC keypair) and the D5 DNS records.
- **While box 0 builds:** C on all 3 boxes at once. Boxes 1 and 2 wait for `ship`.
- **While box 0 quantizes:** nodes and miners on boxes 1 and 2, the AdminConfig, the relay-key funding, and the LC deploy (`deploy lightclient` only needs the .so from the build).
- **Once the names are fixed:** I1-I4 (site, extension, CLI and SDK releases, docs) run independently of the chain.
- **During the inference and e2e mining wait (2 to 3 h):** desktops, hosted agents, practice, agent-dump, the Mac watcher and the backups.
- **Wall clock estimate:** about 6 to 8 h. The critical path is top-up, buy, provision, build (about 1 h), quantize (about 30 min), register, LC init, e2e (about 2.5 h).

## 5. Risks and handling

| Risk | Handling |
|---|---|
| Balance runs out a third time (only $26 now) | Top up before buying (D1), raise the alert to 30 days, add the Mac watcher (J4) |
| New chain_id with `params {}` means 60 s, K 30 and 7 ppm forever, and the services disagree | The params from D2 go in genesis. Check `getParams` after start |
| `header-relay init` on 47mavK fails with "already in use" | Use a new LC id (D4). Rebuild both the program and header-relay with `--features devnet`, otherwise the owner check fails |
| A fresh relayer replays all 219 old deposits | Write the cursor `deposit:"219"` before init enables cadre-relayer |
| New withdrawals 0-8 collide with the old Processed records | Burn them (D8) before e2e, which withdraws |
| `init-done.json` makes init skip the gas and fund steps | Move it aside (F4) |
| Box 0 OOM during the fp32 calibration | Quantize before the box 0 miner, attester and worker start |
| Model upload fails with 413 | Check BOXn_BLOBS in node.json (D5) |
| The attester store path doesn't match the new model_id | Use `--store /var/lib/lode/models`. Check `/var/lib/lode/models/<new id>` exists |
| Half-updated hostnames, allow-host filters block the new hosts | The zero-hit grep gate (I5). Fix the swarm allow defaults or set them in env |
| Let's Encrypt rate limits for about 10 names | A domain (D5) avoids sslip shared limits. Ports 80/443 are open via provision.sh |
| `install` only runs daemon-reload, and UNITS0 skips swarm, desktops, agents, practice, dump and health | Always `ctl N enable --now`. Rerun `harden 0` after adding units |
| Lost drop-ins and TOWN_PROGRAM silently missing | Script fix B5 and the explicit steps H3 and H4 |
| Wrong agent-N to handle mapping breaks house identities | Match pubkeys against agent-dump (D10). Stop and ask if any don't match |
| `sync` and `site` refuse a dirty or shared main | Use a fresh clone. Never touch other sessions' worktrees |
| Helius, OpenRouter or E2B key expired or out of credit | Check before Phases F and H. Stale E2B sandboxes may still be billing |
| The agent-dump writer overwrites the old public record | Tag `5f1f7b9e` first (D14) |

**Stranded on Solana by the lost keys or the dead chain** (all devnet, written off):
- 1.45 + 1.09 SOL in the lost relay payers, plus the rent in their 9 Work accounts and HeaderRecords.
- About 9.94 SOL of rent in the old LC's 2567 accounts (47mavK is orphaned).
- The SOL in the 12 to 18 hosted agents' custodial wallets.
- The old prepaid gateway balances.
- 67.19M CADRE in the bridge vault for the skipped deposits. It stays as surplus.
- The CADRE mint metadata URI (`explorer.64-95-12-116.sslip.io/cadre-token.json`). pump.fun metadata can't be changed, so the token image stays dead unless that exact name is ever served again.

No admin or upgrade authority is lost: the deployer and ops survived.

## 6. Verification (all must pass)

1. **Chain.** `devnet-deploy.sh status` shows peers 2 on all boxes, equal heights and equal finalized hashes. `getChainInfo` has the new chain_id and genesis hash. After WTEMA settles (about 1 h) blocks average about 30 s. `getParams` shows finality_k 60, emission_ppb 3500, max_balance_rows 8192, price 8767 and house_operators [Cu4bS99S].
2. **Solana.** `header-relay status` shows `initialized:true`, the tip rising, and a lag of at most about K. The bridge and storefront Configs point at the new LC id. Relayer `/health` and storefront `/health` are OK. `/v1/summary` `light_client_lag` is low. A withdrawal at nonce 9 or higher releases.
3. **Inference.** `/v1/openai/models` lists `llama-3.2-1b-instruct-w7a7`. `devnet-inference.sh` returns "Paris", a receipt, a completed ticket and `finalize_order_proof` on Solana.
4. **Services.** Every cadre-* unit is `active` and `enabled` on its box. The site, explorer, api, mcp and rpc{,1,2} answer over HTTPS with valid certs. The 30 house agents appear in cadre-api and the explorer with the same pubkeys as in agent-dump. Desktops status shows 30 agents. Contracts resolve from the regenerated `deploy/*-devnet.json`.
5. **Suites.** `/var/log/cadre-health/status` is all OK, and `scripts/devnet-e2e.sh` passes.
6. **Hygiene.** The old-IP grep finds 0 hits in pearl-sol, cadre-web, cadre-extension and cadre-docs. The `install-miner.sh` pin equals the new genesis hash in both copies. The off-box backup of the new keys and `deploy/backup.sh` exists. The Mac watcher has fired one test alert.

### Critical Files for Implementation
- /Users/achi/pearl-sol/scripts/devnet-deploy.sh
- /Users/achi/pearl-sol/deploy/genesis.json
- /Users/achi/pearl-sol/deploy/devnet.env
- /Users/achi/pearl-sol/token/programs/lightclient/src/lib.rs
- /Users/achi/pearl-sol/scripts/devnet-init/init.ts