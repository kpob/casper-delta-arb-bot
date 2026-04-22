// use casper_delta_contracts::price_data::PriceData;
use odra::{casper_types::U256, prelude::Addressable};
use odra_cli::scenario::Error;

use super::path::Path;
use crate::{
    bot::{casper_delta::data::PriceData, utils},
    contracts::ContractRefs,
};

pub struct PriceCalculator<'a> {
    contracts: &'a ContractRefs<'a>,
}

impl<'a> PriceCalculator<'a> {
    pub fn new(contracts: &'a ContractRefs<'a>) -> Self {
        Self { contracts }
    }

    pub fn price_data(&self) -> Result<PriceData, Error> {
        let (long_dex_rate, short_dex_rate) = self.casper_trade_rates()?;
        let (long_protocol_price, short_protocol_price, wcspr_price) = self.protocol_prices()?;

        Ok(PriceData::new(
            long_dex_rate,
            short_dex_rate,
            long_protocol_price,
            short_protocol_price,
            wcspr_price,
        ))
    }

    pub fn effective_price(&self, price_data: PriceData, path: Path) -> Result<Vec<U256>, Error> {
        let amount_in = price_data.amount_per_trade_unit(path);
        let address_path = path.build(self.contracts)?;
        let amounts = self
            .contracts
            .router()?
            .try_get_amounts_out(amount_in, address_path)
            .map_err(|e| Error::OdraError {
                message: format!("Failed to get amounts out: {:?}", e),
            })?;
        if let [amount_in, .., amount_out] = amounts.as_slice() {
            Ok(vec![*amount_in, *amount_out])
        } else {
            Err(Error::OdraError {
                message: "Invalid swap result".to_string(),
            })
        }
    }

    /// Calculates the long and short token prices on the DEX based on the pair reserves.
    /// Does not reflect swap fees or price impact from the trade size.
    fn casper_trade_rates(&self) -> Result<(f64, f64), Error> {
        let (reserves_long, reserves_wcspr_long, _) =
            self.contracts.long_wcspr_pair()?.get_reserves();
        let (reserves_wcspr_short, reserves_short, _) =
            self.contracts.wcspr_short_pair()?.get_reserves();

        let long_token_rate = utils::calculate_price(reserves_wcspr_long, reserves_long);
        let short_token_rate = utils::calculate_price(reserves_wcspr_short, reserves_short);

        Ok((long_token_rate, short_token_rate))
    }

    /// Prices result from the protocol design.
    fn protocol_prices(&self) -> Result<(f64, f64, f64), Error> {
        let market = self.contracts.market()?;
        let state = market
            .try_get_address_market_state(market.address())?
            .market_state;
        let long_price = utils::calculate_price(state.long_liquidity, state.long_total_supply);
        let short_price = utils::calculate_price(state.short_liquidity, state.short_total_supply);
        let wcspr_price = state.price().as_u64() as f64 / 100_000.0f64;

        Ok((long_price, short_price, wcspr_price))
    }

    pub fn cspr_profit(
        amount_in: U256,
        amount_out: U256,
        price_data: PriceData,
        path: Path,
    ) -> f64 {
        let amount_in = amount_in.as_u64() as f64;
        let amount_out = amount_out.as_u64() as f64;
        let average_transaction_cost = if path.is_multi_hop() { 12.5f64 } else { 7.0f64 };
        let (amount_in_cspr, amount_out_cspr) = match path {
            Path::LongWcsprShort => (
                amount_in * price_data.long_protocol_price,
                amount_out * price_data.short_protocol_price,
            ),
            Path::ShortWcsprLong => (
                amount_in * price_data.short_protocol_price,
                amount_out * price_data.long_protocol_price,
            ),
            Path::LongWcspr => (amount_in * price_data.long_protocol_price, amount_out),
            Path::ShortWcspr => (amount_in * price_data.short_protocol_price, amount_out),
            Path::WcsprLong => (amount_in, amount_out * price_data.long_protocol_price),
            Path::WcsprShort => (amount_in, amount_out * price_data.short_protocol_price),
            Path::Empty => return 0.0f64,
        };
        (amount_out_cspr - amount_in_cspr) / 1_000_000_000.0f64 - average_transaction_cost
    }
}
