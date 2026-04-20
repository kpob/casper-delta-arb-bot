mod asset_manager;
mod data;
mod engine;
mod path;
mod utils;

use crate::contracts::ContractRefs;
use asset_manager::{AssetManager, DryRunTokenManager, RealBalances, RealTokenManager, TokenManager};
use odra::{host::HostEnv, prelude::Address};
use odra_cli::scenario::Error;
use utils::PriceCalculator;

pub use engine::CasperDeltaEngine;

/// Owns the dependencies that `CasperDeltaEngine` borrows from.
/// Create once at startup, then call `build_engine` to get the engine.
pub struct CasperDeltaSetup<'a> {
    token_manager: Box<dyn TokenManager + 'a>,
    balances: RealBalances<'a>,
}

impl<'a> CasperDeltaSetup<'a> {
    pub fn new(
        env: &'a HostEnv,
        contracts: &'a ContractRefs<'a>,
        dry_run: bool,
    ) -> Result<Self, Error> {
        let token_manager: Box<dyn TokenManager + 'a> = if dry_run {
            tracing::info!("Dry run mode enabled");
            Box::new(DryRunTokenManager)
        } else {
            Box::new(RealTokenManager::new(env, contracts))
        };
        token_manager.approve_markets()?;
        let balances = RealBalances::new(env, contracts);
        Ok(Self { token_manager, balances })
    }

    pub fn build_engine(
        &'a self,
        contracts: &'a ContractRefs<'a>,
        caller: Address,
    ) -> Result<CasperDeltaEngine<'a>, Error> {
        let calc = PriceCalculator::new(contracts);
        let asset_manager = AssetManager::new(&self.balances, &*self.token_manager);
        asset_manager.print_balances()?;
        Ok(CasperDeltaEngine::new(calc, asset_manager, contracts, caller))
    }
}
