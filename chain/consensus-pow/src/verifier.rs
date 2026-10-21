//! Proof-of-work verification seam. The node uses [`PearlVerifier`] (crates/pearl-job
//! v2 block proofs); tests that are about fork choice use [`MockVerifier`].

use agent_types::Header;
use pearl_job::pow::{BlockHeader, FallbackParams};

pub trait PowVerifier: Clone + Send + Sync + 'static {
    /// Check `proof` for `header`. `job` is the posted job named by `header.job_id`
    /// (None for fallback blocks). Must check `proof_hash` and the header's target.
    fn verify(&self, header: &Header, job: Option<&pearl_job::Job>, proof: &[u8], fallback: &FallbackParams) -> Result<(), String>;
    /// Like `verify` but against `target_bits` instead of the header's own target (weak
    /// blocks, docs/SPEED.md 2).
    fn verify_at(&self, header: &Header, job: Option<&pearl_job::Job>, proof: &[u8], fallback: &FallbackParams, target_bits: u32) -> Result<(), String>;
}

/// Real Pearl plain-proof verification: `pearl_job::pow::verify_block`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PearlVerifier;

pub fn to_pearl_header(h: &Header) -> BlockHeader {
    BlockHeader::from_bytes(&h.encode()).expect("256 bytes")
}

impl PowVerifier for PearlVerifier {
    fn verify(&self, header: &Header, job: Option<&pearl_job::Job>, proof: &[u8], fallback: &FallbackParams) -> Result<(), String> {
        pearl_job::pow::verify_block(&to_pearl_header(header), job, proof, fallback).map(|_| ()).map_err(|e| e.to_string())
    }
    fn verify_at(&self, header: &Header, job: Option<&pearl_job::Job>, proof: &[u8], fallback: &FallbackParams, target_bits: u32) -> Result<(), String> {
        pearl_job::pow::verify_block_at(&to_pearl_header(header), job, proof, fallback, target_bits).map(|_| ()).map_err(|e| e.to_string())
    }
}

/// Test verifier: the proof must be `b"mock" || pow_digest`. It still binds the proof
/// to the header (so a proof cannot move between headers) but costs no work.
#[derive(Clone, Copy, Debug, Default)]
pub struct MockVerifier;

impl MockVerifier {
    pub fn proof_for(header: &Header) -> Vec<u8> {
        let mut p = b"mock".to_vec();
        p.extend_from_slice(&header.pow_digest());
        p
    }

    /// A proof that meets only the weak target: `b"mockweak" || pow_digest`.
    pub fn weak_proof_for(header: &Header) -> Vec<u8> {
        let mut p = b"mockweak".to_vec();
        p.extend_from_slice(&header.pow_digest());
        p
    }
}

impl PowVerifier for MockVerifier {
    fn verify(&self, header: &Header, _job: Option<&pearl_job::Job>, proof: &[u8], _f: &FallbackParams) -> Result<(), String> {
        if agent_types::block::proof_hash(proof) != header.proof_hash {
            return Err("proof_hash does not match the proof bytes".into());
        }
        if proof != Self::proof_for(header).as_slice() {
            return Err("mock proof does not bind this header".into());
        }
        Ok(())
    }
    fn verify_at(&self, header: &Header, job: Option<&pearl_job::Job>, proof: &[u8], f: &FallbackParams, _target_bits: u32) -> Result<(), String> {
        if proof == Self::weak_proof_for(header).as_slice() {
            if agent_types::block::proof_hash(proof) != header.proof_hash {
                return Err("proof_hash does not match the proof bytes".into());
            }
            return Ok(());
        }
        self.verify(header, job, proof, f)
    }
}
