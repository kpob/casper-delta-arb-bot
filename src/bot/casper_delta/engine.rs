use odra::casper_types::U256;
use odra::prelude::Address;
use odra_cli::scenario::Error;
use tracing::instrument;

use super::{path::Path, trader::DeltaAssetManager, utils::PriceCalculator};
use crate::bot::events::BotEvent;

/// The core bot logic, decoupled from the event loop.
pub struct CasperDeltaEngine<'a> {
    calc: PriceCalculator<'a>,
    asset_manager: DeltaAssetManager<'a>,
    caller: Address,
}

impl<'a> CasperDeltaEngine<'a> {
    pub fn new(
        calc: PriceCalculator<'a>,
        asset_manager: DeltaAssetManager<'a>,
        caller: Address,
    ) -> Self {
        Self {
            calc,
            asset_manager,
            caller,
        }
    }

    /// Handle a single event. Returns `Ok(true)` to continue, `Ok(false)` to stop.
    #[instrument(skip(self))]
    pub fn handle_event(&self, event: &BotEvent) -> Result<bool, Error> {
        match event {
            BotEvent::TimerTick | BotEvent::TradeExecuted | BotEvent::PriceChanged => {
                self.try_trade()?;
                Ok(true)
            }
            BotEvent::Shutdown => {
                tracing::info!("Shutdown event received");
                Ok(false)
            }
        }
    }

    /// Fetch prices, find arbitrage path, execute swap if profitable.
    fn try_trade(&self) -> Result<(), Error> {
        let price_data = self.calc.price_data()?;
        price_data.log();

        let path = Path::from(price_data);
        tracing::info!("Swap path: {:?}", path);
        if path == Path::Empty {
            tracing::info!("No arbitrage path found");
            return Ok(());
        }

        let amounts = self.calc.effective_price(price_data, path);
        if let Ok([amount_in, .., amount_out]) = amounts.as_deref() {
            let predicted_profit =
                PriceCalculator::cspr_profit(*amount_in, *amount_out, price_data, path);
            tracing::info!("Predicted profit: {:<10.4} CSPR", predicted_profit);
            if predicted_profit < 1.0f64 {
                tracing::info!("Predicted profit below threshold, skipping swap");
                return Ok(());
            }

            let (actual_amount_in, actual_amount_out) = self.swap(path, *amount_in, *amount_out)?;
            let actual_gain =
                PriceCalculator::cspr_profit(actual_amount_in, actual_amount_out, price_data, path);
            tracing::info!("Actual gain: {:<10.4} CSPR", actual_gain);
        } else {
            tracing::info!("No valid swap amounts found");
        }
        Ok(())
    }

    #[instrument(skip(self))]
    fn swap(&self, path: Path, amount_in: U256, amount_out: U256) -> Result<(U256, U256), Error> {
        tracing::info!("Preparing swap...");
        let result = self
            .asset_manager
            .swap(path, amount_in, amount_out, self.caller)?;
        tracing::info!("Arbitrage swap completed");

        if let [amount_in, .., amount_out] = result.as_slice() {
            Ok((*amount_in, *amount_out))
        } else {
            Err(Error::OdraError {
                message: "Invalid swap result".to_string(),
            })
        }
    }
}
