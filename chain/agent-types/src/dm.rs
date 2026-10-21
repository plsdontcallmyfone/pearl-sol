//! DM and channel tx bodies (SPEC 4.10, docs/ECOSYSTEM.md Phase 2): SendMessage (16),
//! ReclaimMessage (17), SetConsent (18), ChannelConfig (19), ChannelMember (20),
//! ChannelKey (21).
//!
//! The codec checks shapes and length caps only. Every rule that reads state or config
//! (consent, `dm_policy`, `dm_price`, escrow windows, membership, epochs, fees) is in the
//! executor (chain/executor/src/dm.rs). The sealed formats below are checked for framing
//! only; the chain never sees a key or a plaintext.
//!
//! Sealed DM body (mode 1, `to` set), version 1:
//! ```text
//! 0x01 | count u8 (1..=2) | envelope*count
//! envelope = eph_pub [32] | nonce [24] | ct_len u16 | ct (plaintext + 16-byte tag)
//! ```
//! Envelope 0 is sealed to the recipient's `enc_key`; envelope 1 (optional) to the sender's
//! own, so its history opens on another device. Sealed channel body (mode 1, `channel`
//! set), version 2: `0x02 | epoch u32 | nonce [24] | ct` under the channel's group key of
//! that epoch. Key derivation and AAD are in SPEC 4.10 and the vectors in vectors/dm.json.

use crate::codec::{R, W};
use crate::{blake3_hash, DecodeError, Hash, Pubkey};

/// Codec cap of an inline message body; the executor applies the smaller `msg_inline_max`.
pub const MAX_MSG_BODY: usize = 8192;
/// Message modes.
pub const MODE_PLAIN: u8 = 0;
pub const MODE_SEALED: u8 = 1;
/// Sealed body versions (first byte).
pub const SEALED_DM_V1: u8 = 1;
pub const SEALED_CHANNEL_V2: u8 = 2;
/// X25519 public key, XChaCha20 nonce and Poly1305 tag sizes.
pub const EPH_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;
/// Smallest sealed DM body: version, count, one envelope with an empty plaintext.
pub const MIN_SEALED_DM: usize = 2 + EPH_LEN + NONCE_LEN + 2 + TAG_LEN;
/// Smallest sealed channel body: version, epoch, nonce, tag.
pub const MIN_SEALED_CHANNEL: usize = 1 + 4 + NONCE_LEN + TAG_LEN;
/// A ChannelKey envelope: `eph_pub [32] | nonce [24] | ct [48]` (a 32-byte group key + tag).
pub const KEY_ENVELOPE_LEN: usize = EPH_LEN + NONCE_LEN + 32 + TAG_LEN;
/// Envelopes per ChannelKey tx (100 x 136 bytes stays under MAX_TX_SIZE).
pub const MAX_KEY_ENVELOPES: usize = 100;
/// Entries per SetConsent tx.
pub const MAX_CONSENT_BATCH: usize = 64;
/// Channel name 1..=64 bytes of UTF-8; about text 0..=1024 bytes of UTF-8.
pub const MAX_CHANNEL_NAME: usize = 64;
pub const MAX_CHANNEL_ABOUT: usize = 1024;
/// Codec cap of a channel's admin list; the executor applies `channel_max_admins`.
pub const MAX_CHANNEL_ADMINS: usize = 32;
/// Channel policies.
pub const CHANNEL_OPEN: u8 = 0;
pub const CHANNEL_INVITE: u8 = 1;
pub const CHANNEL_PAID: u8 = 2;
/// ChannelMember actions.
pub const MEMBER_JOIN: u8 = 0;
pub const MEMBER_LEAVE: u8 = 1;
pub const MEMBER_INVITE: u8 = 2;
pub const MEMBER_REMOVE: u8 = 3;
pub const MEMBER_ACTION_NAMES: [&str; 4] = ["join", "leave", "invite", "remove"];
/// Consent states (SetConsent); 0 clears the record.
pub const CONSENT_CLEAR: u8 = 0;
pub const CONSENT_ALLOW: u8 = 1;
pub const CONSENT_DENY: u8 = 2;
/// dm_policy values (SetProfile, SPEC 4.6).
pub const POLICY_OPEN: u8 = 0;
pub const POLICY_PRICED: u8 = 1;
pub const POLICY_FOLLOWED: u8 = 2;
pub const POLICY_CLOSED: u8 = 3;

/// Coordinated activation height of the DM kinds and config keys 35-44 on the Cadre public
/// devnet (tip was 4621 at 2026-10-21 19:35 UTC; 60 s blocks). Every other chain has them from
/// genesis.
pub const DEVNET_DM_HEIGHT: u64 = 4800;

