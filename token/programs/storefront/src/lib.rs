//! pearl-sol compute storefront (SPEC 5.1). Any Solana wallet, app or agent
//! buys an AI job on the agent chain with SOL; the payment becomes buy
//! pressure on the token and job pay on the chain; the result commitment is
//! recorded back here, readable by any Solana program.
//!
//! Flow (two transactions, because one buy + deposit with a full job spec
//! does not fit in 1232 bytes):
//! 1. `open_order(draft_id, recipient, spec)`: the buyer writes the job spec
//!    into an `OrderDraft` PDA `["draft", buyer, draft_id u64 LE]`.
//! 2. `buy_job` (bonding curve) or `buy_job_amm` (PumpSwap, after
//!    graduation) with `(nonce, sol_in, min_tokens_out)`:
//!    - the buyer's `sol_in` lamports go to the data-less, system-owned
//!      `trader` PDA `["trader"]`, which buys the token by CPI
//!      (`buy_exact_quote_in_v3` on the curve, `buy_exact_quote_in_v2` on
//!      PumpSwap), bounded by the caller's `min_tokens_out` and
//!      `config.max_slippage_bps` against the program's own fee-free quote
//!      (the reserve's check, token/programs/reserve);
//!    - the bought tokens are split into `P = tokens x protocol_fee_bps /
//!      10000` and `W = tokens - P` (stored in chain units, x1000);
//!    - all tokens are deposited into the bridge (bridge `deposit` CPI) with
//!      `sender = trader PDA` and `agent_recipient = order.recipient`;
//!    - the `Order` PDA `["order", deposit_nonce u64 LE]` is created from the
//!      draft (the draft is closed to the buyer). Unused SOL goes back to the
//!      buyer.
//!
//! ## How a bridge Deposit links to its Order (no bridge change needed)
//! - A bridge `Deposit` whose `sender` is this program's trader PDA
//!   (`trader_pda()`) is a storefront order. Nothing else can deposit with
//!   that sender: only this program signs for it, and only inside `buy_job*`.
//! - Its Order is at `PDA(["order", deposit.nonce LE], storefront)`, and
//!   `order.deposit_nonce == deposit.nonce`, `order.tokens == deposit.amount`,
//!   `order.recipient == deposit.recipient`.
//! - The relayer then submits one `BridgeUnlock` with `job_order = 1` and the
//!   PostJob body `post_job_body(order)` (SPEC 4.1 field order, below).
//!   Attesters sign `order_unlock_message` =
//!   `b"pearl-sol:deposit-order:v1" || nonce u64 || recipient [32] ||
//!   amount_spl u64 || sha256(PostJob body)`, and `order.body_hash` holds
//!   that sha256 so attesters can check the body against the Order.
//! - The job id on the chain for an order job is
//!   `job_id = sha256(b"pearl-sol:order-job:v1" || nonce u64 LE)` (stored in
//!   `order.job_id`), not `blake3(poster || poster_nonce)`: it must be known
//!   on Solana before the chain sees the job. Solana has no usable blake3
//!   syscall on mainnet, hence sha256.
//! - PostJob body for an order (SPEC 4.1 order, little endian):
//!   `m u32, n u32, k u32, config [52], hash_a [32], hash_b [32],
//!   data_uri_len u16, data_uri, W u64, P u64, deadline u64, max_blocks u32,
//!   evaluator_count u8, evaluators [32]*, threshold u8`.
//!   W + P == amount_spl x 1000. `deadline` here is RELATIVE (blocks after
//!   the unlock height); the chain sets `deadline_height = unlock height +
//!   deadline`, because the buyer cannot know the chain height at K-deep
//!   finality time.
//!
//! ## Result and refund
//! - `finalize_order(deliverer, result_hash, result_uri)`: M-of-N of
//!   `config.attesters` sign `b"pearl-sol:result:v1" || job_id || deliverer
//!   || result_hash` (the same message the chain's CompleteJob evaluators
//!   sign, SPEC 4.1) through the ed25519 precompile in the same transaction.
//!   The caller must be the buyer, the admin or an attester (so nobody else
//!   can pin a junk `result_uri`; `result_hash` is the commitment).
//! - `expire_order()`: M-of-N attesters sign `b"pearl-sol:expired:v1" ||
//!   job_id`. Record only: W comes back on the chain to the poster (= the
//!   order's recipient) and returns to Solana by a normal BridgeLock +
//!   bridge `release` to the buyer. P is non-refundable (SPEC 4.1).
//! - Once Finalized or Expired an Order is closed for good (replay guard).
//!
//! ## USDC (mainnet)
//! TODO(usdc): `buy_job_usdc` is designed but NOT implemented (localnet is
//! SOL only). Design: the buyer's USDC goes to a trader-owned USDC ATA; a
//! Jupiter `shared_accounts_route` CPI (route built off chain and passed as
//! remaining accounts, the program checks input mint = USDC, output = wSOL
//! ATA of the trader PDA, and a min SOL out from the caller bounded by
//! `max_slippage_bps` against a Pyth USDC/SOL price) swaps to wSOL; after
//! graduation the wSOL feeds PumpSwap `buy_exact_quote_in_v2` directly, on the
//! curve it is unwrapped (close_account) and spent with
//! `buy_exact_quote_in_v3`. The rest of the path (split, deposit, Order) is
//! shared with `buy_job`.
//!
//! Every limit, the fee split, slippage, caps and the attester set are
//! admin-editable (`update_config`, `set_attesters`).
//!
//! Setup: `initialize`, then (once, anyone pays) pump and PumpSwap
//! `init_user_volume_accumulator` for the trader PDA, so the first order does
//! not pay that rent inside its trade.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::{
    hash::{hash, hashv},
    program::{invoke, invoke_signed},
    system_instruction,
    sysvar::instructions as ix_sysvar,
};

pub mod pump;
pub mod sigverify;
use pump::*;

declare_id!("DxuuzbuTX32Na4yVDpjo7ZBZQWufFGjzGZASMRpPgXkr");

pub const CONFIG_SEED: &[u8] = b"config";
pub const TRADER_SEED: &[u8] = b"trader";
pub const DRAFT_SEED: &[u8] = b"draft";
pub const ORDER_SEED: &[u8] = b"order";

