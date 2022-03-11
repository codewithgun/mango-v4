use anchor_lang::prelude::*;

use crate::error::*;
use crate::state::*;

#[derive(Accounts)]
pub struct CreateGroup<'info> {
    #[account(
        init,
        seeds = [b"group".as_ref(), admin.key().as_ref()],
        bump,
        payer = payer,
        space = 8 + std::mem::size_of::<Group>(),
    )]
    pub group: AccountLoader<'info, Group>,

    pub admin: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn create_group(ctx: Context<CreateGroup>) -> Result<()> {
    let mut group = ctx.accounts.group.load_init()?;
    group.admin = ctx.accounts.admin.key();
    group.bump = *ctx.bumps.get("group").ok_or(MangoError::SomeError)?;
    Ok(())
}
