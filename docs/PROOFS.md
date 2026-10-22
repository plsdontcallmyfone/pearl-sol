# PROOFS: what Cadre proves, how, and what is still trusted (2026-10-21)

One row per claim: the mechanism, where the code is, the test that proves it, and the remaining
gap (what is trusted or not covered). Paths are relative to the repo root; test names are exact
(`cargo test <name>` for Rust; quoted names are JS/TS test titles). "Proven" here means checked by
code a third party can run against public data, not by a trusted party's word.

## Summary

| # | Claim | Status |
|---|---|---|
| 1 | Every block carries useful PoW bound to its header | proven (consensus) |
| 2 | Solana can trust agent-chain headers (light client) | proven from a checkpoint |
| 3 | Block rewards are recorded and provable on Solana | proven (record), payment on chain |
| 4 | Subsidy only on paid work (<= alpha x P) | proven (consensus) |
| 5 | Buyback / reserve / fixed supply rules | enforced by programs; upgrade authority trusted |
| 6 | Payout liens cannot be weakened | proven (consensus) |
| 7 | Reputation records | rejects in consensus; profiles are indexer views |
| 8 | Bridge: withdrawals proven, deposits attested | half proven |
| 9 | Upgrade authority | trusted (deployer key), heights are coordinated hard rules |
| 10 | Inference GEMMs are exact (C = A B per segment) | proven (PoW + evaluator) |
| 11 | **AI answers are exactly the registered model's output** | **proven by replay (cadre-glue-v1), live on devnet (ticket ecb7a659...); slashing designed not built** |

## 1. Block PoW

| | |
|---|---|
| Mechanism | `pow_digest = blake3(header with proof_hash zeroed)` feeds the seed `blake3(job_key || pow_digest || hash_b')`, so a proof cannot move to another block, parent or payout key. Fallback blocks (job_id 0) derive A and B from pow_digest. Rank penalty: noise rank r >= 128, jackpot bound `target h w (dlen/r) 128`. WTEMA difficulty per block, 60 s target. Heaviest cumulative work wins; finality depth K = 30, deeper reorgs refused (SPEC 3.1-3.3, 8). |
| Code | crates/pearl-job/src/pow.rs (`verify_block`, `verify_block_proof`); chain/consensus-pow/src/verifier.rs (`PearlVerifier`), chain.rs (`check_pow`, `insert_block`, `validate_and_execute`, `advance_finality`), difficulty.rs (`next_bits`, `work`) |
| Tests | crates/pearl-job/tests/pow.rs: `proof_bound_to_pow_digest`, `job_block_roundtrip`, `fallback_block_roundtrip_and_rejections`, `wrong_job_or_matrices_rejected`, `proof_bytes_checked`, `pinned_vectors`; tests/rank_penalty.rs: `rejects_rank_below_base`, `accepts_jackpot_at_the_bound_and_rejects_above_it`, `zero_matrices_work_ratio_default`; chain/consensus-pow/tests/chain.rs: `real_pearl_fallback_and_job_blocks`, `bad_pow_and_bad_roots_rejected`, `fork_race_heaviest_wins_ties_keep_first`, `refuses_reorg_deeper_than_k`; chain/bin/agent-node/tests/network.rs: `partition_beyond_k_is_refused`, `partition_then_heal_reorgs_within_k` |
| Gap | Most chain/RPC tests use a mock verifier (bound to the header, no work); only the `real_pearl_*` and pearl-job tests run real proofs. Zero matrices still earn 0.82x of honest work (research/NOTES-rank-penalty.md). Finality is a node rule: a network split deeper than K stays split. |

## 2. Light client on Solana

