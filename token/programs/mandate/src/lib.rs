//! Cadre delegated trading accounts ("mandate", docs/TRADING.md, ECOSYSTEM 2.5, owner decision D7).
//!
//! A mandate is a trading account the USER owns. Its funds sit in a system-owned vault PDA
//! `["vault", mandate]` that only this program can sign for:
//! - only the user can deposit, withdraw (SOL or tokens, any time, any amount, also while
//!   paused, revoked, expired or with the global kill switch on), change limits, rotate or
//!   revoke the agent;
//! - the agent key can only call `swap`: a PumpSwap buy or sell between the vault's SOL and
//!   the vault's own token account, inside the limits stored here, checked on chain before
//!   and after the CPI (input fell by at most `amount_in`, output rose by at least `min_out`);
//! - fees: a performance fee only on realized profit in SOL above a high-water mark, measured
//!   by the permissionless `settle`; `protocol_bps` of every fee (1,000 bps by D7) goes as plain
//!   SOL to the storefront buyback vault, which the existing crank spends on CADRE.
//!
//! Profit base (no oracle, no marks): `pnl = equity + withdrawn + fees_paid - deposited`, with
//! `equity` = the vault's lamports above its rent floor. Only SOL can be deposited, so a token
//! position never counts as profit until the agent sells it back into SOL.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::{program::invoke_signed, system_instruction};

pub mod pump;
use pump::*;

#[cfg(not(feature = "devnet"))]
declare_id!("A2piwF1KFbYzMJo99QWSpjYM9fJUbCb7qvEdsA7YRGco");
#[cfg(feature = "devnet")]
declare_id!("BG5WkxZyYwWXTY2YSAJGhwL7JcxChRXx8UqHEAMAC5yc");

pub const CONFIG_SEED: &[u8] = b"mconfig";
pub const MANDATE_SEED: &[u8] = b"mandate";
pub const VAULT_SEED: &[u8] = b"vault";
/// Rent floor of a 0-byte system account; the vault never goes below it.
pub const VAULT_RENT: u64 = 890_880;
pub const MAX_MINTS: usize = 4;
pub const MAX_PROTOCOL_BPS: u16 = 5_000;
pub const MAX_PERF_BPS: u16 = 5_000;

pub const VENUE_PUMPSWAP: u8 = 1;
pub const SIDE_BUY: u8 = 0;
pub const SIDE_SELL: u8 = 1;

#[program]
pub mod mandate {
    use super::*;

    /// One-time global config: admin, protocol share, buyback vault (the storefront's ["buyback"] PDA).
    pub fn init_config(ctx: Context<InitConfig>, protocol_bps: u16, buyback_vault: Pubkey) -> Result<()> {
        require!(protocol_bps <= MAX_PROTOCOL_BPS, MandateError::BadConfig);
        let c = &mut ctx.accounts.config;
        c.admin = ctx.accounts.admin.key();
        c.protocol_bps = protocol_bps;
        c.buyback_vault = buyback_vault;
        c.swaps_paused = false;
        c.bump = ctx.bumps.config;
        Ok(())
    }

    /// Admin: protocol share, buyback vault, the global swap kill switch, a new admin.
    /// Withdrawals are never affected by anything here.
    pub fn set_config(ctx: Context<SetConfig>, p: ConfigParams) -> Result<()> {
        let c = &mut ctx.accounts.config;
        if let Some(b) = p.protocol_bps {
            require!(b <= MAX_PROTOCOL_BPS, MandateError::BadConfig);
            c.protocol_bps = b;
        }
        if let Some(v) = p.buyback_vault {
            c.buyback_vault = v;
        }
        if let Some(s) = p.swaps_paused {
            c.swaps_paused = s;
        }
        if let Some(a) = p.admin {
            c.admin = a;
        }
        emit!(ConfigSet { admin: c.admin, protocol_bps: c.protocol_bps, buyback_vault: c.buyback_vault, swaps_paused: c.swaps_paused });
        Ok(())
    }

