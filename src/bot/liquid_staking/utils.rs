use odra::casper_types::{U256, U512};
use odra::prelude::Addressable; // required to call .address() on HostRef types
use odra::uints::ToU512;
use odra_cli::scenario::Error;

const STAKE_AND_SELL_TX_COST_CSPR: f64 = 20.0; // unwrap + stake + swap
const BUY_AND_UNSTAKE_TX_COST_CSPR: f64 = 17.0; // swap + unstake + (later) claim

use super::data::LsPriceData;
use super::path::LsPath;
use crate::contracts::ContractRefs;

pub(super) struct LsPriceCalculator<'a> {
    contracts: &'a ContractRefs<'a>,
}

impl<'a> LsPriceCalculator<'a> {
    pub(super) fn new(contracts: &'a ContractRefs<'a>) -> Self {
        Self { contracts }
    }

    /// Fetches DEX price, fair price, and WCSPR/USD price.
    pub(super) fn prices(&self) -> Result<LsPriceData, Error> {
        // DEX price: WCSPR per sCSPR from the pair reserves.
        // Pair name is "WCSPR-StCSPR LP": token0 = WCSPR, token1 = sCSPR.
        let (reserves_wcspr, reserves_stcspr, _) =
            self.contracts.wcspr_stcspr_pair()?.get_reserves();
        let dex_price = Self::calculate_price(reserves_wcspr, reserves_stcspr);

        // Fair price: staked_cspr (U512 motes) / total_supply (U256 motes).
        let sc = self.contracts.staked_cspr()?;
        let staked: U512 = sc.staked_cspr();
        let supply: U256 = sc.total_supply();
        let fair_price = Self::calculate_price_u512(staked, supply);

        // WCSPR USD price: reuse the same Market oracle as Casper Delta.
        let market = self.contracts.market()?;
        let state = market
            .try_get_address_market_state(market.address())?
            .market_state;
        let wcspr_price = state.price().as_u64() as f64 / 100_000.0;

        Ok(LsPriceData::new(dex_price, fair_price, wcspr_price))
    }

    /// Calculates gain in CSPR for a completed (or simulated) LS swap.
    ///
    /// For `StakeAndSell`: `amount_in` is sCSPR motes fed to the DEX,
    ///   `amount_out` is WCSPR motes received.
    /// For `BuyAndUnstake`: `amount_in` is WCSPR motes spent on the DEX,
    ///   `amount_out` is sCSPR motes received.
    pub(super) fn calc_gains_in_cspr(
        amount_in: U256,
        amount_out: U256,
        price_data: &LsPriceData,
        path: LsPath,
    ) -> f64 {
        let (amount_in_cspr, amount_out_cspr, tx_cost) = match path {
            LsPath::StakeAndSell => (
                amount_in.as_u64() as f64 * price_data.fair_price,
                amount_out.as_u64() as f64,
                STAKE_AND_SELL_TX_COST_CSPR,
            ),
            LsPath::BuyAndUnstake => (
                amount_in.as_u64() as f64,
                amount_out.as_u64() as f64 * price_data.fair_price,
                BUY_AND_UNSTAKE_TX_COST_CSPR,
            ),
            LsPath::Empty => return 0.0,
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
