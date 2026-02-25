use std::fmt::Display;

use odra::casper_types::U256;

use crate::bot::path::Path;

const DECIMAL_PLACES: u32 = 9;

#[derive(Debug)]
pub struct PriceData {
    pub long_price: f64,
    pub short_price: f64,
    pub wcspr_price: f64,
    pub long_fair_price: f64,
    pub short_fair_price: f64,
    pub long_diff: f64,
    pub short_diff: f64,
    pub longs_for_one_usd: u64,
    pub shorts_for_one_usd: u64,
    pub wcspr_for_one_usd: u64,
}

impl PriceData {
    pub fn new(
        long_price: f64,
        short_price: f64,
        wcspr_price: f64,
        long_fair_price: f64,
        short_fair_price: f64,
    ) -> Self {
        let long_diff = (long_price / long_fair_price) * 100.0f64 - 100.0f64;
        let short_diff = (short_price / short_fair_price) * 100.0f64 - 100.0f64;
        let longs_for_one_usd = (1.0f64 / wcspr_price / long_fair_price) as u64;
        let shorts_for_one_usd = (1.0f64 / wcspr_price / short_fair_price) as u64;
        let wcspr_for_one_usd = (1.0f64 / wcspr_price) as u64;

        Self {
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
            long_diff,
            short_diff,
            longs_for_one_usd,
            shorts_for_one_usd,
            wcspr_for_one_usd,
        }
    }

    fn shorts_amount_per_usd(&self) -> U256 {
        U256::from(self.shorts_for_one_usd * 10u64.pow(DECIMAL_PLACES))
    }

    fn longs_amount_per_usd(&self) -> U256 {
        U256::from(self.longs_for_one_usd * 10u64.pow(DECIMAL_PLACES))
    }

    fn wcspr_amount_per_usd(&self) -> U256 {
        U256::from(self.wcspr_for_one_usd * 10u64.pow(DECIMAL_PLACES))
    }

