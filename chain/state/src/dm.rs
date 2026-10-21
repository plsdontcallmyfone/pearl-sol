//! DM and channel records (SPEC 4.10) and the DM parameters (AdminConfig keys 35-44).
//!
//! The DM parameters live in their OWN record, not in the config record, so the config
//! record's encoding (and its social / contracts tiers) never changes because of them.
//! A missing record means every DM parameter holds its default.
//!
//! ```text
//! DmParams      ["dm-params"]                   tag 26 | version u8 = 1 | msg_inline_max u32 | reclaim_min u32
//!               | reclaim_max u32 | max_open_escrows u32 | protocol_bps u16 | sealed_max_members u32
//!               | channel_create_fee u64 | channel_max_admins u8
//! MsgEscrow     ["msg-escrow", message_id]      tag 27 | from [32] | to [32] | pay u64 | height u64 | deadline u64
//! DmLast        ["dm-last", from || to]         tag 28 | height u64 (last DM from -> to)
//! Consent       ["consent", account || peer]    tag 29 | state u8 (1 allow, 2 deny) | height u64
//! DmStats       ["dm-stats", account]           tag 30 | sent | received | pay_in | pay_out | open_escrows
//!               | locked | answered | refunded (u64 each)
//! Channel       ["channel", channel_id]         tag 31 | creator [32] | name (u8 len) | about (u16 len) | policy u8
//!               | join_price u64 | mode u8 | admins (count u8, pubkey*) | members u32 | epoch u32
//!               | epoch_height u64 | rekey_due u8 | messages u64 | created_height u64 | set_height u64 | index u64
//! ChannelMember ["chan-member", channel || a]   tag 32 | height u64
//! ChannelInvite ["chan-invite", channel || a]   tag 33 | height u64
//! ChannelIndex  ["chan-index", i u64 LE]        tag 34 | channel_id [32]
//! ChannelCount  ["chan-count"]                  tag 35 | count u64
//! ```

use agent_types::codec::{R, W};
use agent_types::{DecodeError, Hash, Pubkey};

use crate::schema::{key, tag, Key};

/// AdminConfig keys of the DM parameters (valid from `dm_upgrade_height`).
pub mod keys {
    pub const MSG_INLINE_MAX: u8 = 35; // u32 (1..=8192)
    pub const DM_RECLAIM_MIN: u8 = 36; // u32 blocks (>= 1)
    pub const DM_RECLAIM_MAX: u8 = 37; // u32 blocks (>= reclaim_min, <= 1,000,000)
    pub const DM_MAX_OPEN_ESCROWS: u8 = 38; // u32 (1..=65536), per sender
    pub const DM_PROTOCOL_BPS: u8 = 39; // u16 (0..=10000) of settled DM and join pay, to the pool
    pub const CHANNEL_SEALED_MAX_MEMBERS: u8 = 40; // u32 (2..=10000)
    pub const CHANNEL_CREATE_FEE: u8 = 41; // u64, to the pool, on top of the tx fee
    pub const CHANNEL_MAX_ADMINS: u8 = 42; // u8 (1..=32)
    pub const FIRST: u8 = 35;
    pub const LAST: u8 = 42;
    /// Keys 43 and 44 are reserved for later DM parameters.
    pub const RESERVED_LAST: u8 = 44;

    pub fn name(k: u8) -> &'static str {
        match k {
            MSG_INLINE_MAX => "msg_inline_max",
            DM_RECLAIM_MIN => "dm_reclaim_min",
            DM_RECLAIM_MAX => "dm_reclaim_max",
            DM_MAX_OPEN_ESCROWS => "dm_max_open_escrows",
            DM_PROTOCOL_BPS => "dm_protocol_bps",
            CHANNEL_SEALED_MAX_MEMBERS => "channel_sealed_max_members",
            CHANNEL_CREATE_FEE => "channel_create_fee",
            CHANNEL_MAX_ADMINS => "channel_max_admins",
            _ => "unknown",
        }
    }
}

/// Is `k` an AdminConfig key handled by the DM parameters (35..=44)?
pub fn is_dm_key(k: u8) -> bool {
    (keys::FIRST..=keys::RESERVED_LAST).contains(&k)
}

