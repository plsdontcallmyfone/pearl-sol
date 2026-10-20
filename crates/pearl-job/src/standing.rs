//! Standing jobs on registered model weights (SPEC 4.5, Cadre inference).
//!
//! A model's weight matrices are registered once (`RegisterModel`); a paid inference
//! ticket's segments are jobs whose B is a registered matrix and whose A is the
//! activations the server computed. Everything here is chain-agnostic bytes and hashes
//! shared by the node, miners, the attester, the gateway and the CLI:
//! - ids: [`model_id`], [`matrix_terms_hash`], [`matrix_job_header`], [`segment_id`]
//! - the ticket form of a PostJob config ([`ticket_config`], [`parse_ticket_config`])
//! - blob formats: activations `PSA1` ([`encode_activation_blob`]), weight aux `LQA1`
//!   ([`Aux`]), the model manifest ([`Manifest`]) and its chunked files.

use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::job::{job_key, Hash256, JobHeader, MiningConfig};
use crate::merkle::blake3_digest;

/// Magic of a ticket config (first 8 of the 52 config bytes).
pub const TICKET_MAGIC: &[u8; 8] = b"CADRE-T1";
/// Recipe 1: int7 W7A7, SmoothQuant folded, per-token asymmetric activations.
pub const RECIPE_W7A7_SQ_ASYM: u8 = 1;
pub const RECIPE_NAME: &str = "cadre-w7a7-sq-asym-v1";
/// Most rows (prompt plus generated tokens, padded) one ticket may produce.
pub const MAX_TICKET_ROWS: u32 = 4096;
/// Most matrices one model may register (chain/agent-types `MAX_MODEL_MATRICES`; 384 before the
/// chain's upgrade height, see `max_model_matrices` there). An 8B int7 model registers 640.
pub const MAX_MODEL_MATRICES: usize = 1024;
/// Largest k of a registered matrix: wider weights are registered as k shards (row ranges of
/// B, partial products summed outside the proof), since the Solana light client verifies job
/// blocks only up to this k (a k = 8192 opening does not fit one Solana tx).
pub const MAX_MATRIX_K: u32 = 2048;
/// Largest registered matrix, k x n entries.
pub const MAX_MATRIX_ENTRIES: u64 = 1 << 26;
/// Most segments one PostSegments tx may carry.
pub const MAX_SEGMENTS_PER_TX: usize = 64;
/// Raw chunk size of model files in the blob store.
pub const MODEL_CHUNK_BYTES: usize = 3 << 20;
/// Activation blob magic.
pub const ACTIVATION_MAGIC: &[u8; 4] = b"PSA1";
/// Weight aux file magic.
pub const AUX_MAGIC: &[u8; 4] = b"LQA1";
pub const MANIFEST_FORMAT: &str = "cadre-model-v1";

/// `model_id = blake3("pearl-sol:model:v1" || registrant || name)`.
pub fn model_id(registrant: &[u8; 32], name: &[u8]) -> Hash256 {
    blake3_digest(&[&b"pearl-sol:model:v1"[..], registrant, name].concat(), None)
}

/// `terms_hash = blake3("pearl-sol:matrix-terms:v1" || model_id || index u16 || k u32 || n u32)`.
pub fn matrix_terms_hash(model_id: &Hash256, index: u16, k: u32, n: u32) -> Hash256 {
    let mut v = b"pearl-sol:matrix-terms:v1".to_vec();
    v.extend_from_slice(model_id);
    v.extend_from_slice(&index.to_le_bytes());
    v.extend_from_slice(&k.to_le_bytes());
    v.extend_from_slice(&n.to_le_bytes());
    blake3_digest(&v, None)
}

/// The fixed Pearl job header of registered matrix `index`.
pub fn matrix_job_header(model_id: &Hash256, index: u16, k: u32, n: u32) -> JobHeader {
    JobHeader { job_pubkey: *model_id, terms_hash: matrix_terms_hash(model_id, index, k, n), created_slot: 0, target_bits: 0 }
}

