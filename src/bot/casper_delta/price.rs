use crate::{bot::utils, contracts::ContractRefs};

use super::path::Path;
use odra::{casper_types::U256, prelude::Addressable};
use odra_cli::scenario::Error;
use std::fmt::Display;

const DECIMAL_PLACES: u32 = 9;

#[derive(Debug, Clone, Copy)]
pub struct PriceData {
    pub long_dex_rate: f64,
    pub short_dex_rate: f64,
    pub wcspr_price: f64,
    pub long_protocol_price: f64,
    pub short_protocol_price: f64,
    pub long_diff_percent: f64,
    pub short_diff_percent: f64,
    pub longs_per_trade_unit: U256,
    pub shorts_per_trade_unit: U256,
    pub wcspr_per_trade_unit: U256,
}

impl PriceData {
    pub fn new(
        long_dex_rate: f64,
        short_dex_rate: f64,
        wcspr_price: f64,
        long_protocol_price: f64,
        short_protocol_price: f64,
    ) -> Self {
        let trade_size_usd = std::env::var("BOT_DELTA_TRADE_SIZE_USD")
            .unwrap_or_else(|_| "1".to_string())
            .parse::<f64>()
            .expect("Invalid BOT_DELTA_TRADE_SIZE_USD");

        let long_diff_percent = (long_dex_rate / long_protocol_price) * 100.0f64 - 100.0f64;
        let short_diff_percent = (short_dex_rate / short_protocol_price) * 100.0f64 - 100.0f64;
        let longs_per_trade_unit =
            U256::from((trade_size_usd / wcspr_price / long_protocol_price) as u64)
                * 10u64.pow(DECIMAL_PLACES);
        let shorts_per_trade_unit =
            U256::from((trade_size_usd / wcspr_price / short_protocol_price) as u64)
                * 10u64.pow(DECIMAL_PLACES);
        let wcspr_per_trade_unit =
            U256::from((trade_size_usd / wcspr_price) as u64) * 10u64.pow(DECIMAL_PLACES);

        Self {
            long_dex_rate,
            short_dex_rate,
            wcspr_price,
            long_protocol_price,
            short_protocol_price,
            long_diff_percent,
            short_diff_percent,
            longs_per_trade_unit,
            shorts_per_trade_unit,
            wcspr_per_trade_unit,
        }
    }

    pub fn amount_per_trade_unit(&self, path: Path) -> U256 {
        match path {
            Path::LongWcsprShort => self.longs_per_trade_unit,
            Path::ShortWcsprLong => self.shorts_per_trade_unit,
            Path::LongWcspr => self.longs_per_trade_unit,
            Path::ShortWcspr => self.shorts_per_trade_unit,
            Path::WcsprLong => self.wcspr_per_trade_unit,
            Path::WcsprShort => self.wcspr_per_trade_unit,
        }
    }
}

impl crate::bot::engine::LogPrices for PriceData {
    fn log(&self) {
        tracing::info!(
            long_dex_rate = self.long_dex_rate,
            short_dex_rate = self.short_dex_rate,
            wcspr_price = self.wcspr_price,
            long_protocol_price = self.long_protocol_price,
            short_protocol_price = self.short_protocol_price,
            "DEX prices (CSPR)"
        );
        tracing::info!(
            long_diff = format!("{:+.2}%", self.long_diff_percent),
            short_diff = format!("{:+.2}%", self.short_diff_percent),
            "Price deviations from fair value"
        );
        // tracing::info!(
        //     longs_per_trade_unit = self.longs_per_trade_unit.as_u64(),
        //     shorts_per_trade_unit = self.shorts_per_trade_unit.as_u64(),
        //     wcspr_per_trade_unit = self.wcspr_per_trade_unit.as_u64(),
        //     "Token amounts traded per ${} of trade size",
        //     self.trade_size_usd
        // );
    }
}

impl Display for PriceData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "========================")?;
        writeln!(
            f,
            "Long:  {:.6} CSPR  (fair {:.6}, diff {:+.2}%)",
            self.long_dex_rate, self.long_protocol_price, self.long_diff_percent
        )?;
        writeln!(
            f,
            "Short: {:.6} CSPR  (fair {:.6}, diff {:+.2}%)",
            self.short_dex_rate, self.short_protocol_price, self.short_diff_percent
        )?;
        writeln!(f, "WCSPR: {:.6} USD", self.wcspr_price)?;
        writeln!(
            f,
            "Per USD — Long: {}  Short: {}  WCSPR: {}",
            self.longs_per_trade_unit, self.shorts_per_trade_unit, self.wcspr_per_trade_unit
        )?;
        writeln!(f, "========================")
    }
}

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
            wcspr_price,
            long_protocol_price,
            short_protocol_price,
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
        };
        (amount_out_cspr - amount_in_cspr) / 1_000_000_000.0f64 - average_transaction_cost
    }
}