pub fn dm_params_key() -> Key {
    key(tag::DM_PARAMS, &[])
}
pub fn escrow_key(message_id: &Hash) -> Key {
    key(tag::MSG_ESCROW, message_id)
}
pub fn dm_last_key(from: &Pubkey, to: &Pubkey) -> Key {
    key(tag::DM_LAST, &[&from[..], &to[..]].concat())
}
pub fn consent_key(account: &Pubkey, peer: &Pubkey) -> Key {
    key(tag::CONSENT, &[&account[..], &peer[..]].concat())
}
pub fn dm_stats_key(a: &Pubkey) -> Key {
    key(tag::DM_STATS, a)
}
pub fn channel_key(id: &Hash) -> Key {
    key(tag::CHANNEL, id)
}
pub fn member_key(channel: &Hash, a: &Pubkey) -> Key {
    key(tag::CHANNEL_MEMBER, &[&channel[..], &a[..]].concat())
}
pub fn invite_key(channel: &Hash, a: &Pubkey) -> Key {
    key(tag::CHANNEL_INVITE, &[&channel[..], &a[..]].concat())
}
pub fn channel_index_key(i: u64) -> Key {
    key(tag::CHANNEL_INDEX, &i.to_le_bytes())
}
pub fn channel_count_key() -> Key {
    key(tag::CHANNEL_COUNT, &[])
}

fn expect(r: &mut R<'_>, t: u8) -> Result<(), DecodeError> {
    if r.u8()? != t {
        return Err(DecodeError::Invalid("record tag"));
    }
    Ok(())
}

/// The DM parameters (SPEC 4.10). Defaults are design choices, each editable by the admin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmParams {
    pub msg_inline_max: u32,
    pub reclaim_min: u32,
    pub reclaim_max: u32,
    pub max_open_escrows: u32,
    pub protocol_bps: u16,
    pub sealed_max_members: u32,
    pub channel_create_fee: u64,
    pub channel_max_admins: u8,
}

impl Default for DmParams {
    fn default() -> Self {
        Self {
            msg_inline_max: 1024,
            reclaim_min: 10,
            reclaim_max: 10_080,
            max_open_escrows: 256,
            protocol_bps: 0,
            sealed_max_members: 250,
            channel_create_fee: 0,
            channel_max_admins: 8,
        }
    }
}

impl DmParams {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::DM_PARAMS)
            .u8(1)
            .u32(self.msg_inline_max)
            .u32(self.reclaim_min)
            .u32(self.reclaim_max)
            .u32(self.max_open_escrows)
            .u16(self.protocol_bps)
            .u32(self.sealed_max_members)
            .u64(self.channel_create_fee)
            .u8(self.channel_max_admins);
        w.into_inner()
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::DM_PARAMS)?;
        if r.u8()? != 1 {
            return Err(DecodeError::Invalid("dm params version"));
        }
        let p = Self {
            msg_inline_max: r.u32()?,
            reclaim_min: r.u32()?,
            reclaim_max: r.u32()?,
            max_open_escrows: r.u32()?,
            protocol_bps: r.u16()?,
            sealed_max_members: r.u32()?,
            channel_create_fee: r.u64()?,
            channel_max_admins: r.u8()?,
        };
        r.finish()?;
        Ok(p)
    }

    /// Encoded value of key `k` (AdminConfig form).
    pub fn value(&self, k: u8) -> Vec<u8> {
        use keys::*;
        match k {
            MSG_INLINE_MAX => self.msg_inline_max.to_le_bytes().to_vec(),
            DM_RECLAIM_MIN => self.reclaim_min.to_le_bytes().to_vec(),
            DM_RECLAIM_MAX => self.reclaim_max.to_le_bytes().to_vec(),
            DM_MAX_OPEN_ESCROWS => self.max_open_escrows.to_le_bytes().to_vec(),
            DM_PROTOCOL_BPS => self.protocol_bps.to_le_bytes().to_vec(),
            CHANNEL_SEALED_MAX_MEMBERS => self.sealed_max_members.to_le_bytes().to_vec(),
            CHANNEL_CREATE_FEE => self.channel_create_fee.to_le_bytes().to_vec(),
            CHANNEL_MAX_ADMINS => vec![self.channel_max_admins],
            _ => Vec::new(),
        }
    }

    /// Set key `k` from its AdminConfig value, then check every bound.
    pub fn set(&mut self, k: u8, v: &[u8]) -> Result<(), String> {
        use keys::*;
        fn u32v(v: &[u8]) -> Result<u32, String> {
            Ok(u32::from_le_bytes(v.try_into().map_err(|_| "expected u32".to_string())?))
        }
        match k {
            MSG_INLINE_MAX => self.msg_inline_max = u32v(v)?,
            DM_RECLAIM_MIN => self.reclaim_min = u32v(v)?,
            DM_RECLAIM_MAX => self.reclaim_max = u32v(v)?,
            DM_MAX_OPEN_ESCROWS => self.max_open_escrows = u32v(v)?,
            DM_PROTOCOL_BPS => self.protocol_bps = u16::from_le_bytes(v.try_into().map_err(|_| "expected u16".to_string())?),
            CHANNEL_SEALED_MAX_MEMBERS => self.sealed_max_members = u32v(v)?,
            CHANNEL_CREATE_FEE => self.channel_create_fee = u64::from_le_bytes(v.try_into().map_err(|_| "expected u64".to_string())?),
            CHANNEL_MAX_ADMINS => {
                if v.len() != 1 {
                    return Err("expected u8".into());
                }
                self.channel_max_admins = v[0];
            }
            _ => return Err(format!("unknown config key {k}")),
        }
        self.validate()
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(1..=agent_types::dm::MAX_MSG_BODY as u32).contains(&self.msg_inline_max) {
            return Err(format!("msg_inline_max must be 1..={}", agent_types::dm::MAX_MSG_BODY));
        }
        if self.reclaim_min == 0 || self.reclaim_min > self.reclaim_max || self.reclaim_max > 1_000_000 {
            return Err("dm reclaim window must satisfy 1 <= dm_reclaim_min <= dm_reclaim_max <= 1000000".into());
        }
        if !(1..=65_536).contains(&self.max_open_escrows) {
            return Err("dm_max_open_escrows must be 1..=65536".into());
        }
        if self.protocol_bps > 10_000 {
            return Err("dm_protocol_bps must be 0..=10000".into());
        }
        if !(2..=10_000).contains(&self.sealed_max_members) {
            return Err("channel_sealed_max_members must be 2..=10000".into());
        }
        if !(1..=agent_types::dm::MAX_CHANNEL_ADMINS as u8).contains(&self.channel_max_admins) {
            return Err(format!("channel_max_admins must be 1..={}", agent_types::dm::MAX_CHANNEL_ADMINS));
        }
        Ok(())
    }
}

