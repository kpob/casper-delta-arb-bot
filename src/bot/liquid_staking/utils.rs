use odra::casper_types::{U256, U512};
use odra::prelude::Addressable; // required to call .address() on HostRef types
use odra::uints::ToU512;
use odra_cli::scenario::Error;

const STAKE_AND_SELL_TX_COST_CSPR: f64 = 20.0; // unwrap + stake + swap
const BUY_AND_UNSTAKE_TX_COST_CSPR: f64 = 17.0; // swap + unstake + (later) claim
const DECIMAL_PLACES: u32 = 9;

use super::data::PriceData;
use super::path::Path;
use crate::contracts::ContractRefs;

pub(super) struct LsPriceCalculator;

impl LsPriceCalculator {
    /// Fetches DEX price, fair price, and WCSPR/USD price.
    pub(super) fn prices(contracts: &ContractRefs) -> Result<PriceData, Error> {
        let trade_size_usd = std::env::var("BOT_STCSPR_TRADE_SIZE_USD")
            .unwrap_or_else(|_| "1".to_string())
            .parse::<f64>()
            .expect("Invalid BOT_STCSPR_TRADE_SIZE_USD");

        // WCSPR USD price: reuse the same Market oracle as Casper Delta.
        let market = contracts.market()?;
        let state = market
            .try_get_address_market_state(market.address())?
            .market_state;
        let wcspr_price = state.price().as_u64() as f64 / 100_000.0;
        let wcspr_in =
            U256::from((trade_size_usd / wcspr_price) as u64 * 10u64.pow(DECIMAL_PLACES));

        // DEX price: WCSPR per sCSPR from the pair reserves.
        // Pair name is "WCSPR-StCSPR LP": token0 = WCSPR, token1 = sCSPR.
        let (reserves_wcspr, reserves_stcspr, _) = contracts.wcspr_stcspr_pair()?.get_reserves();

        // Fair price: staked_cspr (U512 motes) / total_supply (U256 motes).
        let sc = contracts.staked_cspr()?;
        let staked: U512 = sc.staked_cspr();
        let supply: U256 = sc.total_supply();

        let fair_price = Self::calculate_price_u512(staked, supply);
        let dex_price =
            contracts
                .router()?
                .get_amount_out(wcspr_in, reserves_wcspr, reserves_stcspr);
        let dex_price = Self::calculate_price(dex_price, 1.into());

        Ok(PriceData::new(dex_price, fair_price, wcspr_price))
    }

    /// Calculates gain in CSPR for a completed (or simulated) LS swap.
    ///
    /// For `StakeAndSell`: `amount_in` is sCSPR motes fed to the DEX,
    ///   `amount_out` is WCSPR motes received.
    /// For `BuyAndUnstake`: `amount_in` is WCSPR motes spent on the DEX,
    ///   `amount_out` is sCSPR motes received.
    pub(super) fn cspr_profit(
        amount_in: U256,
        amount_out: U256,
        price_data: PriceData,
        path: Path,
    ) -> f64 {
        let (amount_in_cspr, amount_out_cspr, tx_cost) = match path {
            Path::StCsprCspr => (
                amount_in.as_u64() as f64 * price_data.protocol_price,
                amount_out.as_u64() as f64,
                STAKE_AND_SELL_TX_COST_CSPR,
            ),
            Path::CsprStCspr => (
                amount_in.as_u64() as f64,
                amount_out.as_u64() as f64 * price_data.protocol_price,
                BUY_AND_UNSTAKE_TX_COST_CSPR,
            ),
            Path::Empty => return 0.0,
        };
        (amount_out_cspr - amount_in_cspr) / 1_000_000_000.0 - tx_cost
    }

    fn calculate_price(amount0: U256, amount1: U256) -> f64 {
        (amount0 * U256::from(1_000_000u64) / amount1).as_u64() as f64 / 1_000_000.0
    }

    /// Same as `calculate_price` but accepts U512 as the numerator (for
    /// `staked_cspr()` which returns U512).
    fn calculate_price_u512(amount0: U512, amount1: U256) -> f64 {
        let scaled = amount0 * U512::from(1_000_000u64) / amount1.to_u512();
        scaled.as_u64() as f64 / 1_000_000.0
    }
}
