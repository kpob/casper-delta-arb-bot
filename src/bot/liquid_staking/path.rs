use odra::prelude::{Address, Addressable};
use odra_cli::scenario::Error;

use crate::contracts::ContractRefs;
use super::data::LsPriceData;

const STAKE_AND_SELL_THRESHOLD: f64 = 2.5;
const BUY_AND_UNSTAKE_THRESHOLD: f64 = 5.0;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum LsPath {
    StakeAndSell,
    BuyAndUnstake,
    Empty,
}

impl From<&LsPriceData> for LsPath {
    fn from(data: &LsPriceData) -> Self {
        Self::calc(data)
    }
}

impl LsPath {
    fn calc(data: &LsPriceData) -> Self {
        if data.diff > STAKE_AND_SELL_THRESHOLD {
            LsPath::StakeAndSell
        } else if data.diff < -BUY_AND_UNSTAKE_THRESHOLD {
            LsPath::BuyAndUnstake
        } else {
            LsPath::Empty
        }
    }

    /// Returns the DEX swap path as a list of token addresses.
    /// StakeAndSell: sCSPR → WCSPR; BuyAndUnstake: WCSPR → sCSPR.
    pub fn build(&self, refs: &ContractRefs) -> Result<Vec<Address>, Error> {
        let wcspr = refs.wcspr()?.address();
        let stcspr = refs.staked_cspr()?.address();
        match self {
            LsPath::StakeAndSell => Ok(vec![stcspr, wcspr]),
            LsPath::BuyAndUnstake => Ok(vec![wcspr, stcspr]),
            LsPath::Empty => Ok(vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price_data(dex: f64, fair: f64) -> LsPriceData {
        LsPriceData::new(dex, fair, 0.04)
    }

    #[test]
    fn test_stake_and_sell_when_overpriced_above_threshold() {
        // diff = +3.0% > 2.5% → StakeAndSell
        let data = price_data(1.03, 1.0);
        assert_eq!(LsPath::from(&data), LsPath::StakeAndSell);
    }

    #[test]
    fn test_empty_when_overpriced_below_threshold() {
        // diff = +1.0% < 2.5% → Empty
        let data = price_data(1.01, 1.0);
        assert_eq!(LsPath::from(&data), LsPath::Empty);
    }

    #[test]
    fn test_buy_and_unstake_when_underpriced_above_threshold() {
        // diff = -6.0% and abs > 5.0% → BuyAndUnstake
        let data = price_data(0.94, 1.0);
        assert_eq!(LsPath::from(&data), LsPath::BuyAndUnstake);
    }

    #[test]
    fn test_empty_when_underpriced_below_5_percent_threshold() {
        // diff = -3.0%, abs < 5.0% → Empty (not BuyAndUnstake)
        let data = price_data(0.97, 1.0);
        assert_eq!(LsPath::from(&data), LsPath::Empty);
    }

    #[test]
    fn test_empty_at_exact_stake_and_sell_boundary() {
        // f64: 1.025/1.0*100-100 ≈ 2.4999… (just below 2.5 due to precision) → Empty
        let data = price_data(1.025, 1.0);
        assert_eq!(LsPath::from(&data), LsPath::Empty);
    }

    #[test]
    fn test_stake_and_sell_just_above_boundary() {
        let data = price_data(1.0251, 1.0);
        assert_eq!(LsPath::from(&data), LsPath::StakeAndSell);
    }
}