    /// The user opens a trading account for `agent` with `limits` and fee `terms`, and funds the
    /// vault's rent floor plus `deposit` lamports.
    pub fn open(ctx: Context<Open>, mandate_id: u64, agent: Pubkey, fee_to: Pubkey, limits: Limits, terms: Terms, deposit: u64) -> Result<()> {
        limits.check()?;
        require!(terms.perf_fee_bps <= MAX_PERF_BPS, MandateError::BadTerms);
        let m = &mut ctx.accounts.mandate;
        m.user = ctx.accounts.user.key();
        m.agent = agent;
        m.fee_to = fee_to;
        m.mandate_id = mandate_id;
        m.bump = ctx.bumps.mandate;
        m.vault_bump = ctx.bumps.vault;
        m.limits = limits;
        m.terms = terms;
        m.opened_ts = Clock::get()?.unix_timestamp;
        m.window_start = m.opened_ts;
        let vault = ctx.accounts.vault.to_account_info();
        let user = ctx.accounts.user.to_account_info();
        let system = ctx.accounts.system_program.to_account_info();
        let need = VAULT_RENT.saturating_sub(vault.lamports());
        anchor_lang::solana_program::program::invoke(&system_instruction::transfer(user.key, vault.key, need.saturating_add(deposit)), &[user, vault, system])?;
        m.deposited = deposit;
        emit!(Opened { mandate: m.key(), user: m.user, agent, fee_to, mandate_id, deposit, limits: m.limits.clone(), terms: m.terms.clone() });
        Ok(())
    }

    /// The user adds SOL. Only SOL can enter a mandate (the profit base is SOL).
    pub fn deposit(ctx: Context<UserOp>, lamports: u64) -> Result<()> {
        require!(lamports > 0, MandateError::Zero);
        let a = &ctx.accounts;
        anchor_lang::solana_program::program::invoke(
            &system_instruction::transfer(a.user.key, a.vault.key, lamports),
            &[a.user.to_account_info(), a.vault.to_account_info(), a.system_program.to_account_info()],
        )?;
        let m = &mut ctx.accounts.mandate;
        m.deposited = m.deposited.checked_add(lamports).ok_or(MandateError::Overflow)?;
        emit!(Deposited { mandate: m.key(), lamports, equity: equity(&ctx.accounts.vault.to_account_info()) });
        Ok(())
    }

    /// The user takes SOL out. Never blocked: not by pause, revoke, expiry or the kill switch.
    pub fn withdraw_sol(ctx: Context<UserOp>, lamports: u64) -> Result<()> {
        let vault = ctx.accounts.vault.to_account_info();
        require!(lamports > 0 && lamports <= equity(&vault), MandateError::Insufficient);
        let m = &ctx.accounts.mandate;
        let mk = m.key();
        let seeds: &[&[u8]] = &[VAULT_SEED, mk.as_ref(), &[m.vault_bump]];
        invoke_signed(
            &system_instruction::transfer(vault.key, ctx.accounts.user.key, lamports),
            &[vault.clone(), ctx.accounts.user.to_account_info(), ctx.accounts.system_program.to_account_info()],
            &[seeds],
        )?;
        let m = &mut ctx.accounts.mandate;
        m.withdrawn = m.withdrawn.checked_add(lamports).ok_or(MandateError::Overflow)?;
        emit!(Withdrawn { mandate: mk, mint: Pubkey::default(), amount: lamports, equity: equity(&vault) });
        Ok(())
    }

    /// The user takes tokens out of the vault's token account into their own (created if
    /// missing, paid by the user). Never blocked.
    pub fn withdraw_token(ctx: Context<WithdrawToken>, amount: u64) -> Result<()> {
        let a = &ctx.accounts;
        let tp = a.token_program.key();
        require!(tp == TOKEN_ID || tp == TOKEN_2022_ID, MandateError::BadAccount);
        require_keys_eq!(*a.mint.owner, tp, MandateError::BadAccount);
        require_keys_eq!(a.vault_ata.key(), ata(&a.vault.key(), &a.mint.key(), &tp), MandateError::BadAccount);
        require_keys_eq!(a.user_ata.key(), ata(&a.user.key(), &a.mint.key(), &tp), MandateError::BadAccount);
        require!(amount > 0 && amount <= token_amount(&a.vault_ata)?, MandateError::Insufficient);
        let mk = a.mandate.key();
        let seeds: &[&[u8]] = &[VAULT_SEED, mk.as_ref(), &[a.mandate.vault_bump]];
        ata_create_idempotent(
            &a.associated_token_program,
            &a.user.to_account_info(),
            &a.user_ata,
            &a.user.to_account_info(),
            &a.mint,
            &a.system_program.to_account_info(),
            &a.token_program,
            &[],
        )?;
        let dec = mint_decimals(&a.mint)?;
        transfer_checked(&a.token_program, &a.vault_ata, &a.mint, &a.user_ata, &a.vault.to_account_info(), amount, dec, seeds)?;
        emit!(Withdrawn { mandate: mk, mint: a.mint.key(), amount, equity: equity(&a.vault.to_account_info()) });
        Ok(())
    }

