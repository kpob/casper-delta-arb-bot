use casper_delta_contracts::{
    market::{Market, MarketHostRef},
    position_token::{PositionToken, PositionTokenHostRef},
    wrapped_native::{WrappedNativeToken, WrappedNativeTokenHostRef},
};
use casper_trade_contracts::{
    pair::{Pair, PairHostRef},
    router::{Router, RouterHostRef},
};
use odra::host::HostEnv;
use odra_cli::{scenario::Error, ContractProvider, DeployedContractsContainer};

pub struct ContractRefs<'a> {
    env: &'a HostEnv,
    container: &'a DeployedContractsContainer,
}

impl<'a> ContractRefs<'a> {
    pub fn new(env: &'a HostEnv, container: &'a DeployedContractsContainer) -> Self {
        Self { env, container }
    }

    pub fn router(&self) -> Result<RouterHostRef, Error> {
        Ok(self.container.contract_ref::<Router>(self.env)?)
    }

    pub fn long_wcspr_pair(&self) -> Result<PairHostRef, Error> {
        Ok(self
            .container
            .contract_ref_named::<Pair>(self.env, Some("CD_LONG-WCSPR LP".to_string()))?)
    }

    pub fn wcspr_short_pair(&self) -> Result<PairHostRef, Error> {
        Ok(self
            .container
            .contract_ref_named::<Pair>(self.env, Some("WCSPR-CD_SHORT LP".to_string()))?)
    }

    pub fn market(&self) -> Result<MarketHostRef, Error> {
        Ok(self.container.contract_ref::<Market>(self.env)?)
    }

    pub fn wcspr(&self) -> Result<WrappedNativeTokenHostRef, Error> {
        Ok(self
            .container
            .contract_ref::<WrappedNativeToken>(self.env)?)
    }

    pub fn long(&self) -> Result<PositionTokenHostRef, Error> {
        Ok(self
            .container
            .contract_ref_named::<PositionToken>(self.env, Some("CD_LONG".to_string()))?)
    }

    pub fn short(&self) -> Result<PositionTokenHostRef, Error> {
        Ok(self
            .container
            .contract_ref_named::<PositionToken>(self.env, Some("CD_SHORT".to_string()))?)
    }
}
