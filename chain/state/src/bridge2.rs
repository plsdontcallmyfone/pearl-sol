//! Two-side bridge records (docs/BRIDGE-V2.md).
//!
//! Like the DM and blob parameters these live in their OWN records, so no existing record's
//! encoding changes. A missing params record means "v2 disabled" (activation height u64::MAX,
//! no domains), so a chain where no admin ever set one has exactly the state root it had
//! before this module existed.
//!
//! ```text
//! BridgeParams  ["bridge2-params"]       tag 53 | version u8 = 1 | activation_height u64
//!                                         | count u8 | (domain u8 | ext_chain_id u64 | addr_len u8 | addr | paused u8)*
//! DepositV2     [domain u8 | nonce u64]  tag 54 | height u64 | recipient [32] | amount_ext u64
//! WithdrawalV2  [domain u8 | nonce u64]  tag 55 | the 90-byte v2 log entry (agent_types::bridge2)
//! DomainStats   [domain u8]              tag 56 | unlocked u64 | locked u64 | deposit_count u64 | withdrawal_count u64
//! RouteDomain   [account [32]]           tag 57 | domain u8   (only for a lien paying out to domain >= 2)
//! ```
//!
//! `unlocked` / `locked` are chain base units moved out of / into the escrow for that domain;
//! `outstanding = unlocked - locked` is what the domain's vault must hold, and a BridgeLockV2
//! (or a lien payout) to the domain never takes `locked` above `unlocked`. Domain 1 (Solana)
//! has no stats record: its outstanding is derived, `TOTAL_SUPPLY - escrow - sum(outstanding of
//! the other domains)`, which is exactly the relayer's existing `total - escrow` before any
//! other side exists, so it is correct at activation with no migration step.

use agent_types::bridge2::{is_ext_domain, WithdrawalV2, MAX_DOMAINS};
use agent_types::codec::{R, W};
use agent_types::{DecodeError, Pubkey};

use crate::schema::{key, Key};

pub mod tag {
    pub const PARAMS: u8 = 53;
    pub const DEPOSIT_V2: u8 = 54;
    pub const WITHDRAWAL_V2: u8 = 55;
    pub const DOMAIN_STATS: u8 = 56;
    pub const ROUTE_DOMAIN: u8 = 57;
}

/// AdminConfig keys of the bridge v2 parameters (own record; keys 50-55 reserved).
pub mod keys {
    /// u64: first height at which the v2 kinds are valid (u64::MAX = disabled, the default).
    /// Settable only to a height above the admin tx's block, and only while not yet active.
    pub const BRIDGE_V2_HEIGHT: u8 = 50;
    /// `domain u8 | ext_chain_id u64 | addr_len u8 (20|32) | addr | paused u8`: add or update
    /// one external domain (2..=15).
    pub const BRIDGE_DOMAIN: u8 = 51;
    pub const FIRST: u8 = 50;
    pub const LAST: u8 = 51;
    /// Keys 52-55 are reserved for later bridge parameters.
    pub const RESERVED_LAST: u8 = 55;

    pub fn name(k: u8) -> &'static str {
        match k {
            BRIDGE_V2_HEIGHT => "bridge_v2_height",
            BRIDGE_DOMAIN => "bridge_domain",
            _ => "unknown",
        }
    }
}

/// Is `k` an AdminConfig key handled by the bridge v2 parameters (50..=55)?
pub fn is_bridge_key(k: u8) -> bool {
    (keys::FIRST..=keys::RESERVED_LAST).contains(&k)
}

pub fn params_key() -> Key {
    key(tag::PARAMS, &[])
}
fn dn(domain: u8, nonce: u64) -> [u8; 9] {
    let mut id = [0u8; 9];
    id[0] = domain;
    id[1..].copy_from_slice(&nonce.to_le_bytes());
    id
}
pub fn deposit_v2_key(domain: u8, nonce: u64) -> Key {
    key(tag::DEPOSIT_V2, &dn(domain, nonce))
}
pub fn withdrawal_v2_key(domain: u8, nonce: u64) -> Key {
    key(tag::WITHDRAWAL_V2, &dn(domain, nonce))
}
pub fn domain_stats_key(domain: u8) -> Key {
    key(tag::DOMAIN_STATS, &[domain])
}
pub fn route_domain_key(account: &Pubkey) -> Key {
    key(tag::ROUTE_DOMAIN, account)
}

fn expect_tag(r: &mut R<'_>, t: u8) -> Result<(), DecodeError> {
    if r.u8()? != t {
        return Err(DecodeError::Invalid("record tag"));
    }
    Ok(())
}

/// One configured external side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainCfg {
    pub domain: u8,
    /// The side's chain id (46630 for Robinhood Chain testnet); part of every v2 message.
    pub ext_chain_id: u64,
    /// The side's bridge contract: 20 bytes (EVM) or 32 bytes.
    pub bridge_addr: Vec<u8>,
    pub paused: bool,
}

impl DomainCfg {
    pub fn is_evm(&self) -> bool {
        self.bridge_addr.len() == 20
    }

