use odra::{casper_types::U256, prelude::Addressable};
use odra_cli::scenario::Error;

use crate::bot::{contracts::ContractRefs, data::PriceData, path::Path};

pub(super) struct PriceCalculator<'a> {
    contracts: &'a ContractRefs<'a>,
}

impl<'a> PriceCalculator<'a> {
    pub(super) fn new(contracts: &'a ContractRefs<'a>) -> Self {
        Self { contracts }
    }

    pub(super) fn casper_trade_prices(&self) -> Result<(f64, f64), Error> {
        let (reserves_long, reserves_wcspr_long, _) =
            self.contracts.long_wcspr_pair()?.get_reserves();
        let (reserves_wcspr_short, reserves_short, _) =
            self.contracts.wcspr_short_pair()?.get_reserves();

        let long_token_price = Self::calculate_price(reserves_wcspr_long, reserves_long);
        let short_token_price = Self::calculate_price(reserves_wcspr_short, reserves_short);

        Ok((long_token_price, short_token_price))
    }

    pub(super) fn fair_prices(&self) -> Result<(f64, f64, f64), Error> {
        let market = self.contracts.market()?;
        let state = market
            .get_address_market_state(market.address())
            .market_state;
        let long_token_price = Self::calculate_price(state.long_liquidity, state.long_total_supply);
        let short_token_price =
            Self::calculate_price(state.short_liquidity, state.short_total_supply);
        let wcspr_price = state.price().as_u64() as f64 / 100_000.0f64;

        Ok((long_token_price, short_token_price, wcspr_price))
    }

    fn calculate_price(amount0: U256, amount1: U256) -> f64 {
        (amount0 * U256::from(1_000_000) / amount1).as_u64() as f64 / 1000_000.0f64
    }

    pub(super) fn calc_gains_in_cspr(
        amount_in: U256,
        amount_out: U256,
        price_data: &PriceData,
        path: Path,
    ) -> f64 {
        let average_transaction_cost = if path.is_multi_hop() { 12.5f64 } else { 7.0f64 };
        let (amount_in_cspr, amount_out_cspr) = match path {
            Path::LongWcsprShort => (
                amount_in.as_u64() as f64 * price_data.long_fair_price,
                amount_out.as_u64() as f64 * price_data.short_fair_price,
            ),
            Path::ShortWcsprLong => (
                amount_in.as_u64() as f64 * price_data.short_fair_price,
                amount_out.as_u64() as f64 * price_data.long_fair_price,
            ),
            Path::LongWcspr => (
                amount_in.as_u64() as f64 * price_data.long_fair_price,
                amount_out.as_u64() as f64,
            ),
            Path::ShortWcspr => (
                amount_in.as_u64() as f64 * price_data.short_fair_price,
                amount_out.as_u64() as f64,
            ),
            Path::WcsprLong => (
                amount_in.as_u64() as f64,
                amount_out.as_u64() as f64 * price_data.long_fair_price,
            ),
            Path::WcsprShort => (
                amount_in.as_u64() as f64,
                amount_out.as_u64() as f64 * price_data.short_fair_price,
            ),
            Path::Empty => return 0.0f64,
        };
        (amount_out_cspr - amount_in_cspr) / 1_000_000_000.0f64 - average_transaction_cost
    }
}