/// Mining config of a registered matrix: the model's rank and patterns with `common_dim = k`.
pub fn matrix_config(base: &MiningConfig, k: u32) -> MiningConfig {
    MiningConfig { common_dim: k, ..*base }
}

/// `job_key` of registered matrix `index` (keys its `hash_b` and every segment's `hash_a`).
pub fn matrix_job_key(model_id: &Hash256, index: u16, k: u32, n: u32, base: &MiningConfig) -> Hash256 {
    job_key(&matrix_job_header(model_id, index, k, n), &matrix_config(base, k))
}

/// `segment_id = blake3("pearl-sol:segment:v1" || ticket_id || matrix u16 || round u16)`.
pub fn segment_id(ticket_id: &Hash256, matrix: u16, round: u16) -> Hash256 {
    blake3_digest(&[&b"pearl-sol:segment:v1"[..], ticket_id, &matrix.to_le_bytes(), &round.to_le_bytes()].concat(), None)
}

/// Ticket config: `"CADRE-T1" || model_id [32] || 0 [12]`.
pub fn ticket_config(model_id: &Hash256) -> [u8; 52] {
    let mut c = [0u8; 52];
    c[..8].copy_from_slice(TICKET_MAGIC);
    c[8..40].copy_from_slice(model_id);
    c
}

/// `Some(model_id)` when `config` is the ticket form, `None` when it is something else.
pub fn parse_ticket_config(config: &[u8]) -> Option<Hash256> {
    if config.len() != 52 || &config[..8] != TICKET_MAGIC || config[40..].iter().any(|&b| b != 0) {
        return None;
    }
    Some(config[8..40].try_into().unwrap())
}

/// Rows a ticket may produce: `round_up(prompt + max_new, period)`.
pub fn ticket_rows(prompt: u32, max_new: u32, period: u32) -> u64 {
    let t = prompt as u64 + max_new as u64;
    t.div_ceil(period.max(1) as u64) * period.max(1) as u64
}

/// MAC-proportional subsidy cap of a segment: `floor(cap_total x m k n / (rows x macs_per_row))`.
pub fn segment_cap(cap_total: u64, m: u32, k: u32, n: u32, rows: u64, macs_per_row: u64) -> u64 {
    let den = rows as u128 * macs_per_row as u128;
    if den == 0 {
        return 0;
    }
    ((cap_total as u128 * m as u128 * k as u128 * n as u128) / den) as u64
}

/// Activation blob `"PSA1" || m u32 || k u32 || A (m x k i8 row-major)`.
pub fn encode_activation_blob(m: u32, k: u32, a: &[i8]) -> Result<Vec<u8>> {
    ensure!(a.len() == m as usize * k as usize, "A has {} entries, expected m*k = {}", a.len(), m as usize * k as usize);
    let mut v = Vec::with_capacity(12 + a.len());
    v.extend_from_slice(ACTIVATION_MAGIC);
    v.extend_from_slice(&m.to_le_bytes());
    v.extend_from_slice(&k.to_le_bytes());
    v.extend_from_slice(crate::job::as_u8(a));
    Ok(v)
}

/// `(m, k, A)` from an activation blob.
pub fn decode_activation_blob(b: &[u8]) -> Result<(u32, u32, Vec<i8>)> {
    ensure!(b.len() >= 12 && &b[..4] == ACTIVATION_MAGIC, "not an activation blob (magic PSA1)");
    let m = u32::from_le_bytes(b[4..8].try_into().unwrap());
    let k = u32::from_le_bytes(b[8..12].try_into().unwrap());
    let len = m as usize * k as usize;
    ensure!(b.len() == 12 + len, "activation blob is {} bytes, expected {}", b.len(), 12 + len);
    Ok((m, k, b[12..].iter().map(|&x| x as i8).collect()))
}

/// Per-matrix values outside the proof (file `w/<index>.aux`):
/// `"LQA1" || n u32 || k u32 || flags u32 (bit 0 smooth) || s_w f32[n] || colsum i32[n] || [s f32[k]]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Aux {
    pub n: u32,
    pub k: u32,
    pub s_w: Vec<f32>,
    pub colsum: Vec<i32>,
    pub smooth: Option<Vec<f32>>,
}

