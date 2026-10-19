//! pearl-sol mining reserve (SPEC 2.1, 2.2).
//!
//! - The coin's pump.fun `creator` is this program's data-less, system-owned
//!   PDA `["creator"]`. Pump pays curve creator fees to it as lamports and
//!   PumpSwap pays them as wSOL into its wSOL ATA; buys made with it as `user`
//!   move SOL out with System transfers, which only work from a system-owned
//!   account, so the PDA never holds data.
//! - The mining reserve is the PDA's Token-2022 ATA for the coin.
//! - `launch` does create_v2 + the dev buy in one instruction, so the dev
//!   tokens land in the reserve directly.
//! - `crank_buyback` (curve) / `crank_buyback_amm` (PumpSwap, after
//!   graduation): sweep and collect creator fees, then buy the coin into the
//!   reserve, bounded by a per-call cap, a minimum interval, the caller's
//!   `min_tokens_out` and `max_slippage_bps` against the program's own
//!   pre-trade quote read from the curve or pool.
//! - `fund_mining` moves reserve tokens into the bridge vault with the
//!   agent-chain `mining_pool` as recipient (bridge `deposit` CPI). This is the
//!   only way tokens leave the reserve. PERMISSIONLESS and rate limited (owner
//!   decision 2026-10-18, SPEC 2.2 "fund_mining"): anyone may call it, bounded by
//!   `max_fund_per_call`, `fund_min_interval_secs` and `fund_max_per_window` per
//!   `fund_window_secs`; the admin can pause it (`fund_paused`) and edit the limits
//!   (`set_fund_limits`) but cannot move reserve tokens anywhere else: `mining_pool`
//!   and `bridge_program` freeze at the first funding.
//! - Every limit is admin-editable through `update_config` / `set_fund_limits`.
//! - `migrate_config` (admin) grows a config written before the fund-limit fields.
//!
//! Risk (SPEC 2.2): pump's admin can reassign a coin's creator (community
//! takeover). Future fees would then go elsewhere; tokens already in the
//! reserve stay here.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::{hash::hash, pubkey, system_instruction, program::invoke_signed};

pub mod pump;
use pump::*;

// Localnet id = keys/localnet/reserve-keypair.json; the public devnet build (`--features devnet`,
// docs/DEVNET.md) uses its own program keypair (keys/devnet/, not committed).
#[cfg(not(feature = "devnet"))]
declare_id!("4FkQcoCMJdRpZW7bK3wnaMoLjYdWaJBDB8xTUGskLY6p");
#[cfg(feature = "devnet")]
declare_id!("8m1RBPiiscHWpXRDj4NevSFMN68tbgCC8f4hu6N4hVzi");

pub const CONFIG_SEED: &[u8] = b"config";
/// fund_mining defaults, PROVISIONAL (2026-10-19): 10M tokens (6 dp) per call, one call
/// per hour, at most 20M tokens per 24 h window (2% of the 1B supply a day).
pub const DEFAULT_FUND_MAX_PER_CALL: u64 = 10_000_000_000_000;
pub const DEFAULT_FUND_MIN_INTERVAL_SECS: i64 = 3_600;
pub const DEFAULT_FUND_WINDOW_SECS: i64 = 86_400;
pub const DEFAULT_FUND_MAX_PER_WINDOW: u64 = 20_000_000_000_000;
/// Config layout: 1 = with the fund-limit fields (2026-10-19).
pub const CONFIG_LAYOUT: u8 = 1;
pub const CREATOR_SEED: &[u8] = b"creator";
pub const MAYHEM_ID: Pubkey = pubkey!("MAyhSmzXzV1pTf7LsNkrNwkWKTo4ougAJ1PPg47MD4e");

#[program]
pub mod reserve {
    use super::*;

    /// Creates the config. The signer becomes admin. `mint` stays unset until
    /// `launch` (or `update_config` for a coin launched elsewhere).
    pub fn initialize(ctx: Context<Initialize>, params: ConfigParams) -> Result<()> {
        let c = &mut ctx.accounts.config;
        c.admin = ctx.accounts.admin.key();
        c.bump = ctx.bumps.config;
        c.creator_bump = ctx.bumps.creator;
        c.max_buy_per_call = 1_000_000_000;
        c.min_buy = 1_000_000;
        c.min_interval_secs = 600;
        c.max_slippage_bps = 300;
        c.sol_floor = 5_000_000;
        c.max_fund_per_call = DEFAULT_FUND_MAX_PER_CALL;
        c.set_fund_defaults();
        c.apply(params)?;
        Ok(())
    }

    /// Admin: change any limit, the cranker/funder keys, the bridge target or
    /// (before any reserve tokens exist) the mint.
    pub fn update_config(ctx: Context<AdminOnly>, params: ConfigParams) -> Result<()> {
        ctx.accounts.config.apply(params)?;
        emit!(ConfigUpdated { config: ctx.accounts.config.key() });
        Ok(())
    }

    /// Admin: fund_mining pause and rate limits. Emits every resulting value.
    pub fn set_fund_limits(ctx: Context<AdminOnly>, p: FundLimitParams) -> Result<()> {
        let c = &mut ctx.accounts.config;
        c.apply_fund_limits(p)?;
        emit!(FundLimitsUpdated {
            config: c.key(),
            paused: c.fund_paused,
            max_per_call: c.max_fund_per_call,
            min_interval_secs: c.fund_min_interval_secs,
            window_secs: c.fund_window_secs,
            max_per_window: c.fund_max_per_window,
        });
        Ok(())
    }

