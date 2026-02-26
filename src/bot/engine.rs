use odra::casper_types::U256;
use odra::prelude::Address;
use odra_cli::scenario::Error;
use tracing::instrument;

use crate::bot::asset_manager::AssetManager;
use crate::bot::data::PriceData;
use crate::bot::events::BotEvent;
use crate::bot::path::Path;
use crate::bot::utils::PriceCalculator;
use crate::contracts::ContractRefs;

/// The core bot logic, decoupled from the event loop.
pub struct BotEngine<'a> {
    calc: PriceCalculator<'a>,
    asset_manager: AssetManager<'a>,
    contracts: &'a ContractRefs<'a>,
    caller: Address,
}

impl<'a> BotEngine<'a> {
    pub fn new(
        calc: PriceCalculator<'a>,
        asset_manager: AssetManager<'a>,
        contracts: &'a ContractRefs<'a>,
        caller: Address,
    ) -> Self {
        Self {
            calc,
            asset_manager,
            contracts,
            caller,
        }
    }

    /// Handle a single event. Returns `Ok(true)` to continue, `Ok(false)` to stop.
    #[instrument(skip(self))]
    pub fn handle_event(&self, event: &BotEvent) -> Result<bool, Error> {
        match event {
            BotEvent::TimerTick | BotEvent::TradeExecuted { .. } | BotEvent::PriceChanged { .. } => {
                self.check_and_trade()?;
                Ok(true)
            }
            BotEvent::Shutdown => {
                tracing::info!("Shutdown event received");
                Ok(false)
            }
        }
    }

    /// Fetch prices, find arbitrage path, execute swap if profitable.
    fn check_and_trade(&self) -> Result<(), Error> {
        let price_data = self.get_price_data()?;
        price_data.log();

        self.asset_manager
            .manage_asset_levels(&price_data, self.caller)?;

        let path = Path::from(&price_data);
        tracing::info!("Swap path: {:?}", path);
        if path == Path::Empty {
            tracing::info!("No arbitrage path found");
            return Ok(());
        }

        let amounts = self.get_swap_amounts(&price_data, path);
        if let Ok([amount_in, .., amount_out]) = amounts.as_deref() {
            let gain =
                PriceCalculator::calc_gains_in_cspr(*amount_in, *amount_out, &price_data, path);
            tracing::info!("Gain: {:<10.4} CSPR", gain);
            if gain < 1.0f64 {
                tracing::info!("No arbitrage path found");
                return Ok(());
            }

            let (actual_amount_in, actual_amount_out) =
                self.swap(path, *amount_in, *amount_out)?;
            let actual_gain = PriceCalculator::calc_gains_in_cspr(
                actual_amount_in,
                actual_amount_out,
                &price_data,
                path,
            );
            tracing::info!("Actual gain: {:<10.4} CSPR", actual_gain);
        } else {
            tracing::info!("No valid swap amounts found");
        }
        Ok(())
    }

    fn get_price_data(&self) -> Result<PriceData, Error> {
        let (long_price, short_price) = self.calc.casper_trade_prices()?;
        let (long_fair_price, short_fair_price, wcspr_price) = self.calc.fair_prices()?;
        Ok(PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        ))
    }

    fn swap(&self, path: Path, amount_in: U256, amount_out: U256) -> Result<(U256, U256), Error> {
        tracing::info!("Preparing swap...");
        let result = self
            .asset_manager
            .swap(path, amount_in, amount_out, self.caller)?;
        tracing::info!("Arbitrage swap completed");
        self.asset_manager.print_balances()?;

        if let [amount_in, .., amount_out] = result.as_slice() {
            Ok((*amount_in, *amount_out))
        } else {
            Err(Error::OdraError {
                message: "Invalid swap result".to_string(),
            })
        }
    }

    fn get_swap_amounts(
        &self,
        price_data: &PriceData,
        path: Path,
    ) -> Result<Vec<U256>, Error> {
        let amount_in = price_data.amount_per_one_usd(path);
        let path = path.build(self.contracts)?;
        let amounts = self.contracts
            .router()?
            .try_get_amounts_out(amount_in, path)
            .map_err(|e| Error::OdraError {
                message: format!("Failed to get amounts out: {:?}", e),
            })?;
        if let [amount_in, .., amount_out] = amounts.as_slice() {
            Ok(vec![*amount_in, *amount_out])
        } else {
            Err(Error::OdraError {
                message: "Invalid swap result".to_string(),
            })
        }
    }

}



