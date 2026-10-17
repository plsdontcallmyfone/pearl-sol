# Commonware for the agent chain (own PoW chain)

Research subagent, 2026-10-16. Scope: what Commonware gives an own-chain, Pearl-PoW node
(Nakamoto fork choice, ed25519 accounts shared with Solana, bridged native coin with no
minting). Sources read this session:

- monorepo `github.com/commonwarexyz/monorepo` @ `7ef4477` (2026-10-16, main), workspace
  version `2026.9.0`, cloned to the session scratchpad (`.../scratchpad/commonware`).
- `commonwarexyz/alto` @ `1d87569` (2026-09-09), `commonwarexyz/constantinople` @ `3b6c92e`
  (2026-10-02), `commonwarexyz/battleware` @ `c7bb74b` (2025-10-20, old 0.0.62 API).
- crates.io API (latest published releases), GitHub issues via `gh`.

Tags: [verified] read in source or ran it; [inference] my reasoning.

## TL;DR

- No PoW or Nakamoto support exists anywhere in Commonware (grep of monorepo, alto,
  constantinople, docs, and issues for "proof of work", "nakamoto", "longest/heaviest chain":
  zero hits) [verified]. Consensus = Simplex BFT only (Minimmit is a spec in `pipeline/`).
  `marshal` (block delivery/backfill) states "Only works with simplex" and `glue::stateful`
  (speculative QMDB state) is driven by marshal finalizations. We write our own chain
  manager; we reuse runtime, codec, cryptography, storage, broadcast, resolver, stream.
- State: `commonware-storage::qmdb::current` (ALPHA) is the fit: authenticated root over
  current key/value state, proofs of current value, in-memory forkable batch chains (one
  per branch), and rollback by re-opening at an earlier op count. Copy constantinople's
  layout (state QMDB + keyless tx-history QMDB, both roots in the header).
- Crypto: Commonware ed25519 keys and signatures interoperate byte-for-byte with Solana
  keypairs (ran a check, below). BLS12-381 multisig/threshold + DKG exist, but Solana has no
  cheap BLS verify, so an ed25519 attester set is the simpler bridge.
- Networking: both p2p modes (`discovery`, `lookup`) only admit peers in an
  application-registered peer set and are fully meshed. Not usable as-is for open
  membership. `commonware-stream` (encrypted, authenticated transport) does accept any key
  if our "bouncer" returns true, so the open p2p layer is ours to build on stream.
- Toolchain: 2026.9.0 does not compile on the local stable rustc 1.94.1 (uses
  `atomic_try_update`, stable in 1.95); main declares `rust-version = "1.95.0"`. Pin
  1.95+ per repo with `rust-toolchain.toml`; do not change the machine default.

## 1. Crates (version 2026.9.0, published 2026-09-03)

License: every crate is `MIT OR Apache-2.0` [verified Cargo.toml + crates.io]. Edition 2024.
MSRV: main `Cargo.toml` `rust-version = "1.95.0"`; the 2026.9.0 crates.io packages carry no
`rust-version` field, but `commonware-utils-2026.9.0/src/concurrency.rs:29` uses
`AtomicX::try_update` and fails on 1.94.1 with E0658; it builds on nightly 1.97 [verified].
Release cadence is calendar versioning, roughly monthly (2026.3.0, .4.0, .5.0, .7.0, .7.1,
.9.0). Platforms: Linux and macOS (Linux recommended for prod); x86-64 needs AVX2/BMI2/ADX.

Stability levels (README "Stability"): ALPHA breaking changes with no migration, BETA
wire+storage formats stable with migration path, GAMMA API stable, DELTA bug bounty,
EPSILON frozen. Nothing in the library is above BETA today. `RUSTFLAGS="--cfg
commonware_stability_BETA"` makes the build fail if you depend on ALPHA APIs.

