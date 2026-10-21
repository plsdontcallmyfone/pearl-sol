//! cadre-glue-v1 (docs/SPEC.md 4.7): the deterministic glue between a Cadre model's int7
//! GEMMs, written independently of the Python reference (tools/cadre/glue.py). Given a model
//! directory and a request's token rows, [`forward_round`] rebuilds every segment's A exactly as
//! an honest server must have computed it, hands each A to a [`GemmSink`] (which compares it to
//! the posted activation blob and returns `C = A B`) and returns the greedy token of every row.
//!
//! Arithmetic contract (same as the Python reference, bit for bit):
//! - integers (i64, i128 for the table algorithms) wherever a value crosses a reduction;
//! - binary64 for elementwise scaling, one IEEE operation at a time in the written order
//!   (Rust never contracts `a * b + c` into an FMA);
//! - exp / cos / sin only from the integer tables below.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde_json::Value;

pub mod fraud;
pub mod replay;
pub mod tokenizer;

pub const GLUE_ID: &str = "cadre-glue-v1";
pub const QMAX: i64 = 64;
pub const H_FRAC: i32 = 16;
pub const H_MAX: f64 = 4503599627370496.0; // 2^52
pub const H_MAX_I: i64 = 1 << 52;
pub const QK_FRAC: i32 = 12;
pub const QK_LIM: f64 = 4194303.0; // 2^22 - 1
pub const V_FRAC: i32 = 16;
pub const V_LIM: f64 = 16777216.0; // 2^24
pub const P_FRAC: u32 = 20;
pub const A_PRE_MAX: f64 = 1099511627776.0; // 2^40
pub const T_MAX: usize = 8192;
pub const X_MAX: i64 = 1 << 34;
pub const SIG_X_MAX: f64 = 1073741824.0; // 2^30
pub const LN2_Q60: i128 = 799144290325165978;
pub const LOG2E_Q62: i128 = 6653256548922161245;
pub const LOG2E_Q24: i64 = (LOG2E_Q62 >> 38) as i64;
const ONE60: i128 = 1 << 60;

// ------------------------------------------------------------------ integer tables

/// exp(-x) for 0 <= x <= 1, x in Q60: Taylor to the first zero term, floor steps.
pub fn exp_neg_q60(x: i128) -> i128 {
    let (mut term, mut s, mut k) = (ONE60, ONE60, 0i128);
    loop {
        k += 1;
        term = ((term * x) >> 60) / k;
        if term == 0 {
            return s;
        }
        s = if k & 1 == 1 { s - term } else { s + term };
    }
}

/// E2[i] = 2^(-i/4096) in Q30, i = 0..=4096.
pub fn exp2_table() -> Vec<i64> {
    (0..=4096i128).map(|i| ((exp_neg_q60(i * LN2_Q60 / 4096) + (1 << 29)) >> 30) as i64).collect()
}

/// (cos x, sin x) for 0 <= x <= 1, Q60: Taylor to the first zero term, floor steps.
pub fn cos_sin_q60(x: i128) -> (i128, i128) {
    let (mut c, mut s) = (ONE60, x);
    let (mut term, mut k) = (ONE60, 0i128);
    loop {
        term = (((term * x) >> 60) * x >> 60) / ((k + 1) * (k + 2));
        k += 2;
        if term == 0 {
            break;
        }
        c = if (k / 2) & 1 == 1 { c - term } else { c + term };
    }
    let (mut term, mut k) = (x, 1i128);
    loop {
        term = (((term * x) >> 60) * x >> 60) / ((k + 1) * (k + 2));
        k += 2;
        if term == 0 {
            break;
        }
        s = if (k / 2) & 1 == 1 { s - term } else { s + term };
    }
    (c, s)
}

