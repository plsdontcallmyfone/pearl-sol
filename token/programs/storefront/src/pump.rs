//! Raw CPI builders and account readers for pump.fun (bonding curve) and
//! PumpSwap. Copied from token/programs/reserve/src/pump.rs (the reserve's
//! buy code, hand-built from the IDLs in research/pump-litesvm/idl,
//! pump-public-docs @ 639b87f) and trimmed to what the storefront uses, plus
//! `close_account`. Keep the shared parts in sync with the reserve copy when
//! pump changes its account lists.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    pubkey,
};

use crate::StorefrontError;

pub const PUMP_ID: Pubkey = pubkey!("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
pub const PUMP_AMM_ID: Pubkey = pubkey!("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
pub const WSOL_MINT: Pubkey = pubkey!("So11111111111111111111111111111111111111112");
pub const TOKEN_ID: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const TOKEN_2022_ID: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const ATA_ID: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

// Discriminators from the IDLs.
pub const IX_BUY_EXACT_QUOTE_IN_V3: [u8; 8] = [225, 247, 80, 30, 213, 179, 132, 136];
pub const IX_AMM_BUY_EXACT_QUOTE_IN_V2: [u8; 8] = [194, 171, 28, 70, 104, 77, 91, 47];
pub const ACC_BONDING_CURVE: [u8; 8] = [23, 183, 248, 55, 96, 216, 172, 96];
pub const ACC_POOL: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

/// (account, writable, signer)
pub type Meta<'a, 'info> = (&'a AccountInfo<'info>, bool, bool);

pub fn call<'info>(program: &AccountInfo<'info>, data: Vec<u8>, metas: &[Meta<'_, 'info>], seeds: &[&[&[u8]]]) -> Result<()> {
    let ix = Instruction {
        program_id: *program.key,
        accounts: metas
            .iter()
            .map(|(a, w, s)| if *w { AccountMeta::new(*a.key, *s) } else { AccountMeta::new_readonly(*a.key, *s) })
            .collect(),
        data,
    };
    let mut infos: Vec<AccountInfo<'info>> = metas.iter().map(|(a, _, _)| (*a).clone()).collect();
    infos.push(program.clone());
    invoke_signed(&ix, &infos, seeds)?;
    Ok(())
}

pub fn ata(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[owner.as_ref(), token_program.as_ref(), mint.as_ref()], &ATA_ID).0
}

pub fn bonding_curve(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"bonding-curve", mint.as_ref()], &PUMP_ID).0
}

pub fn canonical_pool(mint: &Pubkey) -> Pubkey {
    let authority = Pubkey::find_program_address(&[b"pool-authority", mint.as_ref()], &PUMP_ID).0;
    Pubkey::find_program_address(
        &[b"pool", &0u16.to_le_bytes(), authority.as_ref(), mint.as_ref(), WSOL_MINT.as_ref()],
        &PUMP_AMM_ID,
    )
    .0
}

fn u64_at(d: &[u8], o: usize) -> u64 {
    d.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap())).unwrap_or(0)
}

/// SPL Token and Token-2022 accounts share the base layout: amount at 64.
pub fn token_amount(a: &AccountInfo) -> Result<u64> {
    let d = a.try_borrow_data()?;
    require!(d.len() >= 72, StorefrontError::BadAccount);
    Ok(u64_at(&d, 64))
}

pub struct Curve {
    pub virtual_token: u64,
    pub virtual_quote: u64,
    pub complete: bool,
}

pub fn read_curve(a: &AccountInfo) -> Result<Curve> {
    require_keys_eq!(*a.owner, PUMP_ID, StorefrontError::BadAccount);
    let d = a.try_borrow_data()?;
    require!(d.len() >= 81 && d[..8] == ACC_BONDING_CURVE, StorefrontError::BadAccount);
    Ok(Curve { virtual_token: u64_at(&d, 8), virtual_quote: u64_at(&d, 16), complete: d[48] != 0 })
}

pub struct Pool {
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    /// i128 in the account; only the non-negative part adds depth.
    pub virtual_quote: u64,
    pub protocol_fees: u64,
    pub creator_fees: u64,
}

pub fn read_pool(a: &AccountInfo) -> Result<Pool> {
    require_keys_eq!(*a.owner, PUMP_AMM_ID, StorefrontError::BadAccount);
    let d = a.try_borrow_data()?;
    require!(d.len() >= 243 && d[..8] == ACC_POOL, StorefrontError::BadAccount);
    let pk = |o: usize| Pubkey::new_from_array(d[o..o + 32].try_into().unwrap());
    let vq = d.get(245..261).map(|b| i128::from_le_bytes(b.try_into().unwrap())).unwrap_or(0);
    Ok(Pool {
        base_mint: pk(43),
        quote_mint: pk(75),
        base_vault: pk(139),
        quote_vault: pk(171),
        virtual_quote: vq.clamp(0, u64::MAX as i128) as u64,
        protocol_fees: u64_at(&d, 271),
        creator_fees: u64_at(&d, 279),
    })
}

/// Fee-free constant-product quote: base out for `quote_in` against
/// (base_reserve, quote_reserve). Pump's real output is lower by its fees,
/// which `config.max_slippage_bps` must cover.
pub fn cp_quote(base_reserve: u64, quote_reserve: u64, quote_in: u64) -> u64 {
    let num = base_reserve as u128 * quote_in as u128;
    let den = quote_reserve as u128 + quote_in as u128;
    if den == 0 {
        0
    } else {
        (num / den).min(u64::MAX as u128) as u64
    }
}

pub fn ata_create_idempotent<'info>(
    ata_program: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    account: &AccountInfo<'info>,
    owner: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    token_program: &AccountInfo<'info>,
) -> Result<()> {
    call(
        ata_program,
        vec![1],
        &[(payer, true, true), (account, true, false), (owner, false, false), (mint, false, false), (system, false, false), (token_program, false, false)],
        &[],
    )
}

pub fn sync_native<'info>(token_program: &AccountInfo<'info>, account: &AccountInfo<'info>) -> Result<()> {
    call(token_program, vec![17], &[(account, true, false)], &[])
}

/// SPL Token CloseAccount (9): all lamports (rent + wrapped SOL) to `dest`.
pub fn close_account<'info>(
    token_program: &AccountInfo<'info>,
    account: &AccountInfo<'info>,
    dest: &AccountInfo<'info>,
    owner: &AccountInfo<'info>,
    seeds: &[&[u8]],
) -> Result<()> {
    call(token_program, vec![9], &[(account, true, false), (dest, true, false), (owner, false, true)], &[seeds])
}
