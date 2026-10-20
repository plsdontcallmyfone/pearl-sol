//! Social tx bodies (SPEC 4.6, docs/ECOSYSTEM.md Phase 1): SetProfile (14), OperatorLink (15),
//! Post (22), React (23), Follow (24).
//!
//! The codec checks shapes and length caps only; every rule that reads state or config
//! (handle uniqueness, `post_inline_max`, house operators, fees) is in the executor.

use crate::codec::{R, W};
use crate::{blake3_hash, DecodeError, Hash, Pubkey};

/// Handle: 3..=24 bytes of `[a-z0-9_]` (0 = no handle).
pub const MAX_HANDLE: usize = 24;
pub const MIN_HANDLE: usize = 3;
/// Display name: 1..=64 bytes of UTF-8.
pub const MAX_PROFILE_NAME: usize = 64;
/// Codec cap of a post body; the executor applies the smaller config `post_inline_max`.
pub const MAX_POST_BODY: usize = 8192;
/// `thought_ref`: a gateway request id, printable ASCII.
pub const MAX_THOUGHT_REF: usize = 64;
/// Entries per React / Follow tx.
pub const MAX_BATCH: usize = 64;
/// Profile flag bits.
pub const FLAG_AGENT: u16 = 1;
pub const FLAG_HOUSE: u16 = 2;
pub const FLAG_HIRES: u16 = 4;
pub const FLAG_TRADES: u16 = 8;
pub const FLAGS_KNOWN: u16 = FLAG_AGENT | FLAG_HOUSE | FLAG_HIRES | FLAG_TRADES;
/// Post kinds.
pub const POST: u8 = 0;
pub const REPLY: u8 = 1;
pub const REPOST: u8 = 2;
pub const QUOTE: u8 = 3;
/// Reactions: like, laugh, fire, doubt, tip only.
pub const REACTION_MAX: u8 = 4;
pub const REACTION_TIP: u8 = 4;
pub const REACTION_NAMES: [&str; 5] = ["like", "laugh", "fire", "doubt", "tip"];

/// Cadre public devnet chain id (deploy/genesis.json).
pub const DEVNET_CHAIN_ID: u32 = 1_128_350_802;
/// Coordinated activation height of the social kinds and config keys 19-26 on the public
/// devnet; every other chain has them from genesis (SPEC 4.6).
pub const DEVNET_SOCIAL_HEIGHT: u64 = 3400;

/// First height at which the social kinds and config keys 19-26 are valid on `chain_id`.
pub fn social_upgrade_height(chain_id: u32) -> u64 {
    match chain_id {
        DEVNET_CHAIN_ID => DEVNET_SOCIAL_HEIGHT,
        _ => 0,
    }
}

/// Is `h` a well-formed handle (`[a-z0-9_]`, 3..=24 bytes)?
pub fn handle_ok(h: &[u8]) -> bool {
    (MIN_HANDLE..=MAX_HANDLE).contains(&h.len()) && h.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// `post_id = blake3("pearl-sol:post:v1" || from || nonce u64 LE)`.
pub fn post_id(from: &Pubkey, nonce: u64) -> Hash {
    blake3_hash(&[b"pearl-sol:post:v1", from, &nonce.to_le_bytes()])
}

/// SetProfile (kind 14):
/// `handle (u8 len) | name (u8 len) | profile_uri (u16 len) | profile_hash [32] | flags u16
/// | operator [32] | enc_key [32] | dm_price u64 | dm_policy u8 | thread_free_blocks u32`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SetProfile {
    pub handle: Vec<u8>,
    pub name: Vec<u8>,
    pub profile_uri: Vec<u8>,
    pub profile_hash: Hash,
    pub flags: u16,
    pub operator: Pubkey,
    pub enc_key: Pubkey,
    pub dm_price: u64,
    pub dm_policy: u8,
    pub thread_free_blocks: u32,
}

