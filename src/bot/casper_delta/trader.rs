use odra::{
    casper_types::U256,
    host::HostEnv,
    prelude::{Address, Addressable},
};
use odra_cli::{cspr, scenario::Error};

#[cfg(test)]
use mockall::automock;

use super::path::Path;
use crate::bot::asset_manager::{log_humanized, AssetManager, Config};
use crate::bot::casper_delta::data::PriceData;
use crate::contracts::ContractRefs;

const MIN_WCSPR_BALANCE: u64 = 1_500_000_000_000; // 1_500 CSPR

/// Balances of Casper Delta position tokens.
#[cfg_attr(test, automock)]
pub trait DeltaBalances {
    fn my_long_balance(&self) -> Result<U256, Error>;
    fn my_short_balance(&self) -> Result<U256, Error>;
}

/// Delta-specific on-chain operations: mint positions, swap. (Approvals are
/// handled by `DeltaAssetManager` via the generic `TokenManager`.)
#[cfg_attr(test, automock)]
pub trait DeltaOps {
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

pub struct RealDeltaBalances<'a> {
    env: &'a HostEnv,
    refs: &'a ContractRefs<'a>,
}

impl<'a> RealDeltaBalances<'a> {
    pub fn new(env: &'a HostEnv, refs: &'a ContractRefs<'a>) -> Self {
        Self { env, refs }
    }
}

impl DeltaBalances for RealDeltaBalances<'_> {
    fn my_long_balance(&self) -> Result<U256, Error> {
        let me = self.env.caller();
        Ok(self.refs.long()?.balance_of(&me))
    }

    fn my_short_balance(&self) -> Result<U256, Error> {
        let me = self.env.caller();
        Ok(self.refs.short()?.balance_of(&me))
    }
}

pub struct RealDeltaOps<'a> {
    cfg: Config,
    env: &'a HostEnv,
    refs: &'a ContractRefs<'a>,
}

impl<'a> RealDeltaOps<'a> {
    pub fn new(cfg: Config, env: &'a HostEnv, refs: &'a ContractRefs<'a>) -> Self {
        Self { cfg, env, refs }
    }
}