/// An escrowed message pay: released to `to` by its reply within the window, else
/// reclaimable by `from` after `deadline`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsgEscrow {
    pub from: Pubkey,
    pub to: Pubkey,
    pub pay: u64,
    pub height: u64,
    pub deadline: u64,
}

impl MsgEscrow {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::MSG_ESCROW).bytes(&self.from).bytes(&self.to).u64(self.pay).u64(self.height).u64(self.deadline);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::MSG_ESCROW)?;
        let e = Self { from: r.arr()?, to: r.arr()?, pay: r.u64()?, height: r.u64()?, deadline: r.u64()? };
        r.finish()?;
        Ok(e)
    }
}

/// A consent record (allow or deny); clearing deletes it.
pub fn encode_consent(state: u8, height: u64) -> Vec<u8> {
    let mut w = W::new();
    w.u8(tag::CONSENT).u8(state).u64(height);
    w.into_inner()
}
pub fn decode_consent(b: &[u8]) -> Result<(u8, u64), DecodeError> {
    let mut r = R::new(b);
    expect(&mut r, tag::CONSENT)?;
    let v = (r.u8()?, r.u64()?);
    r.finish()?;
    Ok(v)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DmStats {
    /// DMs and channel messages sent / DMs received.
    pub sent: u64,
    pub received: u64,
    /// Pay settled to this account (released or immediate DM pay, channel join pay), net of
    /// the protocol share, and pay this account settled to others.
    pub pay_in: u64,
    pub pay_out: u64,
    /// Escrows this account has open as a sender, and the amount they hold.
    pub open_escrows: u64,
    pub locked: u64,
    /// Escrows this account released by replying in time; escrows refunded to this account.
    pub answered: u64,
    pub refunded: u64,
}

impl DmStats {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::DM_STATS)
            .u64(self.sent)
            .u64(self.received)
            .u64(self.pay_in)
            .u64(self.pay_out)
            .u64(self.open_escrows)
            .u64(self.locked)
            .u64(self.answered)
            .u64(self.refunded);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::DM_STATS)?;
        let s = Self {
            sent: r.u64()?,
            received: r.u64()?,
            pay_in: r.u64()?,
            pay_out: r.u64()?,
            open_escrows: r.u64()?,
            locked: r.u64()?,
            answered: r.u64()?,
            refunded: r.u64()?,
        };
        r.finish()?;
        Ok(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Channel {
    pub creator: Pubkey,
    pub name: Vec<u8>,
    pub about: Vec<u8>,
    pub policy: u8,
    pub join_price: u64,
    pub mode: u8,
    pub admins: Vec<Pubkey>,
    pub members: u32,
    /// Sealed channels: current key epoch (0 = no key posted yet), the height it started,
    /// and whether a member left or was removed since (messages wait for a new epoch).
    pub epoch: u32,
    pub epoch_height: u64,
    pub rekey_due: bool,
    pub messages: u64,
    pub created_height: u64,
    pub set_height: u64,
    pub index: u64,
}

impl Channel {
    pub fn is_admin(&self, a: &Pubkey) -> bool {
        *a == self.creator || self.admins.contains(a)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::CHANNEL)
            .bytes(&self.creator)
            .bytes8(&self.name)
            .bytes16(&self.about)
            .u8(self.policy)
            .u64(self.join_price)
            .u8(self.mode)
            .u8(self.admins.len() as u8);
        for a in &self.admins {
            w.bytes(a);
        }
        w.u32(self.members)
            .u32(self.epoch)
            .u64(self.epoch_height)
            .u8(self.rekey_due as u8)
            .u64(self.messages)
            .u64(self.created_height)
            .u64(self.set_height)
            .u64(self.index);
        w.into_inner()
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect(&mut r, tag::CHANNEL)?;
        let creator = r.arr()?;
        let name = r.bytes8(255, "channel name")?;
        let about = r.bytes16(usize::MAX, "channel about")?;
        let policy = r.u8()?;
        let join_price = r.u64()?;
        let mode = r.u8()?;
        let n = r.u8()? as usize;
        let mut admins = Vec::with_capacity(n);
        for _ in 0..n {
            admins.push(r.arr()?);
        }
        let c = Self {
            creator,
            name,
            about,
            policy,
            join_price,
            mode,
            admins,
            members: r.u32()?,
            epoch: r.u32()?,
            epoch_height: r.u64()?,
            rekey_due: r.u8()? != 0,
            messages: r.u64()?,
            created_height: r.u64()?,
            set_height: r.u64()?,
            index: r.u64()?,
        };
        r.finish()?;
        Ok(c)
    }
}

