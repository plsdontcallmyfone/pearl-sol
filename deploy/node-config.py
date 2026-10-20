#!/usr/bin/env python3
"""node.json for devnet box N (docs/NODE.md): open-membership p2p on 0.0.0.0:10640, RPC/WS on
127.0.0.1 (caddy fronts them), seeds = the other boxes pinned by key. The p2p seed comes from
keys/devnet/nodeN-p2p-keypair.json (Mac only, never committed)."""
import json, os, subprocess, sys

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
env = {}
for line in open(os.path.join(root, "deploy/devnet.env")):
    line = line.strip()
    if line and not line.startswith("#") and "=" in line:
        k, v = line.split("=", 1)
        env[k] = v
n = int(sys.argv[1])
key = lambda i: os.path.join(root, f"keys/devnet/node{i}-p2p-keypair.json")
seeds = []
for i in range(3):
    ip = env.get(f"BOX{i}_IP", "")
    if i == n or not ip:
        continue
    pub = subprocess.check_output(["solana-keygen", "pubkey", key(i)], text=True).strip()
    seeds.append(f"{pub}@{ip}:10640")
seed_hex = bytes(json.load(open(key(n)))[:32]).hex()
ip = env[f"BOX{n}_IP"]
# BOXn_BLOBS (devnet.env, JSON in single quotes): this node's blob store limits (docs/NODE.md);
# box 0 holds the registered model's blobs (Llama-3.2-1B int7, about 1.8 GB, put with push=false)
blobs = json.loads(env[f"BOX{n}_BLOBS"].strip("'")) if env.get(f"BOX{n}_BLOBS") else None
cfg = {
    "data_dir": "/var/lib/lode/chain",
    "genesis": "/etc/lode/genesis.json",
    "p2p_key": seed_hex,
    "p2p_listen": "0.0.0.0:10640",
    "p2p_external": f"{ip}:10640",
    "rpc_listen": "127.0.0.1:10630",
    "ws_listen": "127.0.0.1:10631",
    "net": "open",
    "seeds": seeds,
    "verifier": "pearl",
    "tip_interval_ms": 2000,
}
if blobs:
    cfg["blobs"] = blobs
print(json.dumps(cfg, indent=2))
