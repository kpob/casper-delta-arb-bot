use odra::host::HostEnv;
use odra::prelude::*;
use odra::schema::casper_contract_schema::NamedCLType;
use odra_cli::{
    scenario::{Args, Error, Scenario, ScenarioMetadata},
    DeployedContractsContainer,
};

use crate::bot::casper_delta::CasperDeltaSetup;
use crate::contracts::ContractRefs;

use self::events::{EventSource, KafkaConfig, KafkaEventSource, TradeScope};
use self::rebalancer::RealRebalancer;

mod casper_delta;
mod engine;
mod events;
mod liquid_staking;
mod rebalancer;
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
        let caller = env.caller();
        let dry_run = args.get_single("dry-run").unwrap_or(false);

        let setup = CasperDeltaSetup::new(env, &contracts, dry_run);
        let mut cd_engine = setup.build_engine(&contracts, caller)?;
        let mut ls_engine = liquid_staking::build_engine(&contracts, env, caller, dry_run);
        let rebalancer = RealRebalancer::from_env(env, &contracts, dry_run);
        rebalancer.approve_all()?;
        let mut event_source = self.setup_event_source(&contracts)?;

        while let Some(event) = event_source.next_event() {
            let span = tracing::info_span!(
                "event",
                kind = event.kind(),
                tx_hash = event.tx_hash().unwrap_or("-"),
            );
            let _enter = span.enter();
            tracing::info!("event received");
            let _ = rebalancer.rebalance();
            if let Ok(false) = cd_engine.handle_event(&event) {
                break;
            }
            if let Ok(false) = ls_engine.handle_event(&event) {
                break;
            }
        }
        tracing::info!("event source closed; shutting down");
        Ok(())
    }
}

impl Bot {
    fn setup_event_source(&self, contracts: &ContractRefs) -> Result<impl EventSource, Error> {
        let config = KafkaConfig::from_env();
        tracing::info!("Connecting to Kafka at {}", config.bootstrap_servers);
        let scoped_addresses = vec![
            (contracts.long()?.address().to_string(), TradeScope::Delta),
            (contracts.short()?.address().to_string(), TradeScope::Delta),
            (
                contracts.staked_cspr()?.address().to_string(),
                TradeScope::LiquidStaking,
            ),
        ];
        Ok(KafkaEventSource::new(config, scoped_addresses))
    }
}