/// An f32 in [0, 1] as an exact Q60 integer.
pub fn f32_to_q60(v: f32) -> Result<i128> {
    ensure!((0.0..=1.0).contains(&v), "inv_freq {v} outside [0, 1]");
    if v == 0.0 {
        return Ok(0);
    }
    let bits = v.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32;
    let (mant, e2) = if exp == 0 { ((bits & 0x7f_ffff) as i128, -149) } else { (((bits & 0x7f_ffff) | 0x80_0000) as i128, exp - 150) };
    let sh = e2 + 60;
    ensure!(sh >= 0 || mant & ((1 << -sh) - 1) == 0, "inv_freq {v} is not exact in Q60");
    Ok(if sh >= 0 { mant << sh } else { mant >> -sh })
}

/// cos / sin of p * theta_i in Q30 by the rotation recurrence from Taylor seeds.
pub struct RopeTable {
    seeds: Vec<(i128, i128)>,
    state: Vec<(i128, i128)>,
    pub cos: Vec<Vec<i64>>,
    pub sin: Vec<Vec<i64>>,
}

impl RopeTable {
    pub fn new(inv_freq: &[f32]) -> Result<Self> {
        let seeds = inv_freq.iter().map(|&v| f32_to_q60(v).map(cos_sin_q60)).collect::<Result<Vec<_>>>()?;
        let state = vec![(ONE60, 0); seeds.len()];
        Ok(Self { seeds, state, cos: vec![], sin: vec![] })
    }

    pub fn ensure(&mut self, n: usize) {
        while self.cos.len() < n {
            self.cos.push(self.state.iter().map(|(c, _)| ((c + (1 << 29)) >> 30) as i64).collect());
            self.sin.push(self.state.iter().map(|(_, s)| ((s + (1 << 29)) >> 30) as i64).collect());
            self.state = self.state.iter().zip(&self.seeds).map(|(&(c, s), &(c1, s1))| ((c * c1 - s * s1) >> 60, (s * c1 + c * s1) >> 60)).collect();
        }
    }
}

/// Q24 of log2(e) / sqrt(hd).
pub fn softmax_l(hd: u64) -> i64 {
    let r = isqrt((hd as u128) << 40) as i128;
    (((LOG2E_Q62 << 20) / r) >> 38) as i64
}

fn isqrt(v: u128) -> u128 {
    if v < 2 {
        return v;
    }
    let mut x = (v as f64).sqrt() as u128;
    while x * x > v {
        x -= 1;
    }
    while (x + 1) * (x + 1) <= v {
        x += 1;
    }
    x
}

// ------------------------------------------------------------------ binary64 helpers

#[inline]
pub fn rne_clamp(x: f64, lim: f64) -> i64 {
    x.clamp(-lim, lim).round_ties_even() as i64
}

#[inline]
fn pow2(e: i32) -> f64 {
    f64::from_bits(((1023 + e) as u64) << 52)
}

// ------------------------------------------------------------------ model

pub struct Shard {
    pub index: usize,
    pub klo: usize,
    pub k: usize,
    pub n: usize,
    /// B^T (n x k), the registered weight file.
    pub bt: Vec<i8>,
}

pub struct ColShard {
    pub lo: usize,
    pub n: usize,
    pub ks: Vec<Shard>,
    pub colsum: Vec<i64>,
    pub sw: Vec<f64>,
}

pub struct Group {
    pub g: Vec<f64>,
    pub k_total: usize,
    pub cols: Vec<ColShard>,
}

pub struct Model {
    pub dir: PathBuf,
    pub hidden: usize,
    pub layers: usize,
    pub heads: usize,
    pub kv_heads: usize,
    pub head_dim: usize,
    pub vocab: usize,
    pub eps: f64,
    pub period: usize,
    pub matrices: usize,
    pub model_id: String,
    pub eos: Vec<u32>,
    embed: Vec<u16>,
    groups: BTreeMap<String, Group>,
    e2: Vec<i64>,
    l_soft: i64,
    inv_freq: Vec<f32>,
    norm_bits: i64,
}

