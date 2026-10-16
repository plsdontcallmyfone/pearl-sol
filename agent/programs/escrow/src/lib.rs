//! Agent-chain side of the pearl-sol bridge. SPEC section 3.1.
//!
//! The whole agent-chain supply (1e18 lamports) sits in the `escrow` PDA at
//! genesis (on localnet it is preloaded with `--account`). The escrow PDA is
//! owned by this program and carries no data, so the program can debit it
//! directly.
//!
//! - `unlock` pays `amount_spl x 1000` lamports out of the escrow for a token
//!   chain `Deposit`, once M-of-N configured signers have signed the SPEC
//!   section 6 deposit message through the ed25519 precompile in the same
//!   transaction. A `Processed` PDA per (source chain, nonce) blocks replay.
//! - `lock` moves lamports (a multiple of 1000) back into the escrow and writes
//!   a `Withdrawal` under a global nonce for the relayer to release on Solana.
//! - Signer set, threshold and admin are admin-editable.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::sysvar::instructions as ix_sysvar;
use anchor_lang::system_program;

pub mod sigverify;

declare_id!("4EgFJ3Vnck4EqEtuDFnmF8Zrpcz8mmTU2DbnZBHyQuum");

pub const MAX_SIGNERS: usize = 10;
/// Lamports per SPL base unit (SPL 6 dp, lamports 9 dp).
pub const LAMPORTS_PER_SPL_UNIT: u64 = 1000;
/// Chain ids used in `Processed` seeds.
pub const CHAIN_TOKEN: u8 = 0;
pub const CHAIN_AGENT: u8 = 1;

pub const CONFIG_SEED: &[u8] = b"config";
pub const ESCROW_SEED: &[u8] = b"escrow";
pub const WITHDRAWAL_SEED: &[u8] = b"withdrawal";
pub const PROCESSED_SEED: &[u8] = b"processed";

pub const DEPOSIT_DOMAIN: &[u8] = b"pearl-sol:deposit:v1";
pub const WITHDRAW_DOMAIN: &[u8] = b"pearl-sol:withdraw:v1";

/// `domain || nonce u64 LE || recipient [32] || amount_spl u64 LE` (SPEC 6).
pub fn bridge_message(domain: &[u8], nonce: u64, recipient: &Pubkey, amount_spl: u64) -> Vec<u8> {
    let mut m = Vec::with_capacity(domain.len() + 48);
    m.extend_from_slice(domain);
    m.extend_from_slice(&nonce.to_le_bytes());
    m.extend_from_slice(recipient.as_ref());
    m.extend_from_slice(&amount_spl.to_le_bytes());
    m
}

fn check_signer_set(signers: &[Pubkey], threshold: u8) -> Result<()> {
    require!(!signers.is_empty() && signers.len() <= MAX_SIGNERS, EscrowError::BadSignerSet);
    require!(threshold >= 1 && threshold as usize <= signers.len(), EscrowError::BadThreshold);
    for (i, s) in signers.iter().enumerate() {
        require!(!signers[..i].contains(s), EscrowError::BadSignerSet);
    }
    Ok(())
}

#[program]
pub mod escrow {
    use super::*;

    /// Creates the config PDA. The payer becomes admin. The escrow PDA itself
    /// comes from genesis (or `--account` on localnet) and is not created here.
    pub fn initialize(ctx: Context<Initialize>, signers: Vec<Pubkey>, threshold: u8) -> Result<()> {
        check_signer_set(&signers, threshold)?;
        let c = &mut ctx.accounts.config;
        c.admin = ctx.accounts.admin.key();
        c.withdraw_nonce = 0;
        c.threshold = threshold;
        c.bump = ctx.bumps.config;
        c.escrow_bump = ctx.bumps.escrow;
        c.signers = signers;
        Ok(())
    }

    pub fn set_signers(ctx: Context<AdminOnly>, signers: Vec<Pubkey>, threshold: u8) -> Result<()> {
        check_signer_set(&signers, threshold)?;
        let c = &mut ctx.accounts.config;
        c.signers = signers;
        c.threshold = threshold;
        emit!(SignersChanged { signers: c.signers.clone(), threshold });
        Ok(())
    }

