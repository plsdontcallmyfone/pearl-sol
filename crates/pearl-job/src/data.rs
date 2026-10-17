//! Matrix files and the job data manifest (see CLI.md, "Data layout").

use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};

/// `manifest.json` next to `A.i8` and `B.i8`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub m: u32,
    pub n: u32,
    pub k: u32,
    /// Path of A (m x k int8 row-major), relative to the manifest's directory.
    pub a: String,
    /// Path of B (k x n int8 row-major), relative to the manifest's directory.
    pub b: String,
}

pub struct JobData {
    pub manifest: Manifest,
    pub a: Vec<i8>,
    pub b: Vec<i8>,
}

/// Random int7 matrices in [-64, 64]. Deterministic when `seed` is given.
pub fn random_matrices(m: usize, n: usize, k: usize, seed: Option<u64>) -> (Vec<i8>, Vec<i8>) {
    let mut rng: rand::rngs::StdRng = match seed {
        Some(s) => rand::rngs::StdRng::seed_from_u64(s),
        None => rand::rngs::StdRng::from_os_rng(),
    };
    let a = (0..m * k).map(|_| rng.random_range(-64i8..=64)).collect();
    let b = (0..k * n).map(|_| rng.random_range(-64i8..=64)).collect();
    (a, b)
}

pub fn write_i8(path: &Path, v: &[i8]) -> Result<()> {
    std::fs::write(path, crate::job::as_u8(v)).with_context(|| format!("write {}", path.display()))
}

pub fn read_i8(path: &Path, expect_len: usize) -> Result<Vec<i8>> {
    let b = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    ensure!(b.len() == expect_len, "{} has {} bytes, expected {expect_len}", path.display(), b.len());
    Ok(b.into_iter().map(|x| x as i8).collect())
}

pub fn write_i32(path: &Path, v: &[i32]) -> Result<()> {
    std::fs::write(path, crate::freivalds::c_to_bytes(v)).with_context(|| format!("write {}", path.display()))
}

pub fn read_i32(path: &Path, expect_len: usize) -> Result<Vec<i32>> {
    let b = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    ensure!(b.len() == expect_len * 4, "{} has {} bytes, expected {}", path.display(), b.len(), expect_len * 4);
    Ok(b.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect())
}

/// Resolve a data URI to a manifest path. Accepts `file:///abs/dir`,
/// `file:///abs/dir/manifest.json` or a plain filesystem path. Other schemes are M2.
pub fn manifest_path(uri: &str) -> Result<PathBuf> {
    if uri.starts_with(BLOB_URI_PREFIX) {
        bail!("{uri:?} is a blob: URI; fetch it from a node with getBlob (pearl_node::load_job_data)");
    }
    let p = if let Some(rest) = uri.strip_prefix("file://") {
        PathBuf::from(rest)
    } else if uri.contains("://") {
        bail!("unsupported data URI scheme in {uri:?} (M1 supports file:// only)");
    } else {
        PathBuf::from(uri)
    };
    Ok(if p.is_dir() { p.join("manifest.json") } else { p })
}

pub fn load(uri: &str) -> Result<JobData> {
    let mp = manifest_path(uri)?;
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(&mp).with_context(|| format!("read {}", mp.display()))?)
        .with_context(|| format!("parse {}", mp.display()))?;
    let dir = mp.parent().unwrap_or(Path::new("."));
    let (m, n, k) = (manifest.m as usize, manifest.n as usize, manifest.k as usize);
    let a = read_i8(&dir.join(&manifest.a), m * k)?;
    let b = read_i8(&dir.join(&manifest.b), k * n)?;
    Ok(JobData { manifest, a, b })
}

/// Write `A.i8`, `B.i8`, `manifest.json` into `dir`. Returns the manifest path.
pub fn save(dir: &Path, m: u32, n: u32, k: u32, a: &[i8], b: &[i8]) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    write_i8(&dir.join("A.i8"), a)?;
    write_i8(&dir.join("B.i8"), b)?;
    let manifest = Manifest { m, n, k, a: "A.i8".into(), b: "B.i8".into() };
    let mp = dir.join("manifest.json");
    std::fs::write(&mp, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(mp)
}

// ---------------------------------------------------------------- blob data (M2 data availability)

/// URI scheme of the agent chain's content-addressed blob store (docs/RPC.md "Blobs").
pub const BLOB_URI_PREFIX: &str = "blob:";
/// Magic of a job data blob.
pub const JOB_BLOB_MAGIC: &[u8; 4] = b"PSD1";

/// `blob:<64 hex>` -> blob id (`blake3` of the blob bytes).
pub fn parse_blob_uri(uri: &str) -> Option<[u8; 32]> {
    let rest = uri.trim().strip_prefix(BLOB_URI_PREFIX)?;
    if rest.len() != 64 {
        return None;
    }
    hex::decode(rest).ok()?.try_into().ok()
}

pub fn blob_uri(id: &[u8; 32]) -> String {
    format!("{BLOB_URI_PREFIX}{}", hex::encode(id))
}

pub fn blob_id(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// Job data as one blob: `"PSD1" || m u32 || n u32 || k u32 || A (m*k i8, row-major) ||
/// B (k*n i8, row-major)`, integers little-endian.
pub fn encode_job_blob(m: u32, n: u32, k: u32, a: &[i8], b: &[i8]) -> Result<Vec<u8>> {
    ensure!(a.len() == m as usize * k as usize, "A has {} entries, expected m*k", a.len());
    ensure!(b.len() == k as usize * n as usize, "B has {} entries, expected k*n", b.len());
    let mut v = Vec::with_capacity(16 + a.len() + b.len());
    v.extend_from_slice(JOB_BLOB_MAGIC);
    for x in [m, n, k] {
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.extend_from_slice(crate::job::as_u8(a));
    v.extend_from_slice(crate::job::as_u8(b));
    Ok(v)
}

pub fn decode_job_blob(bytes: &[u8]) -> Result<JobData> {
    ensure!(bytes.len() >= 16 && &bytes[..4] == JOB_BLOB_MAGIC, "not a job data blob (magic PSD1)");
    let u = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let (m, n, k) = (u(4), u(8), u(12));
    let (la, lb) = (m as usize * k as usize, k as usize * n as usize);
    ensure!(bytes.len() == 16 + la + lb, "job blob is {} bytes, expected {}", bytes.len(), 16 + la + lb);
    let a = bytes[16..16 + la].iter().map(|&x| x as i8).collect();
    let b = bytes[16 + la..].iter().map(|&x| x as i8).collect();
    Ok(JobData { manifest: Manifest { m, n, k, a: String::new(), b: String::new() }, a, b })
}

/// Result C (m x n i32, row-major, little-endian) from blob bytes.
pub fn decode_result_blob(bytes: &[u8], expect_len: usize) -> Result<Vec<i32>> {
    ensure!(bytes.len() == expect_len * 4, "result blob has {} bytes, expected {}", bytes.len(), expect_len * 4);
    Ok(bytes.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect())
}

#[cfg(test)]
mod blob_tests {
    use super::*;

    #[test]
    fn job_blob_roundtrip() {
        let (a, b) = random_matrices(4, 8, 64, Some(1));
        let v = encode_job_blob(4, 8, 64, &a, &b).unwrap();
        let d = decode_job_blob(&v).unwrap();
        assert_eq!((d.manifest.m, d.manifest.n, d.manifest.k), (4, 8, 64));
        assert_eq!((d.a, d.b), (a, b));
        assert!(decode_job_blob(&v[..v.len() - 1]).is_err());
        let id = blob_id(&v);
        assert_eq!(parse_blob_uri(&blob_uri(&id)), Some(id));
        assert_eq!(parse_blob_uri("blob:12"), None);
        assert!(manifest_path(&blob_uri(&id)).is_err());
    }
}
