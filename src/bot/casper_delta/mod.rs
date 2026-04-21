mod data;
mod engine;
mod path;
mod trader;
mod utils;

use crate::bot::asset_manager::{
    AssetManager, Config, DryRunTokenManager, RealBalances, RealTokenManager, TokenManager,
};
use crate::contracts::ContractRefs;
use odra::{host::HostEnv, prelude::Address};
use odra_cli::scenario::Error;
use utils::PriceCalculator;

pub use engine::CasperDeltaEngine;

use self::trader::{DeltaAssetManager, DeltaOps, DryRunDeltaOps, RealDeltaBalances, RealDeltaOps};

/// Owns the dependencies that `CasperDeltaEngine` borrows from.
/// Create once at startup, then call `build_engine` to get the engine.
pub struct CasperDeltaSetup<'a> {
    token_manager: Box<dyn TokenManager + 'a>,
    balances: RealBalances<'a>,
    delta_balances: RealDeltaBalances<'a>,
    delta_ops: Box<dyn DeltaOps + 'a>,
}

impl<'a> CasperDeltaSetup<'a> {
    pub fn new(env: &'a HostEnv, contracts: &'a ContractRefs<'a>, dry_run: bool) -> Self {
        let token_manager: Box<dyn TokenManager + 'a> = if dry_run {
            tracing::info!("Dry run mode enabled");
            Box::new(DryRunTokenManager)
        } else {
            Box::new(RealTokenManager::new(env, contracts))
        };
        let delta_ops: Box<dyn DeltaOps + 'a> = if dry_run {
            Box::new(DryRunDeltaOps)
        } else {
            Box::new(RealDeltaOps::new(Config::from_env(), env, contracts))
        };
        Self {
            token_manager,
            balances: RealBalances::new(env, contracts),
            delta_balances: RealDeltaBalances::new(env, contracts),
            delta_ops,
        }
    }

    pub fn build_engine(
        &'a self,
        refs: &'a ContractRefs<'a>,
        caller: Address,
    ) -> Result<CasperDeltaEngine<'a>, Error> {
        let calc = PriceCalculator::new(refs);
        let cfg = Config::from_env();
        let assets = AssetManager::new(cfg.clone(), &self.balances, &*self.token_manager);
        let delta_assets =
            DeltaAssetManager::new(cfg, assets, &self.delta_balances, &*self.delta_ops)
                .with_refs(refs);
        delta_assets.approve_delta_markets()?;
        delta_assets.print_balances()?;
        Ok(CasperDeltaEngine::new(calc, delta_assets, caller))
    }
}