    pub fn set_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
        ctx.accounts.config.admin = new_admin;
        Ok(())
    }

    /// Pays `amount_spl x 1000` lamports from the escrow to `recipient` for token
    /// chain deposit `nonce`. Needs `threshold` signer signatures over the
    /// SPEC section 6 deposit message in ed25519 precompile instruction(s).
    pub fn unlock(ctx: Context<Unlock>, nonce: u64, recipient: Pubkey, amount_spl: u64) -> Result<()> {
        require!(amount_spl > 0, EscrowError::ZeroAmount);
        require_keys_eq!(ctx.accounts.recipient.key(), recipient, EscrowError::WrongRecipient);
        let lamports = amount_spl.checked_mul(LAMPORTS_PER_SPL_UNIT).ok_or(EscrowError::Overflow)?;

        let config = &ctx.accounts.config;
        let msg = bridge_message(DEPOSIT_DOMAIN, nonce, &recipient, amount_spl);
        let n = sigverify::count_signers(&ctx.accounts.instructions.to_account_info(), &msg, &config.signers)?;
        require!(n >= config.threshold as usize, EscrowError::NotEnoughSignatures);

        let escrow = ctx.accounts.escrow.to_account_info();
        require!(escrow.lamports() >= lamports, EscrowError::EscrowEmpty);
        escrow.sub_lamports(lamports)?;
        ctx.accounts.recipient.to_account_info().add_lamports(lamports)?;

        let slot = Clock::get()?.slot;
        let p = &mut ctx.accounts.processed;
        p.source_chain = CHAIN_TOKEN;
        p.nonce = nonce;
        p.recipient = recipient;
        p.amount = amount_spl;
        p.slot = slot;
        p.bump = ctx.bumps.processed;

        emit!(UnlockEvent { nonce, recipient, amount_spl, slot });
        Ok(())
    }

    /// Locks `amount_lamports` (a multiple of 1000) into the escrow for
    /// `solana_recipient` and writes a `Withdrawal` the relayer releases on Solana.
    pub fn lock(ctx: Context<Lock>, amount_lamports: u64, solana_recipient: Pubkey) -> Result<()> {
        require!(amount_lamports > 0, EscrowError::ZeroAmount);
        require!(amount_lamports % LAMPORTS_PER_SPL_UNIT == 0, EscrowError::NotMultipleOf1000);

        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.sender.to_account_info(),
                    to: ctx.accounts.escrow.to_account_info(),
                },
            ),
            amount_lamports,
        )?;

        let config = &mut ctx.accounts.config;
        let nonce = config.withdraw_nonce;
        config.withdraw_nonce = nonce.checked_add(1).ok_or(EscrowError::Overflow)?;

        let amount_spl = amount_lamports / LAMPORTS_PER_SPL_UNIT;
        let slot = Clock::get()?.slot;
        let w = &mut ctx.accounts.withdrawal;
        w.nonce = nonce;
        w.sender = ctx.accounts.sender.key();
        w.recipient = solana_recipient;
        w.amount = amount_spl;
        w.slot = slot;
        w.bump = ctx.bumps.withdrawal;

        emit!(WithdrawalEvent { nonce, sender: w.sender, recipient: solana_recipient, amount_spl, slot });
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + Config::INIT_SPACE, seeds = [CONFIG_SEED], bump)]
    pub config: Account<'info, Config>,
    /// CHECK: the genesis-funded escrow PDA; only its address is recorded.
    #[account(seeds = [ESCROW_SEED], bump)]
    pub escrow: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ EscrowError::NotAdmin)]
    pub config: Account<'info, Config>,
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct Unlock<'info> {
    /// Relayer (or anyone holding the signatures); pays for the Processed PDA.
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: program-owned, data-less escrow PDA holding the bridged supply.
    #[account(mut, seeds = [ESCROW_SEED], bump = config.escrow_bump, owner = crate::ID)]
    pub escrow: UncheckedAccount<'info>,
    /// CHECK: any account; must equal the signed recipient (checked in the handler).
    #[account(mut)]
    pub recipient: UncheckedAccount<'info>,
    /// `init` fails if this deposit was already unlocked: the replay guard.
    #[account(
        init,
        payer = payer,
        space = 8 + Processed::INIT_SPACE,
        seeds = [PROCESSED_SEED, &[CHAIN_TOKEN], &nonce.to_le_bytes()],
        bump,
    )]
    pub processed: Account<'info, Processed>,
    /// CHECK: address-checked instructions sysvar.
    #[account(address = ix_sysvar::ID)]
    pub instructions: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Lock<'info> {
    #[account(mut)]
    pub sender: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: program-owned, data-less escrow PDA.
    #[account(mut, seeds = [ESCROW_SEED], bump = config.escrow_bump, owner = crate::ID)]
    pub escrow: UncheckedAccount<'info>,
    #[account(
        init,
        payer = sender,
        space = 8 + Withdrawal::INIT_SPACE,
        seeds = [WITHDRAWAL_SEED, &config.withdraw_nonce.to_le_bytes()],
        bump,
    )]
    pub withdrawal: Account<'info, Withdrawal>,
    pub system_program: Program<'info, System>,
}

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey,
    /// Nonce the next withdrawal gets (= number of withdrawals so far).
    pub withdraw_nonce: u64,
    pub threshold: u8,
    pub bump: u8,
    pub escrow_bump: u8,
    #[max_len(MAX_SIGNERS)]
    pub signers: Vec<Pubkey>,
}

#[account]
#[derive(InitSpace)]
pub struct Withdrawal {
    pub nonce: u64,
    pub sender: Pubkey,
    /// Solana wallet that receives the SPL (its token account owner).
    pub recipient: Pubkey,
    /// SPL base units (lamports / 1000).
    pub amount: u64,
    pub slot: u64,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Processed {
    pub source_chain: u8,
    pub nonce: u64,
    pub recipient: Pubkey,
    /// SPL base units.
    pub amount: u64,
    pub slot: u64,
    pub bump: u8,
}

#[event]
pub struct UnlockEvent {
    pub nonce: u64,
    pub recipient: Pubkey,
    pub amount_spl: u64,
    pub slot: u64,
}

#[event]
pub struct WithdrawalEvent {
    pub nonce: u64,
    pub sender: Pubkey,
    pub recipient: Pubkey,
    pub amount_spl: u64,
    pub slot: u64,
}

#[event]
pub struct SignersChanged {
    pub signers: Vec<Pubkey>,
    pub threshold: u8,
}

#[error_code]
pub enum EscrowError {
    #[msg("signer set must be 1..=10 distinct keys")]
    BadSignerSet,
    #[msg("threshold must be between 1 and the number of signers")]
    BadThreshold,
    #[msg("only the admin may do this")]
    NotAdmin,
    #[msg("amount must be greater than zero")]
    ZeroAmount,
    #[msg("arithmetic overflow")]
    Overflow,
    #[msg("recipient account does not match the signed recipient")]
    WrongRecipient,
    #[msg("not enough valid signer signatures over this message")]
    NotEnoughSignatures,
    #[msg("lock amount must be a multiple of 1000 lamports")]
    NotMultipleOf1000,
    #[msg("escrow balance too low")]
    EscrowEmpty,
}
