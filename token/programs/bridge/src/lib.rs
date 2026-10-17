//! Token-chain (Solana) side of the pearl-sol bridge. SPEC section 2.3.
//!
//! - `deposit` locks the pump Token-2022 SPL in the vault ATA (owned by the
//!   config PDA), writes a `Deposit` record under a global nonce and emits an
//!   event. The relayer unlocks `amount x 1000` lamports on the agent chain.
//! - `release` pays SPL out of the vault for a `Withdrawal` from the agent
//!   chain, once M-of-N configured signers have signed the SPEC section 6
//!   withdraw message through the ed25519 precompile in the same transaction.
//!   A `Processed` PDA per (source chain, nonce) blocks replay.
//! - `release_proof` (M2, SPEC 5.3) pays the same withdrawal from a Commonware MMR
//!   inclusion proof of its withdrawal-log entry against `withdraw_root` of a FINAL
//!   light-client header (token/programs/lightclient). Same `Processed` replay guard,
//!   so a withdrawal is paid once whichever path is used.
//! - `refund_deposit` (owner decision 2026-10-17) returns a Deposit's tokens from the
//!   vault to the original buyer when the agent chain will never unlock it (a held-back
//!   storefront order). M-of-N configured signers sign `b"pearl-sol:refund:v1" ||
//!   nonce || buyer || amount_spl` through the ed25519 precompile. It writes a
//!   `Processed` PDA `["processed", [CHAIN_TOKEN], deposit nonce]` (the refund record):
//!   a deposit is refunded at most once, and relayers never unlock a deposit that has
//!   one. The chain cannot see Solana, so "never both unlocked and refunded" rests on
//!   the signers signing at most one of the two messages per nonce (SPEC 5.1).
//! - `set_release_modes` (admin) enables the signer path, the proof path or both and
//!   names the light-client program.
//! - Signer set, threshold and admin are admin-editable.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::sysvar::instructions as ix_sysvar;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

pub mod sigverify;

declare_id!("7LgyoRiKjNXD1AkMQUePb7TzTvRewEXKKLVzXB5vEjc8");

pub const MAX_SIGNERS: usize = 10;
/// Chain ids used in `Processed` seeds.
pub const CHAIN_TOKEN: u8 = 0;
pub const CHAIN_AGENT: u8 = 1;

pub const CONFIG_SEED: &[u8] = b"config";
pub const DEPOSIT_SEED: &[u8] = b"deposit";
pub const PROCESSED_SEED: &[u8] = b"processed";

/// `Config.release_modes` bits.
pub const MODE_SIGNERS: u8 = 1;
pub const MODE_PROOF: u8 = 2;
/// Withdrawal log entry: `nonce u64 || from [32] || solana_recipient [32] || amount_spl u64 || height u64`.
pub const WITHDRAWAL_ENTRY_LEN: usize = 88;

pub const DEPOSIT_DOMAIN: &[u8] = b"pearl-sol:deposit:v1";
pub const WITHDRAW_DOMAIN: &[u8] = b"pearl-sol:withdraw:v1";
/// `refund_deposit`: `domain || deposit nonce u64 LE || buyer [32] || amount_spl u64 LE`.
pub const REFUND_DOMAIN: &[u8] = b"pearl-sol:refund:v1";

/// Refund record of Solana deposit `nonce`: `Processed ["processed", [CHAIN_TOKEN], nonce]`.
pub fn refund_record_address(nonce: u64) -> Pubkey {
    Pubkey::find_program_address(&[PROCESSED_SEED, &[CHAIN_TOKEN], &nonce.to_le_bytes()], &ID).0
}

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
    require!(!signers.is_empty() && signers.len() <= MAX_SIGNERS, BridgeError::BadSignerSet);
    require!(threshold >= 1 && threshold as usize <= signers.len(), BridgeError::BadThreshold);
    for (i, s) in signers.iter().enumerate() {
        require!(!signers[..i].contains(s), BridgeError::BadSignerSet);
    }
    Ok(())
}

