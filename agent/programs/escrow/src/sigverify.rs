//! M-of-N ed25519 check through the ed25519 precompile.
//!
//! The precompile verifies every signature in its instruction before the
//! transaction runs (a bad signature fails the whole transaction). This module
//! only has to read those already-verified (pubkey, message) pairs back out of
//! the instructions sysvar and count how many distinct configured signers
//! signed exactly `expected_msg`.
//!
//! Only signature entries whose pubkey and message live inside the ed25519
//! instruction itself (instruction index u16::MAX, or the instruction's own
//! index) are counted. Anything else is skipped, which can only lower the count.
//!
//! This file is duplicated in token/programs/bridge/src/sigverify.rs; keep the
//! two in sync.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::ed25519_program;
use anchor_lang::solana_program::sysvar::instructions::load_instruction_at_checked;

const OFFSETS_START: usize = 2;
const OFFSETS_LEN: usize = 14;

fn read_u16(d: &[u8], at: usize) -> Option<u16> {
    let b = d.get(at..at + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

/// Number of distinct members of `signers` that signed `expected_msg` in an
/// ed25519 precompile instruction of the current transaction.
pub fn count_signers(
    ix_sysvar: &AccountInfo,
    expected_msg: &[u8],
    signers: &[Pubkey],
) -> Result<usize> {
    let num_ix = {
        let data = ix_sysvar.try_borrow_data()?;
        read_u16(&data, 0).ok_or(ProgramError::InvalidAccountData)? as usize
    };
    let mut seen: Vec<Pubkey> = Vec::with_capacity(signers.len());
    for i in 0..num_ix {
        let ix = load_instruction_at_checked(i, ix_sysvar)?;
        if ix.program_id != ed25519_program::ID {
            continue;
        }
        let d = &ix.data;
        let n = match d.first() {
            Some(n) => *n as usize,
            None => continue,
        };
        for s in 0..n {
            let base = OFFSETS_START + s * OFFSETS_LEN;
            let (Some(sig_ix), Some(pk_off), Some(pk_ix), Some(msg_off), Some(msg_len), Some(msg_ix)) = (
                read_u16(d, base + 2),
                read_u16(d, base + 4),
                read_u16(d, base + 6),
                read_u16(d, base + 8),
                read_u16(d, base + 10),
                read_u16(d, base + 12),
            ) else {
                break;
            };
            let here = |x: u16| x == u16::MAX || x as usize == i;
            if !(here(sig_ix) && here(pk_ix) && here(msg_ix)) {
                continue;
            }
            let (pk_off, msg_off, msg_len) = (pk_off as usize, msg_off as usize, msg_len as usize);
            let Some(pk) = d.get(pk_off..pk_off + 32) else { continue };
            let Some(msg) = d.get(msg_off..msg_off + msg_len) else { continue };
            if msg != expected_msg {
                continue;
            }
            let pk = Pubkey::new_from_array(pk.try_into().unwrap());
            if signers.contains(&pk) && !seen.contains(&pk) {
                seen.push(pk);
            }
        }
    }
    Ok(seen.len())
}
