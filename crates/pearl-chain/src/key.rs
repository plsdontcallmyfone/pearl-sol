//! Pubkeys, keypair files and PDAs.

use std::path::Path;

use anyhow::{ensure, Context, Result};
use curve25519_dalek::edwards::CompressedEdwardsY;
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Pubkey(pub [u8; 32]);

impl Pubkey {
    pub fn from_b58(s: &str) -> Result<Self> {
        Ok(Self(pearl_job::spec::parse_pubkey(s)?))
    }
    pub fn to_b58(&self) -> String {
        bs58::encode(self.0).into_string()
    }
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for Pubkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_b58())
    }
}
impl std::fmt::Display for Pubkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_b58())
    }
}

pub const SYSTEM_PROGRAM: Pubkey = Pubkey([0; 32]);

pub fn sysvar_instructions() -> Pubkey {
    Pubkey::from_b58("Sysvar1nstructions1111111111111111111111111").unwrap()
}

pub fn ed25519_program() -> Pubkey {
    Pubkey::from_b58("Ed25519SigVerify111111111111111111111111111").unwrap()
}

/// Solana CLI keypair file: JSON array of 64 bytes (secret 32 || public 32).
pub fn read_keypair(path: &Path) -> Result<SigningKey> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read keypair {}", path.display()))?;
    let bytes: Vec<u8> = serde_json::from_str(&raw).with_context(|| format!("parse keypair {}", path.display()))?;
    ensure!(bytes.len() == 64, "keypair {} has {} bytes, expected 64", path.display(), bytes.len());
    let sk = SigningKey::from_bytes(&bytes[..32].try_into().unwrap());
    ensure!(sk.verifying_key().to_bytes()[..] == bytes[32..], "keypair {} public half mismatch", path.display());
    Ok(sk)
}

pub fn pubkey_of(sk: &SigningKey) -> Pubkey {
    Pubkey(sk.verifying_key().to_bytes())
}

fn on_curve(b: &[u8; 32]) -> bool {
    CompressedEdwardsY(*b).decompress().is_some()
}

pub fn create_program_address(seeds: &[&[u8]], program: &Pubkey) -> Option<Pubkey> {
    let mut h = Sha256::new();
    for s in seeds {
        h.update(s);
    }
    h.update(program.0);
    h.update(b"ProgramDerivedAddress");
    let out: [u8; 32] = h.finalize().into();
    if on_curve(&out) {
        None
    } else {
        Some(Pubkey(out))
    }
}

pub fn find_program_address(seeds: &[&[u8]], program: &Pubkey) -> (Pubkey, u8) {
    for bump in (0..=255u8).rev() {
        let b = [bump];
        let mut s: Vec<&[u8]> = seeds.to_vec();
        s.push(&b);
        if let Some(pk) = create_program_address(&s, program) {
            return (pk, bump);
        }
    }
    panic!("no viable bump");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_ids_decode() {
        assert_eq!(sysvar_instructions().to_b58(), "Sysvar1nstructions1111111111111111111111111");
        assert_eq!(SYSTEM_PROGRAM.to_b58(), "11111111111111111111111111111111");
    }

    /// Reference values from `solana find-program-derived-address`.
    #[test]
    fn pda_matches_solana_cli() {
        let mining = Pubkey::from_b58(crate::MINING_PROGRAM).unwrap();
        assert_eq!(find_program_address(&[b"config"], &mining).0.to_b58(), "8PoY9CrAUhu7RzBxcbhhnAshHMekmnPrquvq6FsKZo46");
        assert_eq!(
            find_program_address(&[b"epoch", &0u64.to_le_bytes()], &mining).0.to_b58(),
            "GWxM9UW2H9d2X6koGo6uRdav33QrhBmcs39vcXR9teKT"
        );
    }
}
