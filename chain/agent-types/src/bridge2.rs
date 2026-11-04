//! Two-side bridge (docs/BRIDGE-V2.md): bridge domains, the v2 attested messages and the v2
//! withdrawal-log entry.
//!
//! Domain 1 is Solana and keeps the original kinds (BridgeUnlock 6, BridgeLock 7, SetRoute 9),
//! their messages and the 88-byte v1 log entry, byte for byte. Every other side (domain 2 =
//! Robinhood Chain) uses the v2 kinds (BridgeUnlockV2 40, BridgeLockV2 41, SetRouteV2 42),
//! which are valid only from the chain's `bridge_v2_height` (state::bridge2) and never for
//! domain 1.
//!
//! All integers little-endian, like every other chain encoding.

use crate::codec::{R, W};
use crate::{sha256, DecodeError, Hash, Pubkey, PostJob};

/// Solana: the original bridge (kinds 6, 7, 9).
pub const DOMAIN_SOLANA: u8 = 1;
/// Robinhood Chain (testnet chain id 46630, mainnet 4663).
pub const DOMAIN_RH: u8 = 2;
/// Highest external domain the chain accepts (2..=MAX_DOMAIN may be configured).
pub const MAX_DOMAIN: u8 = 15;
/// Most domains configured at once.
pub const MAX_DOMAINS: usize = 8;

/// Robinhood Chain testnet chain id.
pub const RH_TESTNET_CHAIN_ID: u64 = 46630;

/// Plain BridgeUnlockV2 message prefix.
pub const DEPOSIT_V2: &[u8] = b"cadre:deposit:v2";
/// Order BridgeUnlockV2 message prefix (job_order = 1).
pub const DEPOSIT_ORDER_V2: &[u8] = b"cadre:deposit-order:v2";
/// Order job id prefix of a v2 order.
pub const ORDER_JOB_V2: &[u8] = b"cadre:order-job:v2";

/// External units are 6 decimals like Solana SPL CADRE: 1 external base unit = 1000 chain
/// base units on every domain.
pub const EXT_SCALE: u64 = crate::SPL_SCALE;

/// Is `d` a domain the v2 kinds may name (2..=MAX_DOMAIN)?
pub fn is_ext_domain(d: u8) -> bool {
    (2..=MAX_DOMAIN).contains(&d)
}

/// `prefix || domain u8 || ext_chain_id u64 || bridge_addr (20 or 32 raw bytes) || ext_nonce u64
/// || recipient [32] || amount_ext u64`.
fn write_head(w: &mut W, prefix: &[u8], domain: u8, ext_chain_id: u64, bridge_addr: &[u8], ext_nonce: u64, recipient: &Pubkey, amount_ext: u64) {
    w.bytes(prefix).u8(domain).u64(ext_chain_id).bytes(bridge_addr).u64(ext_nonce).bytes(recipient).u64(amount_ext);
}

/// Attested message for a plain v2 deposit:
/// `b"cadre:deposit:v2" || domain u8 || ext_chain_id u64 || bridge_addr || ext_nonce u64 || recipient [32] || amount_ext u64`.
pub fn deposit_message_v2(domain: u8, ext_chain_id: u64, bridge_addr: &[u8], ext_nonce: u64, recipient: &Pubkey, amount_ext: u64) -> Vec<u8> {
    let mut w = W::new();
    write_head(&mut w, DEPOSIT_V2, domain, ext_chain_id, bridge_addr, ext_nonce, recipient, amount_ext);
    w.into_inner()
}

/// Attested message for a v2 storefront order: the plain fields under
/// `b"cadre:deposit-order:v2"`, then `sha256(PostJob body)`.
pub fn order_message_v2(domain: u8, ext_chain_id: u64, bridge_addr: &[u8], ext_nonce: u64, recipient: &Pubkey, amount_ext: u64, job: &PostJob) -> Vec<u8> {
    let mut w = W::new();
    write_head(&mut w, DEPOSIT_ORDER_V2, domain, ext_chain_id, bridge_addr, ext_nonce, recipient, amount_ext);
    w.bytes(&job.body_hash());
    w.into_inner()
}

/// Order job id of a v2 order: `sha256(b"cadre:order-job:v2" || domain u8 || ext_nonce u64)`.
pub fn order_job_id_v2(domain: u8, ext_nonce: u64) -> Hash {
    sha256(&[ORDER_JOB_V2, &[domain], &ext_nonce.to_le_bytes()])
}

/// Version byte of a v2 log entry.
pub const WITHDRAWAL_V2_VERSION: u8 = 0x02;

/// One v2 withdrawal-log entry (90 bytes; the v1 entry is 88, so the deployed Solana bridge,
/// which requires exactly 88, refuses every v2 entry as it is):
/// `version u8 = 0x02 || domain u8 || nonce u64 || from [32] || recipient [32] || amount_ext u64 || height u64`.
/// `nonce` counts per domain from 0; an EVM recipient is the 20-byte address left-padded with
/// 12 zero bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithdrawalV2 {
    pub domain: u8,
    pub nonce: u64,
    pub from: Pubkey,
    pub recipient: Pubkey,
    pub amount_ext: u64,
    pub height: u64,
}

impl WithdrawalV2 {
    pub const LOG_LEN: usize = 90;

    pub fn log_entry(&self) -> Vec<u8> {
        let mut w = W::new();
        w.u8(WITHDRAWAL_V2_VERSION).u8(self.domain).u64(self.nonce).bytes(&self.from).bytes(&self.recipient).u64(self.amount_ext).u64(self.height);
        w.into_inner()
    }

    pub fn from_log_entry(b: &[u8]) -> Result<Self, DecodeError> {
        if b.len() != Self::LOG_LEN || b[0] != WITHDRAWAL_V2_VERSION {
            return Err(DecodeError::Invalid("not a v2 withdrawal entry"));
        }
        let mut r = R::new(&b[1..]);
        let w = Self { domain: r.u8()?, nonce: r.u64()?, from: r.arr()?, recipient: r.arr()?, amount_ext: r.u64()?, height: r.u64()? };
        r.finish()?;
        Ok(w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_roundtrip_and_length() {
        let w = WithdrawalV2 { domain: 2, nonce: 7, from: [1; 32], recipient: [2; 32], amount_ext: 9, height: 11 };
        let e = w.log_entry();
        assert_eq!(e.len(), WithdrawalV2::LOG_LEN);
        assert_ne!(e.len(), 88, "a v2 entry must never be 88 bytes (the v1 / Solana length)");
        assert_eq!(WithdrawalV2::from_log_entry(&e).unwrap(), w);
        assert!(WithdrawalV2::from_log_entry(&e[..88]).is_err());
        assert!(WithdrawalV2::from_log_entry(&[0u8; 90]).is_err());
    }

    #[test]
    fn messages_are_domain_tagged() {
        let a = deposit_message_v2(2, 46630, &[0xab; 20], 5, &[3; 32], 100);
        let b = deposit_message_v2(3, 46630, &[0xab; 20], 5, &[3; 32], 100);
        assert_ne!(a, b);
        assert_eq!(a.len(), 16 + 1 + 8 + 20 + 8 + 32 + 8);
        assert!(a.starts_with(DEPOSIT_V2));
        assert_ne!(order_job_id_v2(2, 5), order_job_id_v2(3, 5));
        assert_ne!(order_job_id_v2(2, 5), crate::tx::order_job_id(5));
        assert!(!is_ext_domain(1) && is_ext_domain(2) && is_ext_domain(MAX_DOMAIN) && !is_ext_domain(MAX_DOMAIN + 1));
    }
}
