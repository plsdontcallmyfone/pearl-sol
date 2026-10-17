# Running an agent-chain node

Binary: `chain/target/release/agent-node` (`cd chain && cargo build --release -p agent-node`,
Rust 1.95.0 pinned in chain/rust-toolchain.toml). RPC methods: docs/RPC.md.

```
agent-node run --config node.json [--net static|open] [--seeds a,b] [--p2p-listen ip:port] [--p2p-external ip:port]
agent-node mine --rpc URL --miner KEY [--blocks N]     # fallback blocks over RPC (dev)
agent-node pubkey SEED_HEX                             # p2p / account key of a 32-byte seed
```

Local devnets: `scripts/devnet.sh init|start|status|mine N|stop|reset` (3 static nodes,
`.devnet/`) and `scripts/devnet.sh open ...` (5 open-membership nodes, `.devnet-open/`, node 0
is the seed). Ports: node i RPC 10630+2i, WS 10631+2i, p2p 10640+i.

## genesis.json (identical on every node)

| field | meaning |
|---|---|
| `chain_id` | u32, signed into every tx |
| `timestamp_ms` | genesis time |
| `target_bits` | genesis target, compact (`"0x207fffff"` or a number) |
| `admin` | AdminConfig key (base58 or hex) |
| `attesters`, `attester_threshold` | bridge attester set, M of N |
| `params` | overrides by getParams name, e.g. `{"finality_k": 30, "min_fee": 1000, "fallback_m": 4, "fallback_n": 8, "min_protocol_fee": "1000000"}` |

The genesis hash is a digest of this spec, so any change (including a changed default in
`state::config::Config::defaults`, such as the fallback size moving to 4 x 8 on
2026-10-17) makes a new chain: existing data directories refuse to open with it
(`stored genesis does not match`). `scripts/devnet.sh reset` and `init` again.

## node.json (per node)

| field | default | meaning |
|---|---|---|
| `data_dir` | required | storage root (chain, block archive, rewards index, blobs, open-net address book) |
| `genesis` | required | path of genesis.json |
| `p2p_key` | required | 32-byte ed25519 seed (hex) of the node's p2p identity |
| `p2p_listen` | required | `ip:port` to listen on |
| `rpc_listen`, `ws_listen` | required | HTTP JSON-RPC and WebSocket `ip:port` |
| `net` | `"static"` | `"static"` (Commonware discovery over `peers`) or `"open"` |
| `peers` | `[]` | static mode only: `[{"key": base58/hex, "addr": "ip:port"}]`, may include self |
| `seeds` | `[]` | open mode: `"host:port"` or `"key@host:port"` (a key pins the seed's identity) |
| `p2p_external` | none | open mode: address to advertise (default: `p2p_listen` if dialable) |
| `open` | `{}` | open-mode limits, below |
| `blobs` | `{}` | blob store limits, below |
| `verifier` | `"pearl"` | `"pearl"` (real proofs) or `"mock"` (tests only) |
| `tip_interval_ms` | 2000 | how often the node announces its tip (drives backfill) |

### `open` (all optional)

Base values come from `net::open::Config::local` when `p2p_listen` is a loopback/private
address (devnets) and from `Config::recommended` otherwise.

| field | recommended | local | meaning |
|---|---|---|---|
| `allow_private` | false | true | accept, gossip and dial private addresses; exempts them from per-IP limits and IP bans |
| `max_outbound` | 8 | 8 | outbound connections kept (one per /16 or /32 subnet) |
| `max_inbound` | 64 | 64 | inbound connections |
| `max_inbound_per_ip` | 4 | 4 | |
| `max_inbound_per_subnet` | 16 | 16 | |
| `ban_threshold` | 100 | 100 | misbehaviour score that bans a peer (invalid block gossip bans at once) |
| `ban_duration_s` | 86400 | 86400 | |
| `dial_interval_ms` | 1000 | 500 | outbound dial attempt period |
| `gossip_interval_ms` | 60000 | 5000 | address gossip period |
| `ping_interval_ms` | 20000 | 5000 | |
| `idle_timeout_ms` | 90000 | 30000 | drop a silent connection |

Open mode also persists the address book to `<data_dir>/p2p-addrs.json` every 10 s and
loads it at start, so a node rejoins without its seeds. `getPeers` lists live connections.

### `blobs` (all optional)

| field | default | meaning |
|---|---|---|
| `enabled` | true | false: no blob store; putBlob/getBlob fail and templates do not check data availability |
| `max_blob_bytes` | 4194304 | largest blob (RPC or p2p); at most 7 MiB |
| `max_total_bytes` | 1073741824 | total stored; puts beyond it fail until pruning frees room |
| `grace_blocks` | 100 | kept this long after the last referencing job closes (or after storing, if unreferenced) |
| `put_rate_bytes_per_s`, `put_burst_bytes` | 1048576, 16777216 | putBlob token bucket (whole node) |
| `peer_rate_bytes_per_s`, `peer_burst_bytes` | 1048576, 16777216 | per-peer budget for pushed blobs |

Blobs live in `<data_dir>` under the `node-blobs` partition, one file per id. They are not
consensus state: deleting them never makes a node reject a block; it only stops the node
offering those jobs in its templates until it fetches the data again.