    /// The user replaces the limits (pausing is `limits.paused = true`).
    pub fn set_limits(ctx: Context<UserOnly>, limits: Limits) -> Result<()> {
        limits.check()?;
        let m = &mut ctx.accounts.mandate;
        m.limits = limits;
        emit!(LimitsSet { mandate: m.key(), limits: m.limits.clone() });
        Ok(())
    }

    /// The user rotates the agent key; the old key is dead from this instruction on.
    pub fn set_agent(ctx: Context<UserOnly>, agent: Pubkey) -> Result<()> {
        let m = &mut ctx.accounts.mandate;
        m.agent = agent;
        m.revoked = false;
        emit!(AgentSet { mandate: m.key(), agent });
        Ok(())
    }

    /// The user revokes the agent: no key can swap any more; the account stays the user's.
    pub fn revoke(ctx: Context<UserOnly>) -> Result<()> {
        let m = &mut ctx.accounts.mandate;
        m.revoked = true;
        m.agent = Pubkey::default();
        emit!(Revoked { mandate: m.key() });
        Ok(())
    }

    /// Anyone pays the rent of the vault's token account for an allowed mint (before a buy).
    pub fn add_mint(ctx: Context<AddMint>) -> Result<()> {
        let a = &ctx.accounts;
        let tp = a.token_program.key();
        require!(tp == TOKEN_ID || tp == TOKEN_2022_ID, MandateError::BadAccount);
        require_keys_eq!(*a.mint.owner, tp, MandateError::BadAccount);
        require!(a.mandate.limits.allows(&a.mint.key()), MandateError::MintNotAllowed);
        require_keys_eq!(a.vault_ata.key(), ata(&a.vault.key(), &a.mint.key(), &tp), MandateError::BadAccount);
        ata_create_idempotent(
            &a.associated_token_program,
            &a.payer.to_account_info(),
            &a.vault_ata,
            &a.vault.to_account_info(),
            &a.mint,
            &a.system_program.to_account_info(),
            &a.token_program,
            &[],
        )
    }