    /// Admin: grow a config written before the fund-limit fields (layout 0) to the current
    /// size and set those fields to the defaults. No-op on a current config.
    pub fn migrate_config(ctx: Context<MigrateConfig>) -> Result<()> {
        let info = ctx.accounts.config.to_account_info();
        require_keys_eq!(*info.owner, ID, ReserveError::BadAccount);
        let new_len = 8 + Config::INIT_SPACE;
        let old_len = info.data_len();
        {
            let d = info.try_borrow_data()?;
            require!(old_len >= 40 && &d[..8] == Config::DISCRIMINATOR, ReserveError::BadAccount);
            require!(d[8..40] == ctx.accounts.admin.key().to_bytes(), ReserveError::Unauthorized);
        }
        if old_len >= new_len {
            return Ok(());
        }
        let rent = Rent::get()?.minimum_balance(new_len);
        if info.lamports() < rent {
            anchor_lang::solana_program::program::invoke(
                &system_instruction::transfer(ctx.accounts.admin.key, info.key, rent - info.lamports()),
                &[ctx.accounts.admin.to_account_info(), info.clone(), ctx.accounts.system_program.to_account_info()],
            )?;
        }
        #[allow(deprecated)]
        info.realloc(new_len, true)?;
        let mut c = {
            let d = info.try_borrow_data()?;
            Config::try_deserialize(&mut &d[..])?
        };
        c.set_fund_defaults();
        let mut d = info.try_borrow_mut_data()?;
        c.try_serialize(&mut &mut d[..])?;
        emit!(ConfigUpdated { config: info.key() });
        Ok(())
    }

