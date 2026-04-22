use odra::{
    casper_types::{U256, U512},
    host::{HostEnv, HostRef},
    prelude::Address,
};
use odra_cli::{cspr, scenario::Error};
use tracing::instrument;

use super::claims::{PendingClaim, PendingClaims};
use super::path::Path;
use super::price::{LsPriceCalculator, PriceData};
use crate::bot::engine::{Engine, Strategy};
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

    /// Unwraps WCSPR → native CSPR, stakes it, then sells the resulting sCSPR on the DEX.
    #[instrument(skip(self))]
    fn execute_stake_and_sell(
        &mut self,
        cspr_to_stake: U256,
        wcspr_out: U256,
        caller: Address,
    ) -> Result<(U256, U256), Error> {
        if self.dry_run {
            tracing::info!("Dry run — stake_and_sell skipped");
            return Ok((cspr_to_stake, wcspr_out));
        }

        tracing::info!(
            "Unwrapping {:.4} WCSPR",
            cspr_to_stake.as_u64() as f64 / 1e9
        );
        self.env.set_gas(cspr!(4));
        self.refs.wcspr()?.try_withdraw(&cspr_to_stake)?;

        tracing::info!("Staking {:.4} CSPR", cspr_to_stake.as_u64() as f64 / 1e9);
        let cspr_u512 = U512::from(cspr_to_stake.as_u64());
        self.env.set_gas(cspr!(9));
        self.refs
            .staked_cspr()?
            .with_tokens(cspr_u512)
            .try_stake()?;

        let scspr_balance = self.refs.staked_cspr()?.balance_of(&caller);
        let dex_path = Path::StCsprCspr.build(self.refs)?;
        tracing::info!(
            "Swapping {:.4} sCSPR for {:.4} WCSPR",
            scspr_balance.as_u64() as f64 / 1e9,
            wcspr_out.as_u64() as f64 / 1e9
        );
        self.env.set_gas(cspr!(8));
        let amounts = self.refs.router()?.swap_tokens_for_exact_tokens(
            wcspr_out,
            scspr_balance,
            dex_path,
            caller,
            u64::MAX,
        );

        tracing::info!("StakeAndSell complete");
        match amounts.as_slice() {
            [a_in, .., a_out] => Ok((*a_in, *a_out)),
            _ => Err(Error::OdraError {
                message: "Invalid LS swap result".to_string(),
            }),
        }
    }

    /// Buys sCSPR on the DEX then initiates an unstake. Records a pending claim.
    #[instrument(skip(self))]
    fn execute_buy_and_unstake(
        &mut self,
        wcspr_in: U256,
        stcspr_out: U256,
        caller: Address,
    ) -> Result<(U256, U256), Error> {
        if self.dry_run {
            tracing::info!("Dry run — buy_and_unstake skipped");
            return Ok((wcspr_in, stcspr_out));
        }

        let dex_path = Path::CsprStCspr.build(self.refs)?;
        tracing::info!(
            "Buying {:.4} sCSPR with {:.4} WCSPR",
            stcspr_out.as_u64() as f64 / 1e9,
            wcspr_in.as_u64() as f64 / 1e9,
        );
        self.env.set_gas(cspr!(8));
        let amounts = self.refs.router()?.swap_tokens_for_exact_tokens(
            stcspr_out,
            wcspr_in,
            dex_path,
            caller,
            u64::MAX,
        );

        let scspr_balance = self.refs.staked_cspr()?.balance_of(&caller);
        tracing::info!(
            "Initiating unstake of {:.4} sCSPR",
            scspr_balance.as_u64() as f64 / 1e9
        );
        self.env.set_gas(cspr!(6));
        self.refs.staked_cspr()?.try_unstake(scspr_balance)?;

        let claim_delay_ms = self.refs.staked_cspr()?.get_claim_time();
        let claimable_from_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            + claim_delay_ms;

        tracing::info!(
            "Recording pending claim (claimable in ~{} ms)",
            claim_delay_ms
        );
        self.claims.add(PendingClaim { claimable_from_ms })?;

        tracing::info!("BuyAndUnstake complete — claim recorded");
        match amounts.as_slice() {
            [a_in, .., a_out] => Ok((*a_in, *a_out)),
            _ => Err(Error::OdraError {
                message: "Invalid LS swap result".to_string(),
            }),
        }
    }
}

impl<'a> Strategy for LsStrategy<'a> {
    type PriceData = PriceData;
    type Path = Path;

    const NAME: &'static str = "LiquidStaking";
    const MIN_PROFIT_CSPR: f64 = 100.0;

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

    fn execute(
        &mut self,
        data: Self::PriceData,
        path: Self::Path,
        amount_in: U256,
        amount_out: U256,
        caller: Address,
    ) -> Result<(U256, U256), Error> {
        match path {
            Path::StCsprCspr => {
                let cspr_to_stake_motes = (amount_in.as_u64() as f64 * data.protocol_price) as u64;
                self.execute_stake_and_sell(U256::from(cspr_to_stake_motes), amount_out, caller)
            }
            Path::CsprStCspr => self.execute_buy_and_unstake(amount_in, amount_out, caller),
        }
    }
}
