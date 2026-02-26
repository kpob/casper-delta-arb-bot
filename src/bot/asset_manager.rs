use odra::{
    casper_types::U256,
    host::{HostEnv, HostRef},
    prelude::{Address, Addressable},
    uints::ToU256,
};
use odra_cli::{cspr, scenario::Error};

use crate::{
    bot::{data::PriceData, path::Path},
    contracts::ContractRefs,
};

const TOP_UP_AMOUNT: u64 = 2_000_000_000_000; // 2_000 cspr
const MIN_CSPR_BALANCE: u64 = 100_000_000_000; // 100 CSPR
const MIN_WCSPR_BALANCE: u64 = 1_500_000_000_000; // 1_500 CSPR
const UNWRAP_AMOUNT: u64 = 1_500_000_000_000; // 1_500 CSPR

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
pub trait Balances {
    fn my_cspr_balance(&self) -> Result<U256, Error>;
    fn my_wcspr_balance(&self) -> Result<U256, Error>;
    fn my_long_balance(&self) -> Result<U256, Error>;
    fn my_short_balance(&self) -> Result<U256, Error>;
}

#[cfg_attr(test, automock)]
pub trait TokenManager {
    fn approve_markets(&self) -> Result<(), Error>;
    fn wrap_cspr(&self) -> Result<(), Error>;
    fn unwrap_wcspr(&self, amount: U256) -> Result<(), Error>;
    fn buy_longs(&self) -> Result<(), Error>;
    fn buy_shorts(&self) -> Result<(), Error>;
    fn swap(
        &self,
        path: Path,
        amount_in: U256,
        amount_out: U256,
        recipient: Address,
    ) -> Result<Vec<U256>, Error>;
}

pub struct RealTokenManager<'a> {
    env: &'a HostEnv,
    refs: &'a ContractRefs<'a>,
}

impl<'a> RealTokenManager<'a> {
    pub fn new(env: &'a HostEnv, refs: &'a ContractRefs<'a>) -> Self {
        Self { env, refs }
    }

    pub fn wcspr_allowance(&self, spender: &Address) -> Result<U256, Error> {
        let me = self.env.caller();
        Ok(self.refs.wcspr()?.allowance(&me, spender))
    }

    pub fn long_allowance(&self, spender: &Address) -> Result<U256, Error> {
        let me = self.env.caller();
        Ok(self.refs.long()?.allowance(&me, spender))
    }

    pub fn short_allowance(&self, spender: &Address) -> Result<U256, Error> {
        let me = self.env.caller();
        Ok(self.refs.short()?.allowance(&me, spender))
    }
}

impl TokenManager for RealTokenManager<'_> {
    fn approve_markets(&self) -> Result<(), Error> {
        self.env.set_gas(cspr!(4));
        let cspr_trade_address = self.refs.router()?.address();
        let cspr_delta_address = self.refs.market()?.address();
        // Casper trade must be able to spend wcspr, long and short tokens
        if self.wcspr_allowance(&cspr_trade_address)?.is_zero() {
            self.refs.wcspr()?.approve(&cspr_trade_address, &U256::MAX);
        }
        if self.long_allowance(&cspr_trade_address)?.is_zero() {
            self.refs.long()?.approve(&cspr_trade_address, &U256::MAX);
        }
        if self.short_allowance(&cspr_trade_address)?.is_zero() {
            self.refs.short()?.approve(&cspr_trade_address, &U256::MAX);
        }

        // Casper delta must be able to spend wcspr
        if self.wcspr_allowance(&cspr_delta_address)?.is_zero() {
            self.refs.wcspr()?.approve(&cspr_delta_address, &U256::MAX);
        }
        Ok(())
    }

    fn wrap_cspr(&self) -> Result<(), Error> {
        self.env.set_gas(cspr!(4));
        self.refs
            .wcspr()?
            .with_tokens(TOP_UP_AMOUNT.into())
            .try_deposit()?;
        Ok(())
    }

    fn unwrap_wcspr(&self, amount: U256) -> Result<(), Error> {
        self.env.set_gas(cspr!(4));
        self.refs.wcspr()?.try_withdraw(&amount)?;
        Ok(())
    }

    fn buy_longs(&self) -> Result<(), Error> {
        self.env.set_gas(cspr!(4));
        self.refs.market()?.try_deposit_long(TOP_UP_AMOUNT.into())?;
        Ok(())
    }

    fn buy_shorts(&self) -> Result<(), Error> {
        self.env.set_gas(cspr!(4));
        self.refs
            .market()?
            .try_deposit_short(TOP_UP_AMOUNT.into())?;
        Ok(())
    }

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

