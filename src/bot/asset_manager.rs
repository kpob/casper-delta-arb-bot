use casper_delta_contracts::position_token::PositionTokenHostRef;
use odra::{
    casper_types::{U256, U512},
    host::{HostEnv, HostRef},
    prelude::Address,
    uints::ToU256,
};
use odra_cli::{cspr, scenario::Error};

use crate::contracts::ContractRefs;

#[cfg(test)]
use mockall::automock;

#[derive(Debug, Clone)]
pub struct Config {
    pub top_up_amount: U256,
    pub min_cspr_balance: U256,
    pub unwrap_amount: U256,
}

impl Config {
    pub fn from_env() -> Self {
        let top_up_amount = std::env::var("BOT_TOP_UP_AMOUNT")
            .unwrap_or_else(|_| "2000000000000".to_string())
            .parse::<u64>()
            .expect("Invalid BOT_TOP_UP_AMOUNT");
        let min_cspr_balance = std::env::var("BOT_MIN_CSPR_BALANCE")
            .unwrap_or_else(|_| "100000000000".to_string())
            .parse::<u64>()
            .expect("Invalid BOT_MIN_CSPR_BALANCE");
        let unwrap_amount = std::env::var("BOT_UNWRAP_AMOUNT")
            .unwrap_or_else(|_| "1000000000000".to_string())
            .parse::<u64>()
            .expect("Invalid BOT_UNWRAP_AMOUNT");

        Self {
            top_up_amount: U256::from(top_up_amount),
            min_cspr_balance: U256::from(min_cspr_balance),
            unwrap_amount: U256::from(unwrap_amount),
        }
    }
}

#[cfg_attr(test, automock)]
pub trait Balances {
    fn my_cspr_balance(&self) -> Result<U256, Error>;
    fn my_wcspr_balance(&self) -> Result<U256, Error>;
}

#[cfg_attr(test, automock)]
pub trait TokenManager {
    fn wrap_cspr(&self, amount: U256) -> Result<(), Error>;
    fn unwrap_wcspr(&self, amount: U256) -> Result<(), Error>;
    fn approve(&self, token: Address, spender: Address, amount: U256) -> Result<(), Error>;
    fn allowance(&self, token: Address, spender: Address) -> Result<U256, Error>;
}

pub struct RealTokenManager<'a> {
    env: &'a HostEnv,
    refs: &'a ContractRefs<'a>,
}

impl<'a> RealTokenManager<'a> {
    pub fn new(env: &'a HostEnv, refs: &'a ContractRefs<'a>) -> Self {
        Self { env, refs }
    }
}

impl TokenManager for RealTokenManager<'_> {
    fn wrap_cspr(&self, amount: U256) -> Result<(), Error> {
        self.env.set_gas(cspr!(4));
        let amount_u512 = U512::from(amount.as_u128());
        self.refs.wcspr()?.with_tokens(amount_u512).try_deposit()?;
        Ok(())
    }

    fn unwrap_wcspr(&self, amount: U256) -> Result<(), Error> {
        self.env.set_gas(cspr!(4));
        self.refs.wcspr()?.try_withdraw(&amount)?;
        Ok(())
    }

    fn approve(&self, token: Address, spender: Address, amount: U256) -> Result<(), Error> {
        self.env.set_gas(cspr!(4));
        let mut erc20 = PositionTokenHostRef::new(token, self.env.clone());
        erc20.approve(&spender, &amount);
        Ok(())
    }

    fn allowance(&self, token: Address, spender: Address) -> Result<U256, Error> {
        let me = self.env.caller();
        let erc20 = PositionTokenHostRef::new(token, self.env.clone());
        Ok(erc20.allowance(&me, &spender))
    }
}

pub struct DryRunTokenManager;

impl TokenManager for DryRunTokenManager {
    fn wrap_cspr(&self, _amount: U256) -> Result<(), Error> {
        Ok(())
    }

    fn unwrap_wcspr(&self, _amount: U256) -> Result<(), Error> {
        Ok(())
    }

    fn approve(&self, _token: Address, _spender: Address, _amount: U256) -> Result<(), Error> {
        Ok(())
    }