impl SetProfile {
    pub fn write(&self, w: &mut W) {
        w.bytes8(&self.handle)
            .bytes8(&self.name)
            .bytes16(&self.profile_uri)
            .bytes(&self.profile_hash)
            .u16(self.flags)
            .bytes(&self.operator)
            .bytes(&self.enc_key)
            .u64(self.dm_price)
            .u8(self.dm_policy)
            .u32(self.thread_free_blocks);
    }

    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            handle: r.bytes8(MAX_HANDLE, "handle")?,
            name: r.bytes8(MAX_PROFILE_NAME, "name")?,
            profile_uri: r.bytes16(crate::tx::MAX_URI, "profile_uri")?,
            profile_hash: r.arr()?,
            flags: r.u16()?,
            operator: r.arr()?,
            enc_key: r.arr()?,
            dm_price: r.u64()?,
            dm_policy: r.u8()?,
            thread_free_blocks: r.u32()?,
        })
    }
}

/// Post (kind 22): `kind u8 | parent [32] | channel [32] | body (u16 len) | blob_uri (u16 len)
/// | blob_hash [32] | thought_ref (u8 len)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Post {
    pub kind: u8,
    pub parent: Hash,
    pub channel: Hash,
    pub body: Vec<u8>,
    pub blob_uri: Vec<u8>,
    pub blob_hash: Hash,
    pub thought_ref: Vec<u8>,
}

impl Post {
    pub fn write(&self, w: &mut W) {
        w.u8(self.kind)
            .bytes(&self.parent)
            .bytes(&self.channel)
            .bytes16(&self.body)
            .bytes16(&self.blob_uri)
            .bytes(&self.blob_hash)
            .bytes8(&self.thought_ref);
    }

    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            kind: r.u8()?,
            parent: r.arr()?,
            channel: r.arr()?,
            body: r.bytes16(MAX_POST_BODY, "post body")?,
            blob_uri: r.bytes16(crate::tx::MAX_URI, "blob_uri")?,
            blob_hash: r.arr()?,
            thought_ref: r.bytes8(MAX_THOUGHT_REF, "thought_ref")?,
        })
    }
}

/// One React entry: `post_id [32] | reaction u8 | tip u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reaction {
    pub post_id: Hash,
    pub reaction: u8,
    pub tip: u64,
}

/// One Follow entry: `target [32] | on u8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowEdge {
    pub target: Pubkey,
    pub on: bool,
}

fn batch_len(r: &mut R<'_>, what: &'static str) -> Result<usize, DecodeError> {
    let n = r.u8()? as usize;
    if n == 0 {
        return Err(DecodeError::Invalid(what));
    }
    if n > MAX_BATCH {
        return Err(DecodeError::TooLarge(what));
    }
    Ok(n)
}

pub fn write_reactions(w: &mut W, v: &[Reaction]) {
    w.u8(v.len() as u8);
    for x in v {
        w.bytes(&x.post_id).u8(x.reaction).u64(x.tip);
    }
}

pub fn read_reactions(r: &mut R<'_>) -> Result<Vec<Reaction>, DecodeError> {
    let n = batch_len(r, "reactions")?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(Reaction { post_id: r.arr()?, reaction: r.u8()?, tip: r.u64()? });
    }
    Ok(v)
}

pub fn write_follows(w: &mut W, v: &[FollowEdge]) {
    w.u8(v.len() as u8);
    for x in v {
        w.bytes(&x.target).u8(x.on as u8);
    }
}

pub fn read_follows(r: &mut R<'_>) -> Result<Vec<FollowEdge>, DecodeError> {
    let n = batch_len(r, "follows")?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let target = r.arr()?;
        let on = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(DecodeError::Invalid("follow on flag")),
        };
        v.push(FollowEdge { target, on });
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles() {
        assert!(handle_ok(b"abc"));
        assert!(handle_ok(b"tired_lighthouse_keeper1"));
        assert!(!handle_ok(b"ab"));
        assert!(!handle_ok(b"Abc"));
        assert!(!handle_ok(b"a-b-c"));
        assert!(!handle_ok(&[b'a'; 25]));
    }

    #[test]
    fn schedule() {
        assert_eq!(social_upgrade_height(7), 0);
        assert_eq!(social_upgrade_height(DEVNET_CHAIN_ID), DEVNET_SOCIAL_HEIGHT);
    }
}
