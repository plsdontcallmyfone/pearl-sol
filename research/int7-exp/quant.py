"""Fake-quant simulator matching crates/pearl-job's arithmetic.

pearl-job proves C = A B exactly (int32), with A (m x k) and B (k x n) integer
matrices whose entries lie in [-64, 64] (mine.rs PreparedJob::build). Scales are not
part of the job: the poster (or a standing-job registry) picks them, so any scale that
factors out of the integer product is free:
  * per-token (per-row of A) and per-output-channel (per-column of B) scales,
  * an activation zero point (correction z * colsum(B) computed outside the proof),
  * SmoothQuant per-input-channel factors folded into the weights / previous op.
Group-wise scales along k are NOT free (each k-group would be its own job, and every
job needs k >= 16r >= 2048).

A quantized linear here computes y = s_a * s_w * (qa @ qw^T) (+ zero-point fix, bias),
with qa, qw integer-valued fp32 tensors. qa @ qw^T is exactly the C a miner gets from a
job (checked bit-exact against `pearl-job matmul` in verify_pearljob.py).
"""
import torch
import torch.nn as nn

QMAX = {7: 64, 8: 127}  # int7 per pearl-job: [-64, 64]; int8 symmetric: [-127, 127]


def quant_sym(x, bits, dim):
    """Symmetric RTN. dim=None: per-tensor; else absmax over `dim` (kept)."""
    q = QMAX[bits]
    amax = x.abs().amax() if dim is None else x.abs().amax(dim=dim, keepdim=True)
    s = (amax / q).clamp(min=1e-8)
    return torch.clamp(torch.round(x / s), -q, q), s


def quant_sym_mse(w, bits, grid=(1.0, 0.95, 0.9, 0.85, 0.8, 0.75, 0.7, 0.65, 0.6)):
    """Per-output-channel symmetric RTN with the clip ratio (of absmax) that minimizes
    the channel's squared error. Offline weight-only choice: free for a registered B."""
    q = QMAX[bits]
    amax = w.abs().amax(dim=1, keepdim=True).clamp(min=1e-8)
    best_err = None
    for r in grid:
        s = amax * r / q
        v = torch.clamp(torch.round(w / s), -q, q)
        err = ((v * s - w) ** 2).sum(1, keepdim=True)
        if best_err is None:
            best_err, best_v, best_s = err, v, s
        else:
            m = err < best_err
            best_err = torch.where(m, err, best_err)
            best_v = torch.where(m, v, best_v)
            best_s = torch.where(m, s, best_s)
    return best_v, best_s


def quant_asym(x, bits, dim):
    """Asymmetric RTN onto [-q, q] (2q+1 levels): x ~ s * (v - z)."""
    q = QMAX[bits]
    mn = x.amin(dim=dim, keepdim=True)
    mx = x.amax(dim=dim, keepdim=True)
    s = ((mx - mn) / (2 * q)).clamp(min=1e-8)
    z = torch.round(-q - mn / s)
    v = torch.clamp(torch.round(x / s) + z, -q, q)
    return v, s, z


def int_matmul(v, wq_i8):
    """C = v @ wq^T exactly in int32 (the pearl-job product): v, wq integer-valued,
    |v|, |wq| <= 127. Uses torch._int_mm (int8 x int8 -> int32)."""
    m = v.shape[0]
    a = v.to(torch.int8)
    if m < 32:  # _int_mm wants m > 16; pad rows (zero rows do not change C)
        a = torch.cat([a, torch.zeros(32 - m, a.shape[1], dtype=torch.int8)])
    c = torch._int_mm(a, wq_i8.t())
    return c[:m].float()