/// A height under a tag (DmLast, ChannelMember, ChannelInvite) or a count (ChannelCount).
pub fn encode_u64(t: u8, v: u64) -> Vec<u8> {
    let mut w = W::new();
    w.u8(t).u64(v);
    w.into_inner()
}
pub fn decode_u64(t: u8, b: &[u8]) -> Result<u64, DecodeError> {
    let mut r = R::new(b);
    expect(&mut r, t)?;
    let v = r.u64()?;
    r.finish()?;
    Ok(v)
}
pub fn encode_id(t: u8, id: &Hash) -> Vec<u8> {
    let mut w = W::new();
    w.u8(t).bytes(id);
    w.into_inner()
}
pub fn decode_id(t: u8, b: &[u8]) -> Result<Hash, DecodeError> {
    let mut r = R::new(b);
    expect(&mut r, t)?;
    let v = r.arr()?;
    r.finish()?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_bounds() {
        let p = DmParams::default();
        p.validate().unwrap();
        assert_eq!(DmParams::decode(&p.encode()).unwrap(), p);
        let mut q = p;
        q.set(keys::MSG_INLINE_MAX, &2048u32.to_le_bytes()).unwrap();
        assert_eq!(q.msg_inline_max, 2048);
        assert_eq!(q.value(keys::MSG_INLINE_MAX), 2048u32.to_le_bytes().to_vec());
        assert!(q.clone().set(keys::MSG_INLINE_MAX, &0u32.to_le_bytes()).is_err());
        assert!(q.clone().set(keys::DM_RECLAIM_MIN, &20_000u32.to_le_bytes()).is_err());
        assert!(q.clone().set(keys::DM_PROTOCOL_BPS, &10_001u16.to_le_bytes()).is_err());
        assert!(q.clone().set(keys::CHANNEL_MAX_ADMINS, &[0]).is_err());
        assert!(q.clone().set(43, &[0]).is_err());
        assert!(is_dm_key(35) && is_dm_key(44) && !is_dm_key(34) && !is_dm_key(45));
        let e = MsgEscrow { from: [1; 32], to: [2; 32], pay: 5, height: 6, deadline: 16 };
        assert_eq!(MsgEscrow::decode(&e.encode()).unwrap(), e);
        let s = DmStats { sent: 1, refunded: 3, ..Default::default() };
        assert_eq!(DmStats::decode(&s.encode()).unwrap(), s);
        let c = Channel { creator: [3; 32], name: b"desk".to_vec(), admins: vec![[4; 32]], members: 2, epoch: 1, rekey_due: true, ..Default::default() };
        assert_eq!(Channel::decode(&c.encode()).unwrap(), c);
        assert!(c.is_admin(&[3; 32]) && c.is_admin(&[4; 32]) && !c.is_admin(&[5; 32]));
        assert_eq!(decode_consent(&encode_consent(2, 9)).unwrap(), (2, 9));
        assert_eq!(decode_id(tag::CHANNEL_INDEX, &encode_id(tag::CHANNEL_INDEX, &[8; 32])).unwrap(), [8; 32]);
        assert_ne!(consent_key(&[1; 32], &[2; 32]), consent_key(&[2; 32], &[1; 32]));
    }
}
