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

    #[cfg(test)]
    pub(super) fn calculate_price_pub(amount0: U256, amount1: U256) -> f64 {
        Self::calculate_price(amount0, amount1)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::data::PriceData;
    use crate::bot::path::Path;

    fn make_price_data(long_fair: f64, short_fair: f64) -> PriceData {
        // wcspr_price is only used for longs/shorts_for_one_usd, not by calc_gains_in_cspr
        PriceData::new(long_fair, short_fair, 0.05, long_fair, short_fair)
    }

    // ========== calculate_price Tests ==========

    #[test]
    fn test_calculate_price_equal_reserves() {
        // Equal reserves → price = 1.0
        let price = PriceCalculator::calculate_price_pub(
            U256::from(1_000_000u64),
            U256::from(1_000_000u64),
        );
        assert!((price - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_calculate_price_two_to_one() {
        // amount0 = 2 × amount1 → price = 2.0
        let price = PriceCalculator::calculate_price_pub(
            U256::from(2_000_000u64),
            U256::from(1_000_000u64),
        );
        assert!((price - 2.0).abs() < 1e-6);
    }

    #[test]
    fn test_calculate_price_half() {
        // amount0 = 0.5 × amount1 → price = 0.5
        let price = PriceCalculator::calculate_price_pub(
            U256::from(500_000u64),
            U256::from(1_000_000u64),
        );
        assert!((price - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_calculate_price_large_reserves() {
        // 2×10^18 / 10^18 = 2.0 (large but safe values)
        let price = PriceCalculator::calculate_price_pub(
            U256::from(2_000_000_000_000_000_000u128),
            U256::from(1_000_000_000_000_000_000u128),
        );
        assert!((price - 2.0).abs() < 1e-3);
    }

    #[test]
    fn test_calculate_price_six_decimal_precision() {
        // amount0 = 1_000_001, amount1 = 1_000_000 → price ≈ 1.000001
        let price = PriceCalculator::calculate_price_pub(
            U256::from(1_000_001u64),
            U256::from(1_000_000u64),
        );
        assert!((price - 1.000001).abs() < 1e-6);
    }

    // ========== calc_gains_in_cspr Tests ==========

    #[test]
    fn test_calc_gains_empty_path_returns_zero() {
        let data = make_price_data(0.5, 0.5);
        let gain = PriceCalculator::calc_gains_in_cspr(
            U256::from(100_000_000_000u64),
            U256::from(200_000_000_000u64),
            &data,
            Path::Empty,
        );
        assert_eq!(gain, 0.0);
    }

    #[test]
    fn test_calc_gains_long_wcspr_short_profitable() {
        // amount_in = 100×10^9 long-motes, long_fair = 0.5 → in_cspr = 50×10^9
        // amount_out = 200×10^9 short-motes, short_fair = 0.5 → out_cspr = 100×10^9
        // gain = (100e9 - 50e9) / 1e9 - 12.5 = 50 - 12.5 = 37.5
        let data = make_price_data(0.5, 0.5);
        let gain = PriceCalculator::calc_gains_in_cspr(
            U256::from(100_000_000_000u64),
            U256::from(200_000_000_000u64),
            &data,
            Path::LongWcsprShort,
        );
        assert!((gain - 37.5).abs() < 1e-6);
    }

    #[test]
    fn test_calc_gains_short_wcspr_long_profitable() {
        // Symmetric to LongWcsprShort with in=short, out=long
        // amount_in = 100×10^9 short-motes, short_fair = 0.5 → in_cspr = 50×10^9
        // amount_out = 200×10^9 long-motes, long_fair = 0.5 → out_cspr = 100×10^9
        // gain = (100e9 - 50e9) / 1e9 - 12.5 = 37.5
        let data = make_price_data(0.5, 0.5);
        let gain = PriceCalculator::calc_gains_in_cspr(
            U256::from(100_000_000_000u64),
            U256::from(200_000_000_000u64),
            &data,
            Path::ShortWcsprLong,
        );
        assert!((gain - 37.5).abs() < 1e-6);
    }

    #[test]
    fn test_calc_gains_long_wcspr_uses_7_cspr_cost() {
        // amount_in = 100×10^9 long-motes, long_fair = 0.5 → in_cspr = 50×10^9
        // amount_out = 60×10^9 raw WCSPR motes → out_cspr = 60×10^9
        // gain = (60e9 - 50e9) / 1e9 - 7.0 = 10 - 7 = 3.0
        let data = make_price_data(0.5, 0.5);
        let gain = PriceCalculator::calc_gains_in_cspr(
            U256::from(100_000_000_000u64),
            U256::from(60_000_000_000u64),
            &data,
            Path::LongWcspr,
        );
        assert!((gain - 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_calc_gains_short_wcspr_uses_7_cspr_cost() {
        // amount_in = 100×10^9 short-motes, short_fair = 0.5 → in_cspr = 50×10^9
        // amount_out = 60×10^9 raw WCSPR motes → out_cspr = 60×10^9
        // gain = (60e9 - 50e9) / 1e9 - 7.0 = 3.0
        let data = make_price_data(0.5, 0.5);
        let gain = PriceCalculator::calc_gains_in_cspr(
            U256::from(100_000_000_000u64),
            U256::from(60_000_000_000u64),
            &data,
            Path::ShortWcspr,
        );
        assert!((gain - 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_calc_gains_wcspr_long_profitable() {
        // amount_in = 40×10^9 raw WCSPR motes → in_cspr = 40×10^9
        // amount_out = 100×10^9 long-motes, long_fair = 0.5 → out_cspr = 50×10^9
        // gain = (50e9 - 40e9) / 1e9 - 7.0 = 10 - 7 = 3.0
        let data = make_price_data(0.5, 0.5);
        let gain = PriceCalculator::calc_gains_in_cspr(
            U256::from(40_000_000_000u64),
            U256::from(100_000_000_000u64),
            &data,
            Path::WcsprLong,
        );
        assert!((gain - 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_calc_gains_wcspr_short_profitable() {
        // amount_in = 40×10^9 raw WCSPR, amount_out = 100×10^9 short-motes, short_fair = 0.5
        // gain = (50e9 - 40e9) / 1e9 - 7.0 = 3.0
        let data = make_price_data(0.5, 0.5);
        let gain = PriceCalculator::calc_gains_in_cspr(
            U256::from(40_000_000_000u64),
            U256::from(100_000_000_000u64),
            &data,
            Path::WcsprShort,
        );
        assert!((gain - 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_calc_gains_multi_hop_costs_more_than_single_hop() {
        // Same in/out amounts: multi-hop deducts 12.5 CSPR, single-hop deducts 7.0 CSPR
        // Multi-hop: (100e9 - 50e9) / 1e9 - 12.5 = 37.5
        // Single-hop LongWcspr: (60e9 - 50e9) / 1e9 - 7.0 = 3.0
        // Use this to verify the two costs are distinct
        let data = make_price_data(0.5, 0.5);
        let multi_gain = PriceCalculator::calc_gains_in_cspr(
            U256::from(100_000_000_000u64),
            U256::from(200_000_000_000u64),
            &data,
            Path::LongWcsprShort,
        );
        let single_gain = PriceCalculator::calc_gains_in_cspr(
            U256::from(100_000_000_000u64),
            U256::from(200_000_000_000u64),
            &data,
            Path::LongWcspr,
        );
        // multi deducts 12.5, single deducts 7.0 → single gain is 5.5 higher
        assert!((single_gain - multi_gain - 5.5).abs() < 1e-6);
    }

    #[test]
    fn test_calc_gains_unprofitable_swap_is_negative() {
        // amount_in = 100e9, long_fair = 0.5 → in_cspr = 50e9
        // amount_out = 50e9 (raw wcspr) → out_cspr = 50e9
        // gain = (50e9 - 50e9) / 1e9 - 7.0 = -7.0
        let data = make_price_data(0.5, 0.5);
        let gain = PriceCalculator::calc_gains_in_cspr(
            U256::from(100_000_000_000u64),
            U256::from(50_000_000_000u64),
            &data,
            Path::LongWcspr,
        );
        assert!(gain < 0.0);
    }

    #[test]
    fn test_calc_gains_exactly_at_1_cspr_boundary() {
        // Verify a gain of exactly 1.0 CSPR (the bot's profit gate uses `gain < 1.0`,
        // so exactly 1.0 is considered actionable)
        // For LongWcspr: (out_cspr - in_cspr) / 1e9 - 7.0 = 1.0
        // → out_cspr - in_cspr = 8e9
        // With in = 100e9, long_fair = 0.5 → in_cspr = 50e9; out = 58e9
        let data = make_price_data(0.5, 0.5);
        let gain = PriceCalculator::calc_gains_in_cspr(
            U256::from(100_000_000_000u64),
            U256::from(58_000_000_000u64),
            &data,
            Path::LongWcspr,
        );
        assert!((gain - 1.0).abs() < 1e-6);
        // Confirm it would NOT be skipped by the bot's `gain < 1.0` guard
        assert!(!(gain < 1.0));
    }
}
