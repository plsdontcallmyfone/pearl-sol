use anchor_lang::prelude::*;

declare_id!("7LgyoRiKjNXD1AkMQUePb7TzTvRewEXKKLVzXB5vEjc8");

#[program]
pub mod bridge {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        msg!("Greetings from: {:?}", ctx.program_id);
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}
