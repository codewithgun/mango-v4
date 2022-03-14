use anchor_lang::prelude::*;
use anchor_spl::dex;
use anchor_spl::token::{Token, TokenAccount};
use arrayref::array_refs;
use borsh::{BorshDeserialize, BorshSerialize};
use dex::serum_dex;
use num_enum::TryFromPrimitive;
use serum_dex::matching::Side;
use std::io::Write;
use std::num::NonZeroU64;

use crate::error::*;
use crate::state::*;

/// Unfortunately NewOrderInstructionV3 isn't borsh serializable.
///
/// Make a newtype and implement the traits for it.
pub struct NewOrderInstructionData(pub serum_dex::instruction::NewOrderInstructionV3);

/// mango-v3's deserialization code
fn unpack_dex_new_order_v3(
    data: &[u8; 46],
) -> Option<serum_dex::instruction::NewOrderInstructionV3> {
    let (
        &side_arr,
        &price_arr,
        &max_coin_qty_arr,
        &max_native_pc_qty_arr,
        &self_trade_behavior_arr,
        &otype_arr,
        &client_order_id_bytes,
        &limit_arr,
    ) = array_refs![data, 4, 8, 8, 8, 4, 4, 8, 2];

    let side = serum_dex::matching::Side::try_from_primitive(
        u32::from_le_bytes(side_arr).try_into().ok()?,
    )
    .ok()?;
    let limit_price = NonZeroU64::new(u64::from_le_bytes(price_arr))?;
    let max_coin_qty = NonZeroU64::new(u64::from_le_bytes(max_coin_qty_arr))?;
    let max_native_pc_qty_including_fees =
        NonZeroU64::new(u64::from_le_bytes(max_native_pc_qty_arr))?;
    let self_trade_behavior = serum_dex::instruction::SelfTradeBehavior::try_from_primitive(
        u32::from_le_bytes(self_trade_behavior_arr)
            .try_into()
            .ok()?,
    )
    .ok()?;
    let order_type = serum_dex::matching::OrderType::try_from_primitive(
        u32::from_le_bytes(otype_arr).try_into().ok()?,
    )
    .ok()?;
    let client_order_id = u64::from_le_bytes(client_order_id_bytes);
    let limit = u16::from_le_bytes(limit_arr);

    Some(serum_dex::instruction::NewOrderInstructionV3 {
        side,
        limit_price,
        max_coin_qty,
        max_native_pc_qty_including_fees,
        self_trade_behavior,
        order_type,
        client_order_id,
        limit,
    })
}

impl BorshDeserialize for NewOrderInstructionData {
    fn deserialize(buf: &mut &[u8]) -> std::result::Result<Self, std::io::Error> {
        let data: &[u8; 46] = buf[0..46]
            .try_into()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, e))?;
        *buf = &buf[46..];
        Ok(Self(unpack_dex_new_order_v3(data).ok_or(
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                error!(MangoError::SomeError),
            ),
        )?))
    }
}

impl BorshSerialize for NewOrderInstructionData {
    fn serialize<W: Write>(&self, writer: &mut W) -> std::result::Result<(), std::io::Error> {
        let d = &self.0;
        let side: u8 = d.side.into();
        // TODO: why use four bytes here? (also in deserialization above)
        writer.write(&(side as u32).to_le_bytes())?;
        writer.write(&u64::from(d.limit_price).to_le_bytes())?;
        writer.write(&u64::from(d.max_coin_qty).to_le_bytes())?;
        writer.write(&u64::from(d.max_native_pc_qty_including_fees).to_le_bytes())?;
        let self_trade_behavior: u8 = d.self_trade_behavior.into();
        writer.write(&(self_trade_behavior as u32).to_le_bytes())?;
        let order_type: u8 = d.order_type.into();
        writer.write(&(order_type as u32).to_le_bytes())?;
        writer.write(&u64::from(d.client_order_id).to_le_bytes())?;
        writer.write(&u16::from(d.limit).to_le_bytes())?;
        Ok(())
    }
}

