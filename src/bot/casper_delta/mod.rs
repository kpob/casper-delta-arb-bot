mod data;
mod engine;
mod path;
mod trader;
mod utils;

use crate::contracts::ContractRefs;
use odra::{host::HostEnv, prelude::Address};
use odra_cli::scenario::Error;
use utils::PriceCalculator;

pub use engine::CasperDeltaEngine;

use self::trader::{DeltaAssetManager, DeltaOps, DryRunDeltaOps, RealDeltaOps};

/// Owns the dependencies that `CasperDeltaEngine` borrows from.
/// Create once at startup, then call `build_engine` to get the engine.
pub struct CasperDeltaSetup<'a> {
    delta_ops: Box<dyn DeltaOps + 'a>,
}

impl<'a> CasperDeltaSetup<'a> {
    pub fn new(env: &'a HostEnv, contracts: &'a ContractRefs<'a>, dry_run: bool) -> Self {
        let delta_ops: Box<dyn DeltaOps + 'a> = if dry_run {
            Box::new(DryRunDeltaOps)
        } else {
            Box::new(RealDeltaOps::new(env, contracts))
        };
        Self { delta_ops }
    }

    pub fn build_engine(
        &'a self,
        refs: &'a ContractRefs<'a>,
        caller: Address,
    ) -> Result<CasperDeltaEngine<'a>, Error> {
        let calc = PriceCalculator::new(refs);
        let delta_assets = DeltaAssetManager::new(&*self.delta_ops);
        // delta_assets.print_balances()?;
        Ok(CasperDeltaEngine::new(calc, delta_assets, caller))
    }
}