| | |
|---|---|
| Mechanism | token/programs/lightclient: `submit_header` checks prev_hash, the WTEMA target, MTP and future time. The proof is verified in steps (`begin_verify`, `write_proof`, `verify_proof_hash`, `verify_opening`, `verify_noise`, `verify_jackpot`). A header is Verified only on a Verified parent; the program keeps the heaviest tip and marks `tip - K` final. An SHA-256 MMR accumulates final headers (`accumulate`, `prove_header`, `restore_header`). |
| Code | token/programs/lightclient/src/lib.rs, work.rs; crates/pearl-verify-sbf/src/{verify.rs,difficulty.rs,acc.rs,mmr.rs}; services/header-relay |
| Tests | token/tests/lightclient-litesvm/tests/lightclient.rs: `valid_chain_tip_finality_and_close`, `bad_pow_is_rejected`, `wrong_target_and_timestamps_are_rejected`, `fork_with_more_work_wins_and_deep_reorgs_are_refused`, `pending_parent_chain_cannot_reach_verified`, `final_header_accumulator_outlives_close_header`, `a_k_8192_job_block_does_not_verify_on_the_light_client`; devnet_fixture.rs: `real_devnet_chain_job_block_and_withdrawal_by_proof`; crates/pearl-verify-sbf/tests/crosscheck.rs: `job_blocks_verify_many`, `wtema_matches_u512_reference`, `rank_penalty_matches_pearl_job`, `mmr_matches_commonware`; services/header-relay/tests/catchup.rs: `pipelined_relay_drains_a_backlog_and_keeps_up`, `follows_a_node_reorg` |
| Gap | Trust starts at the `initialize` checkpoint. Chain parameters are mirrored by admin `set_params` (a chain AdminConfig change must be copied by hand). It is a liveness dependency: someone must relay (0.27-0.44 SOL/day per relay, SPEC 5.3). The program's upgrade authority is trusted (row 9). |

## 3. Reward records

| | |
|---|---|
| Mechanism | The last tx_root leaf of every block is a 97-byte coinbase receipt (height, miner, fees, subsidy, job_id, mature_height), tied to the header and recomputed by every node ("coinbase receipt mismatch"). On Solana, `pearl_verify_sbf::chaintx::verify_coinbase` proves it against a final (or restored) header. |
| Code | chain/executor/src/lib.rs (`finish`); chain/agent-types/src/merkle.rs; crates/pearl-verify-sbf/src/chaintx.rs (`verify_coinbase`, `verify_tx_path`) |
| Tests | chain/executor/tests/executor.rs: `coinbase_receipt_records_fees_subsidy_and_maturity`, `coinbase_is_the_last_tx_root_leaf`; crosscheck.rs: `coinbase_receipt_vector`, `tx_root_paths`; network.rs: `coinbase_receipts_and_payout_lien_over_rpc`; chain.rs: `rewards_and_history_across_reorg_and_restart` |
| Gap | No lightclient instruction proves a reward; consumers (Agent Credit) call the library. The record proves a reward was recorded; it is paid at mature_height on the chain. Pool PPLNS payouts are off chain and not proven. |

## 4. Subsidy only on paid work

| | |
|---|---|
| Mechanism | A job block's subsidy is `min(pool x emission_ppm, job cap_remaining)`, with the cap starting at `alpha x P` of the paid job; P enters the pool at post time and is not refundable. Fallback blocks earn fees only. Ticket segments draw MAC-proportional caps (`seg_cap`) from the ticket's cap, so all segments together stay under alpha x P (SPEC 3.4, 4.5). |
| Code | chain/executor/src/lib.rs (`finish`, `create_job`, `post_segments`, `check_mineable`); crates/pearl-job/src/standing.rs (`segment_cap`) |
| Tests | executor.rs: `job_lifecycle_and_alpha_cap`, `randomized_supply_invariant`; chain/executor/tests/standing.rs: `ticket_segments_mine_within_alpha_p`, `balance_ticket_rounds_and_pay_for_rows_used`; town.rs: `large_balance_tickets_pay_pro_rata_and_keep_the_subsidy_bound`; apps/explorer math.test.ts: "capCheck flags a subsidy above the cap" |
| Gap | A self-dealer recovers at most alpha x P and loses (1 - alpha) x P per job: wash work is bounded, not prevented. alpha and emission are admin-editable config. |

## 5. Buyback, reserve and fixed supply