#[program]
pub mod bridge {
    use super::*;

    /// Creates the config PDA and the vault ATA for `mint`. The payer becomes admin.
    pub fn initialize(ctx: Context<Initialize>, signers: Vec<Pubkey>, threshold: u8) -> Result<()> {
        check_signer_set(&signers, threshold)?;
        let c = &mut ctx.accounts.config;
        c.admin = ctx.accounts.admin.key();
        c.mint = ctx.accounts.mint.key();
        c.vault = ctx.accounts.vault.key();
        c.token_program = ctx.accounts.token_program.key();
        c.deposit_nonce = 0;
        c.threshold = threshold;
        c.bump = ctx.bumps.config;
        c.signers = signers;
        c.release_modes = MODE_SIGNERS;
        c.light_client = Pubkey::default();
        Ok(())
    }

    /// Which release paths are open (bit 1 signers, bit 2 proof; at least one) and the
    /// light-client program whose final headers the proof path trusts.
    pub fn set_release_modes(ctx: Context<AdminOnly>, modes: u8, light_client: Pubkey) -> Result<()> {
        require!(modes != 0 && modes & !(MODE_SIGNERS | MODE_PROOF) == 0, BridgeError::BadModes);
        require!(modes & MODE_PROOF == 0 || light_client != Pubkey::default(), BridgeError::BadModes);
        let c = &mut ctx.accounts.config;
        c.release_modes = modes;
        c.light_client = light_client;
        emit!(ReleaseModesChanged { modes, light_client });
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

    /// Locks `amount` SPL base units in the vault for `agent_recipient` on the agent chain.
    /// `sender` may be a PDA signing through CPI (the reserve's `fund_mining`).
    pub fn deposit(ctx: Context<DepositIx>, amount: u64, agent_recipient: Pubkey) -> Result<()> {
        require!(amount > 0, BridgeError::ZeroAmount);
        // amount x 1000 lamports must fit in u64 on the agent side.
        require!(amount.checked_mul(1000).is_some(), BridgeError::Overflow);

        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.sender_token.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.vault.to_account_info(),
                    authority: ctx.accounts.sender.to_account_info(),
                },
            ),
            amount,
            ctx.accounts.mint.decimals,
        )?;

        let config = &mut ctx.accounts.config;
        let nonce = config.deposit_nonce;
        config.deposit_nonce = nonce.checked_add(1).ok_or(BridgeError::Overflow)?;

        let slot = Clock::get()?.slot;
        let d = &mut ctx.accounts.deposit;
        d.nonce = nonce;
        d.sender = ctx.accounts.sender.key();
        d.recipient = agent_recipient;
        d.amount = amount;
        d.slot = slot;
        d.bump = ctx.bumps.deposit;

        emit!(DepositEvent { nonce, sender: d.sender, recipient: agent_recipient, amount, slot });
        Ok(())
    }

    /// Pays `amount` SPL from the vault to `recipient` for agent-chain withdrawal `nonce`.
    /// The transaction must carry ed25519 precompile instruction(s) with at least
    /// `threshold` configured signers over the SPEC section 6 withdraw message.
    pub fn release(ctx: Context<Release>, nonce: u64, recipient: Pubkey, amount: u64) -> Result<()> {
        require!(amount > 0, BridgeError::ZeroAmount);
        require_keys_eq!(ctx.accounts.recipient_token.owner, recipient, BridgeError::WrongRecipient);

        let config = &ctx.accounts.config;
        require!(config.release_modes & MODE_SIGNERS != 0, BridgeError::ModeDisabled);
        let msg = bridge_message(WITHDRAW_DOMAIN, nonce, &recipient, amount);
        let n = sigverify::count_signers(&ctx.accounts.instructions.to_account_info(), &msg, &config.signers)?;
        require!(n >= config.threshold as usize, BridgeError::NotEnoughSignatures);

        let bump = [config.bump];
        let seeds: &[&[u8]] = &[CONFIG_SEED, &bump];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.recipient_token.to_account_info(),
                    authority: ctx.accounts.config.to_account_info(),
                },
                &[seeds],
            ),
            amount,
            ctx.accounts.mint.decimals,
        )?;

        let slot = Clock::get()?.slot;
        let p = &mut ctx.accounts.processed;
        p.source_chain = CHAIN_AGENT;
        p.nonce = nonce;
        p.recipient = recipient;
        p.amount = amount;
        p.slot = slot;
        p.bump = ctx.bumps.processed;

        emit!(ReleaseEvent { nonce, recipient, amount, slot });
        Ok(())
    }

    /// Returns Solana deposit `nonce` (its full amount) from the vault to `buyer`, who
    /// must own `buyer_token`. The transaction must carry ed25519 precompile
    /// instruction(s) with at least `threshold` configured signers over
    /// `bridge_message(REFUND_DOMAIN, nonce, buyer, deposit.amount)`. Open whatever
    /// `release_modes` says (inbound stays attested in M2). The refund record PDA is
    /// the replay guard; signers must never sign this for a nonce they signed an
    /// unlock for (and vice versa).
    pub fn refund_deposit(ctx: Context<RefundDeposit>, nonce: u64, buyer: Pubkey) -> Result<()> {
        let d = &ctx.accounts.deposit;
        require!(d.nonce == nonce, BridgeError::WrongDeposit);
        let amount = d.amount;
        require!(amount > 0, BridgeError::ZeroAmount);
        require_keys_eq!(ctx.accounts.buyer_token.owner, buyer, BridgeError::WrongRecipient);

        let config = &ctx.accounts.config;
        let msg = bridge_message(REFUND_DOMAIN, nonce, &buyer, amount);
        let n = sigverify::count_signers(&ctx.accounts.instructions.to_account_info(), &msg, &config.signers)?;
        require!(n >= config.threshold as usize, BridgeError::NotEnoughSignatures);

        let bump = [config.bump];
        let seeds: &[&[u8]] = &[CONFIG_SEED, &bump];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.buyer_token.to_account_info(),
                    authority: ctx.accounts.config.to_account_info(),
                },
                &[seeds],
            ),
            amount,
            ctx.accounts.mint.decimals,
        )?;

        let slot = Clock::get()?.slot;
        let p = &mut ctx.accounts.refund;
        p.source_chain = CHAIN_TOKEN;
        p.nonce = nonce;
        p.recipient = buyer;
        p.amount = amount;
        p.slot = slot;
        p.bump = ctx.bumps.refund;

        emit!(RefundEvent { nonce, buyer, amount, slot });
        Ok(())
    }

    /// Proof mode: pays withdrawal `nonce` given its 88-byte log `entry`, its
    /// `location` in the withdrawal log and a codec-encoded Commonware MMR proof
    /// (node RPC `getWithdrawalProof`) against `withdraw_root` of `lc_header`, a
    /// light-client header marked final. Anyone may submit; funds go to the entry's
    /// `solana_recipient` only.
    pub fn release_proof(ctx: Context<ReleaseProof>, nonce: u64, entry: Vec<u8>, location: u64, proof: Vec<u8>) -> Result<()> {
        let config = &ctx.accounts.config;
        require!(config.release_modes & MODE_PROOF != 0, BridgeError::ModeDisabled);
        require_keys_eq!(*ctx.accounts.lc_header.owner, config.light_client, BridgeError::BadHeader);
        let (_, header) = {
            let d = ctx.accounts.lc_header.try_borrow_data()?;
            lightclient::read_final_header(&d).ok_or(error!(BridgeError::BadHeader))?
        };
        require!(entry.len() == WITHDRAWAL_ENTRY_LEN, BridgeError::BadProof);
        let e_nonce = u64::from_le_bytes(entry[0..8].try_into().unwrap());
        let recipient = Pubkey::new_from_array(entry[40..72].try_into().unwrap());
        let amount = u64::from_le_bytes(entry[72..80].try_into().unwrap());
        require!(e_nonce == nonce, BridgeError::BadProof);
        require!(amount > 0, BridgeError::ZeroAmount);
        require_keys_eq!(ctx.accounts.recipient_token.owner, recipient, BridgeError::WrongRecipient);
        let root: [u8; 32] = pearl_verify_sbf::header::Header(&header).withdraw_root();
        let mut element = [0u8; 1 + 10 + WITHDRAWAL_ENTRY_LEN];
        let n = pearl_verify_sbf::mmr::append_element(&entry, &mut element);
        pearl_verify_sbf::mmr::verify_inclusion(lightclient::sha256, &proof, &element[..n], location, &root).map_err(|e| {
            msg!("withdrawal proof: {:?}", e);
            error!(BridgeError::BadProof)
        })?;

        let bump = [config.bump];
        let seeds: &[&[u8]] = &[CONFIG_SEED, &bump];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.vault.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.recipient_token.to_account_info(),
                    authority: ctx.accounts.config.to_account_info(),
                },
                &[seeds],
            ),
            amount,
            ctx.accounts.mint.decimals,
        )?;

        let slot = Clock::get()?.slot;
        let p = &mut ctx.accounts.processed;
        p.source_chain = CHAIN_AGENT;
        p.nonce = nonce;
        p.recipient = recipient;
        p.amount = amount;
        p.slot = slot;
        p.bump = ctx.bumps.processed;

        emit!(ReleaseEvent { nonce, recipient, amount, slot });
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + Config::INIT_SPACE, seeds = [CONFIG_SEED], bump)]
    pub config: Account<'info, Config>,
    #[account(mint::token_program = token_program)]
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(
        init,
        payer = admin,
        associated_token::mint = mint,
        associated_token::authority = config,
        associated_token::token_program = token_program,
    )]
    pub vault: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ BridgeError::NotAdmin)]
    pub config: Account<'info, Config>,
}

