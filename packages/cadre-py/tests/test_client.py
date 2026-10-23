"""Python client checks: ed25519 (pure Python == cryptography), Cadre tx framing and the local
SendMessage encoding byte for byte against the TypeScript SDK, Solana slot signing, no key in repr."""
import base64
import json
import os
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer

from cadre_connect import Cadre, b58encode, b58decode, sign_solana_tx, framed
from cadre_connect._ed25519 import Signer, pure_sign

# from packages/cadre-sdk (encodeWindowBody + signingBytes + sign), seed 32 x 0x03, recipient seed 32 x 0x04
VEC = {"to": "EdmxWPmx2WH6WgFfTdu9xfkYf3k1g5wD1zccTVySEEh1",
       "signing_hex": "0101005350ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d10900000000000000dc0500000000000010ca93ac1705187071d67b83c7ff0efe8108e8ec4530575d7726879333dbdabe7c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000007000000000000000000000008006869207468657265000000000000000000000000000000000000000000000000000000000000000000000a7265715f616263313233",
       "sig_hex": "a6c65c8c7bd3fd5ec2bf654f8ad5b720b9399504b9667880c4960168403ce6ab7ee750ed50cbeda053195c2bc29e24d7a05d51c4ffc85ee2f672dedb7bf4f003",
       "pub": "GyGKxMyg1p9SsHfm15MkNUu1u9TN2JtTspcdmrtGUdse"}


def key_file(seed):
    s = Signer(seed)
    d = tempfile.mkdtemp()
    p = os.path.join(d, "k.json")
    with open(p, "w") as f:
        json.dump(list(seed) + list(s.public_key), f)
    os.chmod(p, 0o600)
    return p


class T(unittest.TestCase):
    def test_ed25519_pure_matches(self):
        seed = bytes([3] * 32)
        msg = b"cadre"
        self.assertEqual(Signer(seed).sign(msg), pure_sign(seed, msg))
        self.assertEqual(b58encode(Signer(seed).public_key), VEC["pub"])

    def test_send_message_matches_ts(self):
        c = Cadre(key_file(bytes([3] * 32)))
        t = {"kind": "SendMessage", "to": VEC["to"], "mode": 0, "pay": "7", "body": "hi there", "thoughtRef": "req_abc123", "fee": "1500"}
        sb = c._send_message_signing(t, 1347616769, 9)
        self.assertEqual(sb.hex(), VEC["signing_hex"])
        self.assertEqual(c._signer.sign(framed(sb)).hex(), VEC["sig_hex"])
        self.assertNotIn("03030303", repr(c))

    def test_solana_slot(self):
        seed = bytes([3] * 32)
        me, other = Signer(seed), Signer(bytes([9] * 32))
        msg = bytes([2, 0, 1, 3]) + other.public_key + me.public_key + bytes(32) + bytes([7] * 32) + bytes([0])
        tx = base64.b64encode(bytes([2]) + bytes(128) + msg).decode()
        out = base64.b64decode(sign_solana_tx(tx, b58encode(me.public_key), me.sign))
        self.assertEqual(out[1:65], bytes(64))
        self.assertEqual(out[65:129], me.sign(msg))

    def test_key_file_mode(self):
        p = key_file(bytes([5] * 32))
        os.chmod(p, 0o644)
        with self.assertRaises(PermissionError):
            Cadre(p)
        self.assertEqual(b58decode(b58encode(b"\0\0ab")), b"\0\0ab")


    def test_town_post_tx_matches_ts(self):
        """The Solana town post tx signed here equals the TS SDK's bytes (vectors/town-post.json)."""
        with open(os.path.join(os.path.dirname(__file__), "vectors", "town-post.json")) as f:
            vec = json.load(f)
        me = Signer(bytes([3] * 32))
        self.assertEqual(sign_solana_tx(vec["unsigned"], b58encode(me.public_key), me.sign), vec["signed"])
        self.assertEqual(sign_solana_tx(vec["unsigned"], b58encode(me.public_key), lambda m: pure_sign(bytes([3] * 32), m)), vec["signed"], "pure Python ed25519 too")

    def test_build_signs_solana_town_tx_and_sends_to_solana(self):
        with open(os.path.join(os.path.dirname(__file__), "vectors", "town-post.json")) as f:
            vec = json.load(f)
        seen = {}

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["content-length"])))
                seen.setdefault(self.path, []).append(body)
                if self.path == "/v1/tx/post":
                    out = {"transaction": vec["unsigned"], "signers": [body["from"]], "chain": "solana", "post_id": vec["post_id"], "nonce": "42"}
                elif self.path == "/v1/tx/solana":
                    out = {"signature": "5" * 88}
                else:
                    self.send_response(500)
                    self.end_headers()
                    return
                raw = json.dumps(out).encode()
                self.send_response(200)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(raw)))
                self.end_headers()
                self.wfile.write(raw)

        srv = HTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        try:
            c = Cadre(key_file(bytes([3] * 32)), api=f"http://127.0.0.1:{srv.server_port}")
            r = c.post("hello from python", thought_ref="req_py1")
            self.assertEqual(seen["/v1/tx/post"][0], {"body": "hello from python", "thought_ref": "req_py1", "kind": "post", "from": c.address})
            self.assertEqual(seen["/v1/tx/solana"][0]["tx"], vec["signed"])
            self.assertNotIn("/v1/tx/chain", seen)
            self.assertEqual((r["chain"], r["post_id"], r["nonce"]), ("solana", vec["post_id"], "42"))
        finally:
            srv.shutdown()


if __name__ == "__main__":
    unittest.main()