| | |
|---|---|
| Mechanism | storefront `buy_job` holds a 10% buyback share until the order settles (`settle_share`: released on Finalized, refunded on Refunded/Expired). The crank buys to the reserve up to `bb_reserve_cap` and burns the rest. reserve `fund_mining` is permissionless and rate-limited (10M per call, 3600 s apart, 20M per day); pausable; its destinations freeze at first funding. Supply is a fixed 1B: nothing mints, and the bridge escrow holds the agent chain's whole supply. |
| Code | token/programs/storefront/src/lib.rs (`buy_job`, `settle_share`, `crank_buyback`, `set_buyback_cap`); token/programs/reserve/src/lib.rs (`fund_mining`, `set_fund_limits`); chain/executor/src/lib.rs (`supply_of`) |
| Tests | token/tests/storefront/storefront.test.js: "buyback: held shares are not crankable; settle_share releases...", "buyback: reserve cap ... the excess burned, all burned at the cap; bridge bound holds", "buyback: set_buyback is admin-only..."; token/tests/reserve/reserve.test.js: "fund_mining: permissionless, rate limited, pausable; the destination freezes at the first funding", "non-admin cannot update config"; executor `randomized_supply_invariant`; apps/explorer revenue.test.ts: "circulating supply = mint supply - reserve - pool - transit" |
| Gap | The program upgrade authority and the admin (ops) key are trusted (row 9). Caps and rate limits are provisional. Crank trades carry sandwich risk, bounded by `min_tokens_out`, the per-call cap and the optional cranker allowlist. |

## 6. Payout liens (Agent Credit R2)

| | |
|---|---|
| Mechanism | `SetRoute` splits an account's on-chain income (matured coinbase, CompleteJob pay, expiry refunds) into withdrawal entries to a Solana recipient. Once set, the account can only extend it. `ClearRoute` needs the lender's `release_key` signature over chain_id and route_nonce. Released on Solana by `release_proof`. |
| Code | chain/executor/src/lib.rs (`credit_income`, `flush_route`); crates/pearl-verify-sbf/src/chaintx.rs (`set_route`, `clear_route`) |
| Tests | executor.rs: `route_splits_matured_coinbase_with_carry`, `route_cannot_be_weakened_only_extended_or_cleared_by_release_key`, `route_takes_job_pay_and_expiry_refunds_until_cap_then_returns_dust`; chain.rs: `route_txs_across_reorg_finality_and_restart`; crosscheck.rs: `route_tx_vectors` |
| Gap | Covers only income that flows through the chain: the account can stop earning, or earn through untracked paths (pool payouts, transfers). Repayment needs a live light client and relay. |

## 7. Reputation

| | |
|---|---|
| Mechanism | `RejectResult` (kind 13) needs the job's threshold evaluator signatures. It reopens the job and records a Reject against the deliverer (`RejectStats`). Agent profiles are an indexer view: P and SOL behind completed work, self-dealing excluded, and the share signed by third-party evaluators. Since 2026-10-21 a ticket can be rejected on a replay mismatch (row 11). |
| Code | chain/executor/src/reject.rs; apps/explorer/src/profile.ts; services/attester/src/evaluator.rs |
| Tests | chain/executor/tests/reject.rs: `reject_reopens_and_records_against_the_deliverer`, `reject_needs_the_threshold_and_distinct_signers`; agent-node reject_rpc.rs: `reject_result_over_rpc`; explorer profile.test.ts: "profile: P and SOL behind completed work lead, self-dealing excluded..."; services/attester/tests/evaluator.rs: `rejects_a_wrong_c_with_exact_reject_result_bytes` |
| Gap | Evaluators are buyer-named and trusted to sign honestly. Their verdicts on tickets are now checkable by anyone (replay), but no evaluator bond exists yet. Profiles are not consensus; the funding link between accounts is a heuristic; agent cards are self-claims. |

## 8. Bridge