    pub fn set_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
        require_keys_neq!(new_admin, Pubkey::default(), ReserveError::BadConfig);
        ctx.accounts.config.admin = new_admin;
        Ok(())
    }

    /// Admin: pump `create_v2` with the creator PDA as creator, then a dev buy
    /// of `dev_buy_lamports` (funded by the admin) with the creator PDA as the
    /// buyer, so the tokens land in the reserve ATA. One launch per config.
    pub fn launch(
        ctx: Context<Launch>,
        name: String,
        symbol: String,
        uri: String,
        dev_buy_lamports: u64,
        min_tokens_out: u64,
    ) -> Result<()> {
        let a = &ctx.accounts;
        require_keys_eq!(a.config.mint, Pubkey::default(), ReserveError::AlreadyLaunched);
        require_keys_eq!(a.reserve_ata.key(), ata(&a.creator.key(), &a.mint.key(), &TOKEN_2022_ID), ReserveError::BadAccount);
        let bump = a.config.creator_bump;
        let seeds: &[&[u8]] = &[CREATOR_SEED, &[bump]];

        // Fund the creator PDA: the dev buy plus the floor (rent + volume accumulator).
        let need = dev_buy_lamports.checked_add(a.config.sol_floor).ok_or(ReserveError::Overflow)?;
        let have = a.creator.lamports();
        if have < need {
            anchor_lang::solana_program::program::invoke(
                &system_instruction::transfer(a.admin.key, a.creator.key, need - have),
                &[a.admin.to_account_info(), a.creator.to_account_info(), a.system_program.to_account_info()],
            )?;
        }

        let mut data = IX_CREATE_V2.to_vec();
        for s in [&name, &symbol, &uri] {
            data.extend_from_slice(&(s.len() as u32).to_le_bytes());
            data.extend_from_slice(s.as_bytes());
        }
        data.extend_from_slice(a.creator.key.as_ref());
        data.push(0); // is_mayhem_mode
        data.push(0); // is_cashback_enabled
        data.extend_from_slice(&0u64.to_le_bytes()); // creator_fee_bps (ignored for SOL pairs)
        data.push(0); // is_holder_reward: must stay off, fees fund the reserve
        let admin = a.admin.to_account_info();
        let mint = a.mint.to_account_info();
        call(
            &a.pump_program,
            data,
            &[
                (&mint, true, true),
                (&a.mint_authority, false, false),
                (&a.bonding_curve, true, false),
                (&a.curve_base_vault, true, false),
                (&a.pump_global, false, false),
                (&admin, true, true),
                (&a.system_program.to_account_info(), false, false),
                (&a.token_2022_program, false, false),
                (&a.associated_token_program, false, false),
                (&a.mayhem_program, true, false),
                (&a.mayhem_global_params, false, false),
                (&a.mayhem_sol_vault, true, false),
                (&a.mayhem_state, true, false),
                (&a.mayhem_token_vault, true, false),
                (&a.pump_event_authority, false, false),
                (&a.pump_program, false, false),
            ],
            &[],
        )?;
        let creator = a.creator.to_account_info();
        ata_create_idempotent(
            &a.associated_token_program,
            &admin,
            &a.reserve_ata,
            &creator,
            &mint,
            &a.system_program.to_account_info(),
            &a.token_2022_program,
        )?;
        let curve = read_curve(&a.bonding_curve)?;
        require_keys_eq!(curve.creator, a.creator.key(), ReserveError::BadAccount);

        let got = if dev_buy_lamports > 0 {
            let before = token_amount(&a.reserve_ata)?;
            curve_buy(
                CurveBuyAccounts {
                    pump_program: &a.pump_program,
                    pump_global: &a.pump_global,
                    mint: &mint,
                    wsol_mint: &a.wsol_mint,
                    token_2022_program: &a.token_2022_program,
                    token_program: &a.token_program,
                    bonding_curve: &a.bonding_curve,
                    curve_base_vault: &a.curve_base_vault,
                    curve_quote_vault: &a.curve_quote_vault,
                    creator: &creator,
                    reserve_ata: &a.reserve_ata,
                    creator_wsol: &a.creator_wsol,
                    user_volume_accumulator: &a.user_volume_accumulator,
                    fee_config: &a.fee_config,
                    buyback_fee_recipient: &a.buyback_fee_recipient,
                    system_program: &a.system_program.to_account_info(),
                    pump_event_authority: &a.pump_event_authority,
                },
                dev_buy_lamports,
                min_tokens_out.max(1),
                seeds,
            )?;
            let got = token_amount(&a.reserve_ata)? - before;
            require!(got >= min_tokens_out, ReserveError::MinTokensOut);
            got
        } else {
            0
        };
        let c = &mut ctx.accounts.config;
        c.mint = ctx.accounts.mint.key();
        c.total_bought = c.total_bought.saturating_add(got);
        c.total_spent = c.total_spent.saturating_add(dev_buy_lamports);
        emit!(Launched { mint: c.mint, creator: ctx.accounts.creator.key(), dev_buy_lamports, tokens: got });
        Ok(())
    }

    /// Permissionless: curve `sweep_creator_fee` + `collect_creator_fee_v2`
    /// into the creator PDA, no buy. Works before and after graduation (old
    /// curve fees); `crank_buyback_amm` then spends the lamports.
    pub fn collect_curve_fees(ctx: Context<CurveCrank>) -> Result<()> {
        ctx.accounts.check(false)?;
        ctx.accounts.sweep_and_collect()?;
        Ok(())
    }

    /// Curve buyback: sweep + collect creator fees, then `buy_exact_quote_in_v3`
    /// with the creator PDA as buyer, spending
    /// `min(lamports - sol_floor, max_buy_per_call, max_sol_in)`.
    pub fn crank_buyback(ctx: Context<CurveCrank>, max_sol_in: u64, min_tokens_out: u64) -> Result<()> {
        ctx.accounts.check(true)?;
        let now = Clock::get()?.unix_timestamp;
        ctx.accounts.config.check_interval(now)?;
        ctx.accounts.sweep_and_collect()?;

        let a = &ctx.accounts;
        let curve = read_curve(&a.bonding_curve)?;
        require!(!curve.complete, ReserveError::CurveComplete);
        let spend = a.config.spend(a.creator.lamports(), max_sol_in)?;
        let quote = cp_quote(curve.virtual_token, curve.virtual_quote, spend);
        let floor_out = slip_floor(quote, a.config.max_slippage_bps);
        let min_out = min_tokens_out.max(floor_out).max(1);

        let creator = a.creator.to_account_info();
        let mint = a.mint.to_account_info();
        let lamports_before = creator.lamports();
        let tokens_before = token_amount(&a.reserve_ata)?;
        let seeds: &[&[u8]] = &[CREATOR_SEED, &[a.config.creator_bump]];
        curve_buy(
            CurveBuyAccounts {
                pump_program: &a.pump_program,
                pump_global: &a.pump_global,
                mint: &mint,
                wsol_mint: &a.wsol_mint,
                token_2022_program: &a.token_2022_program,
                token_program: &a.token_program,
                bonding_curve: &a.bonding_curve,
                curve_base_vault: &a.curve_base_vault,
                curve_quote_vault: &a.curve_quote_vault,
                creator: &creator,
                reserve_ata: &a.reserve_ata,
                creator_wsol: &a.creator_wsol,
                user_volume_accumulator: &a.user_volume_accumulator,
                fee_config: &a.fee_config,
                buyback_fee_recipient: &a.buyback_fee_recipient,
                system_program: &a.system_program.to_account_info(),
                pump_event_authority: &a.pump_event_authority,
            },
            spend,
            min_out,
            seeds,
        )?;
        let spent = lamports_before.saturating_sub(creator.lamports());
        let got = token_amount(&a.reserve_ata)?.saturating_sub(tokens_before);
        let _ = lamports_before;
        require!(creator.lamports() >= Rent::get()?.minimum_balance(0), ReserveError::BelowFloor);
        check_fill(got, spent, spend, quote, min_tokens_out, a.config.max_slippage_bps)?;
        let c = &mut ctx.accounts.config;
        c.record(now, spent, got);
        emit!(Buyback { venue: 0, spent, tokens: got, quote, spend_cap: spend });
        Ok(())
    }

    /// PumpSwap buyback after graduation: PumpSwap `sweep_creator_fee` +
    /// `collect_coin_creator_fee` (wSOL into the PDA's wSOL ATA), wrap any
    /// native lamports above `sol_floor` (old curve fees), then
    /// `buy_exact_quote_in_v2` on the canonical pool into the reserve.
    pub fn crank_buyback_amm(ctx: Context<AmmCrank>, max_sol_in: u64, min_tokens_out: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        {
            let a = &ctx.accounts;
            a.config.check_cranker(&a.cranker.key())?;
            require_keys_neq!(a.config.mint, Pubkey::default(), ReserveError::NotLaunched);
            require_keys_eq!(a.reserve_ata.key(), ata(&a.creator.key(), &a.mint.key(), &TOKEN_2022_ID), ReserveError::BadAccount);
            require_keys_eq!(a.creator_wsol.key(), ata(&a.creator.key(), &WSOL_MINT, &TOKEN_ID), ReserveError::BadAccount);
            require_keys_eq!(a.bonding_curve.key(), pump::bonding_curve(&a.mint.key()), ReserveError::BadAccount);
            require!(read_curve(&a.bonding_curve)?.complete, ReserveError::CurveNotComplete);
            require_keys_eq!(a.pool.key(), canonical_pool(&a.mint.key()), ReserveError::BadAccount);
            let pool = read_pool(&a.pool)?;
            require_keys_eq!(pool.base_mint, a.mint.key(), ReserveError::BadAccount);
            require_keys_eq!(pool.quote_mint, WSOL_MINT, ReserveError::BadAccount);
            require_keys_eq!(pool.base_vault, a.pool_base_vault.key(), ReserveError::BadAccount);
            require_keys_eq!(pool.quote_vault, a.pool_quote_vault.key(), ReserveError::BadAccount);
            a.config.check_interval(now)?;
        }
        let a = &ctx.accounts;
        let seeds: &[&[u8]] = &[CREATOR_SEED, &[a.config.creator_bump]];
        let creator = a.creator.to_account_info();
        let cranker = a.cranker.to_account_info();
        let system = a.system_program.to_account_info();
        let wsol = &a.wsol_mint;
        ata_create_idempotent(&a.associated_token_program, &cranker, &a.creator_wsol, &creator, wsol, &system, &a.token_program)?;

        let pool = read_pool(&a.pool)?;
        if pool.coin_creator == a.creator.key() {
            // Pool fees kept in the vault (v2 trades) -> creator vault ATA -> our wSOL ATA.
            call(
                &a.amm_program,
                IX_SWEEP_CREATOR_FEE.to_vec(),
                &[
                    (&cranker, true, true),
                    (&a.amm_global_config, false, false),
                    (&a.pool, true, false),
                    (wsol, false, false),
                    (&a.token_program, false, false),
                    (&a.pool_quote_vault, true, false),
                    (&a.coin_creator_vault_authority, false, false),
                    (&a.coin_creator_vault_ata, true, false),
                    (&system, false, false),
                    (&a.associated_token_program, false, false),
                    (&a.amm_event_authority, false, false),
                    (&a.amm_program, false, false),
                ],
                &[],
            )?;
            call(
                &a.amm_program,
                IX_AMM_COLLECT_COIN_CREATOR_FEE.to_vec(),
                &[
                    (wsol, false, false),
                    (&a.token_program, false, false),
                    (&creator, false, false),
                    (&a.coin_creator_vault_authority, false, false),
                    (&a.coin_creator_vault_ata, true, false),
                    (&a.creator_wsol, true, false),
                    (&a.amm_event_authority, false, false),
                    (&a.amm_program, false, false),
                ],
                &[],
            )?;
        }
        // Wrap native lamports above the floor (curve fees collected earlier).
        let extra = creator.lamports().saturating_sub(a.config.sol_floor);
        if extra > 0 {
            invoke_signed(
                &system_instruction::transfer(creator.key, a.creator_wsol.key, extra),
                &[creator.clone(), a.creator_wsol.to_account_info(), system.clone()],
                &[seeds],
            )?;
            sync_native(&a.token_program, &a.creator_wsol)?;
        }

        let wsol_have = token_amount(&a.creator_wsol)?;
        let spend = a.config.spend_from(wsol_have, max_sol_in)?;
        let pool = read_pool(&a.pool)?;
        let base_res = token_amount(&a.pool_base_vault)?;
        let quote_res = token_amount(&a.pool_quote_vault)?
            .saturating_sub(pool.protocol_fees)
            .saturating_sub(pool.creator_fees)
            .saturating_add(pool.virtual_quote);
        let quote = cp_quote(base_res, quote_res, spend);
        let min_out = min_tokens_out.max(slip_floor(quote, a.config.max_slippage_bps)).max(1);

        let wsol_before = wsol_have;
        let tokens_before = token_amount(&a.reserve_ata)?;
        let lamports_before = creator.lamports();
        let mut data = IX_AMM_BUY_EXACT_QUOTE_IN_V2.to_vec();
        data.extend_from_slice(&spend.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        call(
            &a.amm_program,
            data,
            &[
                (&a.pool, true, false),
                (&creator, true, true),
                (&a.amm_global_config, false, false),
                (&a.mint, false, false),
                (wsol, false, false),
                (&a.reserve_ata, true, false),
                (&a.creator_wsol, true, false),
                (&a.pool_base_vault, true, false),
                (&a.pool_quote_vault, true, false),
                (&a.token_2022_program, false, false),
                (&a.token_program, false, false),
                (&system, false, false),
                (&a.amm_user_volume_accumulator, true, false),
                (&a.amm_fee_config, false, false),
                (&a.amm_buyback_fee_recipient, true, false),
                (&a.amm_event_authority, false, false),
                (&a.amm_program, false, false),
            ],
            &[seeds],
        )?;
        let spent = wsol_before.saturating_sub(token_amount(&a.creator_wsol)?);
        let got = token_amount(&a.reserve_ata)?.saturating_sub(tokens_before);
        // PumpSwap creates the PDA's volume accumulator from its lamports on the
        // first trade; sol_floor covers that, the PDA must stay rent exempt.
        let _ = lamports_before;
        require!(creator.lamports() >= Rent::get()?.minimum_balance(0), ReserveError::BelowFloor);
        check_fill(got, spent, spend, quote, min_tokens_out, a.config.max_slippage_bps)?;
        let c = &mut ctx.accounts.config;
        c.record(now, spent, got);
        emit!(Buyback { venue: 1, spent, tokens: got, quote, spend_cap: spend });
        Ok(())
    }

    /// Permissionless, rate limited: deposit `amount` reserve tokens into the
    /// bridge with `config.mining_pool` (agent chain) as recipient. CPI into
    /// the token-side bridge `deposit(amount, agent_recipient)` (SPEC 2.3),
    /// with the creator PDA as `sender` and the caller as rent `payer`.
    /// Refused while `fund_paused`, above `max_fund_per_call`, inside
    /// `fund_min_interval_secs` of the last funding, or past `fund_max_per_window`
    /// in the current `fund_window_secs` window.
    pub fn fund_mining(ctx: Context<FundMining>, amount: u64) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let a = &ctx.accounts;
        require!(a.config.layout >= CONFIG_LAYOUT, ReserveError::NeedsMigration);
        require!(!a.config.fund_paused, ReserveError::FundPaused);
        require!(amount > 0, ReserveError::ZeroAmount);
        require!(a.config.max_fund_per_call == 0 || amount <= a.config.max_fund_per_call, ReserveError::OverFundCap);
        let window_used = a.config.fund_window_check(now, amount)?;
        require_keys_neq!(a.config.mining_pool, Pubkey::default(), ReserveError::BadConfig);
        require_keys_eq!(a.reserve_ata.key(), ata(&a.creator.key(), &a.mint.key(), &TOKEN_2022_ID), ReserveError::BadAccount);
        let before = token_amount(&a.reserve_ata)?;
        require!(before >= amount, ReserveError::InsufficientReserve);

        let mut data = hash(b"global:deposit").to_bytes()[..8].to_vec();
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(a.config.mining_pool.as_ref());
        let seeds: &[&[u8]] = &[CREATOR_SEED, &[a.config.creator_bump]];
        let authority = a.authority.to_account_info();
        call(
            &a.bridge_program,
            data,
            &[
                (&authority, true, true),
                (&a.creator.to_account_info(), false, true),
                (&a.bridge_config, true, false),
                (&a.mint, false, false),
                (&a.reserve_ata, true, false),
                (&a.bridge_vault, true, false),
                (&a.bridge_deposit, true, false),
                (&a.token_2022_program, false, false),
                (&a.system_program.to_account_info(), false, false),
            ],
            &[seeds],
        )?;
        let after = token_amount(&a.reserve_ata)?;
        require!(before - after == amount, ReserveError::BadAccount);
        let caller = ctx.accounts.authority.key();
        let c = &mut ctx.accounts.config;
        c.total_funded = c.total_funded.saturating_add(amount);
        if c.fund_window_secs > 0 && (c.fund_window_start == 0 || now >= c.fund_window_start.saturating_add(c.fund_window_secs)) {
            c.fund_window_start = now;
        }
        c.fund_window_used = window_used;
        c.fund_last_ts = now;
        c.fund_calls = c.fund_calls.saturating_add(1);
        emit!(MiningFunded {
            amount,
            recipient: c.mining_pool,
            total_funded: c.total_funded,
            caller,
            reserve_left: after,
            window_start: c.fund_window_start,
            window_used,
            calls: c.fund_calls,
        });
        Ok(())
    }
}

