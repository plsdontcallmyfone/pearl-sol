"""cadre-glue-v1: the deterministic glue between a model's int7 GEMMs (SPEC 4.7).

Every step that turns one segment's C into the next segment's A is specified bit for bit, so a
CPU, a GPU and an independent implementation (crates/cadre-glue, Rust) produce the same A at
every layer, whatever the batch (prefill or one token at a time with a KV cache):

- values that cross a reduction are integers (fixed point): the residual stream h (Q16), the
  RMS sums, q / k (Q12) and v (Q16) for attention, the attention scores, the softmax weights
  (Q20) and their sums, the attention output (Q16). Integer sums are exact in any order.
- elementwise scaling uses single IEEE-754 binary64 operations (+ - * / sqrt, each rounded to
  nearest even, in the written order, never fused): every IEEE machine rounds them the same.
- exp, cos and sin come only from tables built by the integer algorithms below; nothing calls a
  libm function.
- dot products inside attention run as binary64 matmuls of integers whose partial sums are
  bounded below 2^53, so they are exact (any order, any BLAS).

The GEMMs themselves are unchanged (exact int32 C = A B, torch._int_mm), so the registered B and
recipe 1 (`cadre-w7a7-sq-asym-v1`) are the same; only how A is derived differs from the fp32
worker. The normative text is SPEC 4.7; this file is the reference implementation.
"""
import math

import numpy as np
import torch

GLUE_ID = "cadre-glue-v1"
QMAX = 64

H_FRAC = 16                 # residual h: int64, Q16
H_MAX = 2 ** 52             # |h| and every residual increment clamp
QK_FRAC = 12                # q, k after RoPE: Q12
QK_MAX = 2 ** 22            # |q|, |k| clamp (scores: 128 * 2^44 < 2^53)
V_FRAC = 16                 # v: Q16
V_MAX = 2 ** 24             # |v| clamp
P_FRAC = 20                 # softmax weights: Q20
A_PRE_MAX = 2.0 ** 40       # clamp before converting u/s and the zero point to integers
T_MAX = 8192                # positions per request (PV partial sums < 2^53)
X_MAX = 2 ** 34             # score gap clamp (Q24 dot units), exp(-gap) is 0 there anyway
SIG_X_MAX = 2 ** 30         # |gate| clamp for the sigmoid, Q24 nats (64)

LN2_Q60 = 799144290325165978            # floor(ln 2 * 2^60)
LOG2E_Q62 = 6653256548922161245         # floor(log2 e * 2^62)
LOG2E_Q24 = LOG2E_Q62 >> 38             # log2 e, Q24
ONE60 = 1 << 60


# ----------------------------------------------------------------- integer tables (normative)

def exp_neg_q60(x):
    """exp(-x) for 0 <= x <= 1 (x a Q60 int): Taylor to the first zero term, floor steps."""
    term, s, k = ONE60, ONE60, 0
    while True:
        k += 1
        term = (term * x >> 60) // k
        if term == 0:
            return s
        s = s - term if k & 1 else s + term


def exp2_table():
    """E2[i] = 2^(-i/4096) in Q30, i = 0..4096: exp(-(i * LN2_Q60 // 4096)) rounded half up."""
    return [(exp_neg_q60(i * LN2_Q60 // 4096) + (1 << 29)) >> 30 for i in range(4097)]


def cos_sin_q60(x):
    """cos x, sin x for 0 <= x <= 1 (Q60 ints): Taylor to the first zero term, floor steps."""
    c, s = ONE60, x
    term, k = ONE60, 0
    while True:  # cos: x^2k / (2k)!
        term = ((term * x >> 60) * x >> 60) // ((k + 1) * (k + 2))
        k += 2
        if term == 0:
            break
        c = c - term if (k // 2) & 1 else c + term
    term, k = x, 1
    while True:  # sin: x^(2k+1) / (2k+1)!
        term = ((term * x >> 60) * x >> 60) // ((k + 1) * (k + 2))
        k += 2
        if term == 0:
            break
        s = s - term if (k // 2) & 1 else s + term
    return c, s


def f32_to_q60(v):
    """An f32 value in [0, 1] as an exact Q60 int (refuses a value that is not exact)."""
    f = float(np.float32(v))
    m, e = math.frexp(f)  # f = m * 2^e, m in [0.5, 1)
    mi = int(m * (1 << 53))
    sh = e - 53 + 60
    q = mi << sh if sh >= 0 else mi >> -sh
    assert (q / ONE60) == f, f"inv_freq {f} is not exact in Q60"
    return q


class RopeTable:
    """cos / sin of p * theta_i in Q30: seed (cos, sin) of theta_i by Taylor in Q60, then the
    rotation recurrence c_{p+1} = (c_p c_1 - s_p s_1) >> 60, s_{p+1} = (s_p c_1 + c_p s_1) >> 60
    (Python ints = i128 in the Rust implementation), each rounded to Q30 by (x + 2^29) >> 30."""

    def __init__(self, inv_freq):
        self.seeds = [cos_sin_q60(f32_to_q60(v)) for v in inv_freq]
        self.state = [(ONE60, 0) for _ in self.seeds]
        self.rows_c, self.rows_s = [], []

    def ensure(self, n):
        while len(self.rows_c) < n:
            self.rows_c.append([(c + (1 << 29)) >> 30 for c, _ in self.state])
            self.rows_s.append([(s + (1 << 29)) >> 30 for _, s in self.state])
            self.state = [((c * c1 - s * s1) >> 60, (s * c1 + c * s1) >> 60) for (c, s), (c1, s1) in zip(self.state, self.seeds)]

    def get(self, start, n, dev):
        self.ensure(start + n)
        c = torch.tensor(self.rows_c[start:start + n], dtype=torch.float64)
        s = torch.tensor(self.rows_s[start:start + n], dtype=torch.float64)
        return (c * 2.0 ** -30).to(dev), (s * 2.0 ** -30).to(dev)


def softmax_l(hd):
    """Q24 of log2(e) / sqrt(hd): ((LOG2E_Q62 << 20) // isqrt(hd << 40)) >> 38."""
    return ((LOG2E_Q62 << 20) // math.isqrt(hd << 40)) >> 38


# ----------------------------------------------------------------- the glue


class DetGlue:
    def __init__(self, model):
        self.m = model
        dev = self.dev = model.dev
        a = model.a
        assert a.get("type") == "llama", "cadre-glue-v1 covers the llama architecture"
        self.hidden, self.hd = a["hidden"], a["head_dim"]
        assert float(model.attn_scale) == 1.0, "cadre-glue-v1 needs attention_scaling 1"
        self.eps = float(a["rms_norm_eps"])
        self.e2 = torch.tensor(exp2_table(), dtype=torch.int64, device=dev)
        self.L = softmax_l(self.hd)
        self.rope_tab = RopeTable(model.inv_freq.cpu().numpy())
        self.pow2 = torch.tensor([2.0 ** k for k in range(-128, 128)], dtype=torch.float64, device=dev)
        n = self.hidden
        self.norm_bits = (62 - math.ceil(math.log2(n))) // 2
        self.n_t = torch.tensor(float(n), dtype=torch.float64, device=dev)
        self.eps_t = torch.tensor(self.eps, dtype=torch.float64, device=dev)
        self.one_t = torch.tensor(1.0, dtype=torch.float64, device=dev)
        # per linear group: g (input factor, f64), shards, summed colsum, sw (f64)
        self.groups = {}
        norms = {k: v.double() for k, v in model.norms.items()}
        for base, cols in model.by_name.items():
            shards = []
            g = None
            for c in sorted(cols):
                ks = cols[c]
                smooth = None if ks[0].smooth is None else torch.cat([m.smooth for m in ks]).double()
                colsum = ks[0].colsum.double() if len(ks) == 1 else sum(m.colsum.double() for m in ks)
                shards.append((ks, colsum.to(torch.int64), ks[0].sw.double()))
                w = self._norm_for(base, norms)
                gg = w if w is not None else torch.ones(ks[0].k * len(ks), dtype=torch.float64, device=dev)
                if smooth is not None:
                    gg = gg / smooth
                assert g is None or torch.equal(g, gg), f"{base}: shards disagree on the input factor"
                g = gg
            self.groups[base] = (g, shards)

    @staticmethod
    def _norm_for(base, norms):
        if base == "lm_head":
            return norms["norm.final"]
        li, kind = base.split(".")[1], base.split(".")[2]
        if kind == "qkv":
            return norms[f"norm.{li}.attn"]
        if kind in ("gate", "up"):
            return norms[f"norm.{li}.mlp"]
        return None  # o, down: no norm in front

    # ------------------------------------------------------------- primitives

    @staticmethod
    def rne_clamp(x, lim):
        """clamp a binary64 tensor to [-lim, lim], round half to even, int64."""
        return torch.round(torch.clamp(x, -lim, lim)).to(torch.int64)

    def inv_rms(self, h):
        """1 / sqrt(mean(x^2) + eps) per row, x = h / 2^16 (binary64 from exact integer sums)."""
        amax = h.abs().amax(-1)
        _, e = torch.frexp(amax.double())  # bit length of amax (amax < 2^53 by the clamp)
        shift = torch.clamp(e.to(torch.int64) - self.norm_bits, min=0)
        hs = torch.bitwise_right_shift(h, shift[:, None])
        ss = (hs * hs).sum(-1)
        ms = (ss.double() * self.pow2[128 + 2 * shift - 2 * H_FRAC]) / self.n_t
        return self.one_t / torch.sqrt(ms + self.eps_t)

    def quant(self, u):
        """recipe 1's per-row asymmetric int7 in binary64: s = (max-min)/128 (1 if 0),
        z = rne(-64 - min/s), A = clamp(rne(u/s) + z, -64, 64)."""
        mn = u.amin(-1, keepdim=True)
        mx = u.amax(-1, keepdim=True)
        d = mx - mn
        s = torch.where(d == 0, torch.ones_like(d), d * (1.0 / 128))
        z = self.rne_clamp(-64.0 - mn / s, A_PRE_MAX)
        v = torch.clamp(self.rne_clamp(u / s, A_PRE_MAX) + z, -QMAX, QMAX)
        return v.to(torch.int8), s, z

    def lin(self, base, u, sa_scale, cap):
        from worker import int_mm
        """y (binary64) = ((C - z colsum) * sw) * (s * sa_scale) through every shard of `base`.
        u: binary64 input rows (already times the group's g); sa_scale: per-row binary64 factor
        that turns u's units into the real input (inv_rms * 2^-16, 2^-16, or 1)."""
        g, shards = self.groups[base]
        v, s, z = self.quant(u)
        sa = s * sa_scale
        n_total = sum(ks[0].n for ks, _, _ in shards)
        y = torch.empty(u.shape[0], n_total, dtype=torch.float64, device=self.dev)
        off = 0
        for ks, colsum, sw in shards:
            c = None
            for m in ks:
                vs = v[:, m.klo:m.khi].contiguous()
                part = int_mm(vs, m.qw)
                if cap is not None:
                    cap[m.index][0].append(vs)
                    cap[m.index][1].append(part)
                c = part.to(torch.int64) if c is None else c + part.to(torch.int64)
            cz = c - z * colsum
            w = ks[0].n
            y[:, off:off + w] = (cz.double() * sw) * sa
            off += w
            del c, cz
        return y

    def exp2neg(self, t):
        """2^(-t / 2^24) in Q30 for int64 t >= 0: E2[round(frac * 4096)] >> int part."""
        n = torch.bitwise_right_shift(t, 24)
        f = torch.bitwise_and(t, (1 << 24) - 1)
        idx = torch.bitwise_right_shift(f + (1 << 11), 12)
        return torch.bitwise_right_shift(self.e2[idx], torch.clamp(n, max=62))

    def silu(self, g):
        """g * sigmoid(g) in binary64, sigmoid from the integer exp (Q30)."""
        x = self.rne_clamp(g.abs() * 2.0 ** 24, SIG_X_MAX)  # |g| in Q24 nats
        e = self.exp2neg(torch.bitwise_right_shift(x * LOG2E_Q24, 24))
        den = (1 << 30) + e
        sig = torch.where(g >= 0, torch.div(torch.full_like(e, 1 << 60), den, rounding_mode="floor"),
                          torch.div(torch.bitwise_left_shift(e, 30), den, rounding_mode="floor"))
        return g * (sig.double() * 2.0 ** -30)

    def rope(self, y, c, s):
        """y: r x heads x hd binary64; (x1 c - x2 s, x2 c + x1 s), then Q12 clamp."""
        h = y.shape[-1] // 2
        x1, x2 = y[..., :h], y[..., h:]
        c, s = c[:, None, :], s[:, None, :]
        out = torch.cat([(x1 * c) - (x2 * s), (x2 * c) + (x1 * s)], -1)
        return self.rne_clamp(out * 2.0 ** QK_FRAC, QK_MAX - 1)

    def attention(self, q, K, V, pos):
        """q: r x H x hd (Q12 int64), K: T x KV x hd (Q12), V: T x KV x hd (Q16); pos: r positions.
        Returns o: r x H*hd (Q16 int64)."""
        r, H, hd = q.shape
        T, KV = K.shape[0], K.shape[1]
        g = H // KV
        assert T <= T_MAX
        jpos = torch.arange(T, device=self.dev)
        mask = jpos[None, :] > pos[:, None]  # r x T, True = not visible
        out = torch.empty(r, H, hd, dtype=torch.int64, device=self.dev)
        for kv in range(KV):
            Kd = K[:, kv, :].double()                     # T x hd
            Vi = V[:, kv, :]
            v_hi = torch.bitwise_right_shift(Vi, 12).double()
            v_lo = torch.bitwise_and(Vi, 4095).double()
            qh = q[:, kv * g:(kv + 1) * g, :].transpose(0, 1).double()  # g x r x hd
            S = torch.matmul(qh, Kd.t()).to(torch.int64)  # g x r x T, exact (< 2^53)
            mx = S.masked_fill(mask[None], torch.iinfo(torch.int64).min).amax(-1, keepdim=True)
            x = torch.clamp(mx - S, min=0, max=X_MAX)  # masked entries are zeroed below
            t = torch.bitwise_right_shift(x * self.L, 24)
            P = torch.bitwise_right_shift(self.exp2neg(t), 30 - P_FRAC)
            P = P.masked_fill(mask[None], 0)
            D = P.sum(-1, keepdim=True)
            Pd = P.double()
            num = torch.matmul(Pd, v_hi).to(torch.int64) * 4096 + torch.matmul(Pd, v_lo).to(torch.int64)
            o = torch.div(2 * num + D, 2 * D, rounding_mode="floor")
            out[:, kv * g:(kv + 1) * g, :] = o.transpose(0, 1)
        return out.reshape(r, H * hd)

    def add_res(self, h, y):
        return torch.clamp(h + self.rne_clamp(y * 2.0 ** H_FRAC, H_MAX), -H_MAX, H_MAX)

    # ------------------------------------------------------------- forward

    def forward(self, ids, cache, cap):
        """Same contract as worker.Model.forward; logits are binary64. cache[li] = [K, V] ints."""
        m = self.m
        r = len(ids)
        start = cache[0][0].shape[0] if cache[0][0] is not None else 0
        pos = torch.arange(start, start + r, device=self.dev)
        rc, rs = self.rope_tab.get(start, r, self.dev)
        h = self.rne_clamp(m.embed[torch.tensor(ids, device=self.dev)].double() * 2.0 ** H_FRAC, H_MAX)
        H, KV, hd = m.H, m.KV, m.hd
        p16 = 2.0 ** -H_FRAC
        for li in range(m.L):
            inv = self.inv_rms(h)[:, None]
            g = self.groups[f"layers.{li}.qkv"][0]
            qkv = self.lin(f"layers.{li}.qkv", (h.double() * g) + 0.0, inv * p16, cap)
            q = self.rope(qkv[:, : H * hd].reshape(r, H, hd), rc, rs)
            k = self.rope(qkv[:, H * hd: (H + KV) * hd].reshape(r, KV, hd), rc, rs)
            v = self.rne_clamp(qkv[:, (H + KV) * hd:].reshape(r, KV, hd) * 2.0 ** V_FRAC, V_MAX)
            K = k if cache[li][0] is None else torch.cat([cache[li][0], k])
            V = v if cache[li][1] is None else torch.cat([cache[li][1], v])
            cache[li] = [K, V]
            o = self.attention(q, K, V, pos)
            go = self.groups[f"layers.{li}.o"][0]
            h = self.add_res(h, self.lin(f"layers.{li}.o", (o.double() * go) + 0.0, p16, cap))
            inv = self.inv_rms(h)[:, None]
            gm = self.groups[f"layers.{li}.gate"][0]
            x = (h.double() * gm) + 0.0
            gte = self.lin(f"layers.{li}.gate", x, inv * p16, cap)
            up = self.lin(f"layers.{li}.up", x, inv * p16, cap)
            gd = self.groups[f"layers.{li}.down"][0]
            h = self.add_res(h, self.lin(f"layers.{li}.down", ((self.silu(gte) * up) * gd) + 0.0, 1.0, cap))
        inv = self.inv_rms(h)[:, None]
        gf = self.groups["lm_head"][0]
        return self.lin("lm_head", (h.double() * gf) + 0.0, inv * p16, cap)


def pick(logits_row):
    """Greedy token: the largest logit, lowest index on ties."""
    mx = logits_row.max()
    return int(torch.nonzero(logits_row == mx)[0, 0])


def table_digest(inv_freq, hd, positions):
    """sha256 over E2, the RoPE table for `positions` positions and the softmax constant (i64 LE),
    the digest crates/cadre-glue `cadre-replay tables` prints."""
    import hashlib, struct
    h = hashlib.sha256()
    for x in exp2_table():
        h.update(struct.pack("<q", x))
    rt = RopeTable(inv_freq)
    rt.ensure(positions)
    for p in range(positions):
        for c, s in zip(rt.rows_c[p], rt.rows_s[p]):
            h.update(struct.pack("<qq", c, s))
    h.update(struct.pack("<q", softmax_l(hd)))
    return h.hexdigest()
