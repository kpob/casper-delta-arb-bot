use odra::casper_types::U256;
use odra::prelude::Address;
use odra_cli::scenario::Error;

use super::{
    path::Path,
    price::{PriceCalculator, PriceData},
    trader::DeltaAssetManager,
};
use crate::bot::engine::{Engine, Strategy};
use crate::bot::events::TradeScope;
use crate::bot::report;

pub type CasperDeltaEngine<'a> = Engine<CasperDeltaStrategy<'a>>;

pub struct CasperDeltaStrategy<'a> {
    calc: PriceCalculator<'a>,
    asset_manager: DeltaAssetManager<'a>,
}

impl<'a> CasperDeltaStrategy<'a> {
    pub fn new(calc: PriceCalculator<'a>, asset_manager: DeltaAssetManager<'a>) -> Self {
        Self {
            calc,
            asset_manager,
        }
    }
}

impl<'a> Strategy for CasperDeltaStrategy<'a> {
    type PriceData = PriceData;
    type Path = Path;

    const NAME: &'static str = "CasperDelta";
    const MIN_PROFIT_CSPR: f64 = 1.0;
    const TRADE_SCOPE: TradeScope = TradeScope::Delta;

    fn fetch_prices(&self) -> Result<Self::PriceData, Error> {
        self.calc.price_data()
    }

    fn select_path(data: Self::PriceData) -> Option<Self::Path> {
        Self::Path::select(data)
    }

    fn estimate(&self, data: Self::PriceData, path: Self::Path) -> Result<(U256, U256), Error> {
        let amounts = self.calc.effective_price(data, path)?;
        match amounts.as_slice() {
            [amount_in, .., amount_out] => Ok((*amount_in, *amount_out)),
            _ => Err(Error::OdraError {
                message: "Invalid swap amounts".to_string(),
            }),
        }
    }

    fn cspr_profit(
        amount_in: U256,
        amount_out: U256,
        data: Self::PriceData,
        path: Self::Path,
    ) -> f64 {
        PriceCalculator::cspr_profit(amount_in, amount_out, data, path)
    }

    fn execute(
        &mut self,
        path: Self::Path,
        amount_in: U256,
        amount_out: U256,
        caller: Address,
    ) -> Result<(U256, U256), Error> {
        let result = self
            .asset_manager
            .swap(path, amount_in, amount_out, caller)?;
        match result.as_slice() {
            [amount_in, .., amount_out] => Ok((*amount_in, *amount_out)),
            _ => Err(Error::OdraError {
                message: "Invalid swap result".to_string(),
            }),
        }
    }

    fn log_trade_executed(
        data: Self::PriceData,
        path: Self::Path,
        predicted_cspr: f64,
        actual_cspr: f64,
    ) {
        report::trade::executed_delta(
            &path,
            data.long_diff_percent,
            data.short_diff_percent,
            predicted_cspr,
            actual_cspr,
        );
    }
}