pub const MAX_ATTESTERS: usize = 10;
pub const MAX_EVALUATORS: usize = 8;
pub const MAX_URI: usize = 200;
/// 1 SPL base unit (6 dp) = 1000 chain base units (9 dp), SPEC 1.
pub const SPL_TO_CHAIN: u64 = 1000;

pub const RESULT_DOMAIN: &[u8] = b"pearl-sol:result:v1";
pub const EXPIRED_DOMAIN: &[u8] = b"pearl-sol:expired:v1";
pub const ORDER_JOB_DOMAIN: &[u8] = b"pearl-sol:order-job:v1";
pub const ORDER_DEPOSIT_DOMAIN: &[u8] = b"pearl-sol:deposit-order:v1";

pub const STATUS_PENDING: u8 = 1;
pub const STATUS_FINALIZED: u8 = 2;
pub const STATUS_EXPIRED: u8 = 3;

pub const VENUE_CURVE: u8 = 0;
pub const VENUE_PUMPSWAP: u8 = 1;

/// Bridge `Config.deposit_nonce` offset: disc 8 + admin, mint, vault, token_program.
const BRIDGE_NONCE_OFFSET: usize = 8 + 32 * 4;
const RENT_MIN: u64 = 890_880;

pub fn trader_pda() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[TRADER_SEED], &ID)
}

pub fn order_pda(nonce: u64) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[ORDER_SEED, &nonce.to_le_bytes()], &ID)
}

pub fn order_job_id(nonce: u64) -> [u8; 32] {
    hashv(&[ORDER_JOB_DOMAIN, &nonce.to_le_bytes()]).to_bytes()
}

pub fn result_message(job_id: &[u8; 32], deliverer: &Pubkey, result_hash: &[u8; 32]) -> Vec<u8> {
    [RESULT_DOMAIN, job_id.as_ref(), deliverer.as_ref(), result_hash.as_ref()].concat()
}

pub fn expired_message(job_id: &[u8; 32]) -> Vec<u8> {
    [EXPIRED_DOMAIN, job_id.as_ref()].concat()
}

pub fn order_unlock_message(nonce: u64, recipient: &Pubkey, amount_spl: u64, body_hash: &[u8; 32]) -> Vec<u8> {
    [ORDER_DEPOSIT_DOMAIN, &nonce.to_le_bytes(), recipient.as_ref(), &amount_spl.to_le_bytes(), body_hash.as_ref()].concat()
}

/// The PostJob body the chain executes for an order (see module docs).
pub fn post_job_body(spec: &JobSpec, worker_pay: u64, protocol_fee: u64) -> Vec<u8> {
    let uri = &spec.data_uri[..spec.data_uri_len as usize];
    let evs = &spec.evaluators[..spec.evaluator_count as usize];
    let mut b = Vec::with_capacity(12 + 52 + 64 + 2 + uri.len() + 28 + 1 + 32 * evs.len() + 1);
    b.extend_from_slice(&spec.m.to_le_bytes());
    b.extend_from_slice(&spec.n.to_le_bytes());
    b.extend_from_slice(&spec.k.to_le_bytes());
    b.extend_from_slice(&spec.config);
    b.extend_from_slice(&spec.hash_a);
    b.extend_from_slice(&spec.hash_b);
    b.extend_from_slice(&(uri.len() as u16).to_le_bytes());
    b.extend_from_slice(uri);
    b.extend_from_slice(&worker_pay.to_le_bytes());
    b.extend_from_slice(&protocol_fee.to_le_bytes());
    b.extend_from_slice(&spec.deadline_blocks.to_le_bytes());
    b.extend_from_slice(&spec.max_blocks.to_le_bytes());
    b.push(evs.len() as u8);
    for e in evs {
        b.extend_from_slice(e.as_ref());
    }
    b.push(spec.threshold);
    b
}

fn check_attester_set(attesters: &[Pubkey], threshold: u8) -> Result<()> {
    require!(!attesters.is_empty() && attesters.len() <= MAX_ATTESTERS, StorefrontError::BadAttesterSet);
    require!(threshold >= 1 && threshold as usize <= attesters.len(), StorefrontError::BadAttesterSet);
    for (i, s) in attesters.iter().enumerate() {
        require!(!attesters[..i].contains(s), StorefrontError::BadAttesterSet);
    }
    Ok(())
}

#[program]
pub mod storefront {
    use super::*;

    /// Creates the config (signer = admin) and tops the trader PDA up to rent exemption.
    pub fn initialize(ctx: Context<Initialize>, params: ConfigParams, attesters: Vec<Pubkey>, threshold: u8) -> Result<()> {
        check_attester_set(&attesters, threshold)?;
        let c = &mut ctx.accounts.config;
        c.admin = ctx.accounts.admin.key();
        c.bump = ctx.bumps.config;
        c.trader_bump = ctx.bumps.trader;
        c.protocol_fee_bps = 2000;
        c.max_slippage_bps = 300;
        c.min_sol_in = 10_000_000;
        c.max_sol_in = 10_000_000_000;
        c.max_dim = 8192;
        c.max_blocks_cap = 10_000;
        c.max_deadline_blocks = 100_000;
        c.attesters = attesters;
        c.threshold = threshold;
        c.apply(params)?;
        let have = ctx.accounts.trader.lamports();
        if have < RENT_MIN {
            invoke(
                &system_instruction::transfer(ctx.accounts.admin.key, ctx.accounts.trader.key, RENT_MIN - have),
                &[ctx.accounts.admin.to_account_info(), ctx.accounts.trader.to_account_info(), ctx.accounts.system_program.to_account_info()],
            )?;
        }
        Ok(())
    }

    pub fn update_config(ctx: Context<AdminOnly>, params: ConfigParams) -> Result<()> {
        ctx.accounts.config.apply(params)?;
        emit!(ConfigUpdated { config: ctx.accounts.config.key() });
        Ok(())
    }

    pub fn set_attesters(ctx: Context<AdminOnly>, attesters: Vec<Pubkey>, threshold: u8) -> Result<()> {
        check_attester_set(&attesters, threshold)?;
        let c = &mut ctx.accounts.config;
        c.attesters = attesters;
        c.threshold = threshold;
        emit!(ConfigUpdated { config: c.key() });
        Ok(())
    }

