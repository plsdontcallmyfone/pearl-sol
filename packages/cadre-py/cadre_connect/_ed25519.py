"""Ed25519 signing (RFC 8032). Uses the `cryptography` package when installed; otherwise a small
pure-Python implementation of the RFC 8032 reference (slow, fine for an agent's few signatures).
The secret seed stays inside the Signer object; repr() shows only the public key."""
import hashlib

try:
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    from cryptography.hazmat.primitives import serialization
    _HAVE = True
except Exception:  # pragma: no cover
    _HAVE = False

_p = 2**255 - 19
_L = 2**252 + 27742317777372353535851937790883648493
_d = -121665 * pow(121666, _p - 2, _p) % _p
_I = pow(2, (_p - 1) // 4, _p)


def _xrecover(y):
    xx = (y * y - 1) * pow(_d * y * y + 1, _p - 2, _p)
    x = pow(xx, (_p + 3) // 8, _p)
    if (x * x - xx) % _p != 0:
        x = (x * _I) % _p
    if x % 2 != 0:
        x = _p - x
    return x


_By = 4 * pow(5, _p - 2, _p) % _p
_B = (_xrecover(_By), _By, 1, _xrecover(_By) * _By % _p)


def _add(P, Q):
    A = (P[1] - P[0]) * (Q[1] - Q[0]) % _p
    B = (P[1] + P[0]) * (Q[1] + Q[0]) % _p
    C = 2 * P[3] * Q[3] * _d % _p
    D = 2 * P[2] * Q[2] % _p
    E, F, G, H = B - A, D - C, D + C, B + A
    return (E * F % _p, G * H % _p, F * G % _p, E * H % _p)


def _mul(s, P):
    Q = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            Q = _add(Q, P)
        P = _add(P, P)
        s >>= 1
    return Q


def _enc(P):
    zi = pow(P[2], _p - 2, _p)
    x, y = P[0] * zi % _p, P[1] * zi % _p
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


def _expand(seed):
    h = hashlib.sha512(seed).digest()
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, h[32:]


class Signer:
    def __init__(self, seed: bytes):
        if len(seed) != 32:
            raise ValueError("an ed25519 seed is 32 bytes")
        self.__seed = bytes(seed)
        if _HAVE:
            self.__k = Ed25519PrivateKey.from_private_bytes(self.__seed)
            self.public_key = self.__k.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
        else:
            a, _ = _expand(self.__seed)
            self.public_key = _enc(_mul(a, _B))

    def sign(self, msg: bytes) -> bytes:
        if _HAVE:
            return self.__k.sign(bytes(msg))
        a, prefix = _expand(self.__seed)
        r = int.from_bytes(hashlib.sha512(prefix + msg).digest(), "little") % _L
        R = _enc(_mul(r, _B))
        k = int.from_bytes(hashlib.sha512(R + self.public_key + msg).digest(), "little") % _L
        return R + ((r + k * a) % _L).to_bytes(32, "little")

    def __repr__(self):
        return f"Signer(public_key={self.public_key.hex()})"


def pure_sign(seed: bytes, msg: bytes) -> bytes:
    """The pure-Python path regardless of `cryptography` (tests)."""
    a, prefix = _expand(seed)
    pub = _enc(_mul(a, _B))
    r = int.from_bytes(hashlib.sha512(prefix + msg).digest(), "little") % _L
    R = _enc(_mul(r, _B))
    k = int.from_bytes(hashlib.sha512(R + pub + msg).digest(), "little") % _L
    return R + ((r + k * a) % _L).to_bytes(32, "little")
