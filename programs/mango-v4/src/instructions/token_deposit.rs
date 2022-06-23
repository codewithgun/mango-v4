use anchor_lang::prelude::*;
use anchor_spl::token;
use anchor_spl::token::Token;
use anchor_spl::token::TokenAccount;
use fixed::types::I80F48;

use crate::error::*;
use crate::state::*;

#[derive(Accounts)]
pub struct TokenDeposit<'info> {
    pub group: AccountLoader<'info, Group>,

    #[account(
        mut,
        has_one = group,
    )]
    pub account: AccountLoader<'info, MangoAccount>,

    #[account(
        mut,
        has_one = group,
        has_one = vault,
        // the mints of bank/vault/token_account are implicitly the same because
        // spl::token::transfer succeeds between token_account and vault
    )]
    pub bank: AccountLoader<'info, Bank>,

    #[account(mut)]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub token_account: Box<Account<'info, TokenAccount>>,
    pub token_authority: Signer<'info>,

    pub token_program: Program<'info, Token>,
}

impl<'info> TokenDeposit<'info> {
    pub fn transfer_ctx(&self) -> CpiContext<'_, '_, '_, 'info, token::Transfer<'info>> {
        let program = self.token_program.to_account_info();
        let accounts = token::Transfer {
            from: self.token_account.to_account_info(),
            to: self.vault.to_account_info(),
            authority: self.token_authority.to_account_info(),
        };
        CpiContext::new(program, accounts)
    }
}

// TODO: It may make sense to have the token_index passed in from the outside.
//       That would save a lot of computation that needs to go into finding the
//       right index for the mint.
pub fn token_deposit(ctx: Context<TokenDeposit>, amount: u64) -> Result<()> {
    require!(amount > 0, MangoError::SomeError);

    let token_index = ctx.accounts.bank.load()?.token_index;

    // Get the account's position for that token index
    let mut account = ctx.accounts.account.load_mut()?;
    require!(account.is_bankrupt == 0, MangoError::IsBankrupt);

    let (position, position_index) = account.tokens.get_mut_or_create(token_index)?;

    // Update the bank and position
    let position_is_active = {
        let mut bank = ctx.accounts.bank.load_mut()?;
        bank.deposit(position, I80F48::from(amount))?
    };

    // Transfer the actual tokens
    token::transfer(ctx.accounts.transfer_ctx(), amount)?;

    //
    // Health computation
    // TODO: This will be used to disable is_bankrupt or being_liquidated
    //       when health recovers sufficiently
    //
    let health =
        compute_health_from_fixed_accounts(&account, HealthType::Init, ctx.remaining_accounts)?;
    msg!("health: {}", health);

    //
    // Deactivate the position only after the health check because the user passed in
    // remaining_accounts for all banks/oracles, including the account that will now be
    // deactivated.
    // Deposits can deactivate a position if they cancel out a previous borrow.
    //
    if !position_is_active {
        account.tokens.deactivate(position_index);
    }

    Ok(())
}