    /// The agent trades: PumpSwap `buy_exact_quote_in_v2` (side 0, `amount_in` lamports for at
    /// least `min_out` tokens) or `sell_v2` (side 1, `amount_in` tokens for at least `min_out`
    /// lamports) on the mint's canonical pool, with the vault as the trader. The 17 PumpSwap v2
    /// accounts come as remaining accounts in IDL order.
    pub fn swap<'info>(ctx: Context<'_, '_, 'info, 'info, Swap<'info>>, side: u8, amount_in: u64, min_out: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let cfg = &ctx.accounts.config;
        let mk = ctx.accounts.mandate.key();
        {
            let m = &ctx.accounts.mandate;
            require!(!cfg.swaps_paused, MandateError::SwapsPaused);
            require!(!m.revoked && m.agent != Pubkey::default(), MandateError::Revoked);
            require_keys_eq!(ctx.accounts.agent.key(), m.agent, MandateError::NotAgent);
            require!(!m.limits.paused, MandateError::Paused);
            require!(m.limits.expiry_ts == 0 || now < m.limits.expiry_ts, MandateError::Expired);
            require!(m.limits.venues & (1 << VENUE_PUMPSWAP) != 0, MandateError::VenueNotAllowed);
            require!(side == SIDE_BUY || side == SIDE_SELL, MandateError::BadSide);
            require!(amount_in > 0 && min_out > 0, MandateError::Zero);
        }
        let r = ctx.remaining_accounts;
        require!(r.len() == V2_ACCOUNTS, MandateError::BadAccount);
        let vault = ctx.accounts.vault.to_account_info();
        require_keys_eq!(*r[I_PROGRAM].key, PUMP_AMM_ID, MandateError::BadVenue);
        require_keys_eq!(*r[I_USER].key, vault.key(), MandateError::BadAccount);
        let base_mint = *r[I_BASE_MINT].key;
        require!(ctx.accounts.mandate.limits.allows(&base_mint), MandateError::MintNotAllowed);
        require_keys_eq!(*r[I_POOL].key, canonical_pool(&base_mint), MandateError::BadPool);
        let pool = read_pool(&r[I_POOL])?;
        require_keys_eq!(pool.base_mint, base_mint, MandateError::BadPool);
        require_keys_eq!(pool.quote_mint, WSOL_MINT, MandateError::BadPool);
        require_keys_eq!(*r[I_QUOTE_MINT].key, WSOL_MINT, MandateError::BadPool);
        require_keys_eq!(pool.base_vault, *r[I_POOL_BASE].key, MandateError::BadPool);
        require_keys_eq!(pool.quote_vault, *r[I_POOL_QUOTE].key, MandateError::BadPool);
        let btp = *r[I_BASE_TOKEN_PROGRAM].key;
        require!(btp == TOKEN_ID || btp == TOKEN_2022_ID, MandateError::BadAccount);
        require_keys_eq!(*r[I_QUOTE_TOKEN_PROGRAM].key, TOKEN_ID, MandateError::BadAccount);
        require_keys_eq!(*r[I_USER_BASE].key, ata(&vault.key(), &base_mint, &btp), MandateError::BadAccount);
        require_keys_eq!(*r[I_USER_QUOTE].key, ata(&vault.key(), &WSOL_MINT, &TOKEN_ID), MandateError::BadAccount);
        require_keys_eq!(*r[I_SYSTEM].key, anchor_lang::system_program::ID, MandateError::BadAccount);

        // pool depth and the program's own fee-free quote
        let base_res = token_amount(&r[I_POOL_BASE])?;
        let quote_res = token_amount(&r[I_POOL_QUOTE])?
            .saturating_sub(pool.protocol_fees)
            .saturating_sub(pool.creator_fees)
            .saturating_add(pool.virtual_quote);
        let (quote, sol_value) = if side == SIDE_BUY {
            (cp_out(quote_res, base_res, amount_in), amount_in)
        } else {
            let q = cp_out(base_res, quote_res, amount_in);
            (q, q)
        };
        {
            let l = &ctx.accounts.mandate.limits;
            require!(quote_res >= l.min_pool_quote_reserve, MandateError::ThinPool);
            require!((sol_value as u128) * 10_000 <= (quote_res as u128) * (l.max_trade_share_bps as u128), MandateError::TradeTooLarge);
            require!(sol_value <= l.max_in_per_trade, MandateError::TradeTooLarge);
            let floor = (quote as u128 * (10_000 - l.max_slippage_bps.min(10_000)) as u128 / 10_000) as u64;
            require!(min_out >= floor.max(1), MandateError::SlippageTooWide);
        }
        // the rolling window (Squads spending-limit shape)
        {
            let m = &mut ctx.accounts.mandate;
            if now >= m.window_start.saturating_add(m.limits.window_secs as i64) {
                m.window_start = now;
                m.window_used = 0;
                m.window_trades = 0;
            }
            m.window_used = m.window_used.checked_add(sol_value).ok_or(MandateError::Overflow)?;
            m.window_trades = m.window_trades.checked_add(1).ok_or(MandateError::Overflow)?;
            require!(m.window_used <= m.limits.window_cap, MandateError::WindowCap);
            require!(m.window_trades <= m.limits.max_trades_per_window, MandateError::TooManyTrades);
        }

