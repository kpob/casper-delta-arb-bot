use odra::{casper_types::U256, host::HostEnv, prelude::Address};
use odra_cli::{cspr, scenario::Error};
use tracing::instrument;

use super::claims::PendingClaims;
use super::path::Path;
use super::price::{LsPriceCalculator, PriceData};
use crate::bot::engine::{Engine, Strategy};
use crate::bot::events::TradeScope;
use crate::bot::report;
use crate::bot::utils::motes_to_token;
use crate::contracts::ContractRefs;

const CLAIMS_FILE: &str = "pending_claims.json";

pub type LsEngine<'a> = Engine<LsStrategy<'a>>;

pub struct LsStrategy<'a> {
    refs: &'a ContractRefs<'a>,
    env: &'a HostEnv,
    claims: PendingClaims,
    dry_run: bool,
}

impl<'a> LsStrategy<'a> {
    pub fn new(refs: &'a ContractRefs<'a>, env: &'a HostEnv, dry_run: bool) -> Self {
        Self {
            refs,
            env,
            claims: PendingClaims::load(CLAIMS_FILE),
            dry_run,
        }
    }
}

impl<'a> Strategy for LsStrategy<'a> {
    type PriceData = PriceData;
    type Path = Path;

    const NAME: &'static str = "LiquidStaking";
    const MIN_PROFIT_CSPR: f64 = 75.0;
    const TRADE_SCOPE: TradeScope = TradeScope::LiquidStaking;

    #[instrument(skip(self))]
    fn before_trade(&mut self) -> Result<(), Error> {
        if !self.claims.has_ready_claims() {
            return Ok(());
        }
        if self.dry_run {
            tracing::info!("Dry run — skipping claim (ready claims exist)");
            return Ok(());
        }
        tracing::info!("Claiming matured unstakes...");
        self.env.set_gas(cspr!(5));
        self.refs.staked_cspr()?.try_claim().inspect_err(|e| {
            tracing::error!(op = "ls.claim", error = ?e, "claim failed");
        })?;
        self.claims.remove_ready().inspect_err(|e| {
            tracing::error!(op = "ls.claims.remove_ready", error = ?e, "claims persist failed");
        })?;
        tracing::info!("Claim complete");
        Ok(())
    }

    fn fetch_prices(&self) -> Result<Self::PriceData, Error> {
        LsPriceCalculator::prices(self.refs)
    }

    fn select_path(data: Self::PriceData) -> Option<Self::Path> {
        Self::Path::select(data)
    }

    fn estimate(&self, data: Self::PriceData, path: Self::Path) -> Result<(U256, U256), Error> {
        let amount_in = data.amount_per_trade_unit(path);
        let dex_path = path.build(self.refs)?;
        let amounts = self
            .refs
            .router()?
            .try_get_amounts_out(amount_in, dex_path)
            .map_err(|e| {
                tracing::error!(op = "ls.get_amounts_out", ?path, amount_in_motes = amount_in.as_u64(), error = ?e, "router quote failed");
                Error::OdraError {
                    message: format!("get_amounts_out failed: {e:?}"),
                }
            })?;
        match amounts.as_slice() {
            [a_in, .., a_out] => Ok((*a_in, *a_out)),
            _ => {
                tracing::error!(
                    ?path,
                    len = amounts.len(),
                    "router returned unexpected amounts shape"
                );
                Err(Error::OdraError {
                    message: "Invalid LS swap amounts".to_string(),
                })
            }
        }
    }

    fn cspr_profit(
        amount_in: U256,
        amount_out: U256,
        data: Self::PriceData,
        path: Self::Path,
    ) -> f64 {
        LsPriceCalculator::cspr_profit(amount_in, amount_out, data, path)
    }

    #[instrument(skip(self))]
    fn execute(
        &mut self,
        path: Self::Path,
        amount_in: U256,
        amount_out: U256,
        caller: Address,
    ) -> Result<(U256, U256), Error> {
        if self.dry_run {
            tracing::info!("Dry run — buy_and_unstake skipped");
            return Ok((amount_in, amount_out));
        }

        self.env.set_gas(cspr!(8));
        let dex_path = path.build(&self.refs)?;
        tracing::info!(
            op = "ls.swap",
            ?path,
            amount_in_max = motes_to_token(amount_in),
            amount_out = motes_to_token(amount_out),
            gas_cspr = 8,
            recipient = ?caller,
            "calling on-chain"
        );
        let amounts = self.refs.router()?.swap_tokens_for_exact_tokens(
            amount_out,
            amount_in,
            dex_path,
            caller,
            u64::MAX,
        );
        match amounts.as_slice() {
            [first, .., last] => {
                tracing::info!(
                    op = "ls.swap",
                    ?path,
                    actual_in = motes_to_token(*first),
                    actual_out = motes_to_token(*last),
                    hops = amounts.len(),
                    "swap returned"
                );
                Ok((*first, *last))
            }
            _ => {
                tracing::error!(
                    ?path,
                    len = amounts.len(),
                    "swap returned unexpected amounts shape"
                );
                Err(Error::OdraError {
                    message: "Invalid swap result".to_string(),
                })
            }
        }
    }

    fn log_trade_executed(
        data: Self::PriceData,
        path: Self::Path,
        predicted_cspr: f64,
        actual_cspr: f64,
    ) {
        report::trade::executed_ls(&path, data.diff_percentage, predicted_cspr, actual_cspr);
    }
}