| Crate | What it gives us | Stability (from `stability_scope!` in `src/lib.rs`) |
|---|---|---|
| `commonware-runtime` | Async runtime trait set (Spawner, Clock, Network, Storage, Metrics). `tokio` backend for prod; `deterministic` simulator (seeded, reproducible, fault injection) for tests; `iouring` (Linux). | tokio BETA; deterministic, mocks, iouring ALPHA |
| `commonware-p2p` | `authenticated::discovery` (bootstrappers + signed address gossip) and `authenticated::lookup` (addresses supplied by app); multiplexed channels, per-channel rate limits, priority, `Blocker`. `simulated` network for tests (latency, loss, partitions). Traits `Sender`, `Receiver`, `Provider`, `Manager`, `Blocker` in `p2p/src/lib.rs`. | authenticated BETA; simulated ALPHA |
| `commonware-stream` | Encrypted authenticated transport: `sake` handshake (Simple Authenticated Key Exchange) + `cups` record layer (ChaCha20-Poly1305). `Upgrader` trait. | BETA |
| `commonware-storage` | `journal` (contiguous/segmented, fixed/variable), `archive` (prunable, immutable; `MultiArchive` for several items per index), `freezer`, `ordinal`, `metadata`, `index`, `rmap`; `merkle` (MMR, MMB), `bmt`; `qmdb` (`any`, `current`, `immutable`, `keyless`, `store`, `sync`), `cache`, `queue`. | journal/archive/freezer/ordinal/metadata/index BETA; merkle, bmt, qmdb, cache, queue ALPHA |
| `commonware-codec` | Deterministic, bounded binary encoding (`Encode`, `Read` with `RangeCfg` limits), derive macros. | BETA core |
| `commonware-cryptography` | ed25519, secp256r1, BLS12-381 (multisig, threshold, DKG, timelock), sha256, blake3, sha512, crc32, chacha20-poly1305, `certificate` scheme trait, transcript, handshake. keccak256, lthash, bloomfilter, reed_solomon, zk, banderwagon are ALPHA. | BETA core |
| `commonware-broadcast` | `buffered::Engine`: broadcast to peers, per-peer bounded cache, serve on demand by digest. Only primaries of a tracked peer set may buffer. | BETA |
| `commonware-resolver` | `p2p` resolver: fetch data by fixed-length key from peers, peer scoring, retry, block on invalid; `Consumer` validates, `Producer` serves. | BETA |
| `commonware-consensus` | `simplex` BFT (schemes: ed25519, secp256r1, bls12381_multisig, bls12381_threshold), `marshal` (ordered finalized-block delivery + backfill, simplex only), traits `Block`, `Heightable`, `Automaton`, `Relay`, `Reporter`, `Application`. `aggregation` ALPHA. | simplex, marshal BETA; aggregation ALPHA |
| `commonware-glue` | `stateful` (speculative + finalized QMDB state driven by marshal, state sync), `dkg`, `simulate`. | ALPHA |
| `commonware-coding` | Reed-Solomon erasure coding (marshal coding mode). | ALPHA |
| `commonware-collector` | Send committable requests to peers and collect responses. | ALPHA |
| `commonware-actor` | Mailbox/actor helpers with backpressure feedback. | BETA |
| `commonware-math`, `parallel`, `utils`, `macros`, `formatting`, `simd`, `invariants`, `conformance` | Algebra (BLS fields), parallel fold strategies (`Sequential`, rayon), helpers (`union_unique`, NZ macros), stability macros, format conformance tests. | mostly BETA |
| `commonware-deployer` | Deploy binaries to AWS EC2 (multi-region), metrics/log collection. | ALPHA (`aws` only) |
| `sol/` (not a crate) | Solidity verifiers for MMR/MMB/BMT and QMDB `any`/`current`/`immutable`/`keyless` proofs plus Simplex certificates (`sol/src/qmdb`, `sol/test/LibQMDBCurrent.t.sol`). Reference for a Solana-side QMDB proof verifier. | n/a |

Examples in the monorepo (`examples/`): `bridge` (send BLS threshold certificates between two
simplex networks), `chat`, `dkg`, `estimator` (simulate mechanism latency over real
regions), `flood`, `log`. Separate repos: `alto` (minimal simplex chain, opaque payload),
`constantinople` (account-model chain: QMDB state, mempool, HTTP ingress, indexer,
explorer; the best template), `battleware` (older API; VRF, mempool, MMR).

## 2. Consensus: building PoW on Commonware

### What exists

- BFT only: `consensus/src/simplex` (+ `pipeline/minimmit/minimmit.md`, a spec with Quint
  model, no code) [verified].
