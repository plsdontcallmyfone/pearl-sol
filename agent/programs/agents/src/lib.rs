//! Agent identity on the agent chain (SPEC 3.2).
//!
//! One profile PDA per authority: `["agent", authority]`. M2 replaces this with,
//! or bridges it to, the Solana Agent Registry (ERC-8004 port).

use anchor_lang::prelude::*;

declare_id!("7C92xwB9g7vStXqtwJLmuioFofKQjaxb8ATYAoNVkpit");

pub const AGENT_SEED: &[u8] = b"agent";
pub const MAX_NAME_LEN: usize = 32;
pub const MAX_URI_LEN: usize = 200;

#[program]
pub mod agents {
    use super::*;

    /// Create the caller's profile. `payout` is where the agent wants to be paid.
    pub fn register(ctx: Context<Register>, name: String, uri: String, payout: Pubkey) -> Result<()> {
        check_fields(&name, &uri)?;
        let clock = Clock::get()?;
        let p = &mut ctx.accounts.profile;
        p.authority = ctx.accounts.authority.key();
        p.name = name;
        p.uri = uri;
        p.payout = payout;
        p.registered_slot = clock.slot;
        p.updated_slot = clock.slot;
        p.bump = ctx.bumps.profile;
        emit!(AgentRegistered { authority: p.authority, payout });
        Ok(())
    }

    /// Change any of name, uri, payout. `None` keeps the current value.
    pub fn update_profile(
        ctx: Context<UpdateProfile>,
        name: Option<String>,
        uri: Option<String>,
        payout: Option<Pubkey>,
    ) -> Result<()> {
        let p = &mut ctx.accounts.profile;
        if let Some(n) = name {
            p.name = n;
        }
        if let Some(u) = uri {
            p.uri = u;
        }
        if let Some(k) = payout {
            p.payout = k;
        }
        check_fields(&p.name, &p.uri)?;
        p.updated_slot = Clock::get()?.slot;
        emit!(AgentUpdated { authority: p.authority, payout: p.payout });
        Ok(())
    }

    /// Delete the profile and return its rent to the authority.
    pub fn deregister(_ctx: Context<Deregister>) -> Result<()> {
        Ok(())
    }
}

fn check_fields(name: &str, uri: &str) -> Result<()> {
    require!(!name.is_empty() && name.len() <= MAX_NAME_LEN, AgentsError::BadName);
    require!(uri.len() <= MAX_URI_LEN, AgentsError::UriTooLong);
    Ok(())
}

#[account]
#[derive(InitSpace)]
pub struct AgentProfile {
    pub authority: Pubkey,
    #[max_len(32)]
    pub name: String,
    #[max_len(200)]
    pub uri: String,
    pub payout: Pubkey,
    pub registered_slot: u64,
    pub updated_slot: u64,
    pub bump: u8,
}

#[derive(Accounts)]
pub struct Register<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        init,
        payer = authority,
        space = 8 + AgentProfile::INIT_SPACE,
        seeds = [AGENT_SEED, authority.key().as_ref()],
        bump
    )]
    pub profile: Account<'info, AgentProfile>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateProfile<'info> {
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [AGENT_SEED, authority.key().as_ref()],
        bump = profile.bump,
        has_one = authority
    )]
    pub profile: Account<'info, AgentProfile>,
}

#[derive(Accounts)]
pub struct Deregister<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        mut,
        close = authority,
        seeds = [AGENT_SEED, authority.key().as_ref()],
        bump = profile.bump,
        has_one = authority
    )]
    pub profile: Account<'info, AgentProfile>,
}

#[event]
pub struct AgentRegistered {
    pub authority: Pubkey,
    pub payout: Pubkey,
}

#[event]
pub struct AgentUpdated {
    pub authority: Pubkey,
    pub payout: Pubkey,
}

#[error_code]
pub enum AgentsError {
    #[msg("name must be 1..=32 bytes")]
    BadName,
    #[msg("uri longer than 200 bytes")]
    UriTooLong,
}