fn read_f32(p: &Path) -> Result<Vec<f32>> {
    let b = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
    Ok(b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
}

impl Model {
    /// Load a model directory (manifest.json, w/, t/). Reads every weight file (B^T).
    pub fn load(dir: &Path) -> Result<Self> {
        let man: Value = serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).context("read manifest.json")?)?;
        let a = &man["arch"];
        ensure!(a["type"] == "llama", "cadre-glue-v1 covers the llama architecture");
        let u = |k: &str| a[k].as_u64().map(|x| x as usize).ok_or_else(|| anyhow!("arch.{k} missing"));
        let (hidden, layers, heads, kv_heads, head_dim, vocab) = (u("hidden")?, u("layers")?, u("heads")?, u("kv_heads")?, u("head_dim")?, u("vocab")?);
        let eps = a["rms_norm_eps"].as_f64().ok_or_else(|| anyhow!("arch.rms_norm_eps missing"))?;
        let tensors: BTreeMap<String, Value> = man["tensors"].as_array().ok_or_else(|| anyhow!("no tensors"))?.iter().map(|t| (t["name"].as_str().unwrap_or("").to_string(), t.clone())).collect();
        let tfile = |name: &str| -> Result<PathBuf> { Ok(dir.join(tensors.get(name).and_then(|t| t["file"].as_str()).ok_or_else(|| anyhow!("tensor {name} missing"))?)) };
        let rope = tensors.get("rope_inv_freq").ok_or_else(|| anyhow!("rope_inv_freq missing"))?;
        ensure!(rope["attention_scaling"].as_f64().unwrap_or(1.0) == 1.0, "cadre-glue-v1 needs attention_scaling 1");
        let inv_freq = read_f32(&tfile("rope_inv_freq")?)?;
        let eb = std::fs::read(tfile("embed")?)?;
        let embed: Vec<u16> = eb.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        ensure!(embed.len() == vocab * hidden, "embed has {} values, expected vocab x hidden", embed.len());
        let mut norms = BTreeMap::new();
        for name in tensors.keys().filter(|n| n.starts_with("norm.")) {
            norms.insert(name.clone(), read_f32(&tfile(name)?)?.into_iter().map(|x| x as f64).collect::<Vec<f64>>());
        }
        let mats = man["matrices"].as_array().ok_or_else(|| anyhow!("no matrices"))?;
        ensure!(man["unmined"].as_array().map(|x| x.is_empty()).unwrap_or(true), "unmined layers are not covered");
        // base -> cols lo -> shards
        let mut raw: BTreeMap<String, BTreeMap<(usize, usize), Vec<(Shard, pearl_job::standing::Aux)>>> = BTreeMap::new();
        for (pos, m) in mats.iter().enumerate() {
            let index = m["index"].as_u64().ok_or_else(|| anyhow!("matrix index"))? as usize;
            ensure!(index == pos, "matrix indices are not in order");
            let name = m["name"].as_str().unwrap_or("");
            let base = name.split('[').next().unwrap().split('{').next().unwrap().to_string();
            let (k, n) = (m["k"].as_u64().unwrap_or(0) as usize, m["n"].as_u64().unwrap_or(0) as usize);
            let cols = m["cols"].as_array().map(|c| (c[0].as_u64().unwrap_or(0) as usize, c[1].as_u64().unwrap_or(0) as usize)).unwrap_or((0, n));
            let klo = m["k_range"].as_array().map(|c| c[0].as_u64().unwrap_or(0) as usize).unwrap_or(0);
            let bt: Vec<i8> = std::fs::read(dir.join(m["weights"].as_str().unwrap_or(""))).with_context(|| format!("read weights of {name}"))?.into_iter().map(|x| x as i8).collect();
            ensure!(bt.len() == k * n, "{name}: weight file is not n x k");
            let aux = pearl_job::standing::Aux::decode(&std::fs::read(dir.join(m["aux"].as_str().unwrap_or("")))?)?;
            raw.entry(base).or_default().entry(cols).or_default().push((Shard { index, klo, k, n, bt }, aux));
        }
        let mut groups = BTreeMap::new();
        for (base, cols) in raw {
            let norm = if base == "lm_head" {
                Some(norms.get("norm.final").ok_or_else(|| anyhow!("norm.final missing"))?.clone())
            } else {
                let p: Vec<&str> = base.split('.').collect();
                match p.get(2).copied() {
                    Some("qkv") => Some(norms[&format!("norm.{}.attn", p[1])].clone()),
                    Some("gate") | Some("up") => Some(norms[&format!("norm.{}.mlp", p[1])].clone()),
                    _ => None,
                }
            };
            let mut g: Option<Vec<f64>> = None;
            let mut out = Vec::new();
            let mut k_total = 0;
            for ((lo, hi), mut ks) in cols {
                ks.sort_by_key(|(s, _)| s.klo);
                k_total = ks.iter().map(|(s, _)| s.k).sum();
                let smooth: Option<Vec<f64>> = if ks[0].1.smooth.is_some() {
                    Some(ks.iter().flat_map(|(_, a)| a.smooth.clone().unwrap_or_default()).map(|x| x as f64).collect())
                } else {
                    None
                };
                let n = hi - lo;
                let mut colsum = vec![0i64; n];
                for (_, a) in &ks {
                    for (c, x) in colsum.iter_mut().zip(&a.colsum) {
                        *c += *x as i64;
                    }
                }
                let sw: Vec<f64> = ks[0].1.s_w.iter().map(|&x| x as f64).collect();
                let w = norm.clone().unwrap_or_else(|| vec![1.0; k_total]);
                let gg: Vec<f64> = match &smooth {
                    Some(s) => w.iter().zip(s).map(|(a, b)| a / b).collect(),
                    None => w,
                };
                if let Some(prev) = &g {
                    ensure!(prev.iter().zip(&gg).all(|(a, b)| a.to_bits() == b.to_bits()), "{base}: shards disagree on the input factor");
                }
                g = Some(gg);
                out.push(ColShard { lo, n, ks: ks.into_iter().map(|(s, _)| s).collect(), colsum, sw });
            }
            groups.insert(base, Group { g: g.unwrap(), k_total, cols: out });
        }
        let mut eos: Vec<u32> = vec![];
        let mut add = |v: &Value| match v {
            Value::Number(n) => eos.push(n.as_u64().unwrap_or(0) as u32),
            Value::Array(a) => eos.extend(a.iter().filter_map(|x| x.as_u64()).map(|x| x as u32)),
            _ => {}
        };
        add(&a["eos_token_id"]);
        if let Ok(b) = std::fs::read(dir.join("generation_config.json")) {
            if let Ok(gc) = serde_json::from_slice::<Value>(&b) {
                add(&gc["eos_token_id"]);
            }
        }
        eos.sort();
        eos.dedup();
        let period = man["rows_pattern"].as_array().map(|x| x.len()).unwrap_or(4).max(1);
        let norm_bits = (62 - (hidden as f64).log2().ceil() as i64) / 2;
        Ok(Self {
            dir: dir.to_path_buf(),
            hidden,
            layers,
            heads,
            kv_heads,
            head_dim,
            vocab,
            eps,
            period,
            matrices: mats.len(),
            model_id: man["model_id"].as_str().unwrap_or("").to_string(),
            eos,
            embed,
            groups,
            e2: exp2_table(),
            l_soft: softmax_l(head_dim as u64),
            inv_freq,
            norm_bits,
        })
    }

    /// (k, n) of matrix `index`.
    pub fn shape(&self, index: usize) -> Option<(usize, usize)> {
        self.groups.values().flat_map(|g| g.cols.iter().flat_map(|c| c.ks.iter())).find(|s| s.index == index).map(|s| (s.k, s.n))
    }

    pub fn bt(&self, index: usize) -> Option<&[i8]> {
        self.groups.values().flat_map(|g| g.cols.iter().flat_map(|c| c.ks.iter())).find(|s| s.index == index).map(|s| s.bt.as_slice())
    }

    /// sha256 over the integer tables (E2, then cos/sin per position and frequency, then the
    /// softmax constant, all i64 LE): both implementations must print the same digest.
    pub fn table_digest(&self, positions: usize) -> String {
        let mut rt = RopeTable::new(&self.inv_freq).expect("checked at load");
        rt.ensure(positions);
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        for x in &self.e2 {
            h.update(x.to_le_bytes());
        }
        for p in 0..positions {
            for (c, s) in rt.cos[p].iter().zip(&rt.sin[p]) {
                h.update(c.to_le_bytes());
                h.update(s.to_le_bytes());
            }
        }
        h.update(self.l_soft.to_le_bytes());
        hex::encode(h.finalize())
    }
}