#[derive(Accounts)]
pub struct PlaceSerumOrder<'info> {
    pub group: AccountLoader<'info, Group>,

    #[account(
        mut,
        has_one = group,
        has_one = owner,
    )]
    pub account: AccountLoader<'info, MangoAccount>,
    pub owner: Signer<'info>,

    #[account(
        mut,
        //constraint = open_orders in account.spot_open_orders_map
    )]
    pub open_orders: UncheckedAccount<'info>,

    #[account(
        has_one = group,
        has_one = serum_program,
        has_one = serum_market_external,
    )]
    pub serum_market: AccountLoader<'info, SerumMarket>,

    // TODO: limit?
    pub serum_program: UncheckedAccount<'info>,
    #[account(mut)]
    pub serum_market_external: UncheckedAccount<'info>,

    #[account(mut)]
    pub market_bids: UncheckedAccount<'info>,
    #[account(mut)]
    pub market_asks: UncheckedAccount<'info>,
    #[account(mut)]
    pub market_event_queue: UncheckedAccount<'info>,
    #[account(mut)]
    pub market_request_queue: UncheckedAccount<'info>,
    #[account(mut)]
    pub market_base_vault: UncheckedAccount<'info>,
    #[account(mut)]
    pub market_quote_vault: UncheckedAccount<'info>,

    // TODO: everything; do we need to pass both, or just payer?
    // TODO: if we potentially settle immediately, they all need to be mut?
    // TODO: Can we reduce the number of accounts by requiring the banks
    // to be in the remainingAccounts (where they need to be anyway, for
    // health checks)
    #[account(mut)]
    pub quote_bank: AccountLoader<'info, Bank>,
    #[account(mut)]
    pub quote_vault: Box<Account<'info, TokenAccount>>,
    #[account(mut)]
    pub base_bank: AccountLoader<'info, Bank>,
    #[account(mut)]
    pub base_vault: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn place_serum_order(
    ctx: Context<PlaceSerumOrder>,
    order: NewOrderInstructionData,
) -> Result<()> {
    // unwrap our newtype
    let order = order.0;

    let order_payer_token_account = match order.side {
        Side::Ask => ctx.accounts.base_vault.to_account_info(),
        Side::Bid => ctx.accounts.quote_vault.to_account_info(),
    };

    let context = CpiContext::new(
        ctx.accounts.serum_program.to_account_info(),
        dex::NewOrderV3 {
            // generic accounts
            market: ctx.accounts.serum_market_external.to_account_info(),
            request_queue: ctx.accounts.market_request_queue.to_account_info(),
            event_queue: ctx.accounts.market_event_queue.to_account_info(),
            market_bids: ctx.accounts.market_bids.to_account_info(),
            market_asks: ctx.accounts.market_asks.to_account_info(),
            coin_vault: ctx.accounts.market_base_vault.to_account_info(),
            pc_vault: ctx.accounts.market_quote_vault.to_account_info(),
            token_program: ctx.accounts.token_program.to_account_info(),
            rent: ctx.accounts.rent.to_account_info(),

            // user accounts
            open_orders: ctx.accounts.open_orders.to_account_info(),
            // NOTE: this is also the user token account authority!
            open_orders_authority: ctx.accounts.group.to_account_info(),
            order_payer_token_account,
        },
    );

    let group = ctx.accounts.group.load()?;
    let seeds = group_seeds!(group);
    dex::new_order_v3(
        context.with_signer(&[seeds]),
        order.side,
        order.limit_price,
        order.max_coin_qty,
        order.max_native_pc_qty_including_fees,
        order.self_trade_behavior,
        order.order_type,
        order.client_order_id,
        order.limit,
    )?;

    // Health check
    let account = ctx.accounts.account.load()?;
    let health = compute_health(&account, &ctx.remaining_accounts)?;
    msg!("health: {}", health);
    require!(health >= 0, MangoError::SomeError);

    Ok(())
}