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


def sqrt_rn(v):
    """Correctly rounded binary64 sqrt (IEEE 754). torch's CPU float64 sqrt is vectorized through
    SLEEF and is off by one ulp on some inputs (measured 2026-10-21: 5 of 1024 rows of one window
    against CUDA, math.sqrt and Rust); numpy's sqrt is the hardware's sqrtpd, CUDA's is IEEE."""
    if v.device.type == "cpu":
        return torch.from_numpy(np.sqrt(v.numpy()))
    return torch.sqrt(v)


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
        # sum of squares < 2^52 so its int64 -> binary64 conversion is exact on every device (a
        # vectorized CPU conversion of a value above 2^53 may round differently from CUDA's)
        self.norm_bits = (52 - math.ceil(math.log2(n))) // 2
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
        return self.one_t / sqrt_rn(ms + self.eps_t)

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

    def lin(self, base, u, sa_scale, cap, out_rows=None):
        from worker import int_mm
        """y (binary64) = ((C - z colsum) * sw) * (s * sa_scale) through every shard of `base`.
        u: binary64 input rows (already times the group's g); sa_scale: per-row binary64 factor
        that turns u's units into the real input (inv_rms * 2^-16, 2^-16, or 1).
        out_rows: optional int64 row indices; y is then computed for those rows only (C and A
        are still computed and captured for every row: they are the segments). Every output
        element is a function of its own row, so the selected rows are the same bits as rows of
        the full y; forward_multi uses it for lm_head, whose binary64 logits are only needed for
        the last row of each request (128,256 columns per row, the widest matrix by 5x).
        The scale-out runs in place on fresh tensors: the same IEEE operations in the same order
        ((C - z colsum) to binary64, times sw, then times sa, each rounded), fewer allocations."""
        g, shards = self.groups[base]
        v, s, z = self.quant(u)
        sa = s * sa_scale
        if out_rows is not None:
            z, sa = z.index_select(0, out_rows), sa.index_select(0, out_rows)
        ys = []
        for ks, colsum, sw in shards:
            c = None
            for m in ks:
                vs = v[:, m.klo:m.khi].contiguous()
                part = int_mm(vs, m.qw)
                if callable(cap):  # batched forward: the sink splits the rows by request
                    cap(m.index, vs, part)
                elif cap is not None:
                    cap[m.index][0].append(vs)
                    cap[m.index][1].append(part)
                if out_rows is not None:
                    part = part.index_select(0, out_rows)
                c = part.to(torch.int64) if c is None else c.add_(part)  # exact int64 sums
            c.sub_(z * colsum)                       # C - z colsum, exact int64
            yv = c.double()
            yv.mul_(sw)
            yv.mul_(sa)
            ys.append(yv)
            del c
        return ys[0] if len(ys) == 1 else torch.cat(ys, -1)

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
        # one int64 division per element: pick the numerator first (2^60 for g >= 0, e 2^30
        # otherwise); the same quotients as dividing both and picking after (the old form ran
        # every division twice, and int64 division has no vector instruction on x86)
        num = torch.where(g >= 0, torch.full_like(e, 1 << 60), torch.bitwise_left_shift(e, 30))
        sig = torch.div(num, den, rounding_mode="floor")
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
        self.last_h = h  # the final residual stream (tests hash it against the Rust replay)
        inv = self.inv_rms(h)[:, None]
        gf = self.groups["lm_head"][0]
        return self.lin("lm_head", (h.double() * gf) + 0.0, inv * p16, cap)

    # ------------------------------------------------------------- batched forward (SPEC 4.7 "Batching")

    def attention_kv(self, q, kv, li, T, pos):
        """attention() over one request's cache: the same integers, all KV heads in one binary64
        matmul. Every score and every P V partial sum is an exact integer below 2^53, so grouping
        heads (or requests) into one matmul cannot change a bit (SPEC 4.7)."""
        r, H, hd = q.shape
        KV = kv.KV
        g = H // KV
        assert T <= T_MAX
        jpos = torch.arange(T, device=self.dev)
        mask = jpos[None, :] > pos[:, None]  # r x T, True = not visible
        Qd = q.double().reshape(r, KV, g, hd).permute(1, 2, 0, 3)          # KV x g x r x hd
        # .double(): a binary32 CPU cache widened exactly (a no-op on a binary64 cache)
        S = torch.matmul(Qd, kv.K[li][:, None, :, :T].double()).to(torch.int64)    # KV x g x r x T
        return self._softmax_pv(S, mask, kv.Vhi[li][:, None, :T, :].double(), kv.Vlo[li][:, None, :T, :].double()).permute(2, 0, 1, 3).reshape(r, H * hd)

    def _softmax_pv(self, S, mask, vhi, vlo):
        """Integer softmax weights and P V of attention(), for any leading batch shape."""
        mx = S.masked_fill(mask, torch.iinfo(torch.int64).min).amax(-1, keepdim=True)
        x = torch.clamp(mx - S, min=0, max=X_MAX)
        t = torch.bitwise_right_shift(x * self.L, 24)
        P = torch.bitwise_right_shift(self.exp2neg(t), 30 - P_FRAC)
        P = P.masked_fill(mask, 0)
        D = P.sum(-1, keepdim=True)
        Pd = P.double()
        num = torch.matmul(Pd, vhi).to(torch.int64) * 4096 + torch.matmul(Pd, vlo).to(torch.int64)
        return torch.div(2 * num + D, 2 * D, rounding_mode="floor")

    def attention_decode(self, q, K, Vhi, Vlo, pos):
        """One decode row for each of B requests at once: q B x H x hd (Q12), K B x KV x hd x T,
        Vhi / Vlo B x KV x T x hd (each request's cache, zero past its own length, which the mask
        hides: masked weights are exactly 0, so the padding adds exact zeros), pos B positions."""
        B, H, hd = q.shape
        KV, T = K.shape[1], K.shape[3]
        g = H // KV
        assert T <= T_MAX
        jpos = torch.arange(T, device=self.dev)
        mask = (jpos[None, :] > pos[:, None])[:, None, None, None, :]     # B x 1 x 1 x 1 x T
        Qd = q.double().reshape(B, KV, g, 1, hd)
        S = torch.matmul(Qd, K[:, :, None]).to(torch.int64)                # B x KV x g x 1 x T
        o = self._softmax_pv(S, mask, Vhi[:, :, None], Vlo[:, :, None])    # B x KV x g x 1 x hd
        return o.reshape(B, H * hd)

    def rope_rows(self, pos_list):
        """cos / sin rows (binary64, Q30 / 2^30) for arbitrary positions, from a device copy of
        the integer table (the same values RopeTable.get returns)."""
        need = max(pos_list) + 1
        have = 0 if getattr(self, "_rope_c", None) is None else self._rope_c.shape[0]
        if need > have:
            n = max(need, 2 * have, 256)
            c, s_ = self.rope_tab.get(0, n, self.dev)
            self._rope_c, self._rope_s = c, s_
        idx = torch.tensor(pos_list, device=self.dev)
        return self._rope_c.index_select(0, idx), self._rope_s.index_select(0, idx)

    def forward_multi(self, parts, cap=None):
        """One forward over several requests at once (continuous batching). parts: list of
        (ids, kv): new token ids and the request's KV cache (KVCache or a KVPool slot, extended in
        place). cap(matrix_index, A, C) sees every matrix's A and C over ALL rows of the step,
        stacked in parts order (the caller splits them by request). Every per-row step
        (embedding, RMS, quantization, the int32 GEMMs, scales, RoPE, SiLU, residuals) runs on
        all rows stacked; attention reads each request's own cache: decode rows (one new token)
        of pooled requests in one batched attention, other parts one request at a time. Returns
        the last logits row of every part, stacked (binary64), each bit-identical to forward()
        of that request alone."""
        m = self.m
        dev = self.dev
        spans, lo = [], 0
        for ids, _ in parts:
            spans.append((lo, lo + len(ids)))
            lo += len(ids)
        r = lo
        starts = [kv.t for _, kv in parts]
        pos_list = [s + i for s, (ids, _) in zip(starts, parts) for i in range(len(ids))]
        pos = torch.tensor(pos_list, device=dev)
        rc, rs = self.rope_rows(pos_list)
        all_ids = [x for ids, _ in parts for x in ids]
        # decode rows of requests in one pool: one write and one batched attention per layer
        pool = None
        dec = [i for i, (ids, kv) in enumerate(parts) if len(ids) == 1 and isinstance(kv, KVSlot)]
        if len(dec) >= 2 and all(parts[i][1].pool is parts[dec[0]][1].pool for i in dec):
            pool = parts[dec[0]][1].pool
        else:
            dec = []
        dec_set = set(dec)
        if dec:
            d_rows = torch.tensor([spans[i][0] for i in dec], device=dev)
            d_slots = torch.tensor([parts[i][1].slot for i in dec], device=dev)
            d_pos = torch.tensor([starts[i] for i in dec], device=dev)
            d_T = max(starts[i] for i in dec) + 1
        h = self.rne_clamp(m.embed[torch.tensor(all_ids, device=dev)].double() * 2.0 ** H_FRAC, H_MAX)
        H, KV, hd = m.H, m.KV, m.hd
        p16 = 2.0 ** -H_FRAC
        for li in range(m.L):
            inv = self.inv_rms(h)[:, None]
            g = self.groups[f"layers.{li}.qkv"][0]
            qkv = self.lin(f"layers.{li}.qkv", (h.double() * g) + 0.0, inv * p16, cap)
            q = self.rope(qkv[:, : H * hd].reshape(r, H, hd), rc, rs)
            k = self.rope(qkv[:, H * hd: (H + KV) * hd].reshape(r, KV, hd), rc, rs)
            v = self.rne_clamp(qkv[:, (H + KV) * hd:].reshape(r, KV, hd) * 2.0 ** V_FRAC, V_MAX)
            o = torch.empty(r, H * hd, dtype=torch.int64, device=dev)
            if dec:
                pool.write_rows(li, d_slots, d_pos, k.index_select(0, d_rows), v.index_select(0, d_rows))
                Kb, Vh, Vl = pool.gather(li, d_slots, d_T)
                o.index_copy_(0, d_rows, self.attention_decode(q.index_select(0, d_rows), Kb, Vh, Vl, d_pos))
            for i, ((_, kv), (a, b), s) in enumerate(zip(parts, spans, starts)):
                if i in dec_set:
                    continue
                kv.write(li, s, k[a:b], v[a:b])
                o[a:b] = self.attention_kv(q[a:b], kv, li, s + (b - a), pos[a:b])
            go = self.groups[f"layers.{li}.o"][0]
            h = self.add_res(h, self.lin(f"layers.{li}.o", (o.double() * go) + 0.0, p16, cap))
            inv = self.inv_rms(h)[:, None]
            gm = self.groups[f"layers.{li}.gate"][0]
            x = (h.double() * gm) + 0.0
            gte = self.lin(f"layers.{li}.gate", x, inv * p16, cap)
            up = self.lin(f"layers.{li}.up", x, inv * p16, cap)
            gd = self.groups[f"layers.{li}.down"][0]
            h = self.add_res(h, self.lin(f"layers.{li}.down", ((self.silu(gte) * up) * gd) + 0.0, 1.0, cap))
        for (ids, kv), s in zip(parts, starts):
            kv.t = s + len(ids)
        inv = self.inv_rms(h)[:, None]
        gf = self.groups["lm_head"][0]
        return self.lin("lm_head", (h.double() * gf) + 0.0, inv * p16, cap,
                        out_rows=torch.tensor([b - 1 for _, b in spans], device=dev))