    pub fn set_admin(ctx: Context<AdminOnly>, new_admin: Pubkey) -> Result<()> {
        require_keys_neq!(new_admin, Pubkey::default(), StorefrontError::BadConfig);
        ctx.accounts.config.admin = new_admin;
        Ok(())
    }

    /// Step 1: the buyer stores the job spec. `recipient` = the buyer's
    /// agent-chain address (poster of the job; same ed25519 key works on both).
    pub fn open_order(ctx: Context<OpenOrder>, draft_id: u64, recipient: Pubkey, spec: JobSpecArgs) -> Result<()> {
        require_keys_neq!(recipient, Pubkey::default(), StorefrontError::BadSpec);
        let spec = ctx.accounts.config.check_spec(spec)?;
        let d = &mut ctx.accounts.draft;
        d.buyer = ctx.accounts.buyer.key();
        d.draft_id = draft_id;
        d.recipient = recipient;
        d.bump = ctx.bumps.draft;
        d.spec = spec;
        Ok(())
    }

    /// Buyer abandons a draft (rent back).
    pub fn cancel_draft(_ctx: Context<CancelDraft>) -> Result<()> {
        Ok(())
    }

    /// Step 2 on the bonding curve. `nonce` must equal the bridge's next
    /// deposit nonce (the Order PDA is keyed by it).
    pub fn buy_job(ctx: Context<BuyJobCurve>, nonce: u64, sol_in: u64, min_tokens_out: u64) -> Result<()> {
        let a = &ctx.accounts;
        a.config.check_buy(sol_in)?;
        check_bridge_nonce(&a.bridge_config, &a.bridge_program, nonce)?;
        require_keys_eq!(a.trader_ata.key(), ata(&a.trader.key(), &a.mint.key(), &TOKEN_2022_ID), StorefrontError::BadAccount);
        require_keys_eq!(a.bonding_curve.key(), pump::bonding_curve(&a.mint.key()), StorefrontError::BadAccount);
        let curve = read_curve(&a.bonding_curve)?;
        require!(!curve.complete, StorefrontError::CurveComplete);

        let buyer = a.buyer.to_account_info();
        let trader = a.trader.to_account_info();
        let mint = a.mint.to_account_info();
        let system = a.system_program.to_account_info();
        let seeds: &[&[u8]] = &[TRADER_SEED, &[a.config.trader_bump]];
        ata_create_idempotent(&a.associated_token_program, &buyer, &a.trader_ata, &trader, &mint, &system, &a.token_2022_program)?;

        let lamports_before = trader.lamports();
        invoke(&system_instruction::transfer(buyer.key, trader.key, sol_in), &[buyer.clone(), trader.clone(), system.clone()])?;
        let quote = cp_quote(curve.virtual_token, curve.virtual_quote, sol_in);
        let min_out = min_tokens_out.max(slip_floor(quote, a.config.max_slippage_bps)).max(1);
        let tokens_before = token_amount(&a.trader_ata)?;
        let mut data = IX_BUY_EXACT_QUOTE_IN_V3.to_vec();
        data.extend_from_slice(&sol_in.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        data.push(0); // partial_fill = false
        call(
            &a.pump_program,
            data,
            &[
                (&a.pump_global, false, false),
                (&mint, false, false),
                (&a.wsol_mint, false, false),
                (&a.token_2022_program, false, false),
                (&a.token_program, false, false),
                (&a.bonding_curve, true, false),
                (&a.curve_base_vault, true, false),
                (&a.curve_quote_vault, true, false),
                (&trader, true, true),
                (&a.trader_ata, true, false),
                (&a.trader_wsol, true, false),
                (&a.user_volume_accumulator, true, false),
                (&a.fee_config, false, false),
                (&a.buyback_fee_recipient, true, false),
                (&system, false, false),
                (&a.pump_event_authority, false, false),
                (&a.pump_program, false, false),
            ],
            &[seeds],
        )?;
        let got = token_amount(&a.trader_ata)?.saturating_sub(tokens_before);
        // Everything above the starting balance goes back to the buyer.
        let after = trader.lamports();
        let spent = (lamports_before + sol_in).saturating_sub(after);
        if after > lamports_before {
            invoke_signed(&system_instruction::transfer(trader.key, buyer.key, after - lamports_before), &[trader.clone(), buyer.clone(), system.clone()], &[seeds])?;
        }
        require!(trader.lamports() >= RENT_MIN, StorefrontError::BelowFloor);
        check_fill(got, spent, sol_in, quote, min_tokens_out, a.config.max_slippage_bps)?;

        let o = &ctx.accounts;
        let pa = PlaceAccounts {
            buyer,
            trader,
            mint,
            trader_ata: o.trader_ata.to_account_info(),
            bridge_config: o.bridge_config.to_account_info(),
            bridge_vault: o.bridge_vault.to_account_info(),
            bridge_deposit: o.bridge_deposit.to_account_info(),
            bridge_program: o.bridge_program.to_account_info(),
            token_2022_program: o.token_2022_program.to_account_info(),
            system_program: system,
        };
        let trader_bump = o.config.trader_bump;
        place_order(
            pa,
            &mut ctx.accounts.config,
            &ctx.accounts.draft,
            &mut ctx.accounts.order,
            ctx.bumps.order,
            nonce,
            got,
            spent,
            VENUE_CURVE,
            trader_bump,
        )
    }

    /// Step 2 on PumpSwap (after graduation): the SOL is wrapped into the
    /// trader's wSOL ATA, spent with `buy_exact_quote_in_v2` on the canonical
    /// pool, and the ATA is closed again (rent and leftover back to the buyer).
    pub fn buy_job_amm(ctx: Context<BuyJobAmm>, nonce: u64, sol_in: u64, min_tokens_out: u64) -> Result<()> {
        let a = &ctx.accounts;
        a.config.check_buy(sol_in)?;
        check_bridge_nonce(&a.bridge_config, &a.bridge_program, nonce)?;
        require_keys_eq!(a.trader_ata.key(), ata(&a.trader.key(), &a.mint.key(), &TOKEN_2022_ID), StorefrontError::BadAccount);
        require_keys_eq!(a.trader_wsol.key(), ata(&a.trader.key(), &WSOL_MINT, &TOKEN_ID), StorefrontError::BadAccount);
        require_keys_eq!(a.pool.key(), canonical_pool(&a.mint.key()), StorefrontError::BadAccount);
        let pool = read_pool(&a.pool)?;
        require_keys_eq!(pool.base_mint, a.mint.key(), StorefrontError::BadAccount);
        require_keys_eq!(pool.quote_mint, WSOL_MINT, StorefrontError::BadAccount);
        require_keys_eq!(pool.base_vault, a.pool_base_vault.key(), StorefrontError::BadAccount);
        require_keys_eq!(pool.quote_vault, a.pool_quote_vault.key(), StorefrontError::BadAccount);

        let buyer = a.buyer.to_account_info();
        let trader = a.trader.to_account_info();
        let mint = a.mint.to_account_info();
        let system = a.system_program.to_account_info();
        let wsol = &a.wsol_mint;
        let seeds: &[&[u8]] = &[TRADER_SEED, &[a.config.trader_bump]];
        let lamports_before = trader.lamports();
        ata_create_idempotent(&a.associated_token_program, &buyer, &a.trader_ata, &trader, &mint, &system, &a.token_2022_program)?;
        ata_create_idempotent(&a.associated_token_program, &buyer, &a.trader_wsol, &trader, wsol, &system, &a.token_program)?;
        let wsol_start = token_amount(&a.trader_wsol)?;
        invoke(&system_instruction::transfer(buyer.key, a.trader_wsol.key, sol_in), &[buyer.clone(), a.trader_wsol.to_account_info(), system.clone()])?;
        sync_native(&a.token_program, &a.trader_wsol)?;
        let wsol_before = token_amount(&a.trader_wsol)?;
        require!(wsol_before >= wsol_start.saturating_add(sol_in), StorefrontError::BadAccount);

        let base_res = token_amount(&a.pool_base_vault)?;
        let quote_res = token_amount(&a.pool_quote_vault)?
            .saturating_sub(pool.protocol_fees)
            .saturating_sub(pool.creator_fees)
            .saturating_add(pool.virtual_quote);
        let quote = cp_quote(base_res, quote_res, sol_in);
        let min_out = min_tokens_out.max(slip_floor(quote, a.config.max_slippage_bps)).max(1);
        let tokens_before = token_amount(&a.trader_ata)?;
        let mut data = IX_AMM_BUY_EXACT_QUOTE_IN_V2.to_vec();
        data.extend_from_slice(&sol_in.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        call(
            &a.amm_program,
            data,
            &[
                (&a.pool, true, false),
                (&trader, true, true),
                (&a.amm_global_config, false, false),
                (&mint, false, false),
                (wsol, false, false),
                (&a.trader_ata, true, false),
                (&a.trader_wsol, true, false),
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
        let spent = wsol_before.saturating_sub(token_amount(&a.trader_wsol)?);
        let got = token_amount(&a.trader_ata)?.saturating_sub(tokens_before);
        // Close the wSOL ATA into the trader, then hand everything above the
        // starting balance (ATA rent + unspent SOL) back to the buyer.
        close_account(&a.token_program, &a.trader_wsol, &trader, &trader, seeds)?;
        let after = trader.lamports();
        if after > lamports_before {
            invoke_signed(&system_instruction::transfer(trader.key, buyer.key, after - lamports_before), &[trader.clone(), buyer.clone(), system.clone()], &[seeds])?;
        }
        require!(trader.lamports() >= RENT_MIN, StorefrontError::BelowFloor);
        check_fill(got, spent, sol_in, quote, min_tokens_out, a.config.max_slippage_bps)?;

        let o = &ctx.accounts;
        let pa = PlaceAccounts {
            buyer,
            trader,
            mint,
            trader_ata: o.trader_ata.to_account_info(),
            bridge_config: o.bridge_config.to_account_info(),
            bridge_vault: o.bridge_vault.to_account_info(),
            bridge_deposit: o.bridge_deposit.to_account_info(),
            bridge_program: o.bridge_program.to_account_info(),
            token_2022_program: o.token_2022_program.to_account_info(),
            system_program: system,
        };
        let trader_bump = o.config.trader_bump;
        place_order(
            pa,
            &mut ctx.accounts.config,
            &ctx.accounts.draft,
            &mut ctx.accounts.order,
            ctx.bumps.order,
            nonce,
            got,
            spent,
            VENUE_PUMPSWAP,
            trader_bump,
        )
    }

    /// Records the job result (M1: M-of-N attester signatures through the
    /// ed25519 precompile). Caller: buyer, admin or an attester.
    pub fn finalize_order(ctx: Context<CloseOrder>, deliverer: Pubkey, result_hash: [u8; 32], result_uri: String) -> Result<()> {
        require!(result_uri.len() <= MAX_URI, StorefrontError::UriTooLong);
        ctx.accounts.check_caller()?;
        let o = &ctx.accounts.order;
        require!(o.status == STATUS_PENDING, StorefrontError::NotPending);
        let msg = result_message(&o.job_id, &deliverer, &result_hash);
        let c = &ctx.accounts.config;
        let n = sigverify::count_signers(&ctx.accounts.instructions.to_account_info(), &msg, &c.attesters)?;
        require!(n >= c.threshold as usize, StorefrontError::NotEnoughSignatures);

        let slot = Clock::get()?.slot;
        let o = &mut ctx.accounts.order;
        o.status = STATUS_FINALIZED;
        o.deliverer = deliverer;
        o.result_hash = result_hash;
        o.result_uri_len = result_uri.len() as u16;
        o.result_uri = [0u8; MAX_URI];
        o.result_uri[..result_uri.len()].copy_from_slice(result_uri.as_bytes());
        o.closed_slot = slot;
        emit!(OrderFinalized { order: o.key(), nonce: o.deposit_nonce, job_id: o.job_id, deliverer, result_hash });
        Ok(())
    }

    /// Records that the job expired on the chain (M-of-N attesters over
    /// `b"pearl-sol:expired:v1" || job_id`). Funds return over the bridge.
    pub fn expire_order(ctx: Context<CloseOrder>) -> Result<()> {
        ctx.accounts.check_caller()?;
        let o = &ctx.accounts.order;
        require!(o.status == STATUS_PENDING, StorefrontError::NotPending);
        let msg = expired_message(&o.job_id);
        let c = &ctx.accounts.config;
        let n = sigverify::count_signers(&ctx.accounts.instructions.to_account_info(), &msg, &c.attesters)?;
        require!(n >= c.threshold as usize, StorefrontError::NotEnoughSignatures);
        let slot = Clock::get()?.slot;
        let o = &mut ctx.accounts.order;
        o.status = STATUS_EXPIRED;
        o.closed_slot = slot;
        emit!(OrderExpired { order: o.key(), nonce: o.deposit_nonce, job_id: o.job_id });
        Ok(())
    }
}

fn check_bridge_nonce(bridge_config: &AccountInfo, bridge_program: &AccountInfo, nonce: u64) -> Result<()> {
    require_keys_eq!(*bridge_config.owner, bridge_program.key(), StorefrontError::BadAccount);
    let d = bridge_config.try_borrow_data()?;
    let got = d
        .get(BRIDGE_NONCE_OFFSET..BRIDGE_NONCE_OFFSET + 8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .ok_or(StorefrontError::BadAccount)?;
    require!(got == nonce, StorefrontError::WrongNonce);
    Ok(())
}

fn slip_floor(quote: u64, bps: u16) -> u64 {
    (quote as u128 * (10_000 - bps.min(10_000)) as u128 / 10_000) as u64
}

/// The reserve's fill check: below the caller's minimum, or a realised price
/// (tokens per lamport spent) worse than `bps` from the program's own quote.
fn check_fill(got: u64, spent: u64, spend: u64, quote: u64, min_tokens_out: u64, bps: u16) -> Result<()> {
    require!(got >= min_tokens_out && got > 0, StorefrontError::MinTokensOut);
    require!(spent <= spend, StorefrontError::Overspent);
    let lhs = got as u128 * spend as u128 * 10_000;
    let rhs = quote as u128 * spent as u128 * (10_000 - bps.min(10_000) as u128);
    require!(lhs >= rhs, StorefrontError::Slippage);
    Ok(())
}

struct PlaceAccounts<'info> {
    buyer: AccountInfo<'info>,
    trader: AccountInfo<'info>,
    mint: AccountInfo<'info>,
    trader_ata: AccountInfo<'info>,
    bridge_config: AccountInfo<'info>,
    bridge_vault: AccountInfo<'info>,
    bridge_deposit: AccountInfo<'info>,
    bridge_program: AccountInfo<'info>,
    token_2022_program: AccountInfo<'info>,
    system_program: AccountInfo<'info>,
}

#[allow(clippy::too_many_arguments)]
fn place_order<'info>(
    a: PlaceAccounts<'info>,
    config: &mut Account<'info, Config>,
    draft: &Account<'info, OrderDraft>,
    order: &mut Account<'info, Order>,
    order_bump: u8,
    nonce: u64,
    got: u64,
    spent: u64,
    venue: u8,
    trader_bump: u8,
) -> Result<()> {
    let p_spl = (got as u128 * config.protocol_fee_bps as u128 / 10_000) as u64;
    let w_spl = got - p_spl;
    require!(w_spl > 0 && p_spl > 0, StorefrontError::OrderTooSmall);
    let worker_pay = w_spl.checked_mul(SPL_TO_CHAIN).ok_or(StorefrontError::Overflow)?;
    let protocol_fee = p_spl.checked_mul(SPL_TO_CHAIN).ok_or(StorefrontError::Overflow)?;

    // Bridge deposit: sender = trader PDA, recipient = buyer's chain address.
    let mut data = hash(b"global:deposit").to_bytes()[..8].to_vec();
    data.extend_from_slice(&got.to_le_bytes());
    data.extend_from_slice(draft.recipient.as_ref());
    let seeds: &[&[u8]] = &[TRADER_SEED, &[trader_bump]];
    call(
        &a.bridge_program,
        data,
        &[
            (&a.buyer, true, true),
            (&a.trader, false, true),
            (&a.bridge_config, true, false),
            (&a.mint, false, false),
            (&a.trader_ata, true, false),
            (&a.bridge_vault, true, false),
            (&a.bridge_deposit, true, false),
            (&a.token_2022_program, false, false),
            (&a.system_program, false, false),
        ],
        &[seeds],
    )?;
    {
        // The Deposit record the relayer will read: nonce, sender, recipient, amount.
        require_keys_eq!(*a.bridge_deposit.owner, a.bridge_program.key(), StorefrontError::BadAccount);
        let d = a.bridge_deposit.try_borrow_data()?;
        require!(d.len() >= 88, StorefrontError::BadAccount);
        let u = |o: usize| u64::from_le_bytes(d[o..o + 8].try_into().unwrap());
        require!(u(8) == nonce && d[16..48] == a.trader.key.to_bytes() && d[48..80] == draft.recipient.to_bytes() && u(80) == got, StorefrontError::BadAccount);
    }

    let clock = Clock::get()?;
    let job_id = order_job_id(nonce);
    let body_hash = hashv(&[&post_job_body(&draft.spec, worker_pay, protocol_fee)]).to_bytes();
    order.version = 1;
    order.status = STATUS_PENDING;
    order.venue = venue;
    order.bump = order_bump;
    order.deposit_nonce = nonce;
    order.job_id = job_id;
    order.buyer = draft.buyer;
    order.recipient = draft.recipient;
    order.body_hash = body_hash;
    order.sol_spent = spent;
    order.tokens = got;
    order.worker_pay = worker_pay;
    order.protocol_fee = protocol_fee;
    order.created_slot = clock.slot;
    order.created_ts = clock.unix_timestamp;
    order.deliverer = Pubkey::default();
    order.result_hash = [0; 32];
    order.closed_slot = 0;
    order.result_uri_len = 0;
    order.result_uri = [0; MAX_URI];
    order.spec = draft.spec.clone();

    config.orders = config.orders.saturating_add(1);
    config.total_sol = config.total_sol.saturating_add(spent);
    config.total_tokens = config.total_tokens.saturating_add(got);
    emit!(OrderPlaced {
        order: order.key(),
        nonce,
        job_id,
        buyer: order.buyer,
        recipient: order.recipient,
        venue,
        sol_spent: spent,
        tokens: got,
        worker_pay,
        protocol_fee,
    });
    Ok(())
}

// ---------------------------------------------------------------- state

/// Fixed-size job spec (SPEC 4.1 fields). Fixed arrays keep every field of
/// `Order` at a fixed offset, see `layout`.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, InitSpace)]
pub struct JobSpec {
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub config: [u8; 52],
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
    /// Relative: blocks after the unlock height (see module docs).
    pub deadline_blocks: u64,
    pub max_blocks: u32,
    pub threshold: u8,
    pub evaluator_count: u8,
    pub evaluators: [Pubkey; MAX_EVALUATORS],
    pub data_uri_len: u16,
    pub data_uri: [u8; MAX_URI],
}

/// Instruction form of the spec.
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct JobSpecArgs {
    pub m: u32,
    pub n: u32,
    pub k: u32,
    pub config: [u8; 52],
    pub hash_a: [u8; 32],
    pub hash_b: [u8; 32],
    pub data_uri: String,
    pub deadline_blocks: u64,
    pub max_blocks: u32,
    pub evaluators: Vec<Pubkey>,
    pub threshold: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub admin: Pubkey,
    /// The pump coin (bridge mint).
    pub mint: Pubkey,
    /// token/programs/bridge.
    pub bridge_program: Pubkey,
    pub paused: bool,
    /// Share of the bought tokens that becomes P (mining pool), rest is W.
    pub protocol_fee_bps: u16,
    /// Max shortfall of the fill against the fee-free quote (covers pump's 1.25%).
    pub max_slippage_bps: u16,
    pub min_sol_in: u64,
    pub max_sol_in: u64,
    /// Cap on each of m, n, k.
    pub max_dim: u32,
    pub max_blocks_cap: u32,
    pub max_deadline_blocks: u64,
    pub threshold: u8,
    #[max_len(MAX_ATTESTERS)]
    pub attesters: Vec<Pubkey>,
    pub orders: u64,
    pub total_sol: u64,
    pub total_tokens: u64,
    pub bump: u8,
    pub trader_bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Default)]
pub struct ConfigParams {
    pub mint: Option<Pubkey>,
    pub bridge_program: Option<Pubkey>,
    pub paused: Option<bool>,
    pub protocol_fee_bps: Option<u16>,
    pub max_slippage_bps: Option<u16>,
    pub min_sol_in: Option<u64>,
    pub max_sol_in: Option<u64>,
    pub max_dim: Option<u32>,
    pub max_blocks_cap: Option<u32>,
    pub max_deadline_blocks: Option<u64>,
}

impl Config {
    fn apply(&mut self, p: ConfigParams) -> Result<()> {
        if let Some(v) = p.mint {
            self.mint = v;
        }
        if let Some(v) = p.bridge_program {
            self.bridge_program = v;
        }
        if let Some(v) = p.paused {
            self.paused = v;
        }
        if let Some(v) = p.protocol_fee_bps {
            require!(v > 0 && v < 10_000, StorefrontError::BadConfig);
            self.protocol_fee_bps = v;
        }
        if let Some(v) = p.max_slippage_bps {
            require!(v <= 10_000, StorefrontError::BadConfig);
            self.max_slippage_bps = v;
        }
        if let Some(v) = p.min_sol_in {
            self.min_sol_in = v;
        }
        if let Some(v) = p.max_sol_in {
            self.max_sol_in = v;
        }
        if let Some(v) = p.max_dim {
            self.max_dim = v;
        }
        if let Some(v) = p.max_blocks_cap {
            self.max_blocks_cap = v;
        }
        if let Some(v) = p.max_deadline_blocks {
            self.max_deadline_blocks = v;
        }
        require!(self.min_sol_in > 0 && self.min_sol_in <= self.max_sol_in, StorefrontError::BadConfig);
        Ok(())
    }

    fn check_buy(&self, sol_in: u64) -> Result<()> {
        require!(!self.paused, StorefrontError::Paused);
        require_keys_neq!(self.mint, Pubkey::default(), StorefrontError::BadConfig);
        require!(sol_in >= self.min_sol_in, StorefrontError::BelowMinOrder);
        require!(sol_in <= self.max_sol_in, StorefrontError::AboveMaxOrder);
        Ok(())
    }

    fn check_spec(&self, s: JobSpecArgs) -> Result<JobSpec> {
        for d in [s.m, s.n, s.k] {
            require!(d > 0 && d <= self.max_dim, StorefrontError::BadSpec);
        }
        require!(s.max_blocks > 0 && s.max_blocks <= self.max_blocks_cap, StorefrontError::BadSpec);
        require!(s.deadline_blocks > 0 && s.deadline_blocks <= self.max_deadline_blocks, StorefrontError::BadSpec);
        require!(s.data_uri.len() <= MAX_URI, StorefrontError::UriTooLong);
        let n = s.evaluators.len();
        require!(n >= 1 && n <= MAX_EVALUATORS, StorefrontError::BadSpec);
        require!(s.threshold >= 1 && s.threshold as usize <= n, StorefrontError::BadSpec);
        for (i, e) in s.evaluators.iter().enumerate() {
            require!(*e != Pubkey::default() && !s.evaluators[..i].contains(e), StorefrontError::BadSpec);
        }
        let mut evaluators = [Pubkey::default(); MAX_EVALUATORS];
        evaluators[..n].copy_from_slice(&s.evaluators);
        let mut data_uri = [0u8; MAX_URI];
        data_uri[..s.data_uri.len()].copy_from_slice(s.data_uri.as_bytes());
        Ok(JobSpec {
            m: s.m,
            n: s.n,
            k: s.k,
            config: s.config,
            hash_a: s.hash_a,
            hash_b: s.hash_b,
            deadline_blocks: s.deadline_blocks,
            max_blocks: s.max_blocks,
            threshold: s.threshold,
            evaluator_count: n as u8,
            evaluators,
            data_uri_len: s.data_uri.len() as u16,
            data_uri,
        })
    }
}

#[account]
#[derive(InitSpace)]
pub struct OrderDraft {
    pub buyer: Pubkey,
    pub draft_id: u64,
    pub recipient: Pubkey,
    pub bump: u8,
    pub spec: JobSpec,
}

/// One storefront order, readable by any program: fixed size, every field at
/// a fixed offset (`layout`). Status 1 Pending, 2 Finalized, 3 Expired.
#[account]
#[derive(InitSpace)]
pub struct Order {
    pub version: u8,
    pub status: u8,
    pub venue: u8,
    pub bump: u8,
    /// Bridge deposit nonce; Order PDA = ["order", nonce LE].
    pub deposit_nonce: u64,
    /// sha256("pearl-sol:order-job:v1" || nonce LE): the job id on the chain.
    pub job_id: [u8; 32],
    pub buyer: Pubkey,
    /// Agent-chain poster / bridge recipient.
    pub recipient: Pubkey,
    /// sha256 of the PostJob body the chain must execute.
    pub body_hash: [u8; 32],
    pub sol_spent: u64,
    /// SPL base units deposited (= W + P in SPL units).
    pub tokens: u64,
    /// W, chain base units.
    pub worker_pay: u64,
    /// P, chain base units.
    pub protocol_fee: u64,
    pub created_slot: u64,
    pub created_ts: i64,
    pub deliverer: Pubkey,
    pub result_hash: [u8; 32],
    /// Slot of finalize or expire (0 while pending).
    pub closed_slot: u64,
    pub result_uri_len: u16,
    pub result_uri: [u8; MAX_URI],
    pub spec: JobSpec,
}

/// Byte offsets inside an `Order` account (including the 8-byte Anchor
/// discriminator), so another program can read it without this crate.
pub mod layout {
    pub const DISCRIMINATOR: usize = 0;
    pub const VERSION: usize = 8;
    pub const STATUS: usize = 9;
    pub const VENUE: usize = 10;
    pub const BUMP: usize = 11;
    pub const DEPOSIT_NONCE: usize = 12;
    pub const JOB_ID: usize = 20;
    pub const BUYER: usize = 52;
    pub const RECIPIENT: usize = 84;
    pub const BODY_HASH: usize = 116;
    pub const SOL_SPENT: usize = 148;
    pub const TOKENS: usize = 156;
    pub const WORKER_PAY: usize = 164;
    pub const PROTOCOL_FEE: usize = 172;
    pub const CREATED_SLOT: usize = 180;
    pub const CREATED_TS: usize = 188;
    pub const DELIVERER: usize = 196;
    pub const RESULT_HASH: usize = 228;
    pub const CLOSED_SLOT: usize = 260;
    pub const RESULT_URI_LEN: usize = 268;
    pub const RESULT_URI: usize = 270;
    pub const SPEC: usize = 470;
    pub const SPEC_M: usize = SPEC;
    pub const SPEC_N: usize = SPEC + 4;
    pub const SPEC_K: usize = SPEC + 8;
    pub const SPEC_CONFIG: usize = SPEC + 12;
    pub const SPEC_HASH_A: usize = SPEC + 64;
    pub const SPEC_HASH_B: usize = SPEC + 96;
    pub const SPEC_DEADLINE_BLOCKS: usize = SPEC + 128;
    pub const SPEC_MAX_BLOCKS: usize = SPEC + 136;
    pub const SPEC_THRESHOLD: usize = SPEC + 140;
    pub const SPEC_EVALUATOR_COUNT: usize = SPEC + 141;
    pub const SPEC_EVALUATORS: usize = SPEC + 142;
    pub const SPEC_DATA_URI_LEN: usize = SPEC + 142 + 256;
    pub const SPEC_DATA_URI: usize = SPEC + 144 + 256;
    pub const LEN: usize = SPEC + 144 + 256 + 200;
}

/// What a composing program usually needs: (status, job_id, deliverer,
/// result_hash) from raw `Order` bytes. Checks the discriminator and size;
/// the caller must also check the account owner is this program.
pub fn read_result(data: &[u8]) -> Option<(u8, [u8; 32], Pubkey, [u8; 32])> {
    use layout::*;
    if data.len() < LEN || data[..8] != *Order::DISCRIMINATOR {
        return None;
    }
    let a32 = |o: usize| -> [u8; 32] { data[o..o + 32].try_into().unwrap() };
    Some((data[STATUS], a32(JOB_ID), Pubkey::new_from_array(a32(DELIVERER)), a32(RESULT_HASH)))
}

// ---------------------------------------------------------------- accounts

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(init, payer = admin, space = 8 + Config::INIT_SPACE, seeds = [CONFIG_SEED], bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [TRADER_SEED], bump)]
    pub trader: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AdminOnly<'info> {
    pub admin: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump, has_one = admin @ StorefrontError::Unauthorized)]
    pub config: Box<Account<'info, Config>>,
}

