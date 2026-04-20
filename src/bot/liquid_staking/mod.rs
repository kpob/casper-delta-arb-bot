pub mod claims;
pub mod data;
pub mod path;
pub mod utils;

use odra::casper_types::{U256, U512};
use odra::host::{HostEnv, HostRef}; // HostRef required for .with_tokens() on HostRef types
use odra::prelude::{Address, Addressable};
use odra_cli::{cspr, scenario::Error};
use tracing::instrument;

use crate::bot::events::BotEvent;
use crate::contracts::ContractRefs;
use claims::{PendingClaim, PendingClaims};
use data::LsPriceData;
use path::LsPath;
use utils::LsPriceCalculator;

const CLAIMS_FILE: &str = "pending_claims.json";
const MIN_GAIN_CSPR: f64 = 50.0;

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

    /// Approves the Router to spend sCSPR on behalf of the caller.
    /// Must be called once at startup before any StakeAndSell swap.
    pub fn approve_stcspr(&self) -> Result<(), Error> {
        if self.dry_run {
            return Ok(());
        }
        let me = self.caller;
        let router_addr = self.contracts.router()?.address();
        if self.contracts.staked_cspr()?.allowance(&me, &router_addr).is_zero() {
            self.env.set_gas(cspr!(4));
            self.contracts.staked_cspr()?.approve(&router_addr, &U256::MAX);
        }
        Ok(())
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
                self.check_and_trade()?;
                Ok(true)
            }
            BotEvent::Shutdown => Ok(false),
        }
    }

    fn check_and_trade(&mut self) -> Result<(), Error> {
        let price_data = LsPriceCalculator::new(self.contracts).prices()?;
        price_data.log();

        let path = LsPath::from(&price_data);
        tracing::info!("LS swap path: {:?}", path);
        if path == LsPath::Empty {
            tracing::info!("No LS arbitrage opportunity");
            return Ok(());
        }

        let amounts = self.get_swap_amounts(&price_data, path)?;
        let (amount_in, amount_out) = match amounts.as_slice() {
            [a_in, .., a_out] => (*a_in, *a_out),
            _ => {
                tracing::info!("No valid LS swap amounts");
                return Ok(());
            }
        };

        let gain = LsPriceCalculator::calc_gains_in_cspr(amount_in, amount_out, &price_data, path);
        tracing::info!("LS gain estimate: {:.4} CSPR", gain);

        if gain < MIN_GAIN_CSPR {
            tracing::info!("LS gain below threshold ({MIN_GAIN_CSPR} CSPR), skipping");
            return Ok(());
        }

        match path {
            LsPath::StakeAndSell => {
                // amount_in = sCSPR to sell (estimated from staking)
                // amount_out = WCSPR to receive
                let cspr_to_stake_motes =
                    (amount_in.as_u64() as f64 * price_data.fair_price) as u64;
                self.execute_stake_and_sell(U256::from(cspr_to_stake_motes), amount_out)?;
            }
            LsPath::BuyAndUnstake => {
                // amount_in = WCSPR to spend, amount_out = sCSPR to receive
                self.execute_buy_and_unstake(amount_in, amount_out)?;
            }
            LsPath::Empty => unreachable!(),
        }

        Ok(())
    }

    fn get_swap_amounts(&self, price_data: &LsPriceData, path: LsPath) -> Result<Vec<U256>, Error> {
        let amount_in = price_data.amount_per_ten_usd(path);
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
        tracing::info!("Unwrapping {:.4} WCSPR", cspr_to_stake.as_u64() as f64 / 1e9);
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
        let dex_path = LsPath::StakeAndSell.build(self.contracts)?;
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
    fn execute_buy_and_unstake(
        &mut self,
        wcspr_in: U256,
        stcspr_out: U256,
    ) -> Result<(), Error> {
        if self.dry_run {
            tracing::info!("Dry run — buy_and_unstake skipped");
            return Ok(());
        }

        let me = self.caller;

        // 1. Buy sCSPR on DEX
        let dex_path = LsPath::BuyAndUnstake.build(self.contracts)?;
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
        tracing::info!("Initiating unstake of {:.4} sCSPR", scspr_balance.as_u64() as f64 / 1e9);
        self.env.set_gas(cspr!(6));
        self.contracts.staked_cspr()?.try_unstake(scspr_balance)?;

        // 3. Record pending claim (use claim_time from contract + wall clock)
        let claim_delay_ms = self.contracts.staked_cspr()?.get_claim_time();
        let claimable_from_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            + claim_delay_ms;

        tracing::info!("Recording pending claim (claimable in ~{} ms)", claim_delay_ms);
        self.claims.add(PendingClaim { claimable_from_ms })?;

        tracing::info!("BuyAndUnstake complete — claim recorded");
        Ok(())
    }
}
