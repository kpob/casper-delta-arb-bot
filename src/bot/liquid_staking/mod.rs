pub mod claims;
pub mod path;
pub mod price;

use odra::{
    casper_types::{U256, U512},
    host::{HostEnv, HostRef}, // HostRef required for .with_tokens() on HostRef types
    prelude::Address,
};
use odra_cli::{cspr, scenario::Error};
use tracing::instrument;

use super::events::BotEvent;
use crate::contracts::ContractRefs;
use claims::{PendingClaim, PendingClaims};
use path::Path;
use price::{LsPriceCalculator, PriceData};

const CLAIMS_FILE: &str = "pending_claims.json";
const MIN_PROFIT_CSPR: f64 = 100.0;

pub struct LsEngine<'a> {
    contracts: &'a ContractRefs<'a>,
    env: &'a HostEnv,
    caller: Address,
    claims: PendingClaims,
    dry_run: bool,
}

impl<'a> LsEngine<'a> {
    pub fn new(
        contracts: &'a ContractRefs<'a>,
        env: &'a HostEnv,
        caller: Address,
        dry_run: bool,
    ) -> Self {
        Self {
            contracts,
            env,
            caller,
            claims: PendingClaims::load(CLAIMS_FILE),
            dry_run,
        }
    }

    /// Attempts to claim any matured unstakes. Call this before each arb check.
    #[instrument(skip(self))]
    pub fn try_claim_ready(&mut self) -> Result<(), Error> {
        if !self.claims.has_ready_claims() {
            return Ok(());
        }
        if self.dry_run {
            tracing::info!("Dry run — skipping claim (ready claims exist)");
            return Ok(());
        }
        tracing::info!("Claiming matured unstakes...");
        self.env.set_gas(cspr!(5));
        self.contracts.staked_cspr()?.try_claim()?;
        self.claims.remove_ready()?;
        tracing::info!("Claim complete");
        Ok(())
    }

    /// Handle a single event. Returns `Ok(true)` to continue, `Ok(false)` to stop.
    #[instrument(skip(self))]
    pub fn handle_event(&mut self, event: &BotEvent) -> Result<bool, Error> {
        match event {
            BotEvent::TimerTick | BotEvent::TradeExecuted | BotEvent::PriceChanged => {
                self.try_trade()?;
                Ok(true)
            }
            BotEvent::Shutdown => Ok(false),
        }
    }

    fn try_trade(&mut self) -> Result<(), Error> {
        let price_data = LsPriceCalculator::prices(self.contracts)?;
        price_data.log();

        let path = Path::from(price_data);
        tracing::info!("LS swap path: {:?}", path);
        if path == Path::Empty {
            tracing::info!("No LS arbitrage opportunity");
            return Ok(());
        }

        let amounts = self.get_swap_amounts(price_data, path)?;
        let (amount_in, amount_out) = match amounts.as_slice() {
            [a_in, .., a_out] => (*a_in, *a_out),
            _ => {
                tracing::info!("No valid LS swap amounts");
                return Ok(());
            }
        };

        let profit = LsPriceCalculator::cspr_profit(amount_in, amount_out, price_data, path);
        tracing::info!("LS profit estimate: {:.4} CSPR", profit);

        if profit < MIN_PROFIT_CSPR {
            tracing::info!("LS profit below threshold ({MIN_PROFIT_CSPR} CSPR), skipping");
            return Ok(());
        }

        match path {
            Path::StCsprCspr => {
                // amount_in = sCSPR to sell (estimated from staking)
                // amount_out = WCSPR to receive
                let cspr_to_stake_motes =
                    (amount_in.as_u64() as f64 * price_data.protocol_price) as u64;
                self.execute_stake_and_sell(U256::from(cspr_to_stake_motes), amount_out)?;
            }
            Path::CsprStCspr => {
                // amount_in = WCSPR to spend, amount_out = sCSPR to receive
                self.execute_buy_and_unstake(amount_in, amount_out)?;
            }
            Path::Empty => unreachable!(),
        }

        Ok(())
    }