#[derive(Accounts)]
#[instruction(draft_id: u64)]
pub struct OpenOrder<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(
        init,
        payer = buyer,
        space = 8 + OrderDraft::INIT_SPACE,
        seeds = [DRAFT_SEED, buyer.key().as_ref(), &draft_id.to_le_bytes()],
        bump,
    )]
    pub draft: Box<Account<'info, OrderDraft>>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CancelDraft<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(
        mut,
        close = buyer,
        seeds = [DRAFT_SEED, buyer.key().as_ref(), &draft.draft_id.to_le_bytes()],
        bump = draft.bump,
        has_one = buyer @ StorefrontError::Unauthorized,
    )]
    pub draft: Box<Account<'info, OrderDraft>>,
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct BuyJobCurve<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(
        mut,
        close = buyer,
        seeds = [DRAFT_SEED, buyer.key().as_ref(), &draft.draft_id.to_le_bytes()],
        bump = draft.bump,
        has_one = buyer @ StorefrontError::Unauthorized,
    )]
    pub draft: Box<Account<'info, OrderDraft>>,
    #[account(init, payer = buyer, space = 8 + Order::INIT_SPACE, seeds = [ORDER_SEED, &nonce.to_le_bytes()], bump)]
    pub order: Box<Account<'info, Order>>,
    #[account(mut, seeds = [TRADER_SEED], bump = config.trader_bump)]
    pub trader: SystemAccount<'info>,
    /// CHECK: address = config.mint.
    #[account(address = config.mint @ StorefrontError::BadConfig)]
    pub mint: UncheckedAccount<'info>,
    /// CHECK: checked = ATA(trader, mint, Token-2022); created if missing.
    #[account(mut)]
    pub trader_ata: UncheckedAccount<'info>,
    /// CHECK: pump validates (ATA(trader, wSOL, SPL), unused for SOL coins).
    #[account(mut)]
    pub trader_wsol: UncheckedAccount<'info>,
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
    /// CHECK: owner = bridge_program; bridge validates the rest.
    #[account(mut)]
    pub bridge_config: UncheckedAccount<'info>,
    /// CHECK: bridge validates (has_one vault).
    #[account(mut)]
    pub bridge_vault: UncheckedAccount<'info>,
    /// CHECK: bridge validates (["deposit", nonce]); re-read after the CPI.
    #[account(mut)]
    pub bridge_deposit: UncheckedAccount<'info>,
    /// CHECK: address = config.bridge_program.
    #[account(address = config.bridge_program @ StorefrontError::BadConfig, executable)]
    pub bridge_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