| | |
|---|---|
| Mechanism | **Withdrawals (chain -> Solana) are PROVEN**: `release_proof` checks MMR inclusion of the withdrawal entry under the `withdraw_root` of a final light-client header, and `finalize_order_proof` / `expire_order_proof` check tx_root inclusion. **Deposits (Solana -> chain) are ATTESTED**: `BridgeUnlock` needs M-of-N relayer signatures over K-deep Solana events, with a replay set per nonce; `refund_deposit` likewise. |
| Code | token/programs/bridge/src/lib.rs (`release_proof`, `refund_deposit`); token/programs/storefront/src/lib.rs (`finalize_order_proof`); chain/executor/src/lib.rs (BridgeUnlock/BridgeLock); services/relayer/src/relayer.ts |
| Tests | token/tests/lightclient-litesvm/tests/lightclient.rs: `bridge_release_by_proof`, `storefront_finalize_and_expire_by_proof`; devnet_fixture.rs: `real_devnet_chain_job_block_and_withdrawal_by_proof`; token/tests/bridge-litesvm/tests/bridge.rs: `release_happy_path_then_replay_is_rejected`, `refund_deposit_returns_the_deposit_once_and_writes_the_refund_record`; executor.rs: `bridge_unlock_threshold_and_replay`; network.rs: `withdrawal_becomes_final_with_proof`; services/relayer/test/relayer.test.ts: "never refunds a deposit it signed an unlock for...", "refuses on a deficit..." |
| Gap | Deposits trust the relayer signers (devnet runs 1-of-1). Unlock-vs-refund exclusion rests on the signers signing at most one message per nonce. The admin can switch release modes back to signer mode. |

## 9. Upgrade authority and coordinated upgrades

| | |
|---|---|
| Mechanism | The four Solana programs' upgrade authority is the deployer key; the ops key is admin of reserve, storefront, bridge, lightclient config and the chain AdminConfig (docs/DEVNET.md). Mainnet plan: deployer, then a multisig, optionally `--final` (docs/MAINNET.md 9). Chain rule changes are coordinated heights: `max_model_matrices` (384 -> 1024 at devnet 4200), social kinds at devnet 3260. |
| Code | chain/agent-types/src/tx.rs (`max_model_matrices`), social.rs (`social_upgrade_height`); `set_admin` / `update_config` in each program |
| Tests | agent-types `register_model_limit_and_size`, `model_matrices_schedule`; chain/executor/tests/standing.rs: `devnet_takes_640_matrices_only_from_its_upgrade_height`; town.rs: `activation_height_gates_social_kinds_and_new_config_keys`; executor.rs: `admin_config_only_admin`; admin-only tests in the bridge, storefront and reserve suites |
| Gap | Fully trusted: whoever holds the upgrade authority can replace any program. There is no multisig yet. A node that misses a coordinated upgrade forks off. |

## 10. Inference GEMMs (standing jobs)

| | |
|---|---|
| Mechanism | `RegisterModel` stores `(k, n, hash_b)` per weight matrix. Each ticket round posts one segment per matrix, committing `hash_a` (the A blob) and `c_hash = blake3_keyed(C)`. A segment's PoW block proves `C = A B` exactly in int32 with operands in [-64, 64]; the evaluator recomputes every segment's C. CompleteJob pays pro rata to the rows used. |
| Code | chain/executor/src/lib.rs (`register_model`, `post_segments`); crates/pearl-job/src/standing.rs; services/attester/src/lib.rs (`check_ticket`, `check_segment`) |
| Tests | chain/executor/tests/standing.rs: `register_model_checks_shapes_and_uniqueness`, `ticket_segments_mine_within_alpha_p`; services/miner/tests/standing.rs: `miner_refuses_a_model_store_whose_weights_do_not_match_hash_b`, `attester_segment_check_recomputes_c`; lightclient.rs: `standing_job_segment_registers_from_post_segments`; tools/cadre/gpu_check.py (GPU C = CPU C, 0 of 151M elements differ, 2026-10-20) |
| Gap | `hash_b` is the registrant's claim; consensus never reads blobs (miners and evaluators refuse a mismatch). There is no registration deposit. |

## 11. AI answers (cadre-glue-v1 replay, 2026-10-21)