    fn get_swap_amounts(&self, price_data: PriceData, path: Path) -> Result<Vec<U256>, Error> {
        let amount_in = price_data.amount_per_trade_unit(path);
        let dex_path = path.build(self.contracts)?;
        let amounts = self
            .contracts
            .router()?
            .try_get_amounts_out(amount_in, dex_path)
            .map_err(|e| Error::OdraError {
                message: format!("get_amounts_out failed: {e:?}"),
            })?;
        Ok(amounts)
    }

    /// Unwraps WCSPR → native CSPR, stakes it, then sells the resulting sCSPR on the DEX.
    ///
    /// * `cspr_to_stake` — native CSPR motes to stake
    /// * `wcspr_out`     — exact WCSPR motes the swap should return
    #[instrument(skip(self))]
    fn execute_stake_and_sell(
        &mut self,
        cspr_to_stake: U256,
        wcspr_out: U256,
    ) -> Result<(), Error> {
        if self.dry_run {
            tracing::info!("Dry run — stake_and_sell skipped");
            return Ok(());
        }

        let me = self.caller;

        // 1. Unwrap WCSPR → native CSPR
        tracing::info!(
            "Unwrapping {:.4} WCSPR",
            cspr_to_stake.as_u64() as f64 / 1e9
        );
        self.env.set_gas(cspr!(4));
        self.contracts.wcspr()?.try_withdraw(&cspr_to_stake)?;

        // 2. Stake native CSPR → receive sCSPR
        tracing::info!("Staking {:.4} CSPR", cspr_to_stake.as_u64() as f64 / 1e9);
        let cspr_u512 = U512::from(cspr_to_stake.as_u64());
        self.env.set_gas(cspr!(9));
        self.contracts
            .staked_cspr()?
            .with_tokens(cspr_u512)
            .try_stake()?;

        // 3. Sell sCSPR → WCSPR via Router (use actual balance as amount_in_max)
        let scspr_balance = self.contracts.staked_cspr()?.balance_of(&me);
        let dex_path = Path::StCsprCspr.build(self.contracts)?;
        tracing::info!(
            "Swapping {:.4} sCSPR for {:.4} WCSPR",
            scspr_balance.as_u64() as f64 / 1e9,
            wcspr_out.as_u64() as f64 / 1e9
        );
        self.env.set_gas(cspr!(8));
        self.contracts.router()?.swap_tokens_for_exact_tokens(
            wcspr_out,
            scspr_balance,
            dex_path,
            me,
            u64::MAX,
        );

        tracing::info!("StakeAndSell complete");
        Ok(())
    }

    /// Buys sCSPR on the DEX then initiates an unstake. Records a pending claim.
    ///
    /// * `wcspr_in`   — WCSPR motes to spend (amount_in_max)
    /// * `stcspr_out` — exact sCSPR motes to buy
    #[instrument(skip(self))]
    fn execute_buy_and_unstake(&mut self, wcspr_in: U256, stcspr_out: U256) -> Result<(), Error> {
        if self.dry_run {
            tracing::info!("Dry run — buy_and_unstake skipped");
            return Ok(());
        }

        let me = self.caller;

        // 1. Buy sCSPR on DEX
        let dex_path = Path::CsprStCspr.build(self.contracts)?;
        tracing::info!(
            "Buying {:.4} sCSPR with {:.4} WCSPR",
            stcspr_out.as_u64() as f64 / 1e9,
            wcspr_in.as_u64() as f64 / 1e9,
        );
        self.env.set_gas(cspr!(8));
        self.contracts.router()?.swap_tokens_for_exact_tokens(
            stcspr_out,
            wcspr_in,
            dex_path,
            me,
            u64::MAX,
        );

        // 2. Initiate unstake — use actual on-chain balance to avoid slippage mismatch
        let scspr_balance = self.contracts.staked_cspr()?.balance_of(&me);
        tracing::info!(
            "Initiating unstake of {:.4} sCSPR",
            scspr_balance.as_u64() as f64 / 1e9
        );
        self.env.set_gas(cspr!(6));
        self.contracts.staked_cspr()?.try_unstake(scspr_balance)?;

        // 3. Record pending claim (use claim_time from contract + wall clock)
        let claim_delay_ms = self.contracts.staked_cspr()?.get_claim_time();
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
        Ok(())
    }
}