#[instruction(nonce: u64)]
pub struct BuyJobAmm<'info> {
    #[account(mut)]
    pub buyer: Signer<'info>,
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(
        mut,
        close = buyer,
        seeds = [DRAFT_SEED, buyer.key().as_ref(), &draft.draft_id.to_le_bytes()],
        bump = draft.bump,
        has_one = buyer @ StorefrontError::Unauthorized,
    )]
    pub draft: Box<Account<'info, OrderDraft>>,
    #[account(init, payer = buyer, space = 8 + Order::INIT_SPACE, seeds = [ORDER_SEED, &nonce.to_le_bytes()], bump)]
    pub order: Box<Account<'info, Order>>,
    #[account(mut, seeds = [TRADER_SEED], bump = config.trader_bump)]
    pub trader: SystemAccount<'info>,
    /// CHECK: address = config.mint.
    #[account(address = config.mint @ StorefrontError::BadConfig)]
    pub mint: UncheckedAccount<'info>,
    /// CHECK: checked = ATA(trader, mint, Token-2022); created if missing.
    #[account(mut)]
    pub trader_ata: UncheckedAccount<'info>,
    /// CHECK: checked = ATA(trader, wSOL, SPL); created and closed here.
    #[account(mut)]
    pub trader_wsol: UncheckedAccount<'info>,
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
    /// CHECK: owner = bridge_program; bridge validates the rest.
    #[account(mut)]
    pub bridge_config: UncheckedAccount<'info>,
    /// CHECK: bridge validates (has_one vault).
    #[account(mut)]
    pub bridge_vault: UncheckedAccount<'info>,
    /// CHECK: bridge validates (["deposit", nonce]); re-read after the CPI.
    #[account(mut)]
    pub bridge_deposit: UncheckedAccount<'info>,
    /// CHECK: address = config.bridge_program.
    #[account(address = config.bridge_program @ StorefrontError::BadConfig, executable)]
    pub bridge_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct CloseOrder<'info> {
    /// Buyer, admin or an attester.
    pub caller: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,
    #[account(mut, seeds = [ORDER_SEED, &order.deposit_nonce.to_le_bytes()], bump = order.bump)]
    pub order: Box<Account<'info, Order>>,
    /// CHECK: address-checked instructions sysvar.
    #[account(address = ix_sysvar::ID)]
    pub instructions: UncheckedAccount<'info>,
}