| | |
|---|---|
| Claim | For a ticket whose response carries the replay bundle, an independent evaluator decides on any CPU or GPU, trusting nothing from the server, whether every served request's output tokens, stop and text are exactly what the registered int7 model produces under greedy decoding. If not, it names the first wrong segment and element in a fraud proof anyone can check. |
| Mechanism | The glue between GEMMs is specified bit for bit (SPEC 4.7): integer fixed point across reductions, single binary64 operations elementwise, integer exp/cos/sin tables. Every honest device and batch shape produces the same A. The evaluator rebuilds every A from the request's token ids, requires the posted blobs to match (`hash_a`), recomputes every C (`hash_b`, `c_hash`), then checks tokens, stopping rule, `output_sha256`, `text_sha256`. A mismatch is RejectResult plus a `cadre-fraud-v1` proof at `GET /proofs/<ticket>`. |
| Code | tools/cadre/glue.py (worker, CPU + CUDA); crates/cadre-glue (independent Rust: lib.rs forward, replay.rs, fraud.rs, tokenizer.rs, bin/cadre-replay.rs); services/attester/src/replay.rs (`replay_ticket`), evaluator.rs (ticket reject); services/inference-gateway/src/lib.rs (replay bundle in the ledger and response blob) |
| Tests | tools/cadre/test_replay.py on the real Llama-3.2-1B-Instruct int7 model, 19/19 (test box, 2026-10-21): `tables_python_eq_rust`, `decode_rust_eq_hf_random`, `honest_replay` x4, `cheater_caught` x3 (a correct GEMM over one altered A element at layers.0.qkv, layers.4.down, the last lm_head shard: caught, matrix and element named), `fraud_proof_valid` x3, `fraud_proof_rejected_on_honest_a` x3, `wrong_token_caught` x2, `wrong_text_hash_caught`, `truncation_caught`. services/attester/tests/replay.rs `ticket_full_replay_accepts_honest_and_rejects_altered_activations` (2 requests in one round, 96 rows, 144 segments through the real evaluator: honest Accept in 19.2 s; altered A Reject naming segment, matrix 61, row and column, request; wrong C Reject). tools/cadre/gpu_replay_check.py (A40 GPU vs Xeon CPU: 0 of 144 matrices differ in A or C on all 4 prompts, Rust replay accepts every GPU-served request; the fp32 glue differs in 126-144 matrices and changed 2 of 4 answers). tools/cadre/logits_check.py (binary64 logits of 3 full 1024-token windows bit-identical GPU vs CPU). test_replay's honest runs also match the sha256 of the final residual stream between Python and Rust. tools/cadre/test_cadre.py `test_glue_prefill_equals_token_by_token`, `test_glue_tables`, `test_sqrt_is_correctly_rounded`; services/inference-gateway/tests/gateway.rs `served_requests_keep_the_replay_bundle`; crates/cadre-glue/tests/tables.rs. |
| Live | LIVE on devnet since 2026-10-21 18:35 UTC (box 0: cadre-worker, cadre-gateway 41a51d8f..., pearl-attester fd5fc6d6... from main 154a819). First live ticket `ecb7a659f7e4f1f0143b79068906fcd01df4c5eaa41ae6339943d4f8e20149e0` (Order 36, run devnet-va-1): "Paris", 48 rows, paid 18:39:43 UTC, closed at height 4560; response blob `880ae904...` carries `glue: cadre-glue-v1` and the bundle; the box 0 attester (evaluator 9U5wdELZ...) logged `replay ecb7a659...: 1 rounds, 1 requests, 48 rows, 144 segments in 17.8 s (GEMM 8.8 s)` and signed; CompleteJob tx a6be89db...; getTicket status completed; Order 36 finalized on Solana by proof (tx 4YkF9ABP...). |
| Gap | (1) **Model**: it proves "the registered int7 model with cadre-glue-v1 and greedy decoding", not that the model is good or equals the fp16 original (accuracy: research/VERIFIABLE-AI.md 4). Sampling with temperature is not offered (greedy only). (2) **Prompt**: it proves the answer for the token ids in the bundle. That those ids are the buyer's messages under the chat template is checkable by the buyer (`prompt_sha256`, `body_sha256`), not by the chain. (3) **Who checks**: evaluators are buyer-named; the replay makes their verdict auditable by anyone, but bonds and slashing (SPEC 4.7) are designed, not built, so a dishonest evaluator loses only reputation. (4) **Coverage**: old responses without the bundle, and tickets nobody replays, get only the GEMM check. A ticket that mixes requests from before the rollout (no `glue` in the ledger) is not marked replayable and gets the GEMM check only (the swarm's ticket open at 18:35 UTC). The test box demo stack is down since its 16:26 UTC reboot and will run cadre-glue-v1 when restarted (the worker's default). (5) **Privacy**: none. A blobs already reveal prompts. (6) **Architectures**: Llama 3.x only (attention_scaling 1, head_dim <= 128, <= 8192 positions per request). |