def _kv_tensors(L, KV, hd, lead, cap_t, dev, dtype=torch.float64):
    """Zero-filled (finite) K / Vhi / Vlo per layer; zero past a request's length matters: the
    batched decode reads up to the longest request and the mask zeroes those weights, and
    0 x 0 adds an exact 0 (uninitialized memory could hold NaN, and 0 x NaN is NaN)."""
    f = dict(dtype=dtype, device=dev)
    return ([torch.zeros(*lead, KV, hd, cap_t, **f) for _ in range(L)],
            [torch.zeros(*lead, KV, cap_t, hd, **f) for _ in range(L)],
            [torch.zeros(*lead, KV, cap_t, hd, **f) for _ in range(L)])


def _kv_write(K, Vhi, Vlo, li, start, k, v, cap_t):
    n = k.shape[0]
    assert start + n <= cap_t, "KV cache overflow"
    K[li][:, :, start:start + n] = k.permute(1, 2, 0).double()
    Vhi[li][:, start:start + n, :] = torch.bitwise_right_shift(v, 12).permute(1, 0, 2).double()
    Vlo[li][:, start:start + n, :] = torch.bitwise_and(v, 4095).permute(1, 0, 2).double()


class KVCache:
    """One request's attention cache for forward_multi, preallocated for `cap_t` positions:
    K (KV x hd x T), V split into v >> 12 and v & 4095 (KV x T x hd). Every stored value is an
    integer of magnitude below 2^22 (K: Q12 clamped to 2^22 - 1; v >> 12 and v & 4095: below
    2^12), exact in binary32 and in binary64, so this is the same data as the int64 cache of
    forward(). On CPU it is stored as binary32 (half the memory: 98 KB per position on
    Llama-3.2-1B instead of 196 KB) and widened to binary64 (exact) where attention reads it,
    so the score and P V matmuls see the same binary64 integers as before; on CUDA binary64."""

    def __init__(self, glue_, cap_t):
        m = glue_.m
        self.KV, self.hd, self.cap_t, self.t = m.KV, m.hd, cap_t, 0
        dt = torch.float32 if glue_.dev.type == "cpu" else torch.float64
        self.K, self.Vhi, self.Vlo = _kv_tensors(m.L, m.KV, m.hd, (), cap_t, glue_.dev, dt)

    def write(self, li, start, k, v):
        """k: n x KV x hd (Q12 int64), v: n x KV x hd (Q16 int64) for positions start.."""
        _kv_write(self.K, self.Vhi, self.Vlo, li, start, k, v, self.cap_t)

    def nbytes(self):
        return sum(t.numel() * t.element_size() for ts in (self.K, self.Vhi, self.Vlo) for t in ts)


