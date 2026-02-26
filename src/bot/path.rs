use odra::prelude::{Address, Addressable};
use odra_cli::scenario::Error;

use crate::{bot::data::PriceData, contracts::ContractRefs};

const DIFF_THRESHOLD: f64 = 2.5f64;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Path {
    LongWcsprShort,
    ShortWcsprLong,
    LongWcspr,
    ShortWcspr,
    WcsprLong,
    WcsprShort,
    Empty,
}

impl From<&PriceData> for Path {
    fn from(data: &PriceData) -> Self {
        Self::calc(data)
    }
}

impl Path {
    fn calc(data: &PriceData) -> Self {
        let long_diff = data.long_diff.abs();
        let short_diff = data.short_diff.abs();
        let long_price_diff = data.long_price - data.long_fair_price;
        let short_price_diff = data.short_price - data.short_fair_price;

        if long_price_diff > 0.0f64
            && short_price_diff < 0.0f64
            && long_diff > DIFF_THRESHOLD
            && short_diff > DIFF_THRESHOLD
        {
            Path::LongWcsprShort
        } else if short_price_diff > 0.0f64
            && long_price_diff < 0.0f64
            && long_diff > DIFF_THRESHOLD
            && short_diff > DIFF_THRESHOLD
        {
            Path::ShortWcsprLong
        } else if long_price_diff > 0.0f64 && long_diff > DIFF_THRESHOLD {
            Path::LongWcspr
        } else if short_price_diff > 0.0f64 && short_diff > DIFF_THRESHOLD {
            Path::ShortWcspr
        } else if long_price_diff < 0.0f64 && long_diff > DIFF_THRESHOLD {
            Path::WcsprLong
        } else if short_price_diff < 0.0f64 && short_diff > DIFF_THRESHOLD {
            Path::WcsprShort
        } else {
            Path::Empty
        }
    }

    pub fn build(&self, refs: &ContractRefs) -> Result<Vec<Address>, Error> {
        let long_address = refs.long()?.address();
        let short_address = refs.short()?.address();
        let wcspr_address = refs.wcspr()?.address();
        match self {
            Path::LongWcsprShort => Ok(vec![long_address, wcspr_address, short_address]),
            Path::ShortWcsprLong => Ok(vec![short_address, wcspr_address, long_address]),
            Path::LongWcspr => Ok(vec![long_address, wcspr_address]),
            Path::ShortWcspr => Ok(vec![short_address, wcspr_address]),
            Path::WcsprLong => Ok(vec![wcspr_address, long_address]),
            Path::WcsprShort => Ok(vec![wcspr_address, short_address]),
            Path::Empty => Ok(vec![]),
        }
    }

    pub fn is_multi_hop(&self) -> bool {
        matches!(self, Path::LongWcsprShort | Path::ShortWcsprLong)
    }
}

#[cfg(test)]
mod tests {
    use crate::bot::data::PriceData;

    use super::*;