- `consensus/src/marshal/mod.rs` "Limitations": "Only works with simplex rather than
  general consensus" and "Assumes at-most one notarization per view" [verified]. It
  delivers blocks only once a finalization certificate exists. Not reusable for PoW.
- `glue/src/stateful/mod.rs`: `Application` requires `SigningScheme: certificate::Scheme`
  and an `Epochable` context; finalization arrives as `marshal::Update::Block` and it
  panics on a conflicting block at the tip height (no reorg past what it applied)
  [verified]. Reference design only, do not depend on it.
- Issue #2466 (closed, alto PR 169) built "follower" nodes that run without simplex:
  certificates + a resolver feed. A PoW header is self-certifying, so every PoW full node
  is structurally an alto follower whose "certificate" is the Pearl proof [inference].

### Cleanest build (own `consensus-pow` crate on runtime + p2p + storage)

1. Block = `Header { parent, height, timestamp, nbits, job_id, state_root, state_range,
   txs_root, txs_range, miner(ed25519 32B), pearl_public(164B) } + pearl_proof (~12.6 KB) +
   body(txs)`. Digest = sha256 or blake3 of the encoded header (proof excluded from the
   digest but bound by the work statement, as in Pearl). Implement
   `commonware_consensus::{Heightable, Block}` + `Digestible` + codec so storage/resolver
   generics work. Template: `alto/types/src/block.rs`, `constantinople/crates/primitives/src/block.rs`.
2. Gossip: two p2p channels. `blocks` (new tip announcements: header + proof, small; body
   fetched on demand) via `broadcast::buffered::Engine` or a plain channel; `txs` for
   mempool gossip.
3. Sync/backfill: `commonware_resolver::p2p` keyed by `Digest` (fetch block by hash) and a
   second resolver keyed by `(height, locator)` for headers-first catch-up. `Consumer`
   checks: decodes within bounds, digest matches, PoW proof valid (95 us), nbits matches
   our difficulty rule for that parent; invalid -> `Outcome::Invalid` blocks the peer.
   Pattern: `consensus/src/marshal/resolver/p2p.rs`, `alto/follower/src/resolver.rs`.
4. Fork choice: own actor holding a block tree (all valid headers since the last
   checkpoint) with cumulative work per node; tip = max cumulative work (Pearl's WTEMA
   difficulty per block, `research/NOTES-proof.md` section 6). Validate header (PoW,
   difficulty, timestamp median-time-past) before fetching/executing the body.
5. Storage for blocks: `archive::prunable` implements `MultiArchive` (several values per
   index), so index = height, key = digest stores every competing block at a height
   [verified `storage/src/archive/prunable/storage.rs:876`]. Canonical height -> digest map
   in `metadata` (small, atomic batch commit) or recomputed from the tree. Old
   non-canonical blocks pruned by section.
6. Reorg handling: execute each candidate branch in in-memory QMDB batches forked from the
   parent's batch (section 3). Treat blocks deeper than `K` (e.g. 100, matching Pearl's
   coinbase maturity) as final: only then `apply_batch` + `commit` to disk. Reorgs deeper
   than the applied tip but within the persisted window use the bounded re-open rewind.
   Refuse reorgs deeper than `K` (explicit max-reorg-depth checkpoint) [inference].
7. Mining: miners need not be p2p peers. Expose `getblocktemplate` / `submitblock` style
   RPC (job + header template, miner returns proof); the node verifies and gossips.
8. Testing: `runtime::deterministic` + `p2p::simulated` give reproducible partitions and
   latency to exercise fork races and reorgs; `examples/estimator` for regional latency.

### Reusable patterns

- Account model, executor, tx batch verification: `constantinople/crates/primitives/src/{account,auth,transaction}.rs`,
  `crates/application/src/executor.rs` (all-or-nothing verify, selective executor for
  proposals), `crates/application/src/consensus/db.rs` (state + tx-history merkleize in parallel).
- Mempool + HTTP ingress: `constantinople/crates/mempool/src/webserver/http.rs`
  (axum routes `POST /transactions`, `GET /transactions/{batch_id}`, `GET /account/{public_key}`),
  `battleware/node/src/application/mempool.rs`.