class KVPool:
    """KV caches of up to `slots` requests in one tensor per layer (slot x KV x hd x T), so the
    decode rows of all of them are written with one indexed store and read with one gather per
    layer (the GPU path is launch-bound otherwise). Same values as KVCache."""

    def __init__(self, glue_, slots, cap_t):
        m = glue_.m
        self.KV, self.hd, self.cap_t, self.dev = m.KV, m.hd, cap_t, glue_.dev
        self.K, self.Vhi, self.Vlo = _kv_tensors(m.L, m.KV, m.hd, (slots,), cap_t, glue_.dev)
        self.free = list(range(slots))[::-1]

    def take(self):
        return KVSlot(self, self.free.pop()) if self.free else None

    def give(self, slot):
        # zero the slot again: the batched read relies on finite values past each length
        for ts in (self.K, self.Vhi, self.Vlo):
            for t in ts:
                t[slot.slot].zero_()
        self.free.append(slot.slot)

    def write_rows(self, li, slots, pos, k, v):
        """One new row per slot: k, v B x KV x hd at positions pos (B)."""
        self.K[li][slots, :, :, pos] = k.double()
        self.Vhi[li][slots, :, pos, :] = torch.bitwise_right_shift(v, 12).double()
        self.Vlo[li][slots, :, pos, :] = torch.bitwise_and(v, 4095).double()

    def gather(self, li, slots, T):
        return (self.K[li][..., :T].index_select(0, slots), self.Vhi[li][:, :, :T, :].index_select(0, slots),
                self.Vlo[li][:, :, :T, :].index_select(0, slots))

    def nbytes(self):
        return sum(t.numel() * 8 for ts in (self.K, self.Vhi, self.Vlo) for t in ts)


class KVSlot:
    """A request's view of its pool slot; works wherever a KVCache does."""

    def __init__(self, pool, slot):
        self.pool, self.slot, self.t = pool, slot, 0
        self.KV, self.hd, self.cap_t = pool.KV, pool.hd, pool.cap_t
        self.K = [t[slot] for t in pool.K]
        self.Vhi = [t[slot] for t in pool.Vhi]
        self.Vlo = [t[slot] for t in pool.Vlo]

    def write(self, li, start, k, v):
        _kv_write(self.K, self.Vhi, self.Vlo, li, start, k, v, self.cap_t)


def pick_rows(L):
    """pick() of every row of L at once: the largest logit, lowest index on ties."""
    mx = L.amax(-1, keepdim=True)
    return (L == mx).to(torch.int32).argmax(-1).tolist()


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
