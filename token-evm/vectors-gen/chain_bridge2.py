#!/usr/bin/env python3
"""Copies the chain builder's bridge v2 vectors into token-evm/test/vectors/chain-bridge2.json
in hex form the Foundry tests read (base58 accounts decoded, the PostJob body split into its
fields so the storefront can re-encode it).

Usage: python3 -I chain_bridge2.py <path to chain/agent-types/vectors/bridge2.json> <source commit>
"""
import json
import sys
from pathlib import Path

B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58(s: str) -> bytes:
    n = 0
    for c in s:
        n = n * 58 + B58.index(c)
    raw = n.to_bytes((n.bit_length() + 7) // 8, "big") if n else b""
    pad = len(s) - len(s.lstrip("1"))
    out = b"\0" * pad + raw
    assert len(out) == 32, s
    return out


def h(b: bytes) -> str:
    return "0x" + b.hex()


def split_body(body: bytes) -> dict:
    o = 0

    def take(n):
        nonlocal o
        v = body[o:o + n]
        o += n
        return v

    le = lambda b: int.from_bytes(b, "little")
    f = {}
    f["m"] = le(take(4))
    f["n"] = le(take(4))
    f["k"] = le(take(4))
    f["config"] = h(take(52))
    f["hashA"] = h(take(32))
    f["hashB"] = h(take(32))
    ulen = le(take(2))
    f["dataUri"] = h(take(ulen))
    f["workerPay"] = le(take(8))
    f["protocolFee"] = le(take(8))
    f["deadline"] = le(take(8))
    f["maxBlocks"] = le(take(4))
    ne = take(1)[0]
    f["evaluators"] = [h(take(32)) for _ in range(ne)]
    f["threshold"] = take(1)[0]
    assert o == len(body)
    return f


def main():
    src = json.loads(Path(sys.argv[1]).read_text())
    commit = sys.argv[2]
    msgs = {m["name"]: m for m in src["messages"]}
    d = msgs["deposit_v2"]
    od = msgs["deposit_order_v2"]
    w = src["withdrawal_v2"]
    body = bytes.fromhex(od["post_job_body"])
    out = {
        "source": f"chain/agent-types/vectors/bridge2.json at commit {commit} (chain builder worktree, branch worktree-agent-adec60493c3c242a0)",
        "deposit": {
            "domain": d["domain"], "chainId": d["ext_chain_id"], "bridge": "0x" + d["bridge_addr"],
            "nonce": d["ext_nonce"], "recipient": h(b58(d["recipient"])), "amount": int(d["amount_ext"]),
            "message": "0x" + d["message"],
        },
        "order": {
            "domain": od["domain"], "chainId": od["ext_chain_id"], "bridge": "0x" + od["bridge_addr"],
            "nonce": od["ext_nonce"], "recipient": h(b58(od["recipient"])), "amount": int(od["amount_ext"]),
            "message": "0x" + od["message"], "jobId": "0x" + od["job_id"], "body": "0x" + od["post_job_body"],
            "bodyHash": "0x" + od["post_job_body_sha256"], "bodyFields": split_body(body),
        },
        "withdrawal": {
            "entry": "0x" + w["entry"], "domain": w["fields"]["domain"], "nonce": w["fields"]["nonce"],
            "from": h(b58(w["fields"]["from"])), "recipient": "0x" + w["fields"]["recipient"],
            "recipientEvm": w["fields"]["recipient_evm"], "amount": int(w["fields"]["amount_ext"]),
            "height": w["fields"]["height"],
        },
    }
    dst = Path(__file__).resolve().parent.parent / "test" / "vectors" / "chain-bridge2.json"
    dst.write_text(json.dumps(out, indent=2) + "\n")
    print("wrote", dst)


main()