- Indexer/explorer: `alto/indexer/src/lib.rs` (axum: `/block/{query}`, `/finalization/{query}`,
  `/consensus/ws` websocket), `alto/chain/src/indexer/pusher.rs` (node pushes to indexer),
  `constantinople/crates/indexer` (publishes block artifacts + SQL metadata, QMDB proofs).

## 3. State storage

Fit: `commonware_storage::qmdb::current` (ALPHA), probably `current::ordered` for
non-membership proofs, `unordered::fixed` if values are fixed-size accounts.

- `qmdb::any` proves a key had a value at some point; `qmdb::current` adds a grafted
  activity bitmap so one proof shows the value is current under a single root
  (`storage/src/qmdb/current/mod.rs` header) [verified]. Light-client balance proofs need
  `current`.
- Bridge-out events should go to an append-only `qmdb::keyless` (or `immutable`) log whose
  root is in the header: a withdrawal record never changes, so an inclusion proof against
  a K-deep header's `txs_root`/`withdrawals_root` is enough for the Solana side. Historical
  proofs: `any::Db::historical_proof(historical_size, ...)` and
  `current::Db::ops_historical_proof` [verified `any/db.rs:500`, `current/db.rs:415`].
- Forks: `db.new_batch().write(..).merkleize(&db, metadata, ..)` gives a speculative root;
  `batch.new_batch()` chains children; sibling batches fork from one parent; applying one
  makes the others `StaleBatch` (`qmdb/any/mod.rs` doc examples, `qmdb/chain.rs`)
  [verified]. One batch chain per live branch = free shallow reorgs.
- Rollback on disk: `init(context, config, Some(max_size))` re-opens at the latest commit
  with at most `max_size` ops and durably drops later history (`qmdb/mod.rs`
  "Initialization bounds"; test `qmdb/any/mod.rs:880-925` drops, re-opens bounded, gets
  the old root, then writes a new branch) [verified]. Store block digest + op count in
  each commit's metadata to map heights to `max_size`. Limit: fails with
  `HistoricalFloorPruned` once pruning passes the target's inactivity floor, so prune only
  below the K-deep checkpoint.
- State sync for new nodes: `qmdb::sync` (fetch ops from untrusted peers, verify against a
  target root from a header) and `glue::stateful::db::p2p` show the wiring.
- Merkle family: MMR or MMB (`merkle::{mmr, mmb}`); hasher Sha256 matches Solana's sha256
  syscall, which matters if a Solana program verifies QMDB proofs (port from `sol/src/qmdb`).

## 4. Cryptography

- ed25519 [verified by running code]: `commonware-cryptography 2026.9.0`
  `ed25519::PrivateKey::decode(seed32)` gives the same public key as an ed25519-dalek
  (Solana) keypair from that seed; `PrivateKey::sign(namespace, msg)` signs
  `union_unique(namespace, msg) = varint(len(ns)) || ns || msg`, and the signatures are
  byte-identical to dalek's over those bytes and pass dalek `verify_strict`; wallet-made
  signatures over the framed bytes verify in Commonware. 16 seeds, all pass (scratchpad
  crate `.../scratchpad/edcheck`, built with `cargo +nightly`). So a Solana address (32 B
  pubkey) is directly an agent-chain account key, as constantinople's `AccountKey` already
  does. Wallet UX: the wallet signs the framed tx bytes via `signMessage`.
- Verification rules are ZIP-215 (`cryptography/src/ed25519/mod.rs`), Solana uses dalek
  strict verification; honest signatures pass both, only crafted edge cases differ. Batch
  verification exists (`ed25519/core/batch.rs`, constantinople `TransactionBatchVerifier`).
- BLS12-381 (`bls12381`, feature `bls12381` via `blst`): `certificate::{multisig,
  threshold}`, `dkg` (Feldman-Desmedt, Golden), timelock encryption. `examples/bridge` sends
  threshold certificates between chains. For a Solana-bound bridge attester set an
  ed25519 M-of-N set is cheaper (native ed25519 precompile); BLS would need a pairing
  verifier on Solana [inference].

## 5. Networking and open membership

- `discovery` (`p2p/src/authenticated/discovery/mod.rs`): "fully-connected peers";
  discovery bit vectors are indexed by a peer set the application registers with
  `Manager::track(index, peers)`; a listener accepts a dialer only if its key is in some
  tracked set; `max_peers_per_set` bounds memory and panics if exceeded [verified].