fn slip_floor(quote: u64, bps: u16) -> u64 {
    (quote as u128 * (10_000 - bps.min(10_000)) as u128 / 10_000) as u64
}

/// Rejects a fill below the caller's minimum, or one whose realised price
/// (tokens per lamport actually spent) is worse than `max_slippage_bps` from
/// the program's own pre-trade quote for `spend`.
fn check_fill(got: u64, spent: u64, spend: u64, quote: u64, min_tokens_out: u64, bps: u16) -> Result<()> {
    require!(got >= min_tokens_out && got > 0, ReserveError::MinTokensOut);
    require!(spent <= spend, ReserveError::Overspent);
    // got / spent >= quote / spend * (1 - bps)
    let lhs = got as u128 * spend as u128 * 10_000;
    let rhs = quote as u128 * spent as u128 * (10_000 - bps.min(10_000) as u128);
    require!(lhs >= rhs, ReserveError::Slippage);
    Ok(())
}

struct CurveBuyAccounts<'a, 'info> {
    pump_program: &'a AccountInfo<'info>,
    pump_global: &'a AccountInfo<'info>,
    mint: &'a AccountInfo<'info>,
    wsol_mint: &'a AccountInfo<'info>,
    token_2022_program: &'a AccountInfo<'info>,
    token_program: &'a AccountInfo<'info>,
    bonding_curve: &'a AccountInfo<'info>,
    curve_base_vault: &'a AccountInfo<'info>,
    curve_quote_vault: &'a AccountInfo<'info>,
    creator: &'a AccountInfo<'info>,
    reserve_ata: &'a AccountInfo<'info>,
    creator_wsol: &'a AccountInfo<'info>,
    user_volume_accumulator: &'a AccountInfo<'info>,
    fee_config: &'a AccountInfo<'info>,
    buyback_fee_recipient: &'a AccountInfo<'info>,
    system_program: &'a AccountInfo<'info>,
    pump_event_authority: &'a AccountInfo<'info>,
}