#[derive(Accounts)]
pub struct DepositIx<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub sender: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = mint, has_one = vault, has_one = token_program)]
    pub config: Account<'info, Config>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(mut, token::mint = mint, token::authority = sender, token::token_program = token_program)]
    pub sender_token: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub vault: InterfaceAccount<'info, TokenAccount>,
    #[account(
        init,
        payer = payer,
        space = 8 + Deposit::INIT_SPACE,
        seeds = [DEPOSIT_SEED, &config.deposit_nonce.to_le_bytes()],
        bump,
    )]
    pub deposit: Account<'info, Deposit>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct Release<'info> {
    /// Relayer (or anyone holding the signatures); pays for the Processed PDA.
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = mint, has_one = vault, has_one = token_program)]
    pub config: Account<'info, Config>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(mut)]
    pub vault: InterfaceAccount<'info, TokenAccount>,
    #[account(mut, token::mint = mint, token::token_program = token_program)]
    pub recipient_token: InterfaceAccount<'info, TokenAccount>,
    /// `init` fails if this withdrawal was already released: the replay guard.
    #[account(
        init,
        payer = payer,
        space = 8 + Processed::INIT_SPACE,
        seeds = [PROCESSED_SEED, &[CHAIN_AGENT], &nonce.to_le_bytes()],
        bump,
    )]
    pub processed: Account<'info, Processed>,
    /// CHECK: address-checked instructions sysvar.
    #[account(address = ix_sysvar::ID)]
    pub instructions: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct RefundDeposit<'info> {
    /// Relayer (or anyone holding the signatures); pays for the refund record.
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = mint, has_one = vault, has_one = token_program)]
    pub config: Account<'info, Config>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(mut)]
    pub vault: InterfaceAccount<'info, TokenAccount>,
    #[account(seeds = [DEPOSIT_SEED, &nonce.to_le_bytes()], bump = deposit.bump)]
    pub deposit: Account<'info, Deposit>,
    #[account(mut, token::mint = mint, token::token_program = token_program)]
    pub buyer_token: InterfaceAccount<'info, TokenAccount>,
    /// Refund record; `init` fails on a second refund of the same deposit.
    #[account(
        init,
        payer = payer,
        space = 8 + Processed::INIT_SPACE,
        seeds = [PROCESSED_SEED, &[CHAIN_TOKEN], &nonce.to_le_bytes()],
        bump,
    )]
    pub refund: Account<'info, Processed>,
    /// CHECK: address-checked instructions sysvar.
    #[account(address = ix_sysvar::ID)]
    pub instructions: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct ReleaseProof<'info> {
    /// Anyone; pays for the Processed PDA.
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump, has_one = mint, has_one = vault, has_one = token_program)]
    pub config: Account<'info, Config>,
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(mut)]
    pub vault: InterfaceAccount<'info, TokenAccount>,
    #[account(mut, token::mint = mint, token::token_program = token_program)]
    pub recipient_token: InterfaceAccount<'info, TokenAccount>,
    /// Same replay guard as `release`.
    #[account(
        init,
        payer = payer,
        space = 8 + Processed::INIT_SPACE,
        seeds = [PROCESSED_SEED, &[CHAIN_AGENT], &nonce.to_le_bytes()],
        bump,
    )]
    pub processed: Account<'info, Processed>,
    /// CHECK: a light-client HeaderRecord; owner and final flag checked in the handler.
    pub lc_header: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey,
    pub mint: Pubkey,
    pub vault: Pubkey,
    pub token_program: Pubkey,
    /// Nonce the next deposit gets (= number of deposits so far).
    pub deposit_nonce: u64,
    pub threshold: u8,
    pub bump: u8,
    #[max_len(MAX_SIGNERS)]
    pub signers: Vec<Pubkey>,
    /// Enabled release paths: MODE_SIGNERS | MODE_PROOF (appended in M2; the offsets
    /// above are unchanged for raw readers).
    pub release_modes: u8,
    /// Light-client program whose final headers `release_proof` accepts.
    pub light_client: Pubkey,
}