- `lookup`: same, plus addresses supplied by the app; inbound from IPs not registered is
  rejected [verified].
- So neither supports permissionless public peers, and full mesh does not scale to a
  public node count. No issue or roadmap item for open membership (searched issues);
  #1222 covered DoS from unauthenticated handshakes [verified].
- `commonware-stream`: `Upgrader::listen(.., bouncer, ..)` passes the unverified claimed
  key to our bouncer; `|_| async { true }` admits anyone, then SAKE proves the identity
  and CUPS encrypts (`stream/src/lib.rs:83-110`, `stream/src/sake/mod.rs`) [verified].

Options, best first:
1. Own `net` crate: runtime TCP + stream (`Sake` + `Cups`) with an open bouncer, our own
   address book (Bitcoin-style addr gossip, 8 outbound / ~64 inbound, per-IP/subnet caps,
   anchor peers, ban scores), exposing `commonware_p2p::{Sender, Receiver, Provider,
   Blocker}` so `broadcast` and `resolver` run on it unchanged (Provider returns the current
   connected set as one rolling peer set).
2. Interim: `discovery` with a peer set derived from chain state at the K-deep checkpoint
   (registered node keys, or recent miners), big `max_peers_per_set`. Mining stays
   permissionless through RPC `submitblock` even if the full-node mesh is gated.

## 6. Proposed crate layout

| Crate | Purpose | Commonware deps |
|---|---|---|
| `crates/agent-types` | Block/header/tx/account types, codec, digests, genesis (1B preallocated to bridge escrow, no coinbase mint: miners paid from job escrow + fees) | codec, cryptography (ed25519, sha256), consensus traits (`Block`, `Heightable`) |
| `crates/pearl-job` (exists) | Pearl plain proof mine/verify | none |
| `crates/consensus-pow` | Header validation, WTEMA difficulty, block tree, cumulative-work fork choice, reorg events, max-reorg-depth checkpoint | runtime, storage (`archive::prunable` multi, `metadata`), cryptography |
| `crates/state` | Executor (transfers, job escrow, bridge escrow unlock/lock), QMDB `current` state + `keyless` tx/withdrawal logs, batch chain per branch, bounded rewind | storage (`qmdb`), parallel |
| `crates/mempool` | Tx pool keyed by sender/nonce, re-insert txs from orphaned blocks, gossip | p2p channel or `broadcast`, codec, cryptography (batch verify) |
| `crates/net` | Open p2p (option 1 above) implementing p2p traits | stream, runtime, p2p (traits), codec |
| `crates/sync` | Headers-first + body backfill, state sync | resolver, storage (`qmdb::sync`) |
| `crates/rpc` | axum HTTP/WS: submit tx, account + proof, block, getblocktemplate/submitblock, bridge proofs | storage proofs, codec |
| `bin/agent-node` | Wires actors, config, metrics | runtime (`tokio`), all of the above |
| tests | Fork races, partitions, deep-reorg refusal | runtime `deterministic`, p2p `simulated` |

## Three biggest risks

1. Consensus and networking are ours, not Commonware's. Fork choice, difficulty, reorg
   execution, eclipse/DoS-resistant open p2p and headers-first sync are the parts Bitcoin
   took years to harden, and none of it is in the library (marshal/glue/p2p all assume a
   known BFT validator set). Most of the security surface is new code.
2. Library churn: QMDB, merkle, glue, deterministic runtime are ALPHA ("breaking changes
   expected, no migration path"); monthly calendar releases changed on-disk formats as
   recently as 2026.9.0 (V1 blob layout, `RELEASE_NOTES.md`), and MSRV moves fast (1.95
   today, local stable is 1.94.1). Pin an exact version (or git rev, as constantinople
   does) and treat upgrades as chain migrations.
3. PoW-specific costs on top of storage built for BFT: every header carries a ~12.6 KB
   proof, so the header chain grows ~2 GB/year at Pearl's 3 min 14 s block time and ~40
   GB/year at 10 s; reorg depth is bounded by the in-memory batch window and QMDB pruning
   floor; probabilistic finality means the bridge must wait K blocks (minutes to hours)
   and must be designed so a reorg can never double-release escrow on Solana.
