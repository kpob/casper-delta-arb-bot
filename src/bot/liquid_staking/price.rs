use crate::{
    bot::{liquid_staking::path::Path, utils},
    contracts::ContractRefs,
};
use odra::{
    casper_types::{U256, U512},
    prelude::Addressable,
};
use odra_cli::scenario::Error;

const STAKE_AND_SELL_TX_COST_CSPR: f64 = 20.0; // unwrap + stake + swap
const BUY_AND_UNSTAKE_TX_COST_CSPR: f64 = 17.0; // swap + unstake + (later) claim
const DECIMAL_PLACES: u32 = 9;

#[derive(Debug, Clone, Copy)]
pub(super) struct PriceData {
    pub trade_size_usd: f64,
    pub dex_rate: f64,
    pub protocol_price: f64,
    pub diff_percentage: f64,
    pub wcspr_price: f64,
    pub stcspr_for_trade_unit: U256,
    pub wcspr_for_trade_unit: U256,
}

impl PriceData {
    pub fn new(dex_rate: f64, protocol_price: f64, wcspr_price: f64) -> Self {
        let trade_size_usd = std::env::var("BOT_STCSPR_TRADE_SIZE_USD")
            .unwrap_or_else(|_| "1".to_string())
            .parse::<f64>()
            .expect("Invalid BOT_STCSPR_TRADE_SIZE_USD");

        let diff_percentage = (dex_rate / protocol_price) * 100.0 - 100.0;
        let wcspr_for_trade_unit =
            U256::from((trade_size_usd / wcspr_price) as u64 * 10u64.pow(DECIMAL_PLACES));
        let stcspr_for_trade_unit = U256::from(
            (trade_size_usd / wcspr_price / protocol_price) as u64 * 10u64.pow(DECIMAL_PLACES),
        );
        Self {
            trade_size_usd,
            dex_rate,
            protocol_price,
            diff_percentage,
            stcspr_for_trade_unit,
            wcspr_for_trade_unit,
            wcspr_price,
        }
    }

    pub fn amount_per_trade_unit(&self, path: super::path::Path) -> U256 {
        match path {
            super::path::Path::StCsprCspr => self.stcspr_for_trade_unit,
            super::path::Path::CsprStCspr => self.wcspr_for_trade_unit,
            super::path::Path::Empty => U256::zero(),
        }
    }

    pub fn log(&self) {
        tracing::info!(
            dex_rate = self.dex_rate,
            protocol_price = self.protocol_price,
            diff = format!("{:+.2}%", self.diff_percentage),
            wcspr_price_usd = self.wcspr_price,
            stcspr_for_trade_unit = self.stcspr_for_trade_unit.as_u64(),
            wcspr_for_trade_unit = self.wcspr_for_trade_unit.as_u64(),
            "LS prices (CSPR) for trade size: ${}",
            self.trade_size_usd
        );
    }
}

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

        let fair_price = utils::calculate_price_u512(staked, supply);
        let dex_price =
            contracts
                .router()?
                .get_amount_out(wcspr_in, reserves_wcspr, reserves_stcspr);
        let dex_price = utils::calculate_price(dex_price, 1.into());

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_computed_correctly() {
        let data = PriceData::new(1.05, 1.0, 0.04);
        // (1.05 / 1.0) * 100 - 100 = 5.0
        assert!((data.diff_percentage - 5.0).abs() < 0.001);
    }

    #[test]
    fn test_negative_diff_when_underpriced() {
        let data = PriceData::new(0.94, 1.0, 0.04);
        // (0.94 / 1.0) * 100 - 100 = -6.0
        assert!((data.diff_percentage - (-6.0)).abs() < 0.001);
    }

    #[test]
    fn test_wcspr_for_ten_usd() {
        // 10 / 0.04 = 250 whole WCSPR
        std::env::set_var("BOT_STCSPR_TRADE_SIZE_USD", "10");
        let data = PriceData::new(1.0, 1.0, 0.04);
        assert_eq!(data.wcspr_for_trade_unit, 250_000_000_000u64.into());
    }

    #[test]
    fn test_stcspr_for_ten_usd() {
        // 10 / 0.04 / 1.05 ≈ 238 whole sCSPR
        std::env::set_var("BOT_STCSPR_TRADE_SIZE_USD", "10");
        let data = PriceData::new(1.05, 1.05, 0.04);
        assert_eq!(data.stcspr_for_trade_unit, 238_000_000_000u64.into());
    }

    #[test]
    fn test_amount_per_ten_usd_buy_and_unstake() {
        // wcspr_for_ten_usd = 250, × 10^9 = 250_000_000_000
        std::env::set_var("BOT_STCSPR_TRADE_SIZE_USD", "10");
        let data = PriceData::new(1.0, 1.0, 0.04);
        use crate::bot::liquid_staking::path::Path;
        assert_eq!(
            data.amount_per_trade_unit(Path::CsprStCspr),
            U256::from(250_000_000_000u64)
        );
    }

    #[test]
    fn test_amount_per_ten_usd_stake_and_sell() {
        // stcspr_for_ten_usd = 250 (when fair_price=1.0), × 10^9
        std::env::set_var("BOT_STCSPR_TRADE_SIZE_USD", "10");
        let data = PriceData::new(1.0, 1.0, 0.04);
        use crate::bot::liquid_staking::path::Path;
        assert_eq!(
            data.amount_per_trade_unit(Path::StCsprCspr),
            U256::from(250_000_000_000u64)
        );
    }
}