pub struct DryRunTokenManager;

impl TokenManager for DryRunTokenManager {
    fn approve_markets(&self) -> Result<(), Error> {
        Ok(())
    }

    fn wrap_cspr(&self) -> Result<(), Error> {
        Ok(())
    }

    fn unwrap_wcspr(&self, _amount: U256) -> Result<(), Error> {
        Ok(())
    }

    fn buy_longs(&self) -> Result<(), Error> {
        Ok(())
    }

    fn buy_shorts(&self) -> Result<(), Error> {
        Ok(())
    }

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
        Ok(self
            .env
            .balance_of(&me)
            .to_u256()
            .map_err(|_| Error::OdraError {
                message: "Failed to convert cspr balance to u256".to_string(),
            })?)
    }

    fn my_wcspr_balance(&self) -> Result<U256, Error> {
        let me = self.env.caller();
        Ok(self.refs.wcspr()?.balance_of(&me))
    }

    fn my_long_balance(&self) -> Result<U256, Error> {
        let me = self.env.caller();
        Ok(self.refs.long()?.balance_of(&me))
    }

    fn my_short_balance(&self) -> Result<U256, Error> {
        let me = self.env.caller();
        Ok(self.refs.short()?.balance_of(&me))
    }
}

pub struct AssetManager<'a> {
    balances: &'a dyn Balances,
    token_manager: &'a dyn TokenManager,
}

impl<'a> AssetManager<'a> {
    pub fn new(balances: &'a dyn Balances, token_manager: &'a dyn TokenManager) -> Self {
        Self {
            balances,
            token_manager,
        }
    }

    pub fn swap(
        &self,
        path: Path,
        amount_in: U256,
        amount_out: U256,
        recipient: Address,
    ) -> Result<Vec<U256>, Error> {
        self.ensure_funds(path, amount_in)?;
        let result = self
            .token_manager
            .swap(path, amount_in, amount_out, recipient)?;
        Ok(result)
    }

    pub fn manage_asset_levels(
        &self,
        price_data: &PriceData,
        recipient: Address,
    ) -> Result<(), Error> {
        let cspr_balance = self.balances.my_cspr_balance()?;
        if cspr_balance < MIN_CSPR_BALANCE.into() {
            tracing::warn!(
                "CSPR balance low ({:.2} CSPR), unwrapping {:.2} wCSPR",
                humanize_balance(cspr_balance),
                humanize_balance(UNWRAP_AMOUNT.into()),
            );
            self.token_manager.unwrap_wcspr(UNWRAP_AMOUNT.into())?;
            return Ok(());
        }

        let wcspr_balance = self.balances.my_wcspr_balance()?;
        if wcspr_balance < MIN_WCSPR_BALANCE.into() {
            tracing::warn!(
                "wCSPR balance low ({:.2} CSPR), selling positions for wCSPR",
                humanize_balance(wcspr_balance),
            );
            let long_balance = self.balances.my_long_balance()?;
            let short_balance = self.balances.my_short_balance()?;
            let long_cspr_value = long_balance.as_u64() as f64 * price_data.long_price;
            let short_cspr_value = short_balance.as_u64() as f64 * price_data.short_price;

            if long_cspr_value >= short_cspr_value {
                tracing::info!("Selling longs for wCSPR");
                // amount_in_max: how many longs needed to receive UNWRAP_AMOUNT wCSPR,
                // with 5% slippage tolerance
                let amount_in = (UNWRAP_AMOUNT as f64 / price_data.long_price * 1.05) as u64;
                self.token_manager.swap(
                    Path::LongWcspr,
                    U256::from(amount_in),
                    UNWRAP_AMOUNT.into(),
                    recipient,
                )?;
            } else {
                tracing::info!("Selling shorts for wCSPR");
                let amount_in = (UNWRAP_AMOUNT as f64 / price_data.short_price * 1.05) as u64;
                self.token_manager.swap(
                    Path::ShortWcspr,
                    U256::from(amount_in),
                    UNWRAP_AMOUNT.into(),
                    recipient,
                )?;
            }
        }

        Ok(())
    }