class QLinear(nn.Module):
    """Linear with optional int weight / activation fake-quant; fp32 compute.

    cfg keys: w_bits (None=keep bf16), w_gran ('channel'|'tensor'),
              a_bits (None=fp32 activations), a_gran ('token'|'tensor'), a_asym (bool)
    smooth: optional per-input-channel factor s (x / s, W * s), SmoothQuant.
    """

    def __init__(self, lin: nn.Linear, cfg, smooth=None, name=""):
        super().__init__()
        self.name = name
        self.in_features, self.out_features = lin.in_features, lin.out_features
        self.cfg = dict(cfg)
        self.bias = None if lin.bias is None else nn.Parameter(lin.bias.detach().float(), requires_grad=False)
        w = lin.weight.detach()
        self.smooth = None
        if smooth is not None:
            self.smooth = smooth.float()
            w = w.float() * self.smooth[None, :]
        wb = cfg.get("w_bits")
        if wb:
            if cfg.get("w_mse"):
                qw, sw = quant_sym_mse(w.float(), wb)
            else:
                qw, sw = quant_sym(w.float(), wb, 1 if cfg.get("w_gran", "channel") == "channel" else None)
            self.register_buffer("qw", qw.to(torch.int8))
            self.register_buffer("sw", sw.float().reshape(-1, 1) if sw.dim() else sw.float().reshape(1, 1))
            self.register_buffer("colsum", qw.sum(dim=1).float())  # sum_k qw[n, k]
            self.w = None
        else:
            # Shares storage with the original (bf16) parameter when no smoothing.
            self.w = w if smooth is None else w.to(torch.bfloat16)

    def forward(self, x, rows=None):
        """rows: optional slice of output features (vocab chunking for lm_head)."""
        if rows is not None:
            return self._forward_rows(x, rows)
        x = x.float()
        if self.smooth is not None:
            x = x / self.smooth
        cfg = self.cfg
        shp = x.shape
        x2 = x.reshape(-1, shp[-1])
        ab = cfg.get("a_bits")
        if self.w is None:
            if ab:
                dim = -1 if cfg.get("a_gran", "token") == "token" else None
                if cfg.get("a_asym"):
                    v, sa, z = quant_asym(x2, ab, -1 if dim is not None else None)
                    c = int_matmul(v, self.qw)  # == pearl-job C for A=v, B=qw^T
                    y = sa * (c - z * self.colsum[None, :]) * self.sw.t()
                else:
                    v, sa = quant_sym(x2, ab, dim)
                    c = int_matmul(v, self.qw)
                    y = (c * sa) * self.sw.t()
            else:
                y = x2 @ (self.qw.float() * self.sw).t()
        else:
            w = self.w.float()
            if ab:
                v, sa = quant_sym(x2, ab, -1 if cfg.get("a_gran", "token") == "token" else None)
                x2 = v * sa
            y = x2 @ w.t()
        if self.bias is not None:
            y = y + self.bias
        return y.reshape(*shp[:-1], self.out_features)


    def _forward_rows(self, x, rows):
        x = x.float()
        if self.smooth is not None:
            x = x / self.smooth
        cfg, ab = self.cfg, self.cfg.get("a_bits")
        if self.w is None:
            qw, sw = self.qw[rows], (self.sw[rows] if self.sw.shape[0] > 1 else self.sw)
            if ab and cfg.get("a_asym"):
                v, sa, z = quant_asym(x, ab, -1)
                y = sa * (int_matmul(v, qw) - z * self.colsum[rows][None, :]) * sw.t()
            elif ab:
                v, sa = quant_sym(x, ab, -1 if cfg.get("a_gran", "token") == "token" else None)
                y = int_matmul(v, qw) * sa * sw.t()
            else:
                y = x @ (qw.float() * sw).t()
        else:
            if ab:
                v, sa = quant_sym(x, ab, -1 if cfg.get("a_gran", "token") == "token" else None)
                x = v * sa
            y = x @ self.w[rows].float().t()
        if self.bias is not None:
            y = y + self.bias[rows]
        return y


def head_logprob(head, h, tgt, chunk=16384):
    """log p(tgt) per row of h through lm_head, chunked over the vocab so the fp32
    weight copy and the logits never exceed chunk rows at once."""
    V = head.out_features
    lse = None
    tl = torch.empty(h.shape[0])
    for lo in range(0, V, chunk):
        hi = min(lo + chunk, V)
        lg = head(h, rows=slice(lo, hi))
        c = torch.logsumexp(lg, -1)
        lse = c if lse is None else torch.logaddexp(lse, c)
        m = (tgt >= lo) & (tgt < hi)
        if m.any():
            idx = m.nonzero()[:, 0]
            tl[idx] = lg[idx, tgt[idx] - lo]
    return tl - lse


class Fp32Embedding(nn.Module):
    def __init__(self, emb):
        super().__init__()
        self.emb = emb

    def forward(self, ids):
        return self.emb(ids).float()
