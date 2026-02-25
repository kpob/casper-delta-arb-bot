use std::{thread::sleep, time::Duration};

use odra::host::HostEnv;
use odra::prelude::*;
use odra::{casper_types::U256, schema::casper_contract_schema::NamedCLType};
use odra_cli::{
    cspr,
    scenario::{Args, Error, Scenario, ScenarioMetadata},
    DeployedContractsContainer,
};

use crate::bot::asset_manager::{RealBalances, RealTokenManager, TokenManager};
use crate::bot::contracts::ContractRefs;
use crate::bot::{
    asset_manager::AssetManager, data::PriceData, path::Path, utils::PriceCalculator,
};

mod asset_manager;
mod contracts;
mod data;
mod path;
mod utils;

pub struct UnwrapWcspr;

impl ScenarioMetadata for UnwrapWcspr {
    const NAME: &'static str = "UnwrapWcspr";
    const DESCRIPTION: &'static str = "Unwraps all wCSPR back to CSPR.";
}

impl Scenario for UnwrapWcspr {
    fn args(&self) -> Vec<odra_cli::CommandArg> {
        vec![odra_cli::CommandArg::new(
            "amount",
            "Amount of wCSPR to unwrap (in motes). Defaults to full balance.",
            NamedCLType::U256,
        )]
    }

    fn run(
        &self,
        env: &HostEnv,
        container: &DeployedContractsContainer,
        args: Args,
    ) -> Result<(), Error> {
        let contracts = ContractRefs::new(env, container);
        let me = env.caller();
        let wcspr_balance = contracts.wcspr()?.balance_of(&me);

        if wcspr_balance.is_zero() {
            tracing::info!("No wCSPR to unwrap");
            return Ok(());
        }

        let amount: U256 = args.get_single("amount").unwrap_or(wcspr_balance);

        tracing::info!("Unwrapping {:.2} wCSPR", amount.as_u64() as f64 / 1_000_000_000.0);
        env.set_gas(cspr!(4));
        contracts.wcspr()?.try_withdraw(&amount)?;
        tracing::info!("Unwrapped successfully");
        Ok(())
    }
}

pub struct BotSetup;

impl ScenarioMetadata for BotSetup {
    const NAME: &'static str = "BotSetup";
    const DESCRIPTION: &'static str = "Sets up the environment for the bot.";
}

impl Scenario for BotSetup {
    fn run(
        &self,
        env: &HostEnv,
        container: &DeployedContractsContainer,
        _args: Args,
    ) -> Result<(), Error> {
        let contracts = ContractRefs::new(env, container);
        let token_manager = RealTokenManager::new(env, &contracts);

        token_manager.approve_markets()?;
        Ok(())
    }
}

pub struct Bot;

impl ScenarioMetadata for Bot {
    const NAME: &'static str = "Bot";
    const DESCRIPTION: &'static str = "Runs the bot.";
}

impl Scenario for Bot {
    fn args(&self) -> Vec<odra_cli::CommandArg> {
        vec![odra_cli::CommandArg::new(
            "dry-run",
            "Dry run the bot",
            NamedCLType::Bool,
        )]
    }

    fn run(
        &self,
        env: &HostEnv,
        container: &DeployedContractsContainer,
        args: Args,
    ) -> Result<(), Error> {
        let contracts = ContractRefs::new(env, container);
        let calc = PriceCalculator::new(&contracts);
        let caller = env.caller();

        let dry_run = args.get_single("dry-run").unwrap_or(false);
        if dry_run {
            tracing::info!("Dry run mode enabled");
        }
        let balances = RealBalances::new(env, &contracts);
        let token_manager = RealTokenManager::new(env, &contracts);
        let asset_manager = AssetManager::new(&balances, &token_manager);
        asset_manager.print_balances()?;

        loop {
            tracing::info!("Current time: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));

            let price_data = self.get_price_data(&calc)?;
            tracing::info!("{}", price_data);

            asset_manager.manage_asset_levels(&price_data, caller)?;

            let path = Path::from(&price_data);
            tracing::info!("Swap path: {:?}", path);
            if path == Path::Empty {
                tracing::info!("No arbitrage path found");
                self.cool_down();
                continue;
            }

            let amounts = get_swap_amounts(&contracts, &price_data, path);
            if let Ok([amount_in, .., amount_out]) = amounts.as_deref() {
                let gain =
                    PriceCalculator::calc_gains_in_cspr(*amount_in, *amount_out, &price_data, path);
                tracing::info!("Gain: {:<10.4} CSPR", gain);
                if gain < 1.0f64 {
                    tracing::info!("No arbitrage path found");
                    self.cool_down();
                    continue;
                }
                if dry_run {
                    tracing::info!("Dry run mode - no swap completed");
                    self.cool_down();
                    continue;
                }

                let (actual_amount_in, actual_amount_out) =
                    self.swap(&asset_manager, path, *amount_in, *amount_out, caller)?;
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
            self.cool_down();
        }
    }
}

impl Bot {
    fn swap(
        &self,
        asset_manager: &AssetManager,
        path: Path,
        amount_in: U256,
        amount_out: U256,
        recipient: Address,
    ) -> Result<(U256, U256), Error> {
        tracing::info!("Preparing swap...");
        let result = asset_manager.swap(path, amount_in, amount_out, recipient)?;
        tracing::info!("Arbitrage swap completed");
        asset_manager.print_balances()?;

        if let [amount_in, .., amount_out] = result.as_slice() {
            Ok((*amount_in, *amount_out))
        } else {
            Err(Error::OdraError {
                message: "Invalid swap result".to_string(),
            })
        }
    }

    fn get_price_data(&self, calc: &PriceCalculator) -> Result<PriceData, Error> {
        let (long_price, short_price) = calc.casper_trade_prices()?;
        let (long_fair_price, short_fair_price, wcspr_price) = calc.fair_prices()?;

        Ok(PriceData::new(
            long_price,
            short_price,
            wcspr_price,
            long_fair_price,
            short_fair_price,
        ))
    }

    fn cool_down(&self) {
        tracing::info!("Sleeping for 3 minutes...");
        sleep(Duration::from_secs(180));
    }
}

fn get_swap_amounts(
    refs: &ContractRefs,
    price_data: &PriceData,
    path: Path,
) -> Result<Vec<U256>, Error> {
    let amount_in = price_data.amount_per_one_usd(path);
    let path = path.build(refs)?;
    let amounts = refs
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
