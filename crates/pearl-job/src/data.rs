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