fn curve_buy(a: CurveBuyAccounts, spend: u64, min_out: u64, seeds: &[&[u8]]) -> Result<()> {
    let mut data = IX_BUY_EXACT_QUOTE_IN_V3.to_vec();
    data.extend_from_slice(&spend.to_le_bytes());
    data.extend_from_slice(&min_out.to_le_bytes());
    data.push(0); // partial_fill = false
    call(
        a.pump_program,
        data,
        &[
            (a.pump_global, false, false),
            (a.mint, false, false),
            (a.wsol_mint, false, false),
            (a.token_2022_program, false, false),
            (a.token_program, false, false),
            (a.bonding_curve, true, false),
            (a.curve_base_vault, true, false),
            (a.curve_quote_vault, true, false),
            (a.creator, true, true),
            (a.reserve_ata, true, false),
            (a.creator_wsol, true, false),
            (a.user_volume_accumulator, true, false),
            (a.fee_config, false, false),
            (a.buyback_fee_recipient, true, false),
            (a.system_program, false, false),
            (a.pump_event_authority, false, false),
            (a.pump_program, false, false),
        ],
        &[seeds],
    )
}

// ---------------------------------------------------------------- state

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey,
    /// The pump coin. Unset until `launch`.
    pub mint: Pubkey,
    /// If set, only this key may crank; default (zero) = permissionless.
    pub cranker: Pubkey,
    /// UNUSED since fund_mining became permissionless (2026-10-19); kept for the layout.
    pub funder: Pubkey,
    /// Token-side bridge program (token/programs/bridge).
    pub bridge_program: Pubkey,
    /// Agent-chain `mining_pool` PDA, the `agent_recipient` of every deposit. Frozen
    /// (with bridge_program) once total_funded > 0.
    pub mining_pool: Pubkey,
    /// Most lamports one crank may spend.
    pub max_buy_per_call: u64,
    /// Below this the crank fails with NothingToBuy (no dust buys).
    pub min_buy: u64,
    pub min_interval_secs: i64,
    /// Max shortfall of the fill against the fee-free pre-trade quote. Pump's
    /// fees (1.25% on the curve, up to 1.25% on PumpSwap) count against it.
    pub max_slippage_bps: u16,
    /// Lamports the creator PDA keeps (rent + pump volume accumulator rent).
    pub sol_floor: u64,
    /// 0 = no cap.
    pub max_fund_per_call: u64,
    pub last_crank_ts: i64,
    pub total_bought: u64,
    pub total_spent: u64,
    pub total_funded: u64,
    pub bump: u8,
    pub creator_bump: u8,
    // ---- fund limits (appended 2026-10-19; `migrate_config` grows an older config)
    /// CONFIG_LAYOUT once the fields below are set (0 = an unmigrated config).
    pub layout: u8,
    /// Admin pause of fund_mining.
    pub fund_paused: bool,
    /// Minimum seconds between two fund_mining calls.
    pub fund_min_interval_secs: i64,
    /// Rolling window for fund_max_per_window; 0 = no window cap.
    pub fund_window_secs: i64,
    /// Most tokens fund_mining may move per window; 0 = no window cap.
    pub fund_max_per_window: u64,
    pub fund_last_ts: i64,
    pub fund_window_start: i64,
    pub fund_window_used: u64,
    pub fund_calls: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Default)]
