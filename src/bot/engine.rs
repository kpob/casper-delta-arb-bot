use odra::{casper_types::U256, prelude::Address};
use odra_cli::scenario::Error;
use tracing::instrument;

use super::events::{BotEvent, TradeScope};

/// Implemented by the associated `PriceData` so the engine can render prices
/// without each strategy having to forward a static hook.
pub trait LogPrices {
    fn log(&self);
}

/// Strategy abstracts a single arbitrage domain (Casper Delta, liquid staking, ...).
/// It owns the domain-specific `PriceData` / `Path` types and provides the hooks
/// the generic `Engine` needs to drive one iteration of the arbitrage loop.
pub trait Strategy {
    type PriceData: Copy + LogPrices;
    type Path: Copy + std::fmt::Debug;

    const NAME: &'static str;
    const MIN_PROFIT_CSPR: f64;
    /// Which trade scope this strategy reacts to.
    const TRADE_SCOPE: TradeScope;

    /// Optional pre-trade hook (e.g. claiming matured unstakes). Default: no-op.
    fn before_trade(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn fetch_prices(&self) -> Result<Self::PriceData, Error>;

    /// Returns `None` when no arbitrage opportunity exists.
    fn select_path(data: Self::PriceData) -> Option<Self::Path>;

    /// Simulate the swap and return `(amount_in, amount_out)` in token motes.
    fn estimate(&self, data: Self::PriceData, path: Self::Path) -> Result<(U256, U256), Error>;

    /// Convert a simulated or executed swap into CSPR profit (gains minus tx cost).
    fn cspr_profit(
        amount_in: U256,
        amount_out: U256,
        data: Self::PriceData,
        path: Self::Path,
    ) -> f64;

    /// Execute the swap on-chain and return the actual `(amount_in, amount_out)`.
    fn execute(
        &mut self,
        path: Self::Path,
        amount_in: U256,
        amount_out: U256,
        caller: Address,
    ) -> Result<(U256, U256), Error>;
}

pub struct Engine<S: Strategy> {
    strategy: S,
    caller: Address,
}

impl<S: Strategy> Engine<S> {
    pub fn new(strategy: S, caller: Address) -> Self {
        Self { strategy, caller }
    }

    #[instrument(skip(self), fields(strategy = S::NAME))]
    pub fn handle_event(&mut self, event: &BotEvent) -> Result<bool, Error> {
        match event {
            BotEvent::TimerTick => {
                self.try_trade()?;
                Ok(true)
            }
            BotEvent::TradeExecuted(scope) | BotEvent::PriceChanged(scope)
                if *scope == S::TRADE_SCOPE =>
            {
                self.try_trade()?;
                Ok(true)
            }
            BotEvent::TradeExecuted(_) | BotEvent::PriceChanged(_) => Ok(true),
            BotEvent::Shutdown => {
                tracing::info!("{}: shutdown event received", S::NAME);
                Ok(false)
            }
        }
    }

    fn try_trade(&mut self) -> Result<(), Error> {
        self.strategy.before_trade()?;

        let data = self.strategy.fetch_prices()?;
        data.log();

        let Some(path) = S::select_path(data) else {
            tracing::info!("{}: no arbitrage opportunity", S::NAME);
            return Ok(());
        };
        tracing::info!("{}: swap path {:?}", S::NAME, path);

        let (amount_in, amount_out) = self.strategy.estimate(data, path)?;
        let predicted = S::cspr_profit(amount_in, amount_out, data, path);
        tracing::info!("{}: predicted profit {:<10.4} CSPR", S::NAME, predicted);
        if predicted < S::MIN_PROFIT_CSPR {
            tracing::info!(
                "{}: profit below threshold ({} CSPR), skipping",
                S::NAME,
                S::MIN_PROFIT_CSPR
            );
            return Ok(());
        }

        let (actual_in, actual_out) =
            self.strategy
                .execute(path, amount_in, amount_out, self.caller)?;
        let actual = S::cspr_profit(actual_in, actual_out, data, path);
        tracing::info!("{}: actual profit {:<10.4} CSPR", S::NAME, actual);
        Ok(())
    }
}