        let vault_bump = ctx.accounts.mandate.vault_bump;
        let seeds: &[&[u8]] = &[VAULT_SEED, mk.as_ref(), &[vault_bump]];
        let system = ctx.accounts.system_program.to_account_info();
        let token_program = &r[I_QUOTE_TOKEN_PROGRAM];
        let wsol_ata = &r[I_USER_QUOTE];
        let base_ata = &r[I_USER_BASE];
        // the base account must already exist (add_mint): then nothing but the trade moves the vault's SOL
        require!(base_ata.owner == r[I_BASE_TOKEN_PROGRAM].key && base_ata.data_len() >= 165, MandateError::NoTokenAccount);
        let lamports_before = vault.lamports();
        let base_before = token_amount(base_ata)?;
        ata_create_idempotent(&ctx.accounts.associated_token_program, &vault, wsol_ata, &vault, &r[I_QUOTE_MINT], &system, token_program, &[seeds])?;
        let mut data = if side == SIDE_BUY { IX_AMM_BUY_EXACT_QUOTE_IN_V2.to_vec() } else { IX_AMM_SELL_V2.to_vec() };
        if side == SIDE_BUY {
            require!(equity(&vault) >= amount_in.saturating_add(wsol_rent()), MandateError::Insufficient);
            invoke_signed(&system_instruction::transfer(vault.key, wsol_ata.key, amount_in), &[vault.clone(), wsol_ata.clone(), system.clone()], &[seeds])?;
            sync_native(token_program, wsol_ata)?;
        }
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        let metas: Vec<Meta> = r.iter().enumerate().map(|(i, a)| (a, V2_WRITABLE[i], i == I_USER)).collect();
        call(&r[I_PROGRAM], data, &metas[..], &[seeds])?;
        // unwrap everything: the wSOL account closes back into the vault (rent + any SOL)
        close_account(token_program, wsol_ata, &vault, &vault, seeds)?;
        let lamports_after = vault.lamports();
        let base_after = token_amount(base_ata)?;
        let (spent, got) = if side == SIDE_BUY {
            require!(lamports_after <= lamports_before, MandateError::Accounting);
            (lamports_before.saturating_sub(lamports_after), base_after.saturating_sub(base_before))
        } else {
            require!(base_after <= base_before, MandateError::Accounting);
            (base_before - base_after, lamports_after.saturating_sub(lamports_before))
        };
        // the post-CPI checks: input fell by at most amount_in, output rose by at least min_out,
        // and on a sell the vault's SOL did not fall
        require!(spent <= amount_in, MandateError::Overspent);
        require!(got >= min_out, MandateError::MinOut);
        if side == SIDE_SELL {
            require!(lamports_after >= lamports_before, MandateError::Accounting);
        } else {
            require!(base_after >= base_before, MandateError::Accounting);
        }
        require!(lamports_after >= VAULT_RENT, MandateError::Accounting);

        let m = &mut ctx.accounts.mandate;
        m.trades = m.trades.saturating_add(1);
        m.volume = m.volume.saturating_add(sol_value);
        let slot = Clock::get()?.slot;
        emit!(MandateSwap {
            mandate: mk,
            agent: m.agent,
            venue: VENUE_PUMPSWAP,
            side,
            mint: base_mint,
            amount_in: spent,
            amount_out: got,
            quote,
            pool_base_reserve: base_res,
            pool_quote_reserve: quote_res,
            equity_after: equity(&vault),
            slot,
        });
        Ok(())
    }

    /// Permissionless: charge the performance fee on realized SOL profit above the high-water
    /// mark, at most once per `settle_interval_secs`. `protocol_bps` of the fee goes to the
    /// buyback vault, the rest to the agent's `fee_to`.
    pub fn settle(ctx: Context<Settle>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let cfg = &ctx.accounts.config;
        require_keys_eq!(ctx.accounts.buyback_vault.key(), cfg.buyback_vault, MandateError::BadAccount);
        require_keys_eq!(ctx.accounts.fee_to.key(), ctx.accounts.mandate.fee_to, MandateError::BadAccount);
        let m = &ctx.accounts.mandate;
        require!(m.last_settle == 0 || now >= m.last_settle.saturating_add(m.terms.settle_interval_secs as i64), MandateError::TooSoon);
        let vault = ctx.accounts.vault.to_account_info();
        let eq = equity(&vault);
        let pnl = pnl_of(eq, m);
        let (fee, proto) = if pnl > m.hwm {
            let gain = (pnl - m.hwm) as u128;
            let fee = ((gain * m.terms.perf_fee_bps as u128) / 10_000).min(eq as u128) as u64;
            let proto = ((fee as u128 * cfg.protocol_bps as u128) / 10_000) as u64;
            (fee, proto)
        } else {
            (0, 0)
        };
        let mk = m.key();
        let seeds: &[&[u8]] = &[VAULT_SEED, mk.as_ref(), &[m.vault_bump]];
        let system = ctx.accounts.system_program.to_account_info();
        if proto > 0 {
            invoke_signed(&system_instruction::transfer(vault.key, ctx.accounts.buyback_vault.key, proto), &[vault.clone(), ctx.accounts.buyback_vault.to_account_info(), system.clone()], &[seeds])?;
        }
        if fee > proto {
            invoke_signed(&system_instruction::transfer(vault.key, ctx.accounts.fee_to.key, fee - proto), &[vault.clone(), ctx.accounts.fee_to.to_account_info(), system.clone()], &[seeds])?;
        }
        let m = &mut ctx.accounts.mandate;
        m.fees_agent = m.fees_agent.saturating_add(fee - proto);
        m.fees_protocol = m.fees_protocol.saturating_add(proto);
        // after paying, pnl is unchanged (fees count back in), so the new mark is the old pnl
        if pnl > m.hwm {
            m.hwm = pnl;
        }
        m.last_settle = now;
        emit!(Settled { mandate: mk, pnl, hwm: m.hwm, fee_agent: fee - proto, fee_protocol: proto, equity_after: equity(&vault) });
        Ok(())
    }
}