    fn allowance(&self, _token: Address, _spender: Address) -> Result<U256, Error> {
        Ok(U256::MAX)
    }
}

pub struct RealBalances<'a> {
    env: &'a HostEnv,
    refs: &'a ContractRefs<'a>,
}

impl<'a> RealBalances<'a> {
    pub fn new(env: &'a HostEnv, refs: &'a ContractRefs<'a>) -> Self {
        Self { env, refs }
    }
}

impl Balances for RealBalances<'_> {
    fn my_cspr_balance(&self) -> Result<U256, Error> {
        let me = self.env.caller();
        println!("Fetching CSPR balance for {:?}", me);
        self.env
            .balance_of(&me)
            .to_u256()
            .map_err(|_| Error::OdraError {
                message: "Failed to convert cspr balance to u256".to_string(),
            })
    }

    fn my_wcspr_balance(&self) -> Result<U256, Error> {
        let me = self.env.caller();
        Ok(self.refs.wcspr()?.balance_of(&me))
    }
}

/// Generic, app-agnostic asset manager.
///
/// Handles only CSPR and wCSPR: topping up wCSPR from CSPR, unwrapping wCSPR
/// when CSPR runs low, printing balances. App-specific position management
/// (e.g. longs/shorts, staked CSPR) lives in the respective app module.
pub struct AssetManager<'a> {
    cfg: Config,
    balances: &'a dyn Balances,
    token_manager: &'a dyn TokenManager,
}

impl<'a> AssetManager<'a> {
    pub fn new(
        cfg: Config,
        balances: &'a dyn Balances,
        token_manager: &'a dyn TokenManager,
    ) -> Self {
        Self {
            cfg,
            balances,
            token_manager,
        }
    }

