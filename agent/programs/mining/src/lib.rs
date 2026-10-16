use anchor_lang::prelude::*;

declare_id!("3GrrgQMSHC3PjkW6UmvQJPGK1WBvhxiz3tkigafj3GhP");

#[program]
pub mod mining {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        msg!("Greetings from: {:?}", ctx.program_id);
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}