impl DeltaOps for RealDeltaOps<'_> {
    fn buy_longs(&self) -> Result<(), Error> {
        self.env.set_gas(cspr!(4));
        self.refs
            .market()?
            .try_deposit_long(self.cfg.top_up_amount)?;
        Ok(())
    }

    fn buy_shorts(&self) -> Result<(), Error> {
        self.env.set_gas(cspr!(4));
        self.refs
            .market()?
            .try_deposit_short(self.cfg.top_up_amount)?;
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

pub struct DryRunDeltaOps;

impl DeltaOps for DryRunDeltaOps {
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

/// Casper Delta asset manager. Composes the generic `AssetManager` (for
/// CSPR/wCSPR) with delta-specific balances and ops (longs, shorts, swap).
pub struct DeltaAssetManager<'a> {
    cfg: Config,
    assets: AssetManager<'a>,
    delta_balances: &'a dyn DeltaBalances,
    delta_ops: &'a dyn DeltaOps,
    refs: Option<&'a ContractRefs<'a>>,
}

impl<'a> DeltaAssetManager<'a> {
    pub fn new(
        cfg: Config,
        assets: AssetManager<'a>,
        delta_balances: &'a dyn DeltaBalances,
        delta_ops: &'a dyn DeltaOps,
    ) -> Self {
        Self {
            cfg,
            assets,
            delta_balances,
            delta_ops,
            refs: None,
        }
    }

    pub fn with_refs(mut self, refs: &'a ContractRefs<'a>) -> Self {
        self.refs = Some(refs);
        self
    }

    /// Approve the Casper Trade router (for wCSPR/long/short) and Casper
    /// Delta market (for wCSPR) using the generic `TokenManager`.
    pub fn approve_delta_markets(&self) -> Result<(), Error> {
        let refs = self.refs.ok_or_else(|| Error::OdraError {
            message: "approve_delta_markets requires ContractRefs (use with_refs)".into(),
        })?;
        let tm = self.assets.token_manager();
        let router = refs.router()?.address();
        let market = refs.market()?.address();
        let wcspr = refs.wcspr()?.address();
        let long = refs.long()?.address();
        let short = refs.short()?.address();

        for (token, spender) in [
            (wcspr, router),
            (long, router),
            (short, router),
            (wcspr, market),
        ] {
            if tm.allowance(token, spender)?.is_zero() {
                tm.approve(token, spender, U256::MAX)?;
            }
        }
        Ok(())
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
            .delta_ops
            .swap(path, amount_in, amount_out, recipient)?;
        Ok(result)
    }

    pub fn manage_asset_levels(
        &self,
        price_data: PriceData,
        recipient: Address,
    ) -> Result<(), Error> {
        if self.assets.manage_cspr_levels()? {
            return Ok(());
        }

        let wcspr_balance = self.assets.balances().my_wcspr_balance()?;
        if wcspr_balance < MIN_WCSPR_BALANCE.into() {
            tracing::warn!("wCSPR balance low, selling positions for wCSPR",);
            let long_balance = self.delta_balances.my_long_balance()?;
            let short_balance = self.delta_balances.my_short_balance()?;
            let long_cspr_value = long_balance.as_u64() as f64 * price_data.long_dex_rate;
            let short_cspr_value = short_balance.as_u64() as f64 * price_data.short_dex_rate;

            if long_cspr_value >= short_cspr_value {
                tracing::info!("Selling longs for wCSPR");
                let amount_in = (self.cfg.unwrap_amount.as_u64() as f64 / price_data.long_dex_rate
                    * 1.05) as u64;
                self.delta_ops.swap(
                    Path::LongWcspr,
                    U256::from(amount_in),
                    self.cfg.unwrap_amount,
                    recipient,
                )?;
            } else {
                tracing::info!("Selling shorts for wCSPR");
                let amount_in = (self.cfg.unwrap_amount.as_u64() as f64 / price_data.short_dex_rate
                    * 1.05) as u64;
                self.delta_ops.swap(
                    Path::ShortWcspr,
                    U256::from(amount_in),
                    self.cfg.unwrap_amount,
                    recipient,
                )?;
            }
        }

        Ok(())
    }

    pub fn print_balances(&self) -> Result<(), Error> {
        self.assets.print_balances()?;
        log_humanized("Long balance", self.delta_balances.my_long_balance()?);
        log_humanized("Short balance", self.delta_balances.my_short_balance()?);
        Ok(())
    }

    fn ensure_funds(&self, path: Path, amount_in: U256) -> Result<(), Error> {
        match path {
            Path::LongWcsprShort | Path::LongWcspr => self.top_up_longs_if_required(amount_in)?,
            Path::ShortWcsprLong | Path::ShortWcspr => self.top_up_shorts_if_required(amount_in)?,
            Path::WcsprLong | Path::WcsprShort => self.assets.ensure_wcspr(amount_in)?,
            Path::Empty => panic!("Empty path is not supported"),
        }
        tracing::info!("Funds for swap ready!");
        Ok(())
    }

    fn top_up_longs_if_required(&self, required_balance: U256) -> Result<(), Error> {
        let long_balance = self.delta_balances.my_long_balance()?;
        log_humanized("Required balance", required_balance);
        log_humanized("LONG balance", long_balance);
        if long_balance < required_balance {
            tracing::warn!("Not enough longs, topping up");
            self.assets.ensure_wcspr(self.cfg.top_up_amount)?;
            self.delta_ops.buy_longs()?;
            log_humanized("New LONG balance", self.delta_balances.my_long_balance()?);
        }
        Ok(())
    }

    fn top_up_shorts_if_required(&self, required_balance: U256) -> Result<(), Error> {
        let short_balance = self.delta_balances.my_short_balance()?;
        log_humanized("Required balance", required_balance);
        log_humanized("SHORT balance", short_balance);
        if short_balance < required_balance {
            tracing::warn!("Not enough shorts, topping up");
            self.assets.ensure_wcspr(self.cfg.top_up_amount)?;
            self.delta_ops.buy_shorts()?;
            log_humanized("New SHORT balance", self.delta_balances.my_short_balance()?);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::asset_manager::{MockBalances, MockTokenManager};
    use odra_test::env;

    const TOP_UP_AMOUNT: u64 = 2_000_000_000_000; // 2000 CSPR
    const MIN_CSPR_BALANCE: u64 = 100_000_000_000; // 100 CSPR
    const UNWRAP_AMOUNT: u64 = 1_000_000_000_000; // 1000 CSPR

    fn setup() -> (
        HostEnv,
        MockBalances,
        MockTokenManager,
        MockDeltaBalances,
        MockDeltaOps,
    ) {
        (
            env(),
            MockBalances::new(),
            MockTokenManager::new(),
            MockDeltaBalances::new(),
            MockDeltaOps::new(),
        )
    }

    fn build<'a>(
        balances: &'a MockBalances,
        token_manager: &'a MockTokenManager,
        delta_balances: &'a MockDeltaBalances,
        delta_ops: &'a MockDeltaOps,
    ) -> DeltaAssetManager<'a> {
        let cfg = Config::from_env();
        let assets = AssetManager::new(cfg.clone(), balances, token_manager);
        DeltaAssetManager::new(cfg, assets, delta_balances, delta_ops)
    }

    // ========== Empty path ==========

    #[test]
    #[should_panic(expected = "Empty path is not supported")]
    fn swap_empty_path_panics() {
        let (env, balances, token_manager, delta_balances, delta_ops) = setup();
        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        let _ = am.swap(Path::Empty, U256::from(100), U256::from(100), env.caller());
    }

    // ========== Multi-hop ==========

    #[test]
    fn swap_long_wcspr_short_sufficient() {
        let (env, balances, token_manager, mut delta_balances, mut delta_ops) = setup();

        delta_balances
            .expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1000)));
        delta_ops
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(100), U256::from(50)]));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        let res = am.swap(
            Path::LongWcsprShort,
            U256::from(100),
            U256::from(50),
            env.caller(),
        );
        assert_eq!(res.unwrap(), vec![U256::from(100), U256::from(50)]);
    }

    #[test]
    fn swap_short_wcspr_long_sufficient() {
        let (env, balances, token_manager, mut delta_balances, mut delta_ops) = setup();

        delta_balances
            .expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1000)));
        delta_ops
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(100), U256::from(75)]));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .swap(
                Path::ShortWcsprLong,
                U256::from(100),
                U256::from(75),
                env.caller(),
            )
            .is_ok());
    }

    // ========== Single-hop: position -> wCSPR ==========

    #[test]
    fn swap_long_wcspr_sufficient() {
        let (env, balances, token_manager, mut delta_balances, mut delta_ops) = setup();

        delta_balances
            .expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(500)));
        delta_ops
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(200)]));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .swap(
                Path::LongWcspr,
                U256::from(200),
                U256::from(180),
                env.caller()
            )
            .is_ok());
    }

    #[test]
    fn swap_short_wcspr_sufficient() {
        let (env, balances, token_manager, mut delta_balances, mut delta_ops) = setup();

        delta_balances
            .expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(800)));
        delta_ops
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(300)]));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .swap(
                Path::ShortWcspr,
                U256::from(300),
                U256::from(270),
                env.caller()
            )
            .is_ok());
    }

    // ========== Single-hop: wCSPR -> position ==========

    #[test]
    fn swap_wcspr_long_sufficient() {
        let (env, mut balances, token_manager, delta_balances, mut delta_ops) = setup();

        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1000)));
        delta_ops
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(150)]));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .swap(
                Path::WcsprLong,
                U256::from(150),
                U256::from(140),
                env.caller()
            )
            .is_ok());
    }

    #[test]
    fn swap_wcspr_short_insufficient_cspr_for_wrap() {
        let (env, mut balances, token_manager, delta_balances, delta_ops) = setup();

        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(100)));
        balances
            .expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT - 1)));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        let err = am
            .swap(
                Path::WcsprShort,
                U256::from(250),
                U256::from(230),
                env.caller(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("Not enough cspr to wrap"));
    }

    // ========== Top-up scenarios ==========

    #[test]
    fn top_up_longs_when_insufficient() {
        let (env, mut balances, token_manager, mut delta_balances, mut delta_ops) = setup();

        delta_balances
            .expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(50)));
        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT * 2)));
        delta_balances
            .expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT + 50)));

        delta_ops.expect_buy_longs().times(1).return_once(|| Ok(()));
        delta_ops
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(100)]));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .swap(
                Path::LongWcspr,
                U256::from(100),
                U256::from(90),
                env.caller()
            )
            .is_ok());
    }

    #[test]
    fn top_up_shorts_when_insufficient() {
        let (env, mut balances, token_manager, mut delta_balances, mut delta_ops) = setup();

        delta_balances
            .expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(30)));
        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT * 2)));
        delta_balances
            .expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT + 30)));

        delta_ops
            .expect_buy_shorts()
            .times(1)
            .return_once(|| Ok(()));
        delta_ops
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(100)]));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .swap(
                Path::ShortWcspr,
                U256::from(100),
                U256::from(90),
                env.caller()
            )
            .is_ok());
    }

    #[test]
    fn top_up_wcspr_when_insufficient() {
        let (env, mut balances, mut token_manager, delta_balances, mut delta_ops) = setup();

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
        delta_ops
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(150)]));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .swap(
                Path::WcsprLong,
                U256::from(150),
                U256::from(140),
                env.caller()
            )
            .is_ok());
    }

    #[test]
    fn cascading_top_up_wraps_cspr_before_buying_longs() {
        let (env, mut balances, mut token_manager, mut delta_balances, mut delta_ops) = setup();

        delta_balances
            .expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(50)));
        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT / 2)));
        balances
            .expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT * 10)));
        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT * 2)));
        delta_balances
            .expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(TOP_UP_AMOUNT + 50)));

        token_manager
            .expect_wrap_cspr()
            .times(1)
            .return_once(|_| Ok(()));
        delta_ops.expect_buy_longs().times(1).return_once(|| Ok(()));
        delta_ops
            .expect_swap()
            .times(1)
            .return_once(|_, _, _, _| Ok(vec![U256::from(100)]));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .swap(
                Path::LongWcspr,
                U256::from(100),
                U256::from(90),
                env.caller()
            )
            .is_ok());
    }

    // ========== manage_asset_levels ==========

    fn price_data(long_price: f64, short_price: f64) -> PriceData {
        PriceData::new(long_price, short_price, 0.04, long_price, short_price)
    }

    #[test]
    fn manage_asset_levels_unwraps_when_cspr_low() {
        let (env, mut balances, mut token_manager, delta_balances, delta_ops) = setup();

        balances
            .expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_CSPR_BALANCE - 1)));
        token_manager
            .expect_unwrap_wcspr()
            .times(1)
            .withf(|&amount| amount == U256::from(UNWRAP_AMOUNT))
            .return_once(|_| Ok(()));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .manage_asset_levels(price_data(0.75, 0.75), env.caller())
            .is_ok());
    }

    #[test]
    fn manage_asset_levels_sells_longs_when_longs_more_valuable() {
        let (env, mut balances, token_manager, mut delta_balances, mut delta_ops) = setup();

        balances
            .expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_CSPR_BALANCE)));
        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_WCSPR_BALANCE - 1)));
        delta_balances
            .expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(5_000_000_000_000u64)));
        delta_balances
            .expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1_000_000_000_000u64)));

        delta_ops
            .expect_swap()
            .times(1)
            .withf(|path, _, amount_out, _| {
                *path == Path::LongWcspr && *amount_out == U256::from(UNWRAP_AMOUNT)
            })
            .return_once(|_, _, _, _| Ok(vec![U256::from(UNWRAP_AMOUNT)]));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .manage_asset_levels(price_data(0.75, 0.75), env.caller())
            .is_ok());
    }

    #[test]
    fn manage_asset_levels_sells_shorts_when_shorts_more_valuable() {
        let (env, mut balances, token_manager, mut delta_balances, mut delta_ops) = setup();

        balances
            .expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_CSPR_BALANCE)));
        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_WCSPR_BALANCE - 1)));
        delta_balances
            .expect_my_long_balance()
            .times(1)
            .return_once(|| Ok(U256::from(1_000_000_000_000u64)));
        delta_balances
            .expect_my_short_balance()
            .times(1)
            .return_once(|| Ok(U256::from(5_000_000_000_000u64)));

        delta_ops
            .expect_swap()
            .times(1)
            .withf(|path, _, amount_out, _| {
                *path == Path::ShortWcspr && *amount_out == U256::from(UNWRAP_AMOUNT)
            })
            .return_once(|_, _, _, _| Ok(vec![U256::from(UNWRAP_AMOUNT)]));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .manage_asset_levels(price_data(0.75, 0.75), env.caller())
            .is_ok());
    }

    #[test]
    fn manage_asset_levels_noop_when_healthy() {
        let (env, mut balances, token_manager, delta_balances, delta_ops) = setup();

        balances
            .expect_my_cspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_CSPR_BALANCE)));
        balances
            .expect_my_wcspr_balance()
            .times(1)
            .return_once(|| Ok(U256::from(MIN_WCSPR_BALANCE)));

        let am = build(&balances, &token_manager, &delta_balances, &delta_ops);
        assert!(am
            .manage_asset_levels(price_data(0.75, 0.75), env.caller())
            .is_ok());
    }
}