/// Who multiplies: gets every segment's A as rebuilt by the glue (m x k, the round's rows plus
/// zero padding, exactly the activation blob's matrix) and returns `C = A B` (m x n).
pub trait GemmSink {
    fn gemm(&mut self, index: usize, a: &[i8], m: usize, k: usize, n: usize) -> Result<Vec<i32>>;
}

/// Threaded exact int8 GEMM against B^T.
pub fn gemm_bt(a: &[i8], m: usize, k: usize, bt: &[i8], n: usize) -> Vec<i32> {
    let mut c = vec![0i32; m * n];
    let threads = std::thread::available_parallelism().map(|x| x.get()).unwrap_or(1).min(n.max(1));
    let chunk = n.div_ceil(threads);
    let parts: Vec<(usize, Vec<i32>)> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..threads)
            .map(|t| {
                sc.spawn(move || {
                    let (j0, j1) = (t * chunk, ((t + 1) * chunk).min(n));
                    let w = j1.saturating_sub(j0);
                    let mut out = vec![0i32; m * w];
                    for j in j0..j1 {
                        let b = &bt[j * k..(j + 1) * k];
                        for i in 0..m {
                            let ar = &a[i * k..(i + 1) * k];
                            let mut s = 0i32;
                            for (x, y) in ar.iter().zip(b) {
                                s += *x as i32 * *y as i32;
                            }
                            out[i * w + (j - j0)] = s;
                        }
                    }
                    (j0, out)
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (j0, out) in parts {
        let w = if m == 0 { 0 } else { out.len() / m };
        if w == 0 {
            continue; // a thread whose column range starts past n
        }
        for i in 0..m {
            c[i * n + j0..i * n + j0 + w].copy_from_slice(&out[i * w..(i + 1) * w]);
        }
    }
    c
}

// ------------------------------------------------------------------ forward

struct Quant {
    v: Vec<i8>,
    s: Vec<f64>,
    z: Vec<i64>,
}

fn quant(u: &[f64], rows: usize, k: usize) -> Quant {
    let mut v = vec![0i8; rows * k];
    let mut s = vec![0f64; rows];
    let mut z = vec![0i64; rows];
    for i in 0..rows {
        let r = &u[i * k..(i + 1) * k];
        let mut mn = r[0];
        let mut mx = r[0];
        for &x in r {
            if x < mn {
                mn = x;
            }
            if x > mx {
                mx = x;
            }
        }
        let d = mx - mn;
        let si = if d == 0.0 { 1.0 } else { d * (1.0 / 128.0) };
        let zi = rne_clamp(-64.0 - mn / si, A_PRE_MAX);
        for (o, &x) in v[i * k..(i + 1) * k].iter_mut().zip(r) {
            *o = (rne_clamp(x / si, A_PRE_MAX) + zi).clamp(-QMAX, QMAX) as i8;
        }
        s[i] = si;
        z[i] = zi;
    }
    Quant { v, s, z }
}

impl Model {
    fn exp2neg(&self, t: i64) -> i64 {
        let n = t >> 24;
        let f = t & ((1 << 24) - 1);
        let idx = ((f + (1 << 11)) >> 12) as usize;
        self.e2[idx] >> n.min(62)
    }

    fn inv_rms(&self, h: &[i64], rows: usize) -> Vec<f64> {
        let n = self.hidden;
        (0..rows)
            .map(|i| {
                let r = &h[i * n..(i + 1) * n];
                let amax = r.iter().map(|x| x.unsigned_abs()).max().unwrap_or(0);
                let bits = 64 - amax.leading_zeros() as i64;
                let shift = (bits - self.norm_bits).max(0);
                let ss: i64 = r.iter().map(|x| {
                    let y = x >> shift;
                    y * y
                }).sum();
                let ms = (ss as f64 * pow2((2 * shift - 2 * H_FRAC as i64) as i32)) / n as f64;
                1.0 / (ms + self.eps).sqrt()
            })
            .collect()
    }

    /// One linear group. `on_lm_head`: return (max logit, index) per row instead of y.
    #[allow(clippy::too_many_arguments)]
    fn lin(&self, base: &str, u: &[f64], sa_scale: &[f64], rows: usize, m: usize, sink: &mut dyn GemmSink, argmax: Option<&mut Vec<(f64, u32)>>) -> Result<Vec<f64>> {
        let g = self.groups.get(base).ok_or_else(|| anyhow!("no matrix group {base}"))?;
        let k_total = g.k_total;
        let q = quant(u, rows, k_total);
        let n_total: usize = g.cols.iter().map(|c| c.n).sum();
        let want_y = argmax.is_none();
        let mut y = if want_y { vec![0f64; rows * n_total] } else { vec![] };
        let mut am = argmax;
        let mut off = 0;
        for cs in &g.cols {
            let mut c = vec![0i64; rows * cs.n];
            for sh in &cs.ks {
                let mut a = vec![0i8; m * sh.k];
                for i in 0..rows {
                    a[i * sh.k..(i + 1) * sh.k].copy_from_slice(&q.v[i * k_total + sh.klo..i * k_total + sh.klo + sh.k]);
                }
                let part = sink.gemm(sh.index, &a, m, sh.k, sh.n)?;
                ensure!(part.len() == m * sh.n, "C of matrix {} has {} values, expected m x n", sh.index, part.len());
                for (o, p) in c.iter_mut().zip(&part[..rows * sh.n]) {
                    *o += *p as i64;
                }
            }
            for i in 0..rows {
                let sa = q.s[i] * sa_scale[i];
                for j in 0..cs.n {
                    let cz = c[i * cs.n + j] - q.z[i] * cs.colsum[j];
                    let val = (cz as f64 * cs.sw[j]) * sa;
                    if want_y {
                        y[i * n_total + off + j] = val;
                    } else if let Some(am) = am.as_deref_mut() {
                        let e = &mut am[i];
                        if val > e.0 {
                            *e = (val, (cs.lo + j) as u32);
                        }
                    }
                }
            }
            off += cs.n;
        }
        Ok(y)
    }

    fn silu(&self, g: f64) -> f64 {
        let x = rne_clamp(g.abs() * pow2(24), SIG_X_MAX);
        let e = self.exp2neg((x * LOG2E_Q24) >> 24);
        let den = (1i64 << 30) + e;
        let sig = if g >= 0.0 { (1i64 << 60).div_euclid(den) } else { (e << 30).div_euclid(den) };
        g * (sig as f64 * pow2(-30))
    }

    /// Attention of one request (rows `r0..r0+t`, positions 0..t) for one layer.
    fn attention(&self, q: &[i64], kk: &[i64], vv: &[i64], r0: usize, t: usize, out: &mut [i64]) {
        let (h_n, kv_n, hd) = (self.heads, self.kv_heads, self.head_dim);
        let grp = h_n / kv_n;
        let qw = h_n * hd;
        let kw = kv_n * hd;
        let threads = std::thread::available_parallelism().map(|x| x.get()).unwrap_or(1).min(h_n);
        let per = h_n.div_ceil(threads);
        let res: Vec<Vec<(usize, Vec<i64>)>> = std::thread::scope(|sc| {
            let hs: Vec<_> = (0..threads)
                .map(|th| {
                    sc.spawn(move || {
                        let mut v = Vec::new();
                        for h in th * per..((th + 1) * per).min(h_n) {
                            let kvh = h / grp;
                            let mut o = vec![0i64; t * hd];
                            let mut s = vec![0i64; t];
                            let mut p = vec![0i64; t];
                            for i in 0..t {
                                let qi = &q[(r0 + i) * qw + h * hd..(r0 + i) * qw + (h + 1) * hd];
                                let mut mx = i64::MIN;
                                for j in 0..=i {
                                    let kj = &kk[(r0 + j) * kw + kvh * hd..(r0 + j) * kw + (kvh + 1) * hd];
                                    let d: i64 = qi.iter().zip(kj).map(|(a, b)| a * b).sum();
                                    s[j] = d;
                                    mx = mx.max(d);
                                }
                                let mut den = 0i64;
                                for j in 0..=i {
                                    let x = (mx - s[j]).clamp(0, X_MAX);
                                    let tt = (x * self.l_soft) >> 24;
                                    p[j] = self.exp2neg(tt) >> (30 - P_FRAC);
                                    den += p[j];
                                }
                                for d in 0..hd {
                                    let mut num = 0i64;
                                    for j in 0..=i {
                                        num += p[j] * vv[(r0 + j) * kw + kvh * hd + d];
                                    }
                                    o[i * hd + d] = (2 * num + den).div_euclid(2 * den);
                                }
                            }
                            v.push((h, o));
                        }
                        v
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (h, o) in res.into_iter().flatten() {
            for i in 0..t {
                out[(r0 + i) * qw + h * hd..(r0 + i) * qw + (h + 1) * hd].copy_from_slice(&o[i * hd..(i + 1) * hd]);
            }
        }
    }

    fn add_res(h: &mut [i64], y: &[f64]) {
        for (a, b) in h.iter_mut().zip(y) {
            *a = (*a + rne_clamp(b * pow2(H_FRAC), H_MAX)).clamp(-H_MAX_I, H_MAX_I);
        }
    }
}

/// Replay one round: `requests[r]` are the token rows of request r (prompt, then every output
/// token but the last), stacked in order; `m` is the round's padded row count. Every segment's
/// A goes through `sink` in matrix-index order. Returns the greedy token after every row.
pub fn forward_round(model: &Model, requests: &[Vec<u32>], m: usize, sink: &mut dyn GemmSink) -> Result<Vec<Vec<u32>>> {
    let rows: usize = requests.iter().map(|r| r.len()).sum();
    ensure!(rows <= m && rows > 0, "round of {rows} rows does not fit m = {m}");
    ensure!(requests.iter().all(|r| !r.is_empty() && r.len() <= T_MAX), "a request has 0 or more than {T_MAX} rows");
    let (hn, kvn, hd, n) = (model.heads, model.kv_heads, model.head_dim, model.hidden);
    let maxt = requests.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut rope = RopeTable::new(&model.inv_freq)?;
    rope.ensure(maxt);
    // positions of every stacked row and request boundaries
    let mut pos = Vec::with_capacity(rows);
    let mut starts = Vec::new();
    for r in requests {
        starts.push(pos.len());
        pos.extend(0..r.len());
    }
    let mut h = vec![0i64; rows * n];
    let mut i = 0;
    for r in requests {
        for &t in r {
            ensure!((t as usize) < model.vocab, "token {t} outside the vocabulary");
            for d in 0..n {
                let b = model.embed[t as usize * n + d];
                let x = f32::from_bits((b as u32) << 16) as f64;
                h[i * n + d] = rne_clamp(x * pow2(H_FRAC), H_MAX);
            }
            i += 1;
        }
    }
    let p16 = pow2(-H_FRAC);
    let ones = vec![p16; rows];
    let unit = vec![1.0; rows];
    let mut q = vec![0i64; rows * hn * hd];
    let mut kk = vec![0i64; rows * kvn * hd];
    let mut vv = vec![0i64; rows * kvn * hd];
    let mut o = vec![0i64; rows * hn * hd];
    for li in 0..model.layers {
        let inv = model.inv_rms(&h, rows);
        let sa: Vec<f64> = inv.iter().map(|x| x * p16).collect();
        let g = &model.groups[&format!("layers.{li}.qkv")].g;
        let u: Vec<f64> = (0..rows * n).map(|x| (h[x] as f64 * g[x % n]) + 0.0).collect();
        let qkv = model.lin(&format!("layers.{li}.qkv"), &u, &sa, rows, m, sink, None)?;
        let w = (hn + 2 * kvn) * hd;
        let half = hd / 2;
        for r in 0..rows {
            let (c, s) = (&rope.cos[pos[r]], &rope.sin[pos[r]]);
            let rot = |src: &[f64], dst: &mut [i64]| {
                for f in 0..half {
                    let (x1, x2) = (src[f], src[f + half]);
                    let (cf, sf) = (c[f] as f64 * pow2(-30), s[f] as f64 * pow2(-30));
                    dst[f] = rne_clamp(((x1 * cf) - (x2 * sf)) * pow2(QK_FRAC), QK_LIM);
                    dst[f + half] = rne_clamp(((x2 * cf) + (x1 * sf)) * pow2(QK_FRAC), QK_LIM);
                }
            };
            let row = &qkv[r * w..(r + 1) * w];
            for hh in 0..hn {
                rot(&row[hh * hd..(hh + 1) * hd], &mut q[(r * hn + hh) * hd..(r * hn + hh + 1) * hd]);
            }
            for hh in 0..kvn {
                rot(&row[(hn + hh) * hd..(hn + hh + 1) * hd], &mut kk[(r * kvn + hh) * hd..(r * kvn + hh + 1) * hd]);
                for d in 0..hd {
                    vv[(r * kvn + hh) * hd + d] = rne_clamp(row[(hn + kvn + hh) * hd + d] * pow2(V_FRAC), V_LIM);
                }
            }
        }
        for (ri, r) in requests.iter().enumerate() {
            model.attention(&q, &kk, &vv, starts[ri], r.len(), &mut o);
        }
        let go = &model.groups[&format!("layers.{li}.o")].g;
        let u: Vec<f64> = (0..rows * n).map(|x| (o[x] as f64 * go[x % n]) + 0.0).collect();
        let y = model.lin(&format!("layers.{li}.o"), &u, &ones, rows, m, sink, None)?;
        Model::add_res(&mut h, &y);
        let inv = model.inv_rms(&h, rows);
        let sa: Vec<f64> = inv.iter().map(|x| x * p16).collect();
        let gm = &model.groups[&format!("layers.{li}.gate")].g;
        let u: Vec<f64> = (0..rows * n).map(|x| (h[x] as f64 * gm[x % n]) + 0.0).collect();
        let gate = model.lin(&format!("layers.{li}.gate"), &u, &sa, rows, m, sink, None)?;
        let up = model.lin(&format!("layers.{li}.up"), &u, &sa, rows, m, sink, None)?;
        let gd = &model.groups[&format!("layers.{li}.down")];
        let kd = gd.k_total;
        let u: Vec<f64> = (0..rows * kd).map(|x| ((model.silu(gate[x]) * up[x]) * gd.g[x % kd]) + 0.0).collect();
        let y = model.lin(&format!("layers.{li}.down"), &u, &unit, rows, m, sink, None)?;
        Model::add_res(&mut h, &y);
    }
    let inv = model.inv_rms(&h, rows);
    let sa: Vec<f64> = inv.iter().map(|x| x * p16).collect();
    let gf = &model.groups["lm_head"].g;
    let u: Vec<f64> = (0..rows * n).map(|x| (h[x] as f64 * gf[x % n]) + 0.0).collect();
    let mut am = vec![(f64::NEG_INFINITY, u32::MAX); rows];
    model.lin("lm_head", &u, &sa, rows, m, sink, Some(&mut am))?;
    let mut out = Vec::new();
    let mut i = 0;
    for r in requests {
        out.push(am[i..i + r.len()].iter().map(|x| x.1).collect());
        i += r.len();
    }
    if am.iter().any(|x| x.1 == u32::MAX) {
        bail!("lm_head produced no finite logit");
    }
    Ok(out)
}
