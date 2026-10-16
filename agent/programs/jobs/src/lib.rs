use anchor_lang::prelude::*;

declare_id!("7e9pEhjwEqKemjC6zUugp4isi2zQMvaZPhn9kdnDuzqL");

#[program]
pub mod jobs {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        msg!("Greetings from: {:?}", ctx.program_id);
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}