pub struct FundLimitParams {
    pub paused: Option<bool>,
    pub max_per_call: Option<u64>,
    pub min_interval_secs: Option<i64>,
    pub window_secs: Option<i64>,
    pub max_per_window: Option<u64>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Default)]
pub struct ConfigParams {
    pub mint: Option<Pubkey>,
    pub cranker: Option<Pubkey>,
    pub funder: Option<Pubkey>,
    pub bridge_program: Option<Pubkey>,
    pub mining_pool: Option<Pubkey>,
    pub max_buy_per_call: Option<u64>,
    pub min_buy: Option<u64>,
    pub min_interval_secs: Option<i64>,
    pub max_slippage_bps: Option<u16>,
    pub sol_floor: Option<u64>,
    pub max_fund_per_call: Option<u64>,
}

impl Config {
    fn apply(&mut self, p: ConfigParams) -> Result<()> {
        if let Some(v) = p.mint {
            self.mint = v;
        }
        if let Some(v) = p.cranker {
            self.cranker = v;
        }
        if let Some(v) = p.funder {
            self.funder = v;
        }
        // The funding destination freezes at the first fund_mining: afterwards the admin
        // key cannot redirect reserve tokens (SPEC 2.2).
        if let Some(v) = p.bridge_program {
            require!(v == self.bridge_program || self.total_funded == 0, ReserveError::DestinationFrozen);
            self.bridge_program = v;
        }
        if let Some(v) = p.mining_pool {
            require!(v == self.mining_pool || self.total_funded == 0, ReserveError::DestinationFrozen);
            self.mining_pool = v;
        }
        if let Some(v) = p.max_buy_per_call {
            self.max_buy_per_call = v;
        }
        if let Some(v) = p.min_buy {
            self.min_buy = v;
        }
        if let Some(v) = p.min_interval_secs {
            require!(v >= 0, ReserveError::BadConfig);
            self.min_interval_secs = v;
        }
        if let Some(v) = p.max_slippage_bps {
            require!(v <= 10_000, ReserveError::BadConfig);
            self.max_slippage_bps = v;
        }
        if let Some(v) = p.sol_floor {
            require!(v >= 890_880, ReserveError::BadConfig);
            self.sol_floor = v;
        }
        if let Some(v) = p.max_fund_per_call {
            self.max_fund_per_call = v;
        }
        Ok(())
    }

    fn set_fund_defaults(&mut self) {
        self.layout = CONFIG_LAYOUT;
        if self.max_fund_per_call == 0 {
            self.max_fund_per_call = DEFAULT_FUND_MAX_PER_CALL;
        }
        self.fund_min_interval_secs = DEFAULT_FUND_MIN_INTERVAL_SECS;
        self.fund_window_secs = DEFAULT_FUND_WINDOW_SECS;
        self.fund_max_per_window = DEFAULT_FUND_MAX_PER_WINDOW;
    }

    fn apply_fund_limits(&mut self, p: FundLimitParams) -> Result<()> {
        require!(self.layout >= CONFIG_LAYOUT, ReserveError::NeedsMigration);
        if let Some(v) = p.paused {
            self.fund_paused = v;
        }
        if let Some(v) = p.max_per_call {
            self.max_fund_per_call = v;
        }
        if let Some(v) = p.min_interval_secs {
            require!(v >= 0, ReserveError::BadConfig);
            self.fund_min_interval_secs = v;
        }
        if let Some(v) = p.window_secs {
            require!(v >= 0, ReserveError::BadConfig);
            self.fund_window_secs = v;
        }
        if let Some(v) = p.max_per_window {
            self.fund_max_per_window = v;
        }
        Ok(())
    }

    /// Checks the interval and the window cap for funding `amount` at `now`; returns the
    /// window's use after it.
    fn fund_window_check(&self, now: i64, amount: u64) -> Result<u64> {
        require!(
            self.fund_last_ts == 0 || now >= self.fund_last_ts.saturating_add(self.fund_min_interval_secs),
            ReserveError::FundTooSoon
        );
        if self.fund_window_secs == 0 || self.fund_max_per_window == 0 {
            return Ok(0);
        }
        let fresh = self.fund_window_start == 0 || now >= self.fund_window_start.saturating_add(self.fund_window_secs);
        let used = if fresh { 0 } else { self.fund_window_used };
        let next = used.checked_add(amount).ok_or(ReserveError::Overflow)?;
        require!(next <= self.fund_max_per_window, ReserveError::OverWindowCap);
        Ok(next)
    }

    fn check_cranker(&self, k: &Pubkey) -> Result<()> {
        require!(self.cranker == Pubkey::default() || self.cranker == *k, ReserveError::Unauthorized);
        Ok(())
    }

    fn check_interval(&self, now: i64) -> Result<()> {
        require!(
            self.last_crank_ts == 0 || now >= self.last_crank_ts.saturating_add(self.min_interval_secs),
            ReserveError::TooSoon
        );
        Ok(())
    }

    /// Lamports to spend from a native balance (keeps `sol_floor`).
    fn spend(&self, lamports: u64, max_sol_in: u64) -> Result<u64> {
        self.spend_from(lamports.saturating_sub(self.sol_floor), max_sol_in)
    }

    fn spend_from(&self, available: u64, max_sol_in: u64) -> Result<u64> {
        let s = available.min(self.max_buy_per_call).min(max_sol_in);
        require!(s >= self.min_buy.max(1), ReserveError::NothingToBuy);
        Ok(s)
    }

    fn record(&mut self, now: i64, spent: u64, got: u64) {
        self.last_crank_ts = now;
        self.total_spent = self.total_spent.saturating_add(spent);
        self.total_bought = self.total_bought.saturating_add(got);
    }
}

// ---------------------------------------------------------------- accounts

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + Config::INIT_SPACE, seeds = [CONFIG_SEED], bump)]
    pub config: Account<'info, Config>,
    /// CHECK: the data-less creator PDA; only its bump is stored.
    #[account(seeds = [CREATOR_SEED], bump)]
    pub creator: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ ReserveError::Unauthorized)]
    pub config: Account<'info, Config>,
}

