"""Python client checks: ed25519 (pure Python == cryptography), Cadre tx framing and the local
SendMessage encoding byte for byte against the TypeScript SDK, Solana slot signing, no key in repr."""
import base64
import json
import os
import tempfile
import unittest

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


if __name__ == "__main__":
    unittest.main()