    pub fn print_balances(&self) -> Result<(), Error> {
        log_humanized("CSPR balance", self.balances.my_cspr_balance()?);
        log_humanized("WCSPR balance", self.balances.my_wcspr_balance()?);
        log_humanized("Long balance", self.balances.my_long_balance()?);
        log_humanized("Short balance", self.balances.my_short_balance()?);
        Ok(())
    }

    fn ensure_funds(&self, path: Path, amount_in: U256) -> Result<(), Error> {
        match path {
            Path::LongWcsprShort | Path::LongWcspr => self.top_up_longs_if_required(amount_in)?,
            Path::ShortWcsprLong | Path::ShortWcspr => self.top_up_shorts_if_required(amount_in)?,
            Path::WcsprLong | Path::WcsprShort => self.top_up_wcspr_if_required(amount_in)?,
            Path::Empty => panic!("Empty path is not supported"),
        }
        tracing::info!("Funds for swap ready!");
        Ok(())
    }

    fn top_up_longs_if_required(&self, required_balance: U256) -> Result<(), Error> {
        let long_balance = self.balances.my_long_balance()?;
        log_humanized("Required balance", required_balance);
        log_humanized("LONG balance", long_balance);
        if long_balance < required_balance {
            tracing::warn!("Not enough longs, topping up");
            if self.balances.my_wcspr_balance()? < TOP_UP_AMOUNT.into() {
                tracing::warn!("Not enough wcspr to top up longs, wrapping cspr");
                self.wrap_cspr()?;
            }
            self.token_manager.buy_longs()?;
            log_humanized("New LONG balance", self.balances.my_long_balance()?);
        }
        Ok(())
    }

    fn top_up_shorts_if_required(&self, required_balance: U256) -> Result<(), Error> {
        let short_balance = self.balances.my_short_balance()?;
        log_humanized("Required balance", required_balance);
        log_humanized("SHORT balance", short_balance);
        if short_balance < required_balance {
            tracing::warn!("Not enough shorts, topping up");
            if self.balances.my_wcspr_balance()? < TOP_UP_AMOUNT.into() {
                tracing::warn!("Not enough wcspr to top up shorts, wrapping cspr");
                self.wrap_cspr()?;
            }
            self.token_manager.buy_shorts()?;
            log_humanized("New SHORT balance", self.balances.my_short_balance()?);
        }

        Ok(())
    }

    fn top_up_wcspr_if_required(&self, required_balance: U256) -> Result<(), Error> {
        let wcspr_balance = self.balances.my_wcspr_balance()?;
        log_humanized("Required WCSPR balance", required_balance);
        log_humanized("Current WCSPR balance", wcspr_balance);
        if wcspr_balance < required_balance {
            tracing::warn!("Not enough wcspr, topping up");
            self.wrap_cspr()?;
            log_humanized("New WCSPR balance", self.balances.my_wcspr_balance()?);
        }

        Ok(())
    }

    fn wrap_cspr(&self) -> Result<(), Error> {
        let cspr_balance = self.balances.my_cspr_balance()?;
        if cspr_balance.as_u64() < TOP_UP_AMOUNT {
            return Err(Error::OdraError {
                message: "Not enough cspr to wrap".to_string(),
            });
        }
        self.token_manager.wrap_cspr()?;
        Ok(())
    }
}

fn humanize_balance(balance: U256) -> f64 {
    balance.as_u64() as f64 / 1_000_000_000.0f64
}

