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

impl PriceData {
    pub fn log(&self) {
        tracing::info!(
            long_price = self.long_price,
            short_price = self.short_price,
            wcspr_price = self.wcspr_price,
            long_fair_price = self.long_fair_price,
            short_fair_price = self.short_fair_price,
            "DEX prices (CSPR)"
        );
        tracing::info!(
            long_diff = format!("{:+.2}%", self.long_diff),
            short_diff = format!("{:+.2}%", self.short_diff),
            "Price deviations from fair value"
        );
        tracing::info!(
            longs_per_usd = self.longs_for_one_usd,
            shorts_per_usd = self.shorts_for_one_usd,
            wcspr_per_usd = self.wcspr_for_one_usd,
            "Token amounts per USD"
        );
    }
}

impl Display for PriceData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "========================")?;
        writeln!(f, "Long:  {:.6} CSPR  (fair {:.6}, diff {:+.2}%)", self.long_price, self.long_fair_price, self.long_diff)?;
        writeln!(f, "Short: {:.6} CSPR  (fair {:.6}, diff {:+.2}%)", self.short_price, self.short_fair_price, self.short_diff)?;
        writeln!(f, "WCSPR: {:.6} USD", self.wcspr_price)?;
        writeln!(f, "Per USD — Long: {}  Short: {}  WCSPR: {}", self.longs_for_one_usd, self.shorts_for_one_usd, self.wcspr_for_one_usd)?;
        writeln!(f, "========================")
    }
}