    pub fn balances(&self) -> &'a dyn Balances {
        self.balances
    }

    pub fn token_manager(&self) -> &'a dyn TokenManager {
        self.token_manager
    }

    /// Ensure there is at least `required` wCSPR available, wrapping CSPR if
    /// needed. Errors if CSPR balance is insufficient to wrap.
    pub fn ensure_wcspr(&self, required: U256) -> Result<(), Error> {
        let wcspr_balance = self.balances.my_wcspr_balance()?;
        log_humanized("Required WCSPR balance", required);
        log_humanized("Current WCSPR balance", wcspr_balance);
        if wcspr_balance < required {
            tracing::warn!("Not enough wcspr, topping up");
            self.wrap_cspr(self.cfg.top_up_amount)?;
            log_humanized("New WCSPR balance", self.balances.my_wcspr_balance()?);
        }
        Ok(())
    }

    /// Unwrap `amount` wCSPR into CSPR.
    pub fn unwrap_wcspr(&self, amount: U256) -> Result<(), Error> {
        self.token_manager.unwrap_wcspr(amount)
    }

    /// Wrap `amount` CSPR into wCSPR. Errors if CSPR balance is insufficient.
    pub fn wrap_cspr(&self, amount: U256) -> Result<(), Error> {
        let cspr_balance = self.balances.my_cspr_balance()?;
        if cspr_balance < amount {
            return Err(Error::OdraError {
                message: "Not enough cspr to wrap".to_string(),
            });
        }
        self.token_manager.wrap_cspr(amount)
    }

    /// Keep CSPR/wCSPR levels healthy. Returns `Ok(true)` if an action was
    /// taken (caller may want to short-circuit further work this tick),
    /// `Ok(false)` otherwise.
    pub fn manage_cspr_levels(&self) -> Result<bool, Error> {
        let cspr_balance = self.balances.my_cspr_balance()?;
        if cspr_balance < self.cfg.min_cspr_balance {
            tracing::warn!(
                "CSPR balance low ({:.2} CSPR), unwrapping {:.2} wCSPR",
                humanize_balance(cspr_balance),
                humanize_balance(self.cfg.unwrap_amount),
            );
            self.unwrap_wcspr(self.cfg.unwrap_amount)?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn print_balances(&self) -> Result<(), Error> {
        log_humanized("CSPR balance", self.balances.my_cspr_balance()?);
        log_humanized("WCSPR balance", self.balances.my_wcspr_balance()?);
        Ok(())
    }
}

pub fn humanize_balance(balance: U256) -> f64 {
    balance.as_u64() as f64 / 1_000_000_000.0f64
}

pub fn log_humanized(label: &str, balance: U256) {
    tracing::info!("{}: {:.2}", label, humanize_balance(balance));
}

#[cfg(test)]
mod tests {
    use super::*;
    use odra_test::env;

    const TOP_UP_AMOUNT: u64 = 2_000_000_000_000; // 2000 CSPR
    const MIN_CSPR_BALANCE: u64 = 100_000_000_000; // 100 CSPR
    const UNWRAP_AMOUNT: u64 = 1_000_000_000_000; // 1000 CSPR

    fn setup() -> (HostEnv, MockBalances, MockTokenManager) {
        (env(), MockBalances::new(), MockTokenManager::new())
    }

    #[test]
    fn wrap_cspr_errors_when_cspr_insufficient() {
        let (_, mut balances, token_manager) = setup();
        balances
            .expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT - 1)));

        let cfg = Config::from_env();
        let am = AssetManager::new(cfg, &balances, &token_manager);
        let err = am.wrap_cspr(U256::from(TOP_UP_AMOUNT)).unwrap_err();
        assert!(err.to_string().contains("Not enough cspr to wrap"));
    }

    #[test]
    fn wrap_cspr_succeeds_when_cspr_sufficient() {
        let (_, mut balances, mut token_manager) = setup();
        balances
            .expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT * 2)));
        token_manager
            .expect_wrap_cspr()
            .times(1)
            .return_once(|_| Ok(()));

        let cfg = Config::from_env();
        let am = AssetManager::new(cfg, &balances, &token_manager);
        assert!(am.wrap_cspr(U256::from(TOP_UP_AMOUNT)).is_ok());
    }

    #[test]
    fn ensure_wcspr_wraps_when_below_required() {
        let (_, mut balances, mut token_manager) = setup();
        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(100)));
        balances
            .expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT * 2)));
        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT + 100)));
        token_manager
            .expect_wrap_cspr()
            .times(1)
            .return_once(|_| Ok(()));

        let cfg = Config::from_env();
        let am = AssetManager::new(cfg, &balances, &token_manager);
        assert!(am.ensure_wcspr(U256::from(TOP_UP_AMOUNT)).is_ok());
    }

    #[test]
    fn ensure_wcspr_noop_when_already_sufficient() {
        let (_, mut balances, token_manager) = setup();
        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT * 2)));

        let cfg = Config::from_env();
        let am = AssetManager::new(cfg, &balances, &token_manager);
        assert!(am.ensure_wcspr(U256::from(TOP_UP_AMOUNT)).is_ok());
    }

    #[test]
    fn manage_cspr_levels_unwraps_when_cspr_low() {
        let (_, mut balances, mut token_manager) = setup();
        balances
            .expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_CSPR_BALANCE - 1)));
        token_manager
            .expect_unwrap_wcspr()
            .times(1)
            .withf(|&amount| amount == U256::from(UNWRAP_AMOUNT))
            .return_once(|_| Ok(()));

        let cfg = Config::from_env();
        let am = AssetManager::new(cfg, &balances, &token_manager);
        assert_eq!(am.manage_cspr_levels().unwrap(), true);
    }

    #[test]
    fn manage_cspr_levels_noop_when_cspr_sufficient() {
        let (_, mut balances, token_manager) = setup();
        balances
            .expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_CSPR_BALANCE)));

        let cfg = Config::from_env();
        let am = AssetManager::new(cfg, &balances, &token_manager);
        assert_eq!(am.manage_cspr_levels().unwrap(), false);
    }

    #[test]
    fn humanize_balance_converts_motes_to_cspr() {
        assert_eq!(humanize_balance(U256::from(1_000_000_000u64)), 1.0);
        assert_eq!(humanize_balance(U256::from(2_500_000_000u64)), 2.5);
        assert_eq!(humanize_balance(U256::zero()), 0.0);
    }
}