impl Aux {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = AUX_MAGIC.to_vec();
        v.extend_from_slice(&self.n.to_le_bytes());
        v.extend_from_slice(&self.k.to_le_bytes());
        v.extend_from_slice(&(self.smooth.is_some() as u32).to_le_bytes());
        self.s_w.iter().for_each(|x| v.extend_from_slice(&x.to_le_bytes()));
        self.colsum.iter().for_each(|x| v.extend_from_slice(&x.to_le_bytes()));
        if let Some(s) = &self.smooth {
            s.iter().for_each(|x| v.extend_from_slice(&x.to_le_bytes()));
        }
        v
    }

    pub fn decode(b: &[u8]) -> Result<Self> {
        ensure!(b.len() >= 16 && &b[..4] == AUX_MAGIC, "not an aux file (magic LQA1)");
        let u = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let (n, k, flags) = (u(4), u(8), u(12));
        let smooth = flags & 1 == 1;
        let want = 16 + 8 * n as usize + if smooth { 4 * k as usize } else { 0 };
        ensure!(b.len() == want, "aux file is {} bytes, expected {want}", b.len());
        let f32s = |at: usize, len: usize| (0..len).map(|i| f32::from_le_bytes(b[at + 4 * i..at + 4 * i + 4].try_into().unwrap())).collect::<Vec<_>>();
        let at = 16 + 4 * n as usize;
        let colsum = (0..n as usize).map(|i| i32::from_le_bytes(b[at + 4 * i..at + 4 * i + 4].try_into().unwrap())).collect();
        Ok(Self { n, k, s_w: f32s(16, n as usize), colsum, smooth: smooth.then(|| f32s(16 + 8 * n as usize, k as usize)) })
    }
}

/// `colsum[j] = sum_l B[l, j]` from B^T (n x k).
pub fn colsum_bt(bt: &[i8], n: usize, k: usize) -> Vec<i32> {
    (0..n).map(|j| bt[j * k..(j + 1) * k].iter().map(|&x| x as i32).sum()).collect()
}

