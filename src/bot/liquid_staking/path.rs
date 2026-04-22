use odra::prelude::{Address, Addressable};
use odra_cli::scenario::Error;

use super::price::PriceData;
use crate::contracts::ContractRefs;

const STAKE_AND_SELL_THRESHOLD: f64 = 2.5;
const BUY_AND_UNSTAKE_THRESHOLD: f64 = 5.0;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Path {
    StCsprCspr,
    CsprStCspr,
}

impl Path {
    pub fn select(data: PriceData) -> Option<Self> {
        if data.diff_percentage > STAKE_AND_SELL_THRESHOLD {
            Some(Path::StCsprCspr)
        } else if data.diff_percentage < -BUY_AND_UNSTAKE_THRESHOLD {
            Some(Path::CsprStCspr)
        } else {
            None
        }
    }

    /// Returns the DEX swap path as a list of token addresses.
    /// StCsprCspr: sCSPR → WCSPR; CsprStCspr: WCSPR → sCSPR.
    pub fn build(&self, refs: &ContractRefs) -> Result<Vec<Address>, Error> {
        let wcspr = refs.wcspr()?.address();
        let stcspr = refs.staked_cspr()?.address();
        match self {
            Path::StCsprCspr => Ok(vec![stcspr, wcspr]),
            Path::CsprStCspr => Ok(vec![wcspr, stcspr]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price_data(dex: f64, fair: f64) -> PriceData {
        PriceData::new(dex, fair, 0.04)
    }

    #[test]
    fn test_stake_and_sell_when_overpriced_above_threshold() {
        // diff = +3.0% > 2.5% → StakeAndSell
        let data = price_data(1.03, 1.0);
        assert_eq!(Path::select(data), Some(Path::StCsprCspr));
    }

    #[test]
    fn test_empty_when_overpriced_below_threshold() {
        // diff = +1.0% < 2.5% → Empty
        let data = price_data(1.01, 1.0);
        assert_eq!(Path::select(data), None);
    }

    #[test]
    fn test_buy_and_unstake_when_underpriced_above_threshold() {
        // diff = -6.0% and abs > 5.0% → BuyAndUnstake
        let data = price_data(0.94, 1.0);
        assert_eq!(Path::select(data), Some(Path::CsprStCspr));
    }

    #[test]
    fn test_empty_when_underpriced_below_5_percent_threshold() {
        // diff = -3.0%, abs < 5.0% → Empty (not BuyAndUnstake)
        let data = price_data(0.97, 1.0);
        assert_eq!(Path::select(data), None);
    }

    #[test]
    fn test_empty_at_exact_stake_and_sell_boundary() {
        // f64: 1.025/1.0*100-100 ≈ 2.4999… (just below 2.5 due to precision) → Empty
        let data = price_data(1.025, 1.0);
        assert_eq!(Path::select(data), None);
    }

    #[test]
    fn test_stake_and_sell_just_above_boundary() {
        let data = price_data(1.0251, 1.0);
        assert_eq!(Path::select(data), Some(Path::StCsprCspr));
    }
}