/// The vault's SOL above its rent floor.
pub fn equity(vault: &AccountInfo) -> u64 {
    vault.lamports().saturating_sub(VAULT_RENT)
}

/// Rent of a 165-byte token account (the temporary wSOL account a trade opens and closes).
pub fn wsol_rent() -> u64 {
    2_039_280
}

/// Realized SOL PnL: equity + withdrawn + fees paid - deposited.
pub fn pnl_of(equity: u64, m: &Mandate) -> i64 {
    let v = equity as i128 + m.withdrawn as i128 + m.fees_agent as i128 + m.fees_protocol as i128 - m.deposited as i128;
    v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq, InitSpace)]
pub struct Limits {
    /// Bit i = venue i allowed (bit 1 = PumpSwap).
    pub venues: u8,
    /// true: any mint with a canonical PumpSwap SOL pool; false: only `mints[..n_mints]`.
    pub any_mint: bool,
    pub n_mints: u8,
    pub mints: [Pubkey; MAX_MINTS],
    /// Largest trade, in lamports of SOL (a sell is valued at the program's own quote).
    pub max_in_per_trade: u64,
    /// SOL traded per window and trades per window.
    pub window_cap: u64,
    pub window_secs: u32,
    pub max_trades_per_window: u16,
    /// `min_out` must be within this of the program's fee-free quote.
    pub max_slippage_bps: u16,
    /// Refuse pools with less SOL than this.
    pub min_pool_quote_reserve: u64,
    /// One trade at most this share of the pool's SOL side.
    pub max_trade_share_bps: u16,
    /// 0 = no expiry.
    pub expiry_ts: i64,
    pub paused: bool,
}

impl Limits {
    pub fn check(&self) -> Result<()> {
        require!(self.n_mints as usize <= MAX_MINTS, MandateError::BadLimits);
        require!(self.any_mint || self.n_mints > 0, MandateError::BadLimits);
        require!(self.max_slippage_bps <= 10_000 && self.max_trade_share_bps <= 10_000, MandateError::BadLimits);
        require!(self.window_secs > 0, MandateError::BadLimits);
        Ok(())
    }
    pub fn allows(&self, mint: &Pubkey) -> bool {
        *mint != WSOL_MINT && (self.any_mint || self.mints[..self.n_mints as usize].contains(mint))
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq, InitSpace)]
pub struct Terms {
    /// Share of realized profit above the high-water mark (the agent sets it, the user signs it).
    pub perf_fee_bps: u16,
    pub settle_interval_secs: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default)]
pub struct ConfigParams {
    pub protocol_bps: Option<u16>,
    pub buyback_vault: Option<Pubkey>,
    pub swaps_paused: Option<bool>,
    pub admin: Option<Pubkey>,
}

