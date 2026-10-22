//! Town sunset (docs/TOWN-ON-SOLANA.md; owner decision 2026-10-22): the social kinds (14, 15,
//! 22-24) and the DM and channel kinds (16, 18-21) move to the `town` program on Solana. From the
//! sunset height on, a chain refuses NEW txs of those kinds, in the mempool and in blocks, and
//! contracts can no longer make them as native calls. Everything already on the chain stays:
//! profiles, follows, posts in old blocks, messages, channels and their state are readable as
//! before. `ReclaimMessage` (17) stays valid, so pay held in an open DM escrow can always go back
//! to its sender; DM and social AdminConfig keys stay valid too (they move no funds).
//!
//! Same mechanism as the speed switch (speed.rs): a coordinated height per chain id, the same on
//! every node; old and new nodes agree on every block below it.

use crate::tx::TxKind;

/// Cadre public devnet chain id (deploy/genesis.json).
pub const DEVNET_CHAIN_ID: u32 = 1_128_350_802;
/// First devnet height at which the town and DM kinds are refused (proposed 2026-10-22 at tip
/// ~6050 with 30 s blocks: about two days for the node rollout and the site's switch to Solana).
pub const DEVNET_TOWN_SUNSET_HEIGHT: u64 = 12_000;
/// Test-only chain ids: `TEST_SUNSET_CHAIN_BASE + h` (h < 0x10000) sunset at height `h`. No real
/// chain uses this range (the speed tests use 0x7E57xxxx).
pub const TEST_SUNSET_CHAIN_BASE: u32 = 0x7E58_0000;

/// The sunset height of `chain_id`, if it has one. Other chains (localnet demos, unit tests) keep
/// the kinds; a mainnet genesis should set its own (0 = never had them).
pub fn town_sunset_height(chain_id: u32) -> Option<u64> {
    match chain_id {
        DEVNET_CHAIN_ID => Some(DEVNET_TOWN_SUNSET_HEIGHT),
        c if (TEST_SUNSET_CHAIN_BASE..TEST_SUNSET_CHAIN_BASE + 0x1_0000).contains(&c) => Some((c - TEST_SUNSET_CHAIN_BASE) as u64),
        _ => None,
    }
}

/// Kinds the sunset stops (every social and DM kind but ReclaimMessage).
pub fn sunset_kind(k: TxKind) -> bool {
    (k.is_social() || k.is_dm()) && k != TxKind::ReclaimMessage
}

/// Is a tx of kind `k` at `height` on `chain_id` refused by the sunset?
pub fn sunset_refuses(chain_id: u32, height: u64, k: TxKind) -> bool {
    sunset_kind(k) && town_sunset_height(chain_id).is_some_and(|h| height >= h)
}

/// The refusal text clients see.
pub fn sunset_message(chain_id: u32, k: TxKind) -> String {
    let h = town_sunset_height(chain_id).unwrap_or(0);
    format!("{} txs ended at height {h}: the town and DMs moved to Solana (docs/TOWN-ON-SOLANA.md)", k.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_and_kinds() {
        assert_eq!(town_sunset_height(DEVNET_CHAIN_ID), Some(DEVNET_TOWN_SUNSET_HEIGHT));
        assert_eq!(town_sunset_height(7), None);
        assert_eq!(town_sunset_height(TEST_SUNSET_CHAIN_BASE + 40), Some(40));
        for k in [TxKind::SetProfile, TxKind::OperatorLink, TxKind::Post, TxKind::React, TxKind::Follow, TxKind::SendMessage, TxKind::SetConsent, TxKind::ChannelConfig, TxKind::ChannelMember, TxKind::ChannelKey] {
            assert!(sunset_kind(k), "{k:?}");
            assert!(!sunset_refuses(DEVNET_CHAIN_ID, DEVNET_TOWN_SUNSET_HEIGHT - 1, k));
            assert!(sunset_refuses(DEVNET_CHAIN_ID, DEVNET_TOWN_SUNSET_HEIGHT, k));
            assert!(!sunset_refuses(7, u64::MAX, k));
        }
        for k in [TxKind::ReclaimMessage, TxKind::Transfer, TxKind::AdminConfig, TxKind::RegisterAgent, TxKind::PostJob] {
            assert!(!sunset_kind(k), "{k:?}");
        }
    }
}