    fn write(&self, w: &mut W) {
        w.u8(self.domain).u64(self.ext_chain_id).bytes8(&self.bridge_addr).u8(self.paused as u8);
    }

    fn read(r: &mut R<'_>) -> Result<Self, DecodeError> {
        let domain = r.u8()?;
        let ext_chain_id = r.u64()?;
        let bridge_addr = r.bytes8(32, "bridge_addr")?;
        let paused = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(DecodeError::Invalid("paused flag")),
        };
        let d = Self { domain, ext_chain_id, bridge_addr, paused };
        d.check().map_err(|_| DecodeError::Invalid("bridge domain"))?;
        Ok(d)
    }

    pub fn check(&self) -> Result<(), String> {
        if !is_ext_domain(self.domain) {
            return Err(format!("domain must be 2..={} (domain 1 is Solana on the original kinds)", agent_types::bridge2::MAX_DOMAIN));
        }
        if self.bridge_addr.len() != 20 && self.bridge_addr.len() != 32 {
            return Err("bridge address must be 20 or 32 bytes".into());
        }
        if self.bridge_addr.iter().all(|b| *b == 0) {
            return Err("bridge address must be set".into());
        }
        Ok(())
    }

    /// AdminConfig value: `domain u8 | ext_chain_id u64 | addr_len u8 | addr | paused u8`.
    pub fn encode_value(&self) -> Vec<u8> {
        let mut w = W::new();
        self.write(&mut w);
        w.into_inner()
    }

    pub fn decode_value(v: &[u8]) -> Result<Self, String> {
        let mut r = R::new(v);
        let d = Self::read(&mut r).map_err(|e| e.to_string())?;
        r.finish().map_err(|e| e.to_string())?;
        d.check()?;
        Ok(d)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeParams {
    pub activation_height: u64,
    /// Sorted by domain.
    pub domains: Vec<DomainCfg>,
}

impl Default for BridgeParams {
    fn default() -> Self {
        Self { activation_height: u64::MAX, domains: Vec::new() }
    }
}

impl BridgeParams {
    pub fn is_active(&self, height: u64) -> bool {
        height >= self.activation_height
    }

    pub fn domain(&self, d: u8) -> Option<&DomainCfg> {
        self.domains.iter().find(|x| x.domain == d)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::PARAMS).u8(1).u64(self.activation_height).u8(self.domains.len() as u8);
        for d in &self.domains {
            d.write(&mut w);
        }
        w.into_inner()
    }

    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        if r.u8()? != tag::PARAMS || r.u8()? != 1 {
            return Err(DecodeError::Invalid("bridge params tag"));
        }
        let activation_height = r.u64()?;
        let n = r.u8()? as usize;
        if n > MAX_DOMAINS {
            return Err(DecodeError::TooLarge("bridge domains"));
        }
        let mut domains = Vec::with_capacity(n);
        for _ in 0..n {
            domains.push(DomainCfg::read(&mut r)?);
        }
        r.finish()?;
        Ok(Self { activation_height, domains })
    }

    /// Add or replace a domain's configuration.
    pub fn upsert(&mut self, d: DomainCfg) -> Result<(), String> {
        d.check()?;
        match self.domains.iter_mut().find(|x| x.domain == d.domain) {
            Some(x) => *x = d,
            None => {
                if self.domains.len() >= MAX_DOMAINS {
                    return Err(format!("at most {MAX_DOMAINS} bridge domains"));
                }
                self.domains.push(d);
                self.domains.sort_by_key(|x| x.domain);
            }
        }
        Ok(())
    }
}

/// A processed v2 deposit (replay set entry for `(domain, nonce)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepositV2 {
    pub height: u64,
    pub recipient: Pubkey,
    pub amount_ext: u64,
}

impl DepositV2 {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::DEPOSIT_V2).u64(self.height).bytes(&self.recipient).u64(self.amount_ext);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect_tag(&mut r, tag::DEPOSIT_V2)?;
        let d = Self { height: r.u64()?, recipient: r.arr()?, amount_ext: r.u64()? };
        r.finish()?;
        Ok(d)
    }
}

/// State record of a v2 withdrawal: tag 55 || the log entry.
pub fn encode_withdrawal_v2(w: &WithdrawalV2) -> Vec<u8> {
    let mut v = vec![tag::WITHDRAWAL_V2];
    v.extend_from_slice(&w.log_entry());
    v
}

pub fn decode_withdrawal_v2(b: &[u8]) -> Result<WithdrawalV2, DecodeError> {
    if b.first() != Some(&tag::WITHDRAWAL_V2) {
        return Err(DecodeError::Invalid("record tag"));
    }
    WithdrawalV2::from_log_entry(&b[1..])
}

/// Per-domain bridge counters (chain base units).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DomainStats {
    pub unlocked: u64,
    pub locked: u64,
    pub deposit_count: u64,
    /// Next withdrawal nonce of the domain.
    pub withdrawal_count: u64,
}