#[derive(Accounts)]
pub struct MigrateConfig<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    /// CHECK: the reserve config PDA, read raw (an old layout does not deserialize); owner,
    /// discriminator and admin are checked in the handler.
    #[account(mut, seeds = [CONFIG_SEED], bump)]
    pub config: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Launch<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ ReserveError::Unauthorized)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [CREATOR_SEED], bump = config.creator_bump)]
    pub creator: SystemAccount<'info>,
    /// New pump mint (keypair signs the transaction).
    #[account(mut)]
    pub mint: Signer<'info>,
    /// CHECK: pump validates.
    pub mint_authority: UncheckedAccount<'info>,
    /// CHECK: pump validates (seeds ["bonding-curve", mint]).
    #[account(mut)]
    pub bonding_curve: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    #[account(mut)]
    pub curve_base_vault: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    #[account(mut)]
    pub curve_quote_vault: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    pub pump_global: UncheckedAccount<'info>,
    /// CHECK: checked in the handler = ATA(creator, mint, Token-2022).
    #[account(mut)]
    pub reserve_ata: UncheckedAccount<'info>,
    /// CHECK: pump validates (ATA(creator, wSOL, SPL), unused for SOL).
    #[account(mut)]
    pub creator_wsol: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    #[account(mut)]
    pub user_volume_accumulator: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    pub fee_config: UncheckedAccount<'info>,
    /// CHECK: pump validates (one of Global.buyback_fee_recipients).
    #[account(mut)]
    pub buyback_fee_recipient: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(mut, address = MAYHEM_ID)]
    pub mayhem_program: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    pub mayhem_global_params: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    #[account(mut)]
    pub mayhem_sol_vault: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    #[account(mut)]
    pub mayhem_state: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    #[account(mut)]
    pub mayhem_token_vault: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = WSOL_MINT)]
    pub wsol_mint: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = TOKEN_ID)]
    pub token_program: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = TOKEN_2022_ID)]
    pub token_2022_program: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = ATA_ID)]
    pub associated_token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: pump validates.
    pub pump_event_authority: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = PUMP_ID)]
    pub pump_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CurveCrank<'info> {
    #[account(mut)]
    pub cranker: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [CREATOR_SEED], bump = config.creator_bump)]
    pub creator: SystemAccount<'info>,
    /// CHECK: address = config.mint.
    #[account(address = config.mint @ ReserveError::NotLaunched)]
    pub mint: UncheckedAccount<'info>,
    /// CHECK: checked = ATA(creator, mint, Token-2022).
    #[account(mut)]
    pub reserve_ata: UncheckedAccount<'info>,
    /// CHECK: pump validates (ATA(creator, wSOL, SPL)).
    #[account(mut)]
    pub creator_wsol: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    pub pump_global: UncheckedAccount<'info>,
    /// CHECK: checked = PDA(["bonding-curve", mint]) and owned by pump.
    #[account(mut)]
    pub bonding_curve: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    #[account(mut)]
    pub curve_base_vault: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    #[account(mut)]
    pub curve_quote_vault: UncheckedAccount<'info>,
    /// CHECK: pump validates (["creator-vault", creator]).
    #[account(mut)]
    pub creator_vault: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    #[account(mut)]
    pub creator_vault_wsol: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    #[account(mut)]
    pub user_volume_accumulator: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    pub fee_config: UncheckedAccount<'info>,
    /// CHECK: pump validates.
    #[account(mut)]
    pub buyback_fee_recipient: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = WSOL_MINT)]
    pub wsol_mint: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = TOKEN_ID)]
    pub token_program: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = TOKEN_2022_ID)]
    pub token_2022_program: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = ATA_ID)]
    pub associated_token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: pump validates.
    pub pump_event_authority: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = PUMP_ID)]
    pub pump_program: UncheckedAccount<'info>,
}

impl<'info> CurveCrank<'info> {
    fn check(&self, buying: bool) -> Result<()> {
        if buying {
            self.config.check_cranker(&self.cranker.key())?;
        }
        require_keys_eq!(self.reserve_ata.key(), ata(&self.creator.key(), &self.mint.key(), &TOKEN_2022_ID), ReserveError::BadAccount);
        require_keys_eq!(self.bonding_curve.key(), pump::bonding_curve(&self.mint.key()), ReserveError::BadAccount);
        Ok(())
    }

    fn sweep_and_collect(&self) -> Result<()> {
        let cranker = self.cranker.to_account_info();
        let system = self.system_program.to_account_info();
        call(
            &self.pump_program,
            IX_SWEEP_CREATOR_FEE.to_vec(),
            &[
                (&cranker, true, true),
                (&self.pump_global, false, false),
                (&self.mint, false, false),
                (&self.wsol_mint, false, false),
                (&self.token_program, false, false),
                (&self.associated_token_program, false, false),
                (&system, false, false),
                (&self.bonding_curve, true, false),
                (&self.curve_quote_vault, true, false),
                (&self.creator_vault, true, false),
                (&self.creator_vault_wsol, true, false),
                (&self.pump_event_authority, false, false),
                (&self.pump_program, false, false),
            ],
            &[],
        )?;
        call(
            &self.pump_program,
            IX_COLLECT_CREATOR_FEE_V2.to_vec(),
            &[
                (&self.creator.to_account_info(), true, false),
                (&self.creator_wsol, true, false),
                (&self.creator_vault, true, false),
                (&self.creator_vault_wsol, true, false),
                (&self.wsol_mint, false, false),
                (&self.token_program, false, false),
                (&self.associated_token_program, false, false),
                (&system, false, false),
                (&self.pump_event_authority, false, false),
                (&self.pump_program, false, false),
            ],
            &[],
        )
    }
}