    #[test]
    fn test_path_calc_long_overvalued_short_undervalued() {
        // Long is overvalued (100 > 90), short is undervalued (60 < 77)
        // Both diffs > 2.5% threshold
        let long_price = 100.0;
        let long_fair_price = 90.0;
        let short_price = 60.0;
        let short_fair_price = 77.0;
        let wcspr_price = 1.0;
        let data = PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        );
        assert_eq!(Path::calc(&data), Path::LongWcsprShort);
    }

    #[test]
    fn test_path_calc_short_overvalued_long_undervalued() {
        // Short is overvalued (100 > 90), long is undervalued (60 < 77)
        // Both diffs > 2.5% threshold
        let long_price = 60.0;
        let long_fair_price = 77.0;
        let short_price = 100.0;
        let short_fair_price = 90.0;
        let wcspr_price = 1.0;
        let data = PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        );
        assert_eq!(Path::calc(&data), Path::ShortWcsprLong);
    }

    #[test]
    fn test_path_calc_long_overvalued_only() {
        // Long is overvalued (100 > 90), short diff is below threshold
        let long_price = 100.0;
        let long_fair_price = 90.0;
        let short_price = 50.0;
        let short_fair_price = 50.5;
        let wcspr_price = 1.0;
        let data = PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        );
        assert_eq!(Path::calc(&data), Path::LongWcspr);
    }

    #[test]
    fn test_path_calc_short_overvalued_only() {
        // Short is overvalued (100 > 90), long diff is below threshold
        let long_price = 50.0;
        let long_fair_price = 50.5;
        let short_price = 100.0;
        let short_fair_price = 90.0;
        let wcspr_price = 1.0;
        let data = PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        );
        assert_eq!(Path::calc(&data), Path::ShortWcspr);
    }

    #[test]
    fn test_path_calc_long_undervalued_only() {
        // Long is undervalued (60 < 77), short diff is below threshold
        let long_price = 60.0;
        let long_fair_price = 77.0;
        let short_price = 50.0;
        let short_fair_price = 50.5;
        let wcspr_price = 1.0;
        let data = PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        );
        assert_eq!(Path::calc(&data), Path::WcsprLong);
    }

    #[test]
    fn test_path_calc_short_undervalued_only() {
        // Short is undervalued (60 < 77), long diff is below threshold
        let long_price = 50.0;
        let long_fair_price = 50.5;
        let short_price = 60.0;
        let short_fair_price = 77.0;
        let wcspr_price = 1.0;
        let data = PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        );
        assert_eq!(Path::calc(&data), Path::WcsprShort);
    }

    #[test]
    fn test_path_calc_empty_no_significant_diff() {
        // Both prices are close to fair prices (diffs < 2.5% threshold)
        let long_price = 100.0;
        let long_fair_price = 100.5;
        let short_price = 50.0;
        let short_fair_price = 50.5;
        let wcspr_price = 1.0;
        let data = PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        );
        assert_eq!(Path::calc(&data), Path::Empty);
    }

    #[test]
    fn test_path_calc_both_overvalued() {
        // Both are overvalued with significant diffs
        // Since the paired condition (long > 0 && short < 0) fails,
        // it falls through to check long_price_diff > 0, returning LongWcspr
        let long_price = 100.0;
        let long_fair_price = 90.0;
        let short_price = 100.0;
        let short_fair_price = 90.0;
        let wcspr_price = 1.0;
        let data = PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        );
        assert_eq!(Path::calc(&data), Path::LongWcspr);
    }

    #[test]
    fn test_path_calc_both_undervalued() {
        // Both are undervalued with significant diffs
        // Since the paired condition (short > 0 && long < 0) fails,
        // it falls through to check long_price_diff < 0, returning WcsprLong
        let long_price = 60.0;
        let long_fair_price = 77.0;
        let short_price = 60.0;
        let short_fair_price = 77.0;
        let wcspr_price = 1.0;
        let data = PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        );
        assert_eq!(Path::calc(&data), Path::WcsprLong);
    }

    #[test]
    fn test_path_calc_threshold_boundary_above() {
        // Test exactly at the threshold boundary (2.5%)
        // Long diff = 11.11% (100/90 = 1.111), short diff = 22.07% (60/77.3 = 0.776)
        let long_price = 100.0;
        let long_fair_price = 90.0;
        let short_price = 60.0;
        let short_fair_price = 77.3;
        let wcspr_price = 1.0;
        let data = PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        );
        assert_eq!(Path::calc(&data), Path::LongWcsprShort);
    }

    #[test]
    fn test_path_calc_threshold_boundary_below() {
        // Test just below the threshold (< 2.5%)
        // Long diff = 2.0% (102/100 = 1.02), short diff = 2.0% (51/50 = 1.02)
        let long_price = 102.0;
        let long_fair_price = 100.0;
        let short_price = 51.0;
        let short_fair_price = 50.0;
        let wcspr_price = 1.0;
        let data = PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        );
        assert_eq!(Path::calc(&data), Path::Empty);
    }
}