impl DomainStats {
    pub fn outstanding(&self) -> u64 {
        self.unlocked.saturating_sub(self.locked)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(tag::DOMAIN_STATS).u64(self.unlocked).u64(self.locked).u64(self.deposit_count).u64(self.withdrawal_count);
        w.into_inner()
    }
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        let mut r = R::new(b);
        expect_tag(&mut r, tag::DOMAIN_STATS)?;
        let s = Self { unlocked: r.u64()?, locked: r.u64()?, deposit_count: r.u64()?, withdrawal_count: r.u64()? };
        r.finish()?;
        Ok(s)
    }
}

pub fn encode_route_domain(domain: u8) -> Vec<u8> {
    vec![tag::ROUTE_DOMAIN, domain]
}

pub fn decode_route_domain(b: &[u8]) -> Result<u8, DecodeError> {
    match b {
        [t, d] if *t == tag::ROUTE_DOMAIN => Ok(*d),
        _ => Err(DecodeError::Invalid("route domain record")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rh() -> DomainCfg {
        DomainCfg { domain: 2, ext_chain_id: 46630, bridge_addr: vec![0xab; 20], paused: false }
    }

    #[test]
    fn records_roundtrip() {
        let mut p = BridgeParams::default();
        assert!(!p.is_active(u64::MAX - 1));
        assert_eq!(BridgeParams::decode(&p.encode()).unwrap(), p);
        p.activation_height = 100;
        p.upsert(rh()).unwrap();
        p.upsert(DomainCfg { domain: 3, ext_chain_id: 1, bridge_addr: vec![1; 32], paused: true }).unwrap();
        assert_eq!(BridgeParams::decode(&p.encode()).unwrap(), p);
        assert!(p.is_active(100) && !p.is_active(99));
        assert!(p.upsert(DomainCfg { domain: 1, ..rh() }).is_err());
        assert!(p.upsert(DomainCfg { bridge_addr: vec![1; 21], ..rh() }).is_err());
        assert!(p.upsert(DomainCfg { bridge_addr: vec![0; 20], ..rh() }).is_err());
        assert_eq!(DomainCfg::decode_value(&rh().encode_value()).unwrap(), rh());
        let d = DepositV2 { height: 3, recipient: [4; 32], amount_ext: 5 };
        assert_eq!(DepositV2::decode(&d.encode()).unwrap(), d);
        let s = DomainStats { unlocked: 10, locked: 4, deposit_count: 1, withdrawal_count: 2 };
        assert_eq!(DomainStats::decode(&s.encode()).unwrap(), s);
        assert_eq!(s.outstanding(), 6);
        let w = WithdrawalV2 { domain: 2, nonce: 0, from: [1; 32], recipient: [2; 32], amount_ext: 3, height: 4 };
        assert_eq!(decode_withdrawal_v2(&encode_withdrawal_v2(&w)).unwrap(), w);
        assert_eq!(decode_route_domain(&encode_route_domain(2)).unwrap(), 2);
        assert_ne!(deposit_v2_key(2, 5), deposit_v2_key(3, 5));
        assert_ne!(deposit_v2_key(2, 5), crate::schema::deposit_key(5));
    }

    #[test]
    fn numbers_do_not_collide() {
        for k in keys::FIRST..=keys::RESERVED_LAST {
            assert!(!crate::config::keys::is_config_key(k), "{k}");
            assert!(!crate::dm::is_dm_key(k), "{k}");
            assert!(!crate::blobparams::is_blob_key(k), "{k}");
        }
        for t in [tag::PARAMS, tag::DEPOSIT_V2, tag::WITHDRAWAL_V2, tag::DOMAIN_STATS, tag::ROUTE_DOMAIN] {
            assert_ne!(t, crate::blobparams::TAG);
            assert_ne!(t, crate::schema::tag::UNCLE);
            assert!(t > crate::schema::tag::UNCLE);
        }
    }

    /// The deployed Solana bridge (token/programs/bridge) pays a withdrawal only for an entry of
    /// exactly WITHDRAWAL_ENTRY_LEN = 88 bytes. A v2 entry is 90 bytes, so even if a relayer
    /// handed one to `release_proof`, the program refuses it before reading any field.
    #[test]
    fn solana_bridge_refuses_v2_entries_by_length() {
        let src = include_str!("../../../token/programs/bridge/src/lib.rs");
        assert!(src.contains("pub const WITHDRAWAL_ENTRY_LEN: usize = 88;"), "the bridge's entry length changed");
        assert!(src.contains("require!(entry.len() == WITHDRAWAL_ENTRY_LEN, BridgeError::BadProof);"), "release_proof no longer checks the length");
        let w = WithdrawalV2 { domain: 2, nonce: 0, from: [1; 32], recipient: [2; 32], amount_ext: 3, height: 4 };
        assert_ne!(w.log_entry().len(), 88);
        assert_eq!(crate::schema::Withdrawal::LOG_LEN, 88);
        assert!(crate::schema::Withdrawal::from_log_entry(&w.log_entry()).is_err());
        let v1 = crate::schema::Withdrawal { nonce: 0, from: [1; 32], solana_recipient: [2; 32], amount_spl: 3, height: 4 };
        assert!(WithdrawalV2::from_log_entry(&v1.log_entry()).is_err());
    }
}
