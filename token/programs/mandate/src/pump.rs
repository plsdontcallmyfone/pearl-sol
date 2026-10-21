//! PumpSwap CPI helpers for the mandate program. The account readers and the
//! `call` builder follow token/programs/storefront/src/pump.rs (hand-built from
//! the pinned IDLs in research/pump-litesvm/idl); keep them in sync when pump
//! changes its layouts.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    pubkey,
};

use crate::MandateError;

pub const PUMP_ID: Pubkey = pubkey!("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
pub const PUMP_AMM_ID: Pubkey = pubkey!("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
pub const WSOL_MINT: Pubkey = pubkey!("So11111111111111111111111111111111111111112");
pub const TOKEN_ID: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const TOKEN_2022_ID: Pubkey = pubkey!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const ATA_ID: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

/// PumpSwap `buy_exact_quote_in_v2(spendable_quote_in, min_base_amount_out)`.
pub const IX_AMM_BUY_EXACT_QUOTE_IN_V2: [u8; 8] = [194, 171, 28, 70, 104, 77, 91, 47];
/// PumpSwap `sell_v2(base_amount_in, min_quote_amount_out)`.
pub const IX_AMM_SELL_V2: [u8; 8] = [93, 246, 130, 60, 231, 233, 64, 178];
pub const ACC_POOL: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];

/// The v2 account list (17 accounts, the same for buy_exact_quote_in_v2 and sell_v2).
pub const V2_ACCOUNTS: usize = 17;
pub const I_POOL: usize = 0;
pub const I_USER: usize = 1;
pub const I_BASE_MINT: usize = 3;
pub const I_QUOTE_MINT: usize = 4;
pub const I_USER_BASE: usize = 5;
pub const I_USER_QUOTE: usize = 6;
pub const I_POOL_BASE: usize = 7;
pub const I_POOL_QUOTE: usize = 8;
pub const I_BASE_TOKEN_PROGRAM: usize = 9;
pub const I_QUOTE_TOKEN_PROGRAM: usize = 10;
pub const I_SYSTEM: usize = 11;
pub const I_PROGRAM: usize = 16;
/// Writable flags of the v2 list, from the IDL.
pub const V2_WRITABLE: [bool; V2_ACCOUNTS] =
    [true, true, false, false, false, true, true, true, true, false, false, false, true, false, true, false, false];

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
    require!(d.len() >= 72, MandateError::BadAccount);
    Ok(u64_at(&d, 64))
}

/// Amount of a token account that may not exist yet (0 if empty / uninitialized).
pub fn token_amount_or_zero(a: &AccountInfo) -> u64 {
    a.try_borrow_data().map(|d| if d.len() >= 72 { u64_at(&d, 64) } else { 0 }).unwrap_or(0)
}

/// Mint decimals (SPL and Token-2022 base layout: decimals at 44).
pub fn mint_decimals(a: &AccountInfo) -> Result<u8> {
    let d = a.try_borrow_data()?;
    require!(d.len() >= 82, MandateError::BadAccount);
    Ok(d[44])
}

pub struct Pool {
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub virtual_quote: u64,
    pub protocol_fees: u64,
    pub creator_fees: u64,
}

pub fn read_pool(a: &AccountInfo) -> Result<Pool> {
    require_keys_eq!(*a.owner, PUMP_AMM_ID, MandateError::BadAccount);
    let d = a.try_borrow_data()?;
    require!(d.len() >= 243 && d[..8] == ACC_POOL, MandateError::BadAccount);
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

/// Fee-free constant-product output: `out = r_out * x / (r_in + x)`.
pub fn cp_out(r_in: u64, r_out: u64, x: u64) -> u64 {
    let den = r_in as u128 + x as u128;
    if den == 0 {
        0
    } else {
        ((r_out as u128 * x as u128) / den).min(u64::MAX as u128) as u64
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
    seeds: &[&[&[u8]]],
) -> Result<()> {
    call(
        ata_program,
        vec![1],
        &[(payer, true, true), (account, true, false), (owner, false, false), (mint, false, false), (system, false, false), (token_program, false, false)],
        seeds,
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

/// TransferChecked (12), SPL Token and Token-2022.
#[allow(clippy::too_many_arguments)]
pub fn transfer_checked<'info>(
    token_program: &AccountInfo<'info>,
    from: &AccountInfo<'info>,
    mint: &AccountInfo<'info>,
    to: &AccountInfo<'info>,
    owner: &AccountInfo<'info>,
    amount: u64,
    decimals: u8,
    seeds: &[&[u8]],
) -> Result<()> {
    let mut data = vec![12u8];
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    call(token_program, data, &[(from, true, false), (mint, false, false), (to, true, false), (owner, false, true)], &[seeds])
}
