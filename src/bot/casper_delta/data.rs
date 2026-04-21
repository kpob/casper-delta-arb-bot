use super::path::Path;
use odra::casper_types::U256;
use std::fmt::Display;

const DECIMAL_PLACES: u32 = 9;

#[derive(Debug, Clone, Copy)]
pub struct PriceData {
    pub trade_size_usd: f64,
    pub long_dex_rate: f64,
    pub short_dex_rate: f64,
    pub wcspr_price: f64,
    pub long_protocol_price: f64,
    pub short_protocol_price: f64,
    pub long_diff_percent: f64,
    pub short_diff_percent: f64,
    pub longs_per_trade_unit: U256,
    pub shorts_per_trade_unit: U256,
    pub wcspr_per_trade_unit: U256,
}

impl PriceData {
    pub fn new(
        long_dex_rate: f64,
        short_dex_rate: f64,
        wcspr_price: f64,
        long_protocol_price: f64,
        short_protocol_price: f64,
    ) -> Self {
        let trade_size_usd = std::env::var("BOT_DELTA_TRADE_SIZE_USD")
            .unwrap_or_else(|_| "1".to_string())
            .parse::<f64>()
            .expect("Invalid BOT_DELTA_TRADE_SIZE_USD");

        let long_diff_percent = (long_dex_rate / long_protocol_price) * 100.0f64 - 100.0f64;
        let short_diff_percent = (short_dex_rate / short_protocol_price) * 100.0f64 - 100.0f64;
        let longs_per_trade_unit =
            U256::from((trade_size_usd / wcspr_price / long_protocol_price) as u64)
                * 10u64.pow(DECIMAL_PLACES);
        let shorts_per_trade_unit =
            U256::from((trade_size_usd / wcspr_price / short_protocol_price) as u64)
                * 10u64.pow(DECIMAL_PLACES);
        let wcspr_per_trade_unit =
            U256::from((trade_size_usd / wcspr_price) as u64) * 10u64.pow(DECIMAL_PLACES);

        Self {
            trade_size_usd,
            long_dex_rate,
            short_dex_rate,
            wcspr_price,
            long_protocol_price,
            short_protocol_price,
            long_diff_percent,
            short_diff_percent,
            longs_per_trade_unit,
            shorts_per_trade_unit,
            wcspr_per_trade_unit,
        }
    }

    pub fn amount_per_trade_unit(&self, path: Path) -> U256 {
        match path {
            Path::LongWcsprShort => self.longs_per_trade_unit,
            Path::ShortWcsprLong => self.shorts_per_trade_unit,
            Path::LongWcspr => self.longs_per_trade_unit,
            Path::ShortWcspr => self.shorts_per_trade_unit,
            Path::WcsprLong => self.wcspr_per_trade_unit,
            Path::WcsprShort => self.wcspr_per_trade_unit,
            Path::Empty => U256::zero(),
        }
    }
}

impl PriceData {
    pub fn log(&self) {
        tracing::info!(
            long_dex_rate = self.long_dex_rate,
            short_dex_rate = self.short_dex_rate,
            wcspr_price = self.wcspr_price,
            long_protocol_price = self.long_protocol_price,
            short_protocol_price = self.short_protocol_price,
            "DEX prices (CSPR)"
        );
        tracing::info!(
            long_diff = format!("{:+.2}%", self.long_diff_percent),
            short_diff = format!("{:+.2}%", self.short_diff_percent),
            "Price deviations from fair value"
        );
        tracing::info!(
            longs_per_trade_unit = self.longs_per_trade_unit.as_u64(),
            shorts_per_trade_unit = self.shorts_per_trade_unit.as_u64(),
            wcspr_per_trade_unit = self.wcspr_per_trade_unit.as_u64(),
            "Token amounts traded per ${} of trade size",
            self.trade_size_usd
        );
    }
}

impl Display for PriceData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "========================")?;
        writeln!(
            f,
            "Long:  {:.6} CSPR  (fair {:.6}, diff {:+.2}%)",
            self.long_dex_rate, self.long_protocol_price, self.long_diff_percent
        )?;
        writeln!(
            f,
            "Short: {:.6} CSPR  (fair {:.6}, diff {:+.2}%)",
            self.short_dex_rate, self.short_protocol_price, self.short_diff_percent
        )?;
        writeln!(f, "WCSPR: {:.6} USD", self.wcspr_price)?;
        writeln!(
            f,
            "Per USD — Long: {}  Short: {}  WCSPR: {}",
            self.longs_per_trade_unit, self.shorts_per_trade_unit, self.wcspr_per_trade_unit
        )?;
        writeln!(f, "========================")
    }
}