#[account]
#[derive(InitSpace)]
pub struct MConfig {
    pub admin: Pubkey,
    pub protocol_bps: u16,
    pub buyback_vault: Pubkey,
    /// Global kill switch: stops every swap; deposits, withdrawals and settles still work.
    pub swaps_paused: bool,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Mandate {
    pub user: Pubkey,
    pub agent: Pubkey,
    pub fee_to: Pubkey,
    pub mandate_id: u64,
    pub bump: u8,
    pub vault_bump: u8,
    pub revoked: bool,
    pub limits: Limits,
    pub terms: Terms,
    pub opened_ts: i64,
    pub window_start: i64,
    pub window_used: u64,
    pub window_trades: u16,
    pub deposited: u64,
    pub withdrawn: u64,
    pub fees_agent: u64,
    pub fees_protocol: u64,
    pub hwm: i64,
    pub last_settle: i64,
    pub trades: u64,
    pub volume: u64,
}

#[derive(Accounts)]
pub struct InitConfig<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + MConfig::INIT_SPACE, seeds = [CONFIG_SEED], bump)]
    pub config: Account<'info, MConfig>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SetConfig<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ MandateError::Unauthorized)]
    pub config: Account<'info, MConfig>,
}

#[derive(Accounts)]
#[instruction(mandate_id: u64)]
pub struct Open<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(init, payer = user, space = 8 + Mandate::INIT_SPACE, seeds = [MANDATE_SEED, user.key().as_ref(), &mandate_id.to_le_bytes()], bump)]
    pub mandate: Box<Account<'info, Mandate>>,
    #[account(mut, seeds = [VAULT_SEED, mandate.key().as_ref()], bump)]
    pub vault: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UserOp<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut, seeds = [MANDATE_SEED, user.key().as_ref(), &mandate.mandate_id.to_le_bytes()], bump = mandate.bump, has_one = user @ MandateError::Unauthorized)]
    pub mandate: Box<Account<'info, Mandate>>,
    #[account(mut, seeds = [VAULT_SEED, mandate.key().as_ref()], bump = mandate.vault_bump)]
    pub vault: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UserOnly<'info> {
    pub user: Signer<'info>,
    #[account(mut, seeds = [MANDATE_SEED, user.key().as_ref(), &mandate.mandate_id.to_le_bytes()], bump = mandate.bump, has_one = user @ MandateError::Unauthorized)]
    pub mandate: Box<Account<'info, Mandate>>,
}

#[derive(Accounts)]
pub struct WithdrawToken<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(seeds = [MANDATE_SEED, user.key().as_ref(), &mandate.mandate_id.to_le_bytes()], bump = mandate.bump, has_one = user @ MandateError::Unauthorized)]
    pub mandate: Box<Account<'info, Mandate>>,
    #[account(mut, seeds = [VAULT_SEED, mandate.key().as_ref()], bump = mandate.vault_bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: owner = token_program (checked).
    pub mint: UncheckedAccount<'info>,
    /// CHECK: = ATA(vault, mint) (checked).
    #[account(mut)]
    pub vault_ata: UncheckedAccount<'info>,
    /// CHECK: = ATA(user, mint) (checked), created if missing.
    #[account(mut)]
    pub user_ata: UncheckedAccount<'info>,
    /// CHECK: SPL Token or Token-2022 (checked).
    pub token_program: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = ATA_ID)]
    pub associated_token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AddMint<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub mandate: Box<Account<'info, Mandate>>,
    #[account(seeds = [VAULT_SEED, mandate.key().as_ref()], bump = mandate.vault_bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: owner = token_program (checked).
    pub mint: UncheckedAccount<'info>,
    /// CHECK: = ATA(vault, mint) (checked).
    #[account(mut)]
    pub vault_ata: UncheckedAccount<'info>,
    /// CHECK: SPL Token or Token-2022 (checked).
    pub token_program: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = ATA_ID)]
    pub associated_token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Swap<'info> {
    pub agent: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, MConfig>>,
    #[account(mut, seeds = [MANDATE_SEED, mandate.user.as_ref(), &mandate.mandate_id.to_le_bytes()], bump = mandate.bump)]
    pub mandate: Box<Account<'info, Mandate>>,
    #[account(mut, seeds = [VAULT_SEED, mandate.key().as_ref()], bump = mandate.vault_bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: address.
    #[account(address = ATA_ID)]
    pub associated_token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Settle<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, MConfig>>,
    #[account(mut, seeds = [MANDATE_SEED, mandate.user.as_ref(), &mandate.mandate_id.to_le_bytes()], bump = mandate.bump)]
    pub mandate: Box<Account<'info, Mandate>>,
    #[account(mut, seeds = [VAULT_SEED, mandate.key().as_ref()], bump = mandate.vault_bump)]
    pub vault: SystemAccount<'info>,
    /// CHECK: = config.buyback_vault (checked).
    #[account(mut)]
    pub buyback_vault: UncheckedAccount<'info>,
    /// CHECK: = mandate.fee_to (checked).
    #[account(mut)]
    pub fee_to: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[event]
