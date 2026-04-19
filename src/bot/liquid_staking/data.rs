use odra::casper_types::U256;

const DECIMAL_PLACES: u32 = 9;

#[derive(Debug)]
pub struct LsPriceData {
    pub dex_price: f64,
    pub fair_price: f64,
    pub diff: f64,
    pub stcspr_for_ten_usd: u64,
    pub wcspr_for_ten_usd: u64,
    pub wcspr_price: f64,
}

impl LsPriceData {
    pub fn new(dex_price: f64, fair_price: f64, wcspr_price: f64) -> Self {
        let diff = (dex_price / fair_price) * 100.0 - 100.0;
        // $10 / USD_per_CSPR = whole CSPR units needed
        let wcspr_for_ten_usd = (10.0 / wcspr_price) as u64;
        // sCSPR has a fair redemption rate > 1 CSPR, so divide by fair_price too
        let stcspr_for_ten_usd = (10.0 / wcspr_price / fair_price) as u64;
        Self {
            dex_price,
            fair_price,
            diff,
            stcspr_for_ten_usd,
            wcspr_for_ten_usd,
            wcspr_price,
        }
    }

    pub fn amount_per_ten_usd(&self, path: super::path::LsPath) -> U256 {
        match path {
            super::path::LsPath::StakeAndSell => {
                U256::from(self.stcspr_for_ten_usd * 10u64.pow(DECIMAL_PLACES))
            }
            super::path::LsPath::BuyAndUnstake => {
                U256::from(self.wcspr_for_ten_usd * 10u64.pow(DECIMAL_PLACES))
            }
            super::path::LsPath::Empty => U256::zero(),
        }
    }

    pub fn log(&self) {
        tracing::info!(
            dex_price = self.dex_price,
            fair_price = self.fair_price,
            diff = format!("{:+.2}%", self.diff),
            stcspr_for_ten_usd = self.stcspr_for_ten_usd,
            wcspr_for_ten_usd = self.wcspr_for_ten_usd,
            "LS prices (CSPR)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_computed_correctly() {
        let data = LsPriceData::new(1.05, 1.0, 0.04);
        // (1.05 / 1.0) * 100 - 100 = 5.0
        assert!((data.diff - 5.0).abs() < 0.001);
    }

    #[test]
    fn test_negative_diff_when_underpriced() {
        let data = LsPriceData::new(0.94, 1.0, 0.04);
        // (0.94 / 1.0) * 100 - 100 = -6.0
        assert!((data.diff - (-6.0)).abs() < 0.001);
    }

    #[test]
    fn test_wcspr_for_ten_usd() {
        // 10 / 0.04 = 250 whole WCSPR
        let data = LsPriceData::new(1.0, 1.0, 0.04);
        assert_eq!(data.wcspr_for_ten_usd, 250);
    }

    #[test]
    fn test_stcspr_for_ten_usd() {
        // 10 / 0.04 / 1.05 ≈ 238 whole sCSPR
        let data = LsPriceData::new(1.05, 1.05, 0.04);
        assert_eq!(data.stcspr_for_ten_usd, 238);
    }

    #[test]
    fn test_amount_per_ten_usd_buy_and_unstake() {
        // wcspr_for_ten_usd = 250, × 10^9 = 250_000_000_000
        let data = LsPriceData::new(1.0, 1.0, 0.04);
        use crate::bot::liquid_staking::path::LsPath;
        assert_eq!(
            data.amount_per_ten_usd(LsPath::BuyAndUnstake),
            U256::from(250_000_000_000u64)
        );
    }

    #[test]
    fn test_amount_per_ten_usd_stake_and_sell() {
        // stcspr_for_ten_usd = 250 (when fair_price=1.0), × 10^9
        let data = LsPriceData::new(1.0, 1.0, 0.04);
        use crate::bot::liquid_staking::path::LsPath;
        assert_eq!(
            data.amount_per_ten_usd(LsPath::StakeAndSell),
            U256::from(250_000_000_000u64)
        );
    }
}
