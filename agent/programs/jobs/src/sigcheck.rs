//! Attester signature check through the ed25519 precompile (SPEC section 6).
//!
//! The caller puts one or more ed25519 precompile instructions in the same
//! transaction. The runtime verifies those signatures before any program runs, so a
//! program only has to confirm, through the instructions sysvar, that a verified
//! signature exists over the exact message it expects, by a key in its signer set.
//!
//! Every offset in an accepted ed25519 instruction must point into that same
//! instruction (index `u16::MAX`). Otherwise the precompile could have verified bytes
//! in some other instruction while we read different bytes here.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::ed25519_program;
use anchor_lang::solana_program::sysvar::instructions::load_instruction_at_checked;

const OFFSETS_START: usize = 2;
const OFFSETS_LEN: usize = 14;
const PUBKEY_LEN: usize = 32;
const SIG_LEN: usize = 64;
const SAME_IX: u16 = u16::MAX;

fn rd_u16(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

/// Count distinct keys from `signers` that signed `expected_msg` in any ed25519
/// precompile instruction of the current transaction.
pub fn count_signers(ix_sysvar: &AccountInfo, expected_msg: &[u8], signers: &[Pubkey]) -> Result<u32> {
    require_keys_eq!(
        *ix_sysvar.key,
        anchor_lang::solana_program::sysvar::instructions::ID,
        SigError::BadSysvar
    );
    let n_ix = {
        let data = ix_sysvar.try_borrow_data()?;
        rd_u16(&data, 0).ok_or(SigError::BadSysvar)? as usize
    };
    let mut seen: u64 = 0;
    for i in 0..n_ix {
        let ix = load_instruction_at_checked(i, ix_sysvar)?;
        if ix.program_id != ed25519_program::ID {
            continue;
        }
        let d = &ix.data;
        let n_sig = *d.first().ok_or(SigError::BadEd25519)? as usize;
        for s in 0..n_sig {
            let o = OFFSETS_START + s * OFFSETS_LEN;
            let f = |k: usize| rd_u16(d, o + 2 * k).ok_or(SigError::BadEd25519);
            let (_sig_off, sig_ix, pk_off, pk_ix, msg_off, msg_len, msg_ix) =
                (f(0)?, f(1)?, f(2)?, f(3)?, f(4)?, f(5)?, f(6)?);
            if sig_ix != SAME_IX || pk_ix != SAME_IX || msg_ix != SAME_IX {
                continue;
            }
            // signature bytes are checked by the precompile; we only need them in range
            let sig_off = _sig_off as usize;
            if d.len() < sig_off + SIG_LEN {
                continue;
            }
            let pk_off = pk_off as usize;
            let (msg_off, msg_len) = (msg_off as usize, msg_len as usize);
            let (Some(pk), Some(msg)) = (d.get(pk_off..pk_off + PUBKEY_LEN), d.get(msg_off..msg_off + msg_len))
            else {
                continue;
            };
            if msg != expected_msg {
                continue;
            }
            if let Some(idx) = signers.iter().position(|k| k.as_ref() == pk) {
                seen |= 1u64 << idx;
            }
        }
    }
    Ok(seen.count_ones())
}

/// Require at least `threshold` distinct signers over `expected_msg`.
pub fn require_signed(ix_sysvar: &AccountInfo, expected_msg: &[u8], signers: &[Pubkey], threshold: u8) -> Result<()> {
    require!(threshold >= 1 && signers.len() <= 64, SigError::BadThreshold);
    let n = count_signers(ix_sysvar, expected_msg, signers)?;
    require!(n >= threshold as u32, SigError::MissingAttesterSignature);
    Ok(())
}

#[error_code(offset = 7000)]
pub enum SigError {
    #[msg("instructions sysvar account expected")]
    BadSysvar,
    #[msg("malformed ed25519 precompile instruction")]
    BadEd25519,
    #[msg("bad signer threshold")]
    BadThreshold,
    #[msg("no valid attester signature over the expected message")]
    MissingAttesterSignature,
}