    pub fn amount_per_one_usd(&self, path: Path) -> U256 {
        match path {
            Path::LongWcsprShort => self.longs_amount_per_usd(),
            Path::ShortWcsprLong => self.shorts_amount_per_usd(),
            Path::LongWcspr => self.longs_amount_per_usd(),
            Path::ShortWcspr => self.shorts_amount_per_usd(),
            Path::WcsprLong => self.wcspr_amount_per_usd(),
            Path::WcsprShort => self.wcspr_amount_per_usd(),
            Path::Empty => U256::zero(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::path::Path;

    fn make_data(
        long_price: f64,
        short_price: f64,
        wcspr_price: f64,
        long_fair: f64,
        short_fair: f64,
    ) -> PriceData {
        PriceData::new(long_price, short_price, wcspr_price, long_fair, short_fair)
    }

    // ========== Diff Calculation Tests ==========

    #[test]
    fn test_long_overvalued_diff_is_positive() {
        // (110 / 100) * 100 - 100 = 10.0
        let data = make_data(110.0, 50.0, 1.0, 100.0, 50.0);
        assert!((data.long_diff - 10.0).abs() < 1e-9);
    }

    #[test]
    fn test_long_undervalued_diff_is_negative() {
        // (90 / 100) * 100 - 100 = -10.0
        let data = make_data(90.0, 50.0, 1.0, 100.0, 50.0);
        assert!((data.long_diff - (-10.0)).abs() < 1e-9);
    }

    #[test]
    fn test_long_at_parity_diff_is_zero() {
        let data = make_data(100.0, 50.0, 1.0, 100.0, 50.0);
        assert!((data.long_diff - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_short_overvalued_diff_is_positive() {
        // (55 / 50) * 100 - 100 = 10.0
        let data = make_data(100.0, 55.0, 1.0, 100.0, 50.0);
        assert!((data.short_diff - 10.0).abs() < 1e-9);
    }

    #[test]
    fn test_short_undervalued_diff_is_negative() {
        // (45 / 50) * 100 - 100 = -10.0
        let data = make_data(100.0, 45.0, 1.0, 100.0, 50.0);
        assert!((data.short_diff - (-10.0)).abs() < 1e-9);
    }

    // ========== Tokens-per-USD Tests ==========

    #[test]
    fn test_longs_for_one_usd() {
        // wcspr_price = 0.05 USD/CSPR, long_fair = 0.1 CSPR/token
        // longs per USD = 1 / 0.05 / 0.1 = 200
        let data = make_data(0.1, 0.05, 0.05, 0.1, 0.05);
        assert_eq!(data.longs_for_one_usd, 200);
    }

    #[test]
    fn test_shorts_for_one_usd() {
        // wcspr_price = 0.05, short_fair = 0.05 CSPR/token
        // shorts per USD = 1 / 0.05 / 0.05 = 400
        let data = make_data(0.1, 0.05, 0.05, 0.1, 0.05);
        assert_eq!(data.shorts_for_one_usd, 400);
    }

    #[test]
    fn test_wcspr_for_one_usd() {
        // wcspr_price = 0.05 USD/CSPR → 1 / 0.05 = 20 CSPR per USD
        let data = make_data(0.1, 0.05, 0.05, 0.1, 0.05);
        assert_eq!(data.wcspr_for_one_usd, 20);
    }

    // ========== amount_per_one_usd Tests ==========

    #[test]
    fn test_amount_per_one_usd_long_wcspr_short_uses_longs() {
        let data = make_data(0.1, 0.05, 0.05, 0.1, 0.05);
        let expected = U256::from(data.longs_for_one_usd * 10u64.pow(9));
        assert_eq!(data.amount_per_one_usd(Path::LongWcsprShort), expected);
    }

    #[test]
    fn test_amount_per_one_usd_short_wcspr_long_uses_shorts() {
        let data = make_data(0.1, 0.05, 0.05, 0.1, 0.05);
        let expected = U256::from(data.shorts_for_one_usd * 10u64.pow(9));
        assert_eq!(data.amount_per_one_usd(Path::ShortWcsprLong), expected);
    }

    #[test]
    fn test_amount_per_one_usd_long_wcspr_uses_longs() {
        let data = make_data(0.1, 0.05, 0.05, 0.1, 0.05);
        let expected = U256::from(data.longs_for_one_usd * 10u64.pow(9));
        assert_eq!(data.amount_per_one_usd(Path::LongWcspr), expected);
    }

    #[test]
    fn test_amount_per_one_usd_short_wcspr_uses_shorts() {
        let data = make_data(0.1, 0.05, 0.05, 0.1, 0.05);
        let expected = U256::from(data.shorts_for_one_usd * 10u64.pow(9));
        assert_eq!(data.amount_per_one_usd(Path::ShortWcspr), expected);
    }

    #[test]
    fn test_amount_per_one_usd_wcspr_long_uses_wcspr() {
        let data = make_data(0.1, 0.05, 0.05, 0.1, 0.05);
        let expected = U256::from(data.wcspr_for_one_usd * 10u64.pow(9));
        assert_eq!(data.amount_per_one_usd(Path::WcsprLong), expected);
    }

    #[test]
    fn test_amount_per_one_usd_wcspr_short_uses_wcspr() {
        let data = make_data(0.1, 0.05, 0.05, 0.1, 0.05);
        let expected = U256::from(data.wcspr_for_one_usd * 10u64.pow(9));
        assert_eq!(data.amount_per_one_usd(Path::WcsprShort), expected);
    }

    #[test]
    fn test_amount_per_one_usd_empty_is_zero() {
        let data = make_data(100.0, 50.0, 1.0, 100.0, 50.0);
        assert_eq!(data.amount_per_one_usd(Path::Empty), U256::zero());
    }

    #[test]
    fn test_amount_has_nine_decimal_places() {
        // wcspr_price = 1.0, long_fair = 1.0 → longs_for_one_usd = 1
        // amount = 1 * 10^9 = 1_000_000_000
        let data = make_data(1.0, 1.0, 1.0, 1.0, 1.0);
        assert_eq!(
            data.amount_per_one_usd(Path::LongWcspr),
            U256::from(1_000_000_000u64)
        );
    }

    // ========== Display Formatting Tests ==========

    #[test]
    fn test_display_long_overvalued() {
        let data = make_data(110.0, 50.0, 1.0, 100.0, 50.0);
        let s = format!("{}", data);
        assert!(s.contains("Long diff overvalued by"));
        assert!(!s.contains("Long diff undervalued by"));
    }

    #[test]
    fn test_display_long_undervalued() {
        let data = make_data(90.0, 50.0, 1.0, 100.0, 50.0);
        let s = format!("{}", data);
        assert!(s.contains("Long diff undervalued by"));
        assert!(!s.contains("Long diff overvalued by"));
    }

    #[test]
    fn test_display_short_overvalued() {
        let data = make_data(100.0, 55.0, 1.0, 100.0, 50.0);
        let s = format!("{}", data);
        assert!(s.contains("Short diff overvalued by"));
        assert!(!s.contains("Short diff undervalued by"));
    }

    #[test]
    fn test_display_short_undervalued() {
        let data = make_data(100.0, 45.0, 1.0, 100.0, 50.0);
        let s = format!("{}", data);
        assert!(s.contains("Short diff undervalued by"));
        assert!(!s.contains("Short diff overvalued by"));
    }

    #[test]
    fn test_display_contains_all_price_fields() {
        let data = make_data(110.0, 45.0, 0.05, 100.0, 50.0);
        let s = format!("{}", data);
        assert!(s.contains("Long price:"));
        assert!(s.contains("Short price:"));
        assert!(s.contains("WCSPR price:"));
        assert!(s.contains("Long fair price:"));
        assert!(s.contains("Short fair price:"));
    }
}

impl Display for PriceData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "======================\n")?;
        write!(
            f,
            "Long price: {} CSPR\nShort price: {} CSPR\nWCSPR price: {} USD\nLong fair price: {} CSPR\nShort fair price: {} CSPR\n",
            self.long_price,
            self.short_price,
            self.wcspr_price,
            self.long_fair_price,
            self.short_fair_price
        )?;
        if self.long_diff > 0.0f64 {
            write!(f, "Long diff overvalued by {:.2}%\n", self.long_diff)?;
        } else {
            write!(f, "Long diff undervalued by {:.2}%\n", self.long_diff.abs())?;
        }
        if self.short_diff > 0.0f64 {
            write!(f, "Short diff overvalued by {:.2}%\n", self.short_diff)?;
        } else {
            write!(
                f,
                "Short diff undervalued by {:.2}%\n",
                self.short_diff.abs()
            )?;
        }
        write!(
            f,
            "Long/USD: {}\nShort/USD: {}\n",
            self.longs_for_one_usd, self.shorts_for_one_usd
        )?;
        write!(f, "===========================\n")?;
        Ok(())
    }
}
