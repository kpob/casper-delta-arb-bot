use odra::{casper_types::U256, host::HostEnv, schema::casper_contract_schema::NamedCLType};
use odra_cli::{
    cspr,
    scenario::{Args, Error, Scenario, ScenarioMetadata},
    DeployedContractsContainer,
};

use crate::contracts::ContractRefs;

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

        tracing::info!(
            "Unwrapping {:.2} wCSPR",
            amount.as_u64() as f64 / 1_000_000_000.0
        );
        env.set_gas(cspr!(4));
        contracts.wcspr()?.try_withdraw(&amount)?;
        tracing::info!("Unwrapped successfully");
        Ok(())
    }
}