/// First height at which the DM kinds and config keys 35-44 are valid on `chain_id`.
pub fn dm_upgrade_height(chain_id: u32) -> u64 {
    match chain_id {
        crate::social::DEVNET_CHAIN_ID => DEVNET_DM_HEIGHT,
        _ => 0,
    }
}

/// `message_id = blake3("pearl-sol:msg:v1" || from || nonce u64 LE)`.
pub fn message_id(from: &Pubkey, nonce: u64) -> Hash {
    blake3_hash(&[b"pearl-sol:msg:v1", from, &nonce.to_le_bytes()])
}

/// `channel_id = blake3("pearl-sol:channel:v1" || creator || nonce u64 LE)`.
pub fn channel_id(creator: &Pubkey, nonce: u64) -> Hash {
    blake3_hash(&[b"pearl-sol:channel:v1", creator, &nonce.to_le_bytes()])
}

/// SendMessage (kind 16):
/// `to [32] | channel [32] | mode u8 | reply_to [32] | pay u64 | reclaim_after u32
/// | body (u16 len) | blob_uri (u16 len) | blob_hash [32] | thought_ref (u8 len)`. Exactly one of
/// `to` / `channel` is non-zero (checked by the executor). `thought_ref` is the gateway request id
/// whose completion is the (plain) body, as for posts (SPEC 4.6), or empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SendMessage {
    pub to: Pubkey,
    pub channel: Hash,
    pub mode: u8,
    pub reply_to: Hash,
    pub pay: u64,
    pub reclaim_after: u32,
    pub body: Vec<u8>,
    pub blob_uri: Vec<u8>,
    pub blob_hash: Hash,
    pub thought_ref: Vec<u8>,
}

impl SendMessage {
    pub fn write(&self, w: &mut W) {
        w.bytes(&self.to)
            .bytes(&self.channel)
            .u8(self.mode)
            .bytes(&self.reply_to)
            .u64(self.pay)
            .u32(self.reclaim_after)
            .bytes16(&self.body)
            .bytes16(&self.blob_uri)
            .bytes(&self.blob_hash)
            .bytes8(&self.thought_ref);
    }

    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            to: r.arr()?,
            channel: r.arr()?,
            mode: r.u8()?,
            reply_to: r.arr()?,
            pay: r.u64()?,
            reclaim_after: r.u32()?,
            body: r.bytes16(MAX_MSG_BODY, "message body")?,
            blob_uri: r.bytes16(crate::tx::MAX_URI, "blob_uri")?,
            blob_hash: r.arr()?,
            thought_ref: r.bytes8(crate::social::MAX_THOUGHT_REF, "thought_ref")?,
        })
    }
}

/// One SetConsent entry: `peer [32] | state u8 (0 clear, 1 allow, 2 deny)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consent {
    pub peer: Pubkey,
    pub state: u8,
}

pub fn write_consents(w: &mut W, v: &[Consent]) {
    w.u8(v.len() as u8);
    for x in v {
        w.bytes(&x.peer).u8(x.state);
    }
}

pub fn read_consents(r: &mut R<'_>) -> Result<Vec<Consent>, DecodeError> {
    let n = r.u8()? as usize;
    if n == 0 {
        return Err(DecodeError::Invalid("consents"));
    }
    if n > MAX_CONSENT_BATCH {
        return Err(DecodeError::TooLarge("consents"));
    }
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let peer = r.arr()?;
        let state = r.u8()?;
        if state > CONSENT_DENY {
            return Err(DecodeError::Invalid("consent state"));
        }
        v.push(Consent { peer, state });
    }
    Ok(v)
}

/// ChannelConfig (kind 19): create (`channel_id` zero) or update.
/// `channel_id [32] | name (u8 len) | about (u16 len) | policy u8 | join_price u64 | mode u8
/// | admins (count u8, pubkey*)`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChannelConfig {
    pub channel_id: Hash,
    pub name: Vec<u8>,
    pub about: Vec<u8>,
    pub policy: u8,
    pub join_price: u64,
    pub mode: u8,
    pub admins: Vec<Pubkey>,
}

impl ChannelConfig {
    pub fn write(&self, w: &mut W) {
        w.bytes(&self.channel_id)
            .bytes8(&self.name)
            .bytes16(&self.about)
            .u8(self.policy)
            .u64(self.join_price)
            .u8(self.mode)
            .u8(self.admins.len() as u8);
        for a in &self.admins {
            w.bytes(a);
        }
    }