// ------------------------------------------------------------------ manifest

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestFile {
    pub size: u64,
    /// blake3 of the whole file, hex.
    pub blake3: String,
    /// Chunk blob ids in order (`blob:<hex>`), each at most [`MODEL_CHUNK_BYTES`].
    #[serde(default)]
    pub blobs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestMatrix {
    pub index: u16,
    pub name: String,
    #[serde(default)]
    pub layer: Option<u32>,
    pub role: String,
    pub k: u32,
    pub n: u32,
    /// Output channels `[lo, hi)` of the source weight this matrix holds (column shard).
    pub cols: [u32; 2],
    /// Hex, filled at registration (depends on the registrant through `model_id`).
    #[serde(default)]
    pub hash_b: String,
    /// `w/<index>.i8` (B^T, n x k int8).
    pub weights: String,
    /// `w/<index>.aux`.
    pub aux: String,
    /// Fields this crate does not use (kept on rewrite).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// The model manifest (JSON). `files` maps every path under the model directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub format: String,
    pub name: String,
    #[serde(default)]
    pub base_model: String,
    pub recipe: serde_json::Value,
    /// Model architecture (hidden size, layers, heads, ...), as the worker needs it.
    pub arch: serde_json::Value,
    pub rank: u16,
    /// Pattern lists, e.g. rows [0,1,2,3], cols [0..8].
    pub rows_pattern: Vec<u32>,
    pub cols_pattern: Vec<u32>,
    pub matrices: Vec<ManifestMatrix>,
    /// GEMMs that run unmined (k too small etc.): name and reason.
    #[serde(default)]
    pub unmined: Vec<serde_json::Value>,
    #[serde(default)]
    pub files: std::collections::BTreeMap<String, ManifestFile>,
    /// Filled at registration.
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub registrant: String,
    /// Fields this crate does not use (tensors, macs_per_row, ...), kept on rewrite.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Manifest {
    pub fn load(dir: &Path) -> Result<Self> {
        let p = dir.join("manifest.json");
        serde_json::from_slice(&std::fs::read(&p).with_context(|| format!("read {}", p.display()))?).with_context(|| format!("parse {}", p.display()))
    }

    pub fn base_config(&self) -> Result<MiningConfig> {
        let rows = crate::pattern::PeriodicPattern::from_list(&self.rows_pattern)?;
        let cols = crate::pattern::PeriodicPattern::from_list(&self.cols_pattern)?;
        let k = self.matrices.first().map(|m| m.k).unwrap_or(2048);
        Ok(MiningConfig { common_dim: k, rank: self.rank, rows_pattern: rows, cols_pattern: cols })
    }

    pub fn macs_per_row(&self) -> u64 {
        self.matrices.iter().map(|m| m.k as u64 * m.n as u64).sum()
    }
}

/// `hash_b` from B^T (n x k row-major, the stored layout): `blake3_keyed(pad1024(B^T), job_key)`,
/// equal to `commit_b(job_key, B, k, n)`.
pub fn commit_bt(job_key: &Hash256, bt: &[i8]) -> Hash256 {
    blake3_digest(&crate::merkle::pad_to_chunk_boundary(crate::job::as_u8(bt)), Some(job_key))
}

/// Every file under `dir` (except `manifest.json`), relative path -> size, blake3, chunk
/// blob URIs. Paths use `/`.
pub fn list_files(dir: &Path) -> Result<std::collections::BTreeMap<String, ManifestFile>> {
    fn walk(root: &Path, d: &Path, out: &mut std::collections::BTreeMap<String, ManifestFile>) -> Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(d)?.collect::<std::result::Result<_, _>>()?;
        entries.sort_by_key(|e| e.path());
        for e in entries {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out)?;
                continue;
            }
            let rel = p.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
            if rel == "manifest.json" || rel.ends_with(".part") {
                continue;
            }
            let b = std::fs::read(&p).with_context(|| format!("read {}", p.display()))?;
            let blobs = chunks(&b).map(|c| crate::data::blob_uri(&crate::data::blob_id(c))).collect();
            out.insert(rel, ManifestFile { size: b.len() as u64, blake3: hex::encode(blake3::hash(&b).as_bytes()), blobs });
        }
        Ok(())
    }
    let mut out = std::collections::BTreeMap::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// Split `bytes` into blob-sized chunks.
pub fn chunks(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes.chunks(MODEL_CHUNK_BYTES)
}

/// A model store: one directory per model id (hex) holding the model's files.
#[derive(Debug, Clone)]
pub struct ModelStore {
    pub root: PathBuf,
}

