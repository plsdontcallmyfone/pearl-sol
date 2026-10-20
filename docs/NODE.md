# Running an agent-chain node

To mine you do not need a node of your own: `pearl-miner` talks to any node's public RPC, and
`curl -fsSL https://cadre.64-95-12-116.sslip.io/install-miner.sh | bash` builds and starts one
against the public devnet (docs/MINING.md). `agent-node mine` below is a dev helper, not the miner.

Binary: `chain/target/release/agent-node` (`cd chain && cargo build --release -p agent-node`,
Rust 1.95.0 pinned in chain/rust-toolchain.toml). RPC methods: docs/RPC.md.

```
agent-node run --config node.json [--net static|open] [--seeds a,b] [--p2p-listen ip:port] [--p2p-external ip:port]
agent-node mine --rpc URL --miner KEY [--blocks N]     # fallback blocks over RPC (dev)
agent-node pubkey SEED_HEX                             # p2p / account key of a 32-byte seed
```

## Join the public devnet with your own node

Scheduled upgrade: from devnet height 4200 a RegisterModel may carry up to 1024 matrices (was 384,
SPEC 4.5). Build from main of 2026-10-20 or later (any build that has
`DEVNET_MODEL_MATRICES_HEIGHT` in chain/agent-types/src/tx.rs) before then; an older node stops
following the chain at the first block that registers a bigger model.

Verified 2026-10-20 on the test box (Ubuntu 24.04 x86_64, 4 vCPU, load about 19 from another
stack, fresh directory, public repo at main): build 474 s with `CARGO_BUILD_JOBS=2` (largest
process 988 MiB RSS; build cache 778 MB, cargo registry 300 MB, binary 19 MB); a fresh data dir
synced 3,022 blocks in 19 s (102 MB of chain data, node peak RSS 118 MiB) and followed the tip with
3 peers. Rust: same as the miner (rustup, 1.95.0; `chain/rust-toolchain.toml` pins it).

1. Build (needs build-essential, git, curl):

        git clone --depth 1 https://github.com/plsdontcallmyfone/pearl-sol.git && cd pearl-sol/chain
        cargo +1.95.0 build --release --locked -p agent-node     # low memory: CARGO_BUILD_JOBS=2
        install -m 755 target/release/agent-node ~/.local/bin/    # or anywhere on PATH

2. A p2p identity (a 32-byte ed25519 seed, hex; keep it private, it is your node's identity):

        mkdir -p ~/cadre-node/data && cd ~/cadre-node
        (umask 077; head -c 32 /dev/urandom | od -An -v -tx1 | tr -d ' \n' > p2p-key.hex)
        agent-node pubkey "$(cat p2p-key.hex)"                    # your node's public key

3. `~/cadre-node/node.json`, with the genesis from the repo (`deploy/genesis.json`, genesis hash
   `51c86b67...4d52028a`) and the three devnet nodes as seeds, pinned by key (`getPeers` on any
   node lists them). Pick ports that are free (`ss -ltn`); RPC on 127.0.0.1 unless you put a
   proxy in front:

        {
          "data_dir": "/home/you/cadre-node/data",
          "genesis": "/home/you/pearl-sol/deploy/genesis.json",
          "p2p_key": "<contents of p2p-key.hex>",
          "p2p_listen": "0.0.0.0:10640",
          "rpc_listen": "127.0.0.1:10630",
          "ws_listen": "127.0.0.1:10631",
          "net": "open",
          "seeds": [
            "33DN9By34qf7dvbTGuatKq8dMjx7gkDbWtpoRnCsjQdx@64.95.12.116:10640",
            "DGMwftkptkEDRyweCGup6TeAADuQMb5NHtEdL6YNpxX7@64.95.13.213:10640",
            "8HiMBfdEf1NinBYsoePbCUEfxTzCr619qKZxbNvWHQFJ@64.95.12.22:10640"
          ],
          "verifier": "pearl"
        }

   Outbound connections are enough to sync and mine. To accept peers too, open the p2p port in
   your firewall and add `"p2p_external": "<your public ip>:10640"`. `chmod 600 node.json`.

4. Run it and check it caught up (`height` and `tip` equal to a public node's, `syncing` false):

        agent-node run --config ~/cadre-node/node.json
        curl -s -X POST -H 'content-type: application/json' \
          -d '{"jsonrpc":"2.0","id":1,"method":"getChainInfo","params":{}}' http://127.0.0.1:10630

5. As a service, `/etc/systemd/system/cadre-node.service` (the devnet boxes run the same unit,
   deploy/systemd/cadre-node.service), then `sudo systemctl daemon-reload && sudo systemctl
   enable --now cadre-node`; logs: `journalctl -u cadre-node -f`:

        [Unit]
        Description=Cadre devnet agent-chain node
        After=network-online.target
        Wants=network-online.target

        [Service]
        User=you
        ExecStart=/home/you/.local/bin/agent-node run --config /home/you/cadre-node/node.json
        Environment=RUST_LOG=info
        Restart=always
        RestartSec=5
        LimitNOFILE=65536

        [Install]
        WantedBy=multi-user.target

6. Mine against it: `curl -fsSL https://cadre.64-95-12-116.sslip.io/install-miner.sh | bash -s --
   --rpc http://127.0.0.1:10630`. The installer sees the devnet genesis and adds the three public
   nodes after yours, so the miner fails over to them while your node is down.

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