fn log_humanized(label: &str, balance: U256) {
    tracing::info!("{}: {:.2}", label, humanize_balance(balance));
}

#[cfg(test)]
mod tests {

    use super::*;
    use odra_test::env;

    fn setup_test_env() -> (HostEnv, MockBalances, MockTokenManager) {
        let env = env();
        let refs = MockBalances::new();
        let token_manager = MockTokenManager::new();
        (env, refs, token_manager)
    }

    // ========== Empty Path Tests ==========

    #[test]
    #[should_panic(expected = "Empty path is not supported")]
    fn test_swap_for_empty_path_panics() {
        let env = env();
        let refs = MockBalances::new();
        let token_manager = MockTokenManager::new();
        let asset_manager = AssetManager::new(&refs, &token_manager);
        let _ = asset_manager.swap(Path::Empty, U256::from(100), U256::from(100), env.caller());
    }

    // ========== Multi-Hop Path Tests ==========

    #[test]
    fn test_swap_long_wcspr_short_with_sufficient_balance() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1000)));
        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(100), U256::from(50)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::LongWcsprShort,
            U256::from(100),
            U256::from(50),
            env.caller(),
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), vec![U256::from(100), U256::from(50)]);
    }

    #[test]
    fn test_swap_short_wcspr_long_with_sufficient_balance() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1000)));
        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(100), U256::from(75)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::ShortWcsprLong,
            U256::from(100),
            U256::from(75),
            env.caller(),
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), vec![U256::from(100), U256::from(75)]);
    }

    // ========== Single-Hop Path Tests: Long -> WCSPR ==========

    #[test]
    fn test_swap_long_wcspr_with_sufficient_balance() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(500)));
        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(200)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::LongWcspr,
            U256::from(200),
            U256::from(180),
            env.caller(),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_swap_long_wcspr_with_zero_amount() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(500)));
        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::zero()]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(Path::LongWcspr, U256::zero(), U256::zero(), env.caller());

        assert!(result.is_ok());
    }

    // ========== Single-Hop Path Tests: Short -> WCSPR ==========

    #[test]
    fn test_swap_short_wcspr_with_sufficient_balance() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(800)));
        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(300)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::ShortWcspr,
            U256::from(300),
            U256::from(270),
            env.caller(),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_swap_short_wcspr_with_large_balance() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        // Use a large but not MAX value to avoid overflow in humanize_balance
        let large_balance = U256::from(100_000_000_000_000u64);
        refs.expect_my_short_balance()
            .times(1)
            .return_once(move || Ok(large_balance));
        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(1000)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::ShortWcspr,
            U256::from(1000),
            U256::from(900),
            env.caller(),
        );

        assert!(result.is_ok());
    }

    // ========== Single-Hop Path Tests: WCSPR -> Long ==========

    #[test]
    fn test_swap_wcspr_long_with_sufficient_balance() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1000)));
        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(150)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::WcsprLong,
            U256::from(150),
            U256::from(140),
            env.caller(),
        );

        assert!(result.is_ok());
    }

    // ========== Single-Hop Path Tests: WCSPR -> Short ==========

    #[test]
    fn test_swap_wcspr_short_with_sufficient_balance() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(2000)));
        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(250)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::WcsprShort,
            U256::from(250),
            U256::from(230),
            env.caller(),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_swap_wcspr_short_with_insufficient_cspr_for_wrap() {
        let (env, mut refs, token_manager) = setup_test_env();

        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(100)));

        refs.expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT - 1)));
        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::WcsprShort,
            U256::from(250),
            U256::from(230),
            env.caller(),
        );

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Not enough cspr to wrap"));
    }

    // ========== Edge Cases ==========

    #[test]
    fn test_swap_with_exact_required_balance() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(100)));
        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(100)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::LongWcspr,
            U256::from(100),
            U256::from(90),
            env.caller(),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_swap_with_large_amounts() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        let large_amount = U256::from(1_000_000_000_000_000u64);

        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(move || Ok(large_amount));
        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(1_000_000_000_000u64)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::WcsprLong,
            large_amount,
            U256::from(900_000_000_000_000u64),
            env.caller(),
        );

        assert!(result.is_ok());
    }

    // ========== Balance Top-Up Scenarios ==========

    #[test]
    fn test_top_up_long_balance_when_insufficient() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        // First call: insufficient Long balance
        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(50)));

        // WCSPR balance is sufficient, no need to wrap
        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT * 2)));

        // After buying longs, balance is sufficient
        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT + 50)));

        token_manager
            .expect_buy_longs()
            .times(1)
            .return_once(|| Ok(()));

        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(100)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::LongWcspr,
            U256::from(100),
            U256::from(90),
            env.caller(),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_top_up_short_balance_when_insufficient() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        // First call: insufficient Short balance
        refs.expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(30)));

        // WCSPR balance is sufficient, no need to wrap
        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT * 2)));

        // After buying shorts, balance is sufficient
        refs.expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT + 30)));

        token_manager
            .expect_buy_shorts()
            .times(1)
            .return_once(|| Ok(()));

        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(100)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::ShortWcspr,
            U256::from(100),
            U256::from(90),
            env.caller(),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_top_up_wcspr_balance_when_insufficient() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        // First call: insufficient WCSPR balance
        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(100)));

        // CSPR balance is sufficient for wrapping
        refs.expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT * 2)));

        // After wrapping, WCSPR balance is sufficient
        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT + 100)));

        token_manager
            .expect_wrap_cspr()
            .times(1)
            .return_once(|| Ok(()));

        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(150)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::WcsprLong,
            U256::from(150),
            U256::from(140),
            env.caller(),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn test_cascading_top_up_insufficient_wcspr_triggers_cspr_wrap() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        // First call: insufficient Long balance
        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(50)));

        // WCSPR balance is also insufficient (< TOP_UP_AMOUNT)
        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT / 2)));

        // CSPR balance is sufficient for wrapping
        refs.expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT * 10)));

        // After buying longs, Long balance is sufficient
        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT + 50)));

        // Cascading operations: wrap CSPR first, then buy longs
        token_manager
            .expect_wrap_cspr()
            .times(1)
            .return_once(|| Ok(()));

        token_manager
            .expect_buy_longs()
            .times(1)
            .return_once(|| Ok(()));

        token_manager
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(100)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.swap(
            Path::LongWcspr,
            U256::from(100),
            U256::from(90),
            env.caller(),
        );

        assert!(result.is_ok());
    }

    // ========== Utility Function Tests ==========

    #[test]
    fn test_print_balances_success() {
        let (_, mut refs, token_manager) = setup_test_env();

        refs.expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1_000_000_000_000u64)));
        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(2_000_000_000_000u64)));
        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(3_000_000_000_000u64)));
        refs.expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(4_000_000_000_000u64)));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let result = asset_manager.print_balances();

        assert!(result.is_ok());
    }

    #[test]
    fn test_humanize_balance() {
        assert_eq!(humanize_balance(U256::from(1_000_000_000u64)), 1.0);
        assert_eq!(humanize_balance(U256::from(2_500_000_000u64)), 2.5);
        assert_eq!(humanize_balance(U256::from(1_234_567_890u64)), 1.23456789);
        assert_eq!(humanize_balance(U256::zero()), 0.0);
    }

    // ========== manage_asset_levels Tests ==========

    fn make_price_data(long_price: f64, short_price: f64) -> PriceData {
        // wcspr_price in USD; fair prices equal DEX prices (no arb opportunity needed here)
        PriceData::new(long_price, short_price, 0.04, long_price, short_price)
    }

    #[test]
    fn test_manage_asset_levels_unwraps_when_cspr_low() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_CSPR_BALANCE - 1)));

        token_manager
            .expect_unwrap_wcspr()
            .times(1)
            .withf(|&amount| amount == U256::from(UNWRAP_AMOUNT))
            .return_once(|_| Ok(()));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let price_data = make_price_data(0.75, 0.75);
        assert!(asset_manager
            .manage_asset_levels(&price_data, env.caller())
            .is_ok());
    }

    #[test]
    fn test_manage_asset_levels_sells_longs_when_wcspr_low_and_longs_more_valuable() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_CSPR_BALANCE)));
        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_WCSPR_BALANCE - 1)));
        // 5_000 longs @ 0.75 CSPR = 3_750 CSPR value
        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(5_000_000_000_000u64)));
        // 1_000 shorts @ 0.75 CSPR = 750 CSPR value
        refs.expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1_000_000_000_000u64)));

        token_manager
            .expect_swap()
            .times(1)
            .withf(|path, _, amount_out, _| {
                *path == Path::LongWcspr && *amount_out == U256::from(UNWRAP_AMOUNT)
            })
            .return_once(|_, _, _, _| Ok(vec![U256::from(UNWRAP_AMOUNT)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let price_data = make_price_data(0.75, 0.75);
        assert!(asset_manager
            .manage_asset_levels(&price_data, env.caller())
            .is_ok());
    }

    #[test]
    fn test_manage_asset_levels_sells_shorts_when_wcspr_low_and_shorts_more_valuable() {
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_CSPR_BALANCE)));
        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_WCSPR_BALANCE - 1)));
        // 1_000 longs @ 0.75 = 750 CSPR value
        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1_000_000_000_000u64)));
        // 5_000 shorts @ 0.75 = 3_750 CSPR value
        refs.expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(5_000_000_000_000u64)));

        token_manager
            .expect_swap()
            .times(1)
            .withf(|path, _, amount_out, _| {
                *path == Path::ShortWcspr && *amount_out == U256::from(UNWRAP_AMOUNT)
            })
            .return_once(|_, _, _, _| Ok(vec![U256::from(UNWRAP_AMOUNT)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let price_data = make_price_data(0.75, 0.75);
        assert!(asset_manager
            .manage_asset_levels(&price_data, env.caller())
            .is_ok());
    }

    #[test]
    fn test_manage_asset_levels_uses_price_to_determine_amount_in() {
        // With long_price = 0.5, to get 1500 wCSPR we need 1500/0.5 * 1.05 = 3150 longs
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_CSPR_BALANCE)));
        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_WCSPR_BALANCE - 1)));
        refs.expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(5_000_000_000_000u64)));
        refs.expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1_000_000_000_000u64)));

        let expected_amount_in = (UNWRAP_AMOUNT as f64 / 0.5f64 * 1.05) as u64;
        token_manager
            .expect_swap()
            .times(1)
            .withf(move |path, amount_in, _, _| {
                *path == Path::LongWcspr && *amount_in == U256::from(expected_amount_in)
            })
            .return_once(|_, _, _, _| Ok(vec![U256::from(UNWRAP_AMOUNT)]));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let price_data = make_price_data(0.5, 0.75);
        assert!(asset_manager
            .manage_asset_levels(&price_data, env.caller())
            .is_ok());
    }

    #[test]
    fn test_manage_asset_levels_does_nothing_when_balances_sufficient() {
        let (env, mut refs, token_manager) = setup_test_env();

        refs.expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_CSPR_BALANCE)));
        refs.expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_WCSPR_BALANCE)));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let price_data = make_price_data(0.75, 0.75);
        assert!(asset_manager
            .manage_asset_levels(&price_data, env.caller())
            .is_ok());
    }

    #[test]
    fn test_manage_asset_levels_prefers_unwrap_over_sell_when_cspr_critically_low() {
        // When CSPR is low, should unwrap wCSPR and NOT proceed to check wCSPR level
        let (env, mut refs, mut token_manager) = setup_test_env();

        refs.expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::zero()));

        token_manager
            .expect_unwrap_wcspr()
            .times(1)
            .return_once(|_| Ok(()));

        let asset_manager = AssetManager::new(&refs, &token_manager);
        let price_data = make_price_data(0.75, 0.75);
        assert!(asset_manager
            .manage_asset_levels(&price_data, env.caller())
            .is_ok());
    }
}