#[derive(Accounts)]
pub struct AmmCrank<'info> {
    #[account(mut)]
    pub cranker: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [CREATOR_SEED], bump = config.creator_bump)]
    pub creator: SystemAccount<'info>,
    /// CHECK: address = config.mint.
    #[account(address = config.mint @ ReserveError::NotLaunched)]
    pub mint: UncheckedAccount<'info>,
    /// CHECK: checked = ATA(creator, mint, Token-2022).
    #[account(mut)]
    pub reserve_ata: UncheckedAccount<'info>,
    /// CHECK: checked = ATA(creator, wSOL, SPL); created if missing.
    #[account(mut)]
    pub creator_wsol: UncheckedAccount<'info>,
    /// CHECK: checked = PDA(["bonding-curve", mint]); must be complete.
    pub bonding_curve: UncheckedAccount<'info>,
    /// CHECK: checked = canonical pool for mint.
    #[account(mut)]
    pub pool: UncheckedAccount<'info>,
    /// CHECK: checked = pool.pool_base_token_account.
    #[account(mut)]
    pub pool_base_vault: UncheckedAccount<'info>,
    /// CHECK: checked = pool.pool_quote_token_account.
    #[account(mut)]
    pub pool_quote_vault: UncheckedAccount<'info>,
    /// CHECK: PumpSwap validates.
    pub amm_global_config: UncheckedAccount<'info>,
    /// CHECK: PumpSwap validates (["creator_vault", coin_creator]).
    pub coin_creator_vault_authority: UncheckedAccount<'info>,
    /// CHECK: PumpSwap validates.
    #[account(mut)]
    pub coin_creator_vault_ata: UncheckedAccount<'info>,
    /// CHECK: PumpSwap validates.
    #[account(mut)]
    pub amm_user_volume_accumulator: UncheckedAccount<'info>,
    /// CHECK: PumpSwap validates.
    pub amm_fee_config: UncheckedAccount<'info>,
    /// CHECK: PumpSwap validates (listed recipient's wSOL ATA).
    #[account(mut)]
    pub amm_buyback_fee_recipient: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = WSOL_MINT)]
    pub wsol_mint: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = TOKEN_ID)]
    pub token_program: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = TOKEN_2022_ID)]
    pub token_2022_program: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = ATA_ID)]
    pub associated_token_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: PumpSwap validates.
    pub amm_event_authority: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = PUMP_AMM_ID)]
    pub amm_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct FundMining<'info> {
    /// Anyone (fund_mining is permissionless); pays the bridge Deposit record rent.
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(seeds = [CREATOR_SEED], bump = config.creator_bump)]
    pub creator: SystemAccount<'info>,
    /// CHECK: address = config.mint.
    #[account(address = config.mint @ ReserveError::NotLaunched)]
    pub mint: UncheckedAccount<'info>,
    /// CHECK: checked = ATA(creator, mint, Token-2022).
    #[account(mut)]
    pub reserve_ata: UncheckedAccount<'info>,
    /// CHECK: bridge validates (its config PDA).
    #[account(mut)]
    pub bridge_config: UncheckedAccount<'info>,
    /// CHECK: bridge validates (has_one vault).
    #[account(mut)]
    pub bridge_vault: UncheckedAccount<'info>,
    /// CHECK: bridge validates (["deposit", nonce]).
    #[account(mut)]
    pub bridge_deposit: UncheckedAccount<'info>,
    /// CHECK: address.
    #[account(address = TOKEN_2022_ID)]
    pub token_2022_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: address = config.bridge_program.
    #[account(address = config.bridge_program @ ReserveError::BadConfig, executable)]
    pub bridge_program: UncheckedAccount<'info>,
}

// ---------------------------------------------------------------- events, errors

#[event]
pub struct ConfigUpdated {
    pub config: Pubkey,
}

#[event]
pub struct Launched {
    pub mint: Pubkey,
    pub creator: Pubkey,
    pub dev_buy_lamports: u64,
    pub tokens: u64,
}

#[event]
pub struct Buyback {
    /// 0 = bonding curve, 1 = PumpSwap
    pub venue: u8,
    pub spent: u64,
    pub tokens: u64,
    pub quote: u64,
    pub spend_cap: u64,
}

#[event]
pub struct MiningFunded {
    pub amount: u64,
    pub recipient: Pubkey,
    pub total_funded: u64,
    /// Appended 2026-10-19 (older decoders stop after total_funded).
    pub caller: Pubkey,
    pub reserve_left: u64,
    pub window_start: i64,
    pub window_used: u64,
    pub calls: u64,
}

/// `set_fund_limits`: every fund_mining limit after the change.
#[event]
pub struct FundLimitsUpdated {
    pub config: Pubkey,
    pub paused: bool,
    pub max_per_call: u64,
    pub min_interval_secs: i64,
    pub window_secs: i64,
    pub max_per_window: u64,
}

#[error_code]
pub enum ReserveError {
    #[msg("signer is not allowed")]
    Unauthorized,
    #[msg("bad config value")]
    BadConfig,
    #[msg("unexpected account")]
    BadAccount,
    #[msg("coin already launched")]
    AlreadyLaunched,
    #[msg("coin not launched")]
    NotLaunched,
    #[msg("crank called before min_interval_secs")]
    TooSoon,
    #[msg("nothing to buy (below min_buy)")]
    NothingToBuy,
    #[msg("fill below min_tokens_out")]
    MinTokensOut,
    #[msg("fill worse than max_slippage_bps from the pre-trade quote")]
    Slippage,
    #[msg("spent more than the spend cap")]
    Overspent,
    #[msg("creator PDA fell below sol_floor")]
    BelowFloor,
    #[msg("bonding curve is complete, use crank_buyback_amm")]
    CurveComplete,
    #[msg("bonding curve not complete yet, use crank_buyback")]
    CurveNotComplete,
    #[msg("amount is zero")]
    ZeroAmount,
    #[msg("amount above max_fund_per_call")]
    OverFundCap,
    #[msg("reserve holds less than amount")]
    InsufficientReserve,
    #[msg("overflow")]
    Overflow,
    #[msg("fund_mining is paused")]
    FundPaused,
    #[msg("fund_mining called before fund_min_interval_secs")]
    FundTooSoon,
    #[msg("amount above fund_max_per_window for this window")]
    OverWindowCap,
    #[msg("mining_pool / bridge_program are frozen after the first fund_mining")]
    DestinationFrozen,
    #[msg("config predates the fund limits: run migrate_config")]
    NeedsMigration,
}
