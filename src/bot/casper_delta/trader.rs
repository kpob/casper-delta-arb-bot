use odra::{casper_types::U256, host::HostEnv, prelude::Address};
use odra_cli::{cspr, scenario::Error};

#[cfg(test)]
use mockall::automock;

use super::path::Path;
use crate::contracts::ContractRefs;

/// Delta-specific on-chain operations: mint positions, swap. (Approvals are
/// handled by `DeltaAssetManager` via the generic `TokenManager`.)
#[cfg_attr(test, automock)]
pub trait DeltaOps {
    fn swap(
        &self,
        path: Path,
        amount_in: U256,
        amount_out: U256,
        recipient: Address,
    ) -> Result<Vec<U256>, Error>;
}

pub struct RealDeltaOps<'a> {
    env: &'a HostEnv,
    refs: &'a ContractRefs<'a>,
}

impl<'a> RealDeltaOps<'a> {
    pub fn new(env: &'a HostEnv, refs: &'a ContractRefs<'a>) -> Self {
        Self { env, refs }
    }
}

impl DeltaOps for RealDeltaOps<'_> {
    fn swap(
        &self,
        path: Path,
        amount_in: U256,
        amount_out: U256,
        recipient: Address,
    ) -> Result<Vec<U256>, Error> {
        if path.is_multi_hop() {
            self.env.set_gas(cspr!(13));
        } else {
            self.env.set_gas(cspr!(8));
        }
        let result = self.refs.router()?.swap_tokens_for_exact_tokens(
            amount_out,
            amount_in,
            path.build(self.refs)?,
            recipient,
            u64::MAX,
        );
        Ok(result)
    }
}

pub struct DryRunDeltaOps;

impl DeltaOps for DryRunDeltaOps {
    fn swap(
        &self,
        _path: Path,
        amount_in: U256,
        amount_out: U256,
        _recipient: Address,
    ) -> Result<Vec<U256>, Error> {
        tracing::info!("Dry run - swap skipped");
        Ok(vec![amount_in, amount_out])
    }
}

/// Casper Delta asset manager. Composes the generic `AssetManager` (for
/// CSPR/wCSPR) with delta-specific balances and ops (longs, shorts, swap).
pub struct DeltaAssetManager<'a> {
    delta_ops: &'a dyn DeltaOps,
}

impl<'a> DeltaAssetManager<'a> {
    pub fn new(delta_ops: &'a dyn DeltaOps) -> Self {
        Self { delta_ops }
    }

    pub fn swap(
        &self,
        path: Path,
        amount_in: U256,
        amount_out: U256,
        recipient: Address,
    ) -> Result<Vec<U256>, Error> {
        let result = self
            .delta_ops
            .swap(path, amount_in, amount_out, recipient)?;
        Ok(result)
    }
}