impl<'info> CloseOrder<'info> {
    fn check_caller(&self) -> Result<()> {
        let k = self.caller.key();
        require!(k == self.order.buyer || k == self.config.admin || self.config.attesters.contains(&k), StorefrontError::Unauthorized);
        Ok(())
    }
}

// ---------------------------------------------------------------- events, errors

#[event]
pub struct ConfigUpdated {
    pub config: Pubkey,
}

#[event]
pub struct OrderPlaced {
    pub order: Pubkey,
    pub nonce: u64,
    pub job_id: [u8; 32],
    pub buyer: Pubkey,
    pub recipient: Pubkey,
    pub venue: u8,
    pub sol_spent: u64,
    pub tokens: u64,
    pub worker_pay: u64,
    pub protocol_fee: u64,
}

#[event]
pub struct OrderFinalized {
    pub order: Pubkey,
    pub nonce: u64,
    pub job_id: [u8; 32],
    pub deliverer: Pubkey,
    pub result_hash: [u8; 32],
}

#[event]
pub struct OrderExpired {
    pub order: Pubkey,
    pub nonce: u64,
    pub job_id: [u8; 32],
}

#[error_code]
pub enum StorefrontError {
    #[msg("signer is not allowed")]
    Unauthorized,
    #[msg("bad config value")]
    BadConfig,
    #[msg("attester set must be 1..=10 distinct keys with 1 <= threshold <= len")]
    BadAttesterSet,
    #[msg("unexpected account")]
    BadAccount,
    #[msg("job spec outside the configured caps")]
    BadSpec,
    #[msg("uri longer than 200 bytes")]
    UriTooLong,
    #[msg("storefront is paused")]
    Paused,
    #[msg("sol_in below min_sol_in")]
    BelowMinOrder,
    #[msg("sol_in above max_sol_in")]
    AboveMaxOrder,
    #[msg("nonce is not the bridge's next deposit nonce")]
    WrongNonce,
    #[msg("fill below min_tokens_out")]
    MinTokensOut,
    #[msg("fill worse than max_slippage_bps from the pre-trade quote")]
    Slippage,
    #[msg("spent more than sol_in")]
    Overspent,
    #[msg("trader PDA fell below rent exemption")]
    BelowFloor,
    #[msg("bonding curve is complete, use buy_job_amm")]
    CurveComplete,
    #[msg("order too small to split into W and P")]
    OrderTooSmall,
    #[msg("order is not pending")]
    NotPending,
    #[msg("not enough valid attester signatures over this message")]
    NotEnoughSignatures,
    #[msg("overflow")]
    Overflow,
}