    pub fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        let channel_id = r.arr()?;
        let name = r.bytes8(MAX_CHANNEL_NAME, "channel name")?;
        let about = r.bytes16(MAX_CHANNEL_ABOUT, "channel about")?;
        let policy = r.u8()?;
        let join_price = r.u64()?;
        let mode = r.u8()?;
        let n = r.u8()? as usize;
        if n > MAX_CHANNEL_ADMINS {
            return Err(DecodeError::TooLarge("channel admins"));
        }
        let mut admins = Vec::with_capacity(n);
        for _ in 0..n {
            admins.push(r.arr()?);
        }
        Ok(Self { channel_id, name, about, policy, join_price, mode, admins })
    }
}

/// One ChannelKey envelope: the epoch's group key sealed to `member`'s enc_key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEnvelope {
    pub member: Pubkey,
    pub envelope: [u8; KEY_ENVELOPE_LEN],
}

pub fn write_envelopes(w: &mut W, v: &[KeyEnvelope]) {
    w.u8(v.len() as u8);
    for x in v {
        w.bytes(&x.member).bytes(&x.envelope);
    }
}

pub fn read_envelopes(r: &mut R<'_>) -> Result<Vec<KeyEnvelope>, DecodeError> {
    let n = r.u8()? as usize;
    if n == 0 {
        return Err(DecodeError::Invalid("key envelopes"));
    }
    if n > MAX_KEY_ENVELOPES {
        return Err(DecodeError::TooLarge("key envelopes"));
    }
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(KeyEnvelope { member: r.arr()?, envelope: r.arr()? });
    }
    Ok(v)
}

/// Framing check of a sealed DM body (version 1): every envelope's length adds up and the
/// body ends after the last one. Returns the envelope count.
pub fn check_sealed_dm(b: &[u8]) -> Result<usize, &'static str> {
    if b.len() < MIN_SEALED_DM {
        return Err("sealed body too short");
    }
    if b[0] != SEALED_DM_V1 {
        return Err("sealed DM body must start with version 1");
    }
    let count = b[1] as usize;
    if !(1..=2).contains(&count) {
        return Err("sealed DM body carries 1 or 2 envelopes");
    }
    let mut pos = 2;
    for _ in 0..count {
        if b.len() < pos + EPH_LEN + NONCE_LEN + 2 {
            return Err("sealed envelope truncated");
        }
        pos += EPH_LEN + NONCE_LEN;
        let ct_len = u16::from_le_bytes([b[pos], b[pos + 1]]) as usize;
        pos += 2;
        if ct_len < TAG_LEN {
            return Err("sealed ciphertext shorter than its tag");
        }
        if b.len() < pos + ct_len {
            return Err("sealed ciphertext truncated");
        }
        pos += ct_len;
    }
    if pos != b.len() {
        return Err("bytes after the last sealed envelope");
    }
    Ok(count)
}

/// Framing check of a sealed channel body (version 2). Returns the epoch it names.
pub fn check_sealed_channel(b: &[u8]) -> Result<u32, &'static str> {
    if b.len() < MIN_SEALED_CHANNEL {
        return Err("sealed channel body too short");
    }
    if b[0] != SEALED_CHANNEL_V2 {
        return Err("sealed channel body must start with version 2");
    }
    Ok(u32::from_le_bytes([b[1], b[2], b[3], b[4]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_and_ids() {
        assert_eq!(dm_upgrade_height(7), 0);
        assert_eq!(dm_upgrade_height(crate::social::DEVNET_CHAIN_ID), DEVNET_DM_HEIGHT);
        assert_ne!(message_id(&[1; 32], 0), message_id(&[1; 32], 1));
        assert_ne!(message_id(&[1; 32], 0), channel_id(&[1; 32], 0));
    }

    #[test]
    fn sealed_framing() {
        let mut b = vec![1u8, 1];
        b.extend([7u8; 56]);
        b.extend((20u16).to_le_bytes());
        b.extend([9u8; 20]);
        assert_eq!(check_sealed_dm(&b), Ok(1));
        let mut two = b.clone();
        two[1] = 2;
        two.extend([7u8; 56]);
        two.extend((16u16).to_le_bytes());
        two.extend([9u8; 16]);
        assert_eq!(check_sealed_dm(&two), Ok(2));
        let mut extra = b.clone();
        extra.push(0);
        assert!(check_sealed_dm(&extra).is_err());
        let mut short = b.clone();
        short.pop();
        assert!(check_sealed_dm(&short).is_err());
        let mut v0 = b.clone();
        v0[0] = 2;
        assert!(check_sealed_dm(&v0).is_err());
        let mut c = vec![2u8];
        c.extend(3u32.to_le_bytes());
        c.extend([0u8; 40]);
        assert_eq!(check_sealed_channel(&c), Ok(3));
        assert!(check_sealed_channel(&c[..20]).is_err());
    }
}
