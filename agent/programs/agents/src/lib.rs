use anchor_lang::prelude::*;

declare_id!("7C92xwB9g7vStXqtwJLmuioFofKQjaxb8ATYAoNVkpit");

#[program]
pub mod agents {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        msg!("Greetings from: {:?}", ctx.program_id);
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize {}
