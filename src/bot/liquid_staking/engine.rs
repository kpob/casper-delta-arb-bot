use odra::{casper_types::U256, host::HostEnv, prelude::Address};
use odra_cli::{cspr, scenario::Error};
use tracing::instrument;

use super::claims::PendingClaims;
use super::path::Path;
use super::price::{LsPriceCalculator, PriceData};
use crate::bot::engine::{Engine, Strategy};
use crate::bot::events::TradeScope;
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
        self.refs.staked_cspr()?.try_claim()?;
        self.claims.remove_ready()?;
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
            .map_err(|e| Error::OdraError {
                message: format!("get_amounts_out failed: {e:?}"),
            })?;
        match amounts.as_slice() {
            [a_in, .., a_out] => Ok((*a_in, *a_out)),
            _ => Err(Error::OdraError {
                message: "Invalid LS swap amounts".to_string(),
            }),
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
        // match path {
        //     Path::StCsprCspr => {
        //         // let cspr_to_stake_motes = (amount_in.as_u64() as f64 * data.protocol_price) as u64;
        //         self.execute_sell_stcspr(amount_in, amount_out, caller)
        //     }
        //     Path::CsprStCspr => self.execute_buy_stcspr(amount_in, amount_out, caller),
        // }

        if self.dry_run {
            tracing::info!("Dry run — buy_and_unstake skipped");
            return Ok((amount_in, amount_out));
        }

        self.env.set_gas(cspr!(8));
        let dex_path = path.build(&self.refs)?;
        let amounts = self.refs.router()?.swap_tokens_for_exact_tokens(
            amount_out,
            amount_in,
            dex_path,
            caller,
            u64::MAX,
        );
        match amounts.as_slice() {
            [amount_in, .., amount_out] => Ok((*amount_in, *amount_out)),
            _ => Err(Error::OdraError {
                message: "Invalid swap result".to_string(),
            }),
        }
    }
}
