mod claims;
mod engine;
mod path;
mod price;

use odra::{host::HostEnv, prelude::Address};

use crate::contracts::ContractRefs;

pub use engine::LsEngine;
use engine::LsStrategy;

/// Convenience constructor mirroring `CasperDeltaSetup::build_engine`.
pub fn build_engine<'a>(
    refs: &'a ContractRefs<'a>,
    env: &'a HostEnv,
    caller: Address,
    dry_run: bool,
) -> LsEngine<'a> {
    LsEngine::new(LsStrategy::new(refs, env, dry_run), caller)
}