impl ModelStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn dir(&self, model_id: &Hash256) -> PathBuf {
        self.root.join(hex::encode(model_id))
    }

    /// B^T (n x k int8) of matrix `index` of a model in the store, size-checked.
    pub fn load_bt(&self, model_id: &Hash256, index: u16, k: u32, n: u32) -> Result<Vec<i8>> {
        let p = self.dir(model_id).join(format!("w/{index}.i8"));
        let b = std::fs::read(&p).with_context(|| format!("read {}", p.display()))?;
        let want = k as usize * n as usize;
        if b.len() != want {
            bail!("{} has {} bytes, expected n x k = {want}", p.display(), b.len());
        }
        Ok(b.into_iter().map(|x| x as i8).collect())
    }

    pub fn load_aux(&self, model_id: &Hash256, index: u16) -> Result<Aux> {
        let p = self.dir(model_id).join(format!("w/{index}.aux"));
        Aux::decode(&std::fs::read(&p).with_context(|| format!("read {}", p.display()))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_config_roundtrip() {
        let id = [7u8; 32];
        let c = ticket_config(&id);
        assert_eq!(parse_ticket_config(&c), Some(id));
        assert!(MiningConfig::from_bytes(&c).is_err(), "a ticket config is never a mining config");
        assert_eq!(parse_ticket_config(&MiningConfig::localnet_default().to_bytes()), None);
        let mut bad = c;
        bad[51] = 1;
        assert_eq!(parse_ticket_config(&bad), None);
    }

    #[test]
    fn ids_and_caps() {
        let reg = [1u8; 32];
        let a = model_id(&reg, b"m");
        assert_ne!(a, model_id(&reg, b"n"));
        assert_ne!(a, model_id(&[2u8; 32], b"m"));
        assert_ne!(segment_id(&[3; 32], 0, 0), segment_id(&[3; 32], 1, 0));
        assert_ne!(segment_id(&[3; 32], 0, 0), segment_id(&[3; 32], 0, 1));
        assert_ne!(matrix_terms_hash(&a, 0, 2048, 8), matrix_terms_hash(&a, 1, 2048, 8));
        assert_eq!(ticket_rows(5, 6, 4), 12);
        assert_eq!(ticket_rows(4, 4, 4), 8);
        // Two matrices, every row used: the caps sum to at most cap_total.
        let (k1, n1, k2, n2) = (2048u32, 3072u32, 8192u32, 2048u32);
        let macs = k1 as u64 * n1 as u64 + k2 as u64 * n2 as u64;
        let c1 = segment_cap(1_000_003, 12, k1, n1, 12, macs);
        let c2 = segment_cap(1_000_003, 12, k2, n2, 12, macs);
        assert!(c1 + c2 <= 1_000_003 && c1 + c2 >= 1_000_001);
        assert_eq!(segment_cap(100, 4, k1, n1, 12, macs) * 3 / 3, segment_cap(100, 4, k1, n1, 12, macs));
        assert_eq!(segment_cap(100, 4, k1, n1, 0, macs), 0);
    }

    #[test]
    fn blobs_roundtrip() {
        let a: Vec<i8> = (0..4 * 64).map(|i| (i % 129 - 64) as i8).collect();
        let b = encode_activation_blob(4, 64, &a).unwrap();
        assert_eq!(decode_activation_blob(&b).unwrap(), (4, 64, a.clone()));
        assert!(decode_activation_blob(&b[..b.len() - 1]).is_err());
        assert!(encode_activation_blob(4, 63, &a).is_err());
        for smooth in [None, Some(vec![0.5f32; 64])] {
            let x = Aux { n: 4, k: 64, s_w: vec![1.0, 2.0, 3.0, 4.5], colsum: vec![-1, 2, 3, -4], smooth };
            assert_eq!(Aux::decode(&x.encode()).unwrap(), x);
        }
        let bt: Vec<i8> = vec![1, 2, 3, -1, -2, -3];
        assert_eq!(colsum_bt(&bt, 2, 3), vec![6, -6]);
    }

    #[test]
    fn commit_bt_matches_commit_b() {
        let (_, b) = crate::data::random_matrices(1, 16, 2048, Some(3));
        let bt = crate::job::transpose(&b, 2048, 16);
        let key = [5u8; 32];
        assert_eq!(commit_bt(&key, &bt), crate::job::commit_b(&key, &b, 2048, 16));
    }

    #[test]
    fn list_files_chunks_every_file() {
        let d = std::env::temp_dir().join(format!("cadre-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("w")).unwrap();
        std::fs::write(d.join("manifest.json"), b"{}").unwrap();
        std::fs::write(d.join("w/0.i8"), vec![1u8; MODEL_CHUNK_BYTES + 10]).unwrap();
        std::fs::write(d.join("config.json"), b"{}").unwrap();
        let f = list_files(&d).unwrap();
        assert_eq!(f.keys().cloned().collect::<Vec<_>>(), vec!["config.json".to_string(), "w/0.i8".to_string()]);
        assert_eq!(f["w/0.i8"].blobs.len(), 2);
        assert_eq!(f["w/0.i8"].size, MODEL_CHUNK_BYTES as u64 + 10);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn matrix_key_matches_job_key() {
        let base = MiningConfig::localnet_default();
        let id = [9u8; 32];
        let h = matrix_job_header(&id, 3, 8192, 2048);
        assert_eq!(matrix_job_key(&id, 3, 8192, 2048, &base), job_key(&h, &matrix_config(&base, 8192)));
        assert_eq!(h.job_pubkey, id);
    }
}
