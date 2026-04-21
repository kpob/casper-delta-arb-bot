use odra::host::HostEnv;
use odra::prelude::*;
use odra::schema::casper_contract_schema::NamedCLType;
use odra_cli::{
    scenario::{Args, Error, Scenario, ScenarioMetadata},
    DeployedContractsContainer,
};

use crate::bot::casper_delta::CasperDeltaSetup;
use crate::contracts::ContractRefs;

use self::events::{EventSource, KafkaConfig, KafkaEventSource};
use self::liquid_staking::LsEngine;

mod arb;
mod asset_manager;
mod casper_delta;
mod events;
mod liquid_staking;

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
        let cd_engine = setup.build_engine(&contracts, caller)?;
        let mut ls_engine = LsEngine::new(&contracts, env, caller, dry_run);
        ls_engine.approve_stcspr()?;

        let mut event_source = self.setup_event_source(&contracts);

        while let Some(event) = event_source.next_event() {
            tracing::info!("Event: {:?}", event);
            if let Err(e) = ls_engine.try_claim_ready() {
                tracing::error!("LS claim error: {:?}", e);
            }
            match cd_engine.handle_event(&event) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => tracing::error!("Delta engine error: {:?}", e),
            }
            match ls_engine.handle_event(&event) {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => tracing::error!("LS engine error: {:?}", e),
            }
        }
        Ok(())
    }
}

impl Bot {
    fn setup_event_source(&self, contracts: &ContractRefs) -> impl EventSource {
        let config = KafkaConfig::from_env();
        tracing::info!("Connecting to Kafka at {}", config.bootstrap_servers);
        let relevant_addresses = vec![
            contracts.long().unwrap().address().to_string(),
            contracts.short().unwrap().address().to_string(),
            contracts.staked_cspr().unwrap().address().to_string(),
        ];
        KafkaEventSource::new(config, relevant_addresses)
    }
}