#[account]
#[derive(InitSpace)]
pub struct Deposit {
    pub nonce: u64,
    pub sender: Pubkey,
    /// Agent-chain recipient of `amount x 1000` lamports.
    pub recipient: Pubkey,
    /// SPL base units (6 dp).
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
    pub amount: u64,
    pub slot: u64,
    pub bump: u8,
}

#[event]
pub struct DepositEvent {
    pub nonce: u64,
    pub sender: Pubkey,
    pub recipient: Pubkey,
    pub amount: u64,
    pub slot: u64,
}

#[event]
pub struct ReleaseEvent {
    pub nonce: u64,
    pub recipient: Pubkey,
    pub amount: u64,
    pub slot: u64,
}

#[event]
pub struct RefundEvent {
    pub nonce: u64,
    pub buyer: Pubkey,
    pub amount: u64,
    pub slot: u64,
}

#[event]
pub struct ReleaseModesChanged {
    pub modes: u8,
    pub light_client: Pubkey,
}

#[event]
pub struct SignersChanged {
    pub signers: Vec<Pubkey>,
    pub threshold: u8,
}

#[error_code]
pub enum BridgeError {
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
    #[msg("recipient token account is not owned by the signed recipient")]
    WrongRecipient,
    #[msg("not enough valid signer signatures over this message")]
    NotEnoughSignatures,
    #[msg("release modes must be a non-empty subset of signers|proof (proof needs a light client)")]
    BadModes,
    #[msg("this release path is disabled")]
    ModeDisabled,
    #[msg("not a final light-client header")]
    BadHeader,
    #[msg("withdrawal proof does not verify")]
    BadProof,
    #[msg("deposit record does not match the nonce")]
    WrongDeposit,
}