pub struct ConfigSet {
    pub admin: Pubkey,
    pub protocol_bps: u16,
    pub buyback_vault: Pubkey,
    pub swaps_paused: bool,
}
#[event]
pub struct Opened {
    pub mandate: Pubkey,
    pub user: Pubkey,
    pub agent: Pubkey,
    pub fee_to: Pubkey,
    pub mandate_id: u64,
    pub deposit: u64,
    pub limits: Limits,
    pub terms: Terms,
}
#[event]
pub struct Deposited {
    pub mandate: Pubkey,
    pub lamports: u64,
    pub equity: u64,
}
#[event]
pub struct Withdrawn {
    pub mandate: Pubkey,
    /// default (all zero) = SOL
    pub mint: Pubkey,
    pub amount: u64,
    pub equity: u64,
}
#[event]
pub struct LimitsSet {
    pub mandate: Pubkey,
    pub limits: Limits,
}
#[event]
pub struct AgentSet {
    pub mandate: Pubkey,
    pub agent: Pubkey,
}
#[event]
pub struct Revoked {
    pub mandate: Pubkey,
}
#[event]
pub struct MandateSwap {
    pub mandate: Pubkey,
    pub agent: Pubkey,
    pub venue: u8,
    pub side: u8,
    pub mint: Pubkey,
    pub amount_in: u64,
    pub amount_out: u64,
    pub quote: u64,
    pub pool_base_reserve: u64,
    pub pool_quote_reserve: u64,
    pub equity_after: u64,
    pub slot: u64,
}
#[event]
pub struct Settled {
    pub mandate: Pubkey,
    pub pnl: i64,
    pub hwm: i64,
    pub fee_agent: u64,
    pub fee_protocol: u64,
    pub equity_after: u64,
}

#[error_code]
pub enum MandateError {
    #[msg("only the admin")]
    Unauthorized,
    #[msg("bad config value")]
    BadConfig,
    #[msg("bad limits")]
    BadLimits,
    #[msg("bad terms")]
    BadTerms,
    #[msg("bad account")]
    BadAccount,
    #[msg("amount is zero")]
    Zero,
    #[msg("arithmetic overflow")]
    Overflow,
    #[msg("not enough in the vault")]
    Insufficient,
    #[msg("swaps are paused by the global kill switch")]
    SwapsPaused,
    #[msg("the agent was revoked")]
    Revoked,
    #[msg("signer is not this mandate's agent")]
    NotAgent,
    #[msg("the user paused this mandate")]
    Paused,
    #[msg("the mandate expired")]
    Expired,
    #[msg("venue not allowed by the limits")]
    VenueNotAllowed,
    #[msg("venue program is not PumpSwap")]
    BadVenue,
    #[msg("side must be 0 (buy) or 1 (sell)")]
    BadSide,
    #[msg("mint not allowed by the limits")]
    MintNotAllowed,
    #[msg("not the mint's canonical PumpSwap SOL pool")]
    BadPool,
    #[msg("pool SOL reserve below the limit")]
    ThinPool,
    #[msg("trade larger than the limits allow")]
    TradeTooLarge,
    #[msg("min_out is looser than max_slippage_bps from the program's quote")]
    SlippageTooWide,
    #[msg("window cap reached")]
    WindowCap,
    #[msg("too many trades in this window")]
    TooManyTrades,
    #[msg("the vault's token account for this mint does not exist (add_mint first)")]
    NoTokenAccount,
    #[msg("trade moved more than amount_in")]
    Overspent,
    #[msg("trade returned less than min_out")]
    MinOut,
    #[msg("a balance moved the wrong way")]
    Accounting,
    #[msg("settle interval not reached")]
    TooSoon,
}
