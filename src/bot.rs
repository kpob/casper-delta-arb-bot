use std::time::Duration;

use odra::host::HostEnv;
use odra::prelude::*;
use odra::schema::casper_contract_schema::NamedCLType;
use odra_cli::{
    scenario::{Args, Error, Scenario, ScenarioMetadata},
    DeployedContractsContainer,
};

use crate::bot::asset_manager::{DryRunTokenManager, RealBalances, RealTokenManager, TokenManager};
use crate::bot::{
    asset_manager::AssetManager, utils::PriceCalculator,
};
use crate::contracts::ContractRefs;

use self::engine::BotEngine;
use self::events::{EventSource, TimerEventSource};

mod asset_manager;
mod data;
mod engine;
mod events;
mod path;
mod utils;

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
        let token_manager = self.build_token_manager(dry_run, env, &contracts);
        let balances = RealBalances::new(env, &contracts);
        let asset_manager = AssetManager::new(&balances, &*token_manager);
        token_manager.approve_markets()?;
        asset_manager.print_balances()?;

        let engine = BotEngine::new(calc, asset_manager, &contracts, caller);
        let mut event_source = TimerEventSource::new(Duration::from_secs(180));

        while let Some(event) = event_source.next_event() {
            tracing::info!("Event: {:?}", event);
            match engine.handle_event(&event) {
                Ok(true) => continue,
                Ok(false) => break,
                Err(e) => {
                    tracing::error!("Error handling event: {:?}", e);
                }
            }
        }
        Ok(())
    }
}

impl Bot {
    fn build_token_manager<'a>(
        &self,
        dry_run: bool,
        env: &'a HostEnv,
        contracts: &'a ContractRefs<'a>,
    ) -> Box<dyn TokenManager + 'a> {
        if dry_run {
            tracing::info!("Dry run mode enabled");
            Box::new(DryRunTokenManager)
        } else {
            Box::new(RealTokenManager::new(env, contracts))
        }
    }
}
