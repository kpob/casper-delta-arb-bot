//! Rebalancer — keeps trading-account asset levels within per-asset bands.
//!
//! Policy (see `Rebalancer.md`):
//! * Target when an asset is **below min** or **above max**: the band midpoint
//!   `(min+max)/2`.
//! * A source asset used to refill another must not drop below its own
//!   `min * 1.25` floor.
//! * When refilling **CSPR**, drain sources in order of **highest balance
//!   first** (WCSPR → unwrap, stCSPR → unstake).
//! * When draining excess **CSPR**, prefer **wrap** (reversible, cheaper gas);
//!   fall back to **stake** only when WCSPR is at its max. The amount is
//!   clamped to the sink's own headroom (`sink_max - sink_balance`) so the
//!   drain never pushes the sink past its max.
//! * At most one action per rebalance tick per asset is expected in practice;
//!   assets are processed in priority order CSPR → WCSPR → stCSPR → long →
//!   short and the first imbalance short-circuits the tick.
//! * Unstake is asynchronous (~16h). Only one unstake may be in-flight at a
//!   time; its start timestamp is persisted to a JSON file, and the matured
//!   claim is auto-issued on the next tick.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use odra::casper_types::{U256, U512};
use odra::host::{HostEnv, HostRef};
use odra::prelude::{Address, Addressable};
use odra::uints::ToU256;
use odra_cli::{cspr, scenario::Error};
use serde::{Deserialize, Serialize};

use crate::contracts::ContractRefs;

#[cfg(test)]
use mockall::automock;

pub const DEFAULT_STATE_PATH: &str = "./rebalancer-state.json";
pub const UNSTAKE_MATURITY_SECS: u64 = 16 * 60 * 60;

// ---------------------------------------------------------------------------
// Assets & levels
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asset {
    Cspr,
    Wcspr,
    StCspr,
    Long,
    Short,
}

impl Asset {
    pub const PRIORITY: [Asset; 5] = [
        Asset::Cspr,
        Asset::Wcspr,
        Asset::StCspr,
        Asset::Long,
        Asset::Short,
    ];
}

#[derive(Debug, Clone, Copy)]
pub struct Levels {
    pub min: U256,
    pub max: U256,
}

impl Levels {
    /// Target when balance is below `min`.
    pub fn midpoint(&self) -> U256 {
        (self.min + self.max) / U256::from(2u64)
    }

    /// Floor below which a source asset must not be drained when used to
    /// refill another asset (`min * 1.25`).
    pub fn spend_floor(&self) -> U256 {
        self.min + self.min / U256::from(4u64)
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub cspr: Levels,
    pub wcspr: Levels,
    pub stcspr: Levels,
    pub long: Levels,
    pub short: Levels,
    pub state_path: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            cspr: Levels {
                min: env_u256("BOT_MIN_BALANCE_CSPR", 10_000_000_000_000),
                max: env_u256("BOT_MAX_BALANCE_CSPR", 30_000_000_000_000),
            },
            wcspr: Levels {
                min: env_u256("BOT_MIN_BALANCE_WCSPR", 10_000_000_000_000),
                max: env_u256("BOT_MAX_BALANCE_WCSPR", 30_000_000_000_000),
            },
            stcspr: Levels {
                min: env_u256("BOT_MIN_BALANCE_STCSPR", 10_000_000_000_000),
                max: env_u256("BOT_MAX_BALANCE_STCSPR", 30_000_000_000_000),
            },
            long: Levels {
                min: env_u256("BOT_MIN_BALANCE_LONG", 1_000_000_000_000),
                max: env_u256("BOT_MAX_BALANCE_LONG", 4_000_000_000_000),
            },
            short: Levels {
                min: env_u256("BOT_MIN_BALANCE_SHORT", 80_000_000_000),
                max: env_u256("BOT_MAX_BALANCE_SHORT", 300_000_000_000),
            },
            state_path: std::env::var("BOT_REBALANCER_STATE_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_STATE_PATH)),
        }
    }

    pub fn levels(&self, asset: Asset) -> Levels {
        match asset {
            Asset::Cspr => self.cspr,
            Asset::Wcspr => self.wcspr,
            Asset::StCspr => self.stcspr,
            Asset::Long => self.long,
            Asset::Short => self.short,
        }
    }
}

fn env_u256(key: &str, default: u128) -> U256 {
    let raw = std::env::var(key).ok();
    let v = raw
        .as_deref()
        .map(|s| {
            s.parse::<u128>()
                .unwrap_or_else(|e| panic!("Invalid {key} {e} {s}."))
        })
        .unwrap_or(default);
    U256::from(v)
}

// ---------------------------------------------------------------------------
// Traits (read-side, write-side, persistence, clock)
// ---------------------------------------------------------------------------

#[cfg_attr(test, automock)]
pub trait Balances {
    fn cspr(&self) -> Result<U256, Error>;
    fn wcspr(&self) -> Result<U256, Error>;
    fn stcspr(&self) -> Result<U256, Error>;
    fn long(&self) -> Result<U256, Error>;
    fn short(&self) -> Result<U256, Error>;
}

/// Conversion rates used when deficit/excess of one asset must be expressed
/// in units of another asset.
#[cfg_attr(test, automock)]
pub trait Rates {
    /// CSPR motes for the given stCSPR motes, via the staking ratio.
    fn stcspr_to_cspr(&self, stcspr: U256) -> Result<U256, Error>;
    /// stCSPR motes that would be received by staking the given CSPR motes.
    fn cspr_to_stcspr(&self, cspr: U256) -> Result<U256, Error>;
    /// WCSPR motes required to buy exactly `long_out` long tokens on the DEX.
    fn wcspr_in_for_long_out(&self, long_out: U256) -> Result<U256, Error>;
    /// WCSPR motes required to buy exactly `short_out` short tokens.
    fn wcspr_in_for_short_out(&self, short_out: U256) -> Result<U256, Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    WcsprMarket,
    WcsprRouter,
    StCspr,
    Short,
    Long,
}

impl Approval {
    pub const ALL: [Approval; 5] = [
        Approval::WcsprMarket,
        Approval::WcsprRouter,
        Approval::StCspr,
        Approval::Short,
        Approval::Long,
    ];
}

#[cfg_attr(test, automock)]
pub trait Ops {
    fn wrap(&self, cspr_amount: U256) -> Result<(), Error>;
    fn unwrap(&self, wcspr_amount: U256) -> Result<(), Error>;
    fn stake(&self, cspr_amount: U256) -> Result<(), Error>;
    fn unstake(&self, stcspr_amount: U256) -> Result<(), Error>;
    fn claim(&self) -> Result<(), Error>;
    fn buy_long(&self, wcspr_amount: U256) -> Result<(), Error>;
    fn sell_long(&self, long_amount: U256) -> Result<(), Error>;
    fn buy_short(&self, wcspr_amount: U256) -> Result<(), Error>;
    fn sell_short(&self, short_amount: U256) -> Result<(), Error>;
    fn has_allowance(&self, approval: Approval) -> Result<bool, Error>;
    fn approve_max(&self, approval: Approval) -> Result<(), Error>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingUnstake {
    pub created_at_secs: u64,
}

#[cfg_attr(test, automock)]
pub trait StateStore {
    fn load(&self) -> Result<Option<PendingUnstake>, Error>;
    fn save(&self, pending: &PendingUnstake) -> Result<(), Error>;
    fn clear(&self) -> Result<(), Error>;
}

#[cfg_attr(test, automock)]
pub trait Clock {
    fn now_secs(&self) -> u64;
}

// ---------------------------------------------------------------------------
// Rebalancer
// ---------------------------------------------------------------------------

pub struct Rebalancer<'a> {
    cfg: &'a Config,
    dry_run: bool,
    balances: &'a dyn Balances,
    rates: &'a dyn Rates,
    ops: &'a dyn Ops,
    state: &'a dyn StateStore,
    clock: &'a dyn Clock,
}

impl<'a> Rebalancer<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cfg: &'a Config,
        dry_run: bool,
        balances: &'a dyn Balances,
        rates: &'a dyn Rates,
        ops: &'a dyn Ops,
        state: &'a dyn StateStore,
        clock: &'a dyn Clock,
    ) -> Self {
        Self {
            cfg,
            dry_run,
            balances,
            rates,
            ops,
            state,
            clock,
        }
    }

    /// Inspect balances and perform at most one rebalancing step for the
    /// first imbalanced asset, in priority order. Should be called before any
    /// engine action on each event-loop tick.
    pub fn rebalance(&self) -> Result<(), Error> {
        self.log_state()?;
        if self.try_claim_matured()? {
            return Ok(());
        }
        for asset in Asset::PRIORITY {
            if self.try_rebalance_asset(asset)? {
                return Ok(());
            }
        }
        Ok(())
    }

    fn log_state(&self) -> Result<(), Error> {
        let cspr = self.balances.cspr()?;
        let wcspr = self.balances.wcspr()?;
        let stcspr = self.balances.stcspr()?;
        let long = self.balances.long()?;
        let short = self.balances.short()?;
        let row = |name: &str, bal: U256, lvls: Levels| {
            format!(
                "  {name:<7} {:>14}  (min {}, max {})",
                format_amount(bal),
                format_amount(lvls.min),
                format_amount(lvls.max),
            )
        };
        tracing::info!(
            "Rebalancer state:\n{}\n{}\n{}\n{}\n{}",
            row("CSPR", cspr, self.cfg.cspr),
            row("WCSPR", wcspr, self.cfg.wcspr),
            row("stCSPR", stcspr, self.cfg.stcspr),
            row("Long", long, self.cfg.long),
            row("Short", short, self.cfg.short),
        );
        Ok(())
    }

    fn try_claim_matured(&self) -> Result<bool, Error> {
        let Some(pending) = self.state.load()? else {
            return Ok(false);
        };
        let now = self.clock.now_secs();
        let elapsed = now.saturating_sub(pending.created_at_secs);
        if elapsed < UNSTAKE_MATURITY_SECS {
            tracing::info!(
                "Pending unstake still maturing ({elapsed}s of {UNSTAKE_MATURITY_SECS}s)"
            );
            return Ok(false);
        }
        tracing::info!("Claiming matured unstake");
        if !self.dry_run {
            self.ops.claim()?;
        }
        self.state.clear()?;
        Ok(true)
    }

    fn balance(&self, asset: Asset) -> Result<U256, Error> {
        match asset {
            Asset::Cspr => self.balances.cspr(),
            Asset::Wcspr => self.balances.wcspr(),
            Asset::StCspr => self.balances.stcspr(),
            Asset::Long => self.balances.long(),
            Asset::Short => self.balances.short(),
        }
    }

    fn try_rebalance_asset(&self, asset: Asset) -> Result<bool, Error> {
        let bal = self.balance(asset)?;
        let lvls = self.cfg.levels(asset);
        if bal < lvls.min {
            let deficit = lvls.midpoint() - bal;
            let formatted_bal = format_amount(bal);
            let formatted_deficit = format_amount(deficit);
            tracing::info!(
                "Asset {asset:?} below min ({formatted_bal}); filling deficit {formatted_deficit}"
            );
            self.fill(asset, deficit)?;
            return Ok(true);
        }
        if bal > lvls.max {
            let excess = bal - lvls.midpoint();
            let formatted_bal = format_amount(bal);
            let formatted_excess = format_amount(excess);
            tracing::info!(
                "Asset {asset:?} above max ({formatted_bal}); draining excess {formatted_excess}"
            );
            self.drain(asset, excess)?;
            return Ok(true);
        }
        Ok(false)
    }

    // ---- Fill -------------------------------------------------------------

    fn fill(&self, asset: Asset, deficit: U256) -> Result<(), Error> {
        match asset {
            Asset::Cspr => self.fill_cspr(deficit),
            Asset::Wcspr => self.single_source_fill_cspr_to(Asset::Wcspr, deficit),
            Asset::StCspr => self.fill_stcspr(deficit),
            Asset::Long => self.fill_position(Asset::Long, deficit),
            Asset::Short => self.fill_position(Asset::Short, deficit),
        }
    }

    /// Fill CSPR from WCSPR (unwrap) then stCSPR (unstake). Sources are tried
    /// in descending balance order, each bounded by its own `min*1.25` floor.
    fn fill_cspr(&self, deficit: U256) -> Result<(), Error> {
        let wcspr_avail = self.spendable(Asset::Wcspr)?;
        let stcspr_avail_cspr = {
            let spendable = self.spendable(Asset::StCspr)?;
            self.rates.stcspr_to_cspr(spendable)?
        };

        let mut sources: Vec<(Asset, U256)> = vec![
            (Asset::Wcspr, wcspr_avail),
            (Asset::StCspr, stcspr_avail_cspr),
        ];
        // highest-balance source first
        sources.sort_by(|a, b| b.1.cmp(&a.1));

        let mut remaining = deficit;
        for (src, avail_in_cspr) in sources {
            if remaining.is_zero() {
                break;
            }
            let take_cspr = min_u256(avail_in_cspr, remaining);
            if take_cspr.is_zero() {
                continue;
            }
            match src {
                Asset::Wcspr => {
                    tracing::info!("Unwrapping {take_cspr} WCSPR for CSPR");
                    if !self.dry_run {
                        self.ops.unwrap(take_cspr)?;
                    }
                    remaining -= take_cspr;
                }
                Asset::StCspr => {
                    if self.state.load()?.is_some() {
                        tracing::warn!(
                            "Cannot unstake while a prior unstake is pending; deferring"
                        );
                        break;
                    }
                    let stcspr_amount = self.rates.cspr_to_stcspr(take_cspr)?;
                    if stcspr_amount.is_zero() {
                        continue;
                    }
                    tracing::info!("Unstaking {stcspr_amount} stCSPR (~{take_cspr} CSPR)");
                    if !self.dry_run {
                        self.ops.unstake(stcspr_amount)?;
                    }
                    self.state.save(&PendingUnstake {
                        created_at_secs: self.clock.now_secs(),
                    })?;
                    remaining = remaining.saturating_sub(take_cspr);
                }
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    /// Fill WCSPR (only source is CSPR). Argument is WCSPR deficit; WCSPR is
    /// 1:1 with CSPR on wrap.
    fn single_source_fill_cspr_to(&self, target: Asset, deficit: U256) -> Result<(), Error> {
        debug_assert_eq!(target, Asset::Wcspr);
        let avail = self.spendable(Asset::Cspr)?;
        let amount = min_u256(avail, deficit);
        if amount.is_zero() {
            tracing::warn!("Cannot fill WCSPR: CSPR source below floor");
            return Ok(());
        }
        tracing::info!("Wrapping {amount} CSPR → WCSPR");
        if !self.dry_run {
            self.ops.wrap(amount)?;
        }
        Ok(())
    }

    /// Fill stCSPR (only source is CSPR via staking).
    fn fill_stcspr(&self, stcspr_deficit: U256) -> Result<(), Error> {
        let cspr_avail = self.spendable(Asset::Cspr)?;
        let cspr_needed = self.rates.stcspr_to_cspr(stcspr_deficit)?;
        let cspr_amount = min_u256(cspr_avail, cspr_needed);
        if cspr_amount.is_zero() {
            tracing::warn!("Cannot fill stCSPR: CSPR source below floor");
            return Ok(());
        }
        tracing::info!("Staking {cspr_amount} CSPR → stCSPR");
        if !self.dry_run {
            self.ops.stake(cspr_amount)?;
        }
        Ok(())
    }

    /// Fill a position token (long or short) by buying with WCSPR.
    fn fill_position(&self, asset: Asset, token_deficit: U256) -> Result<(), Error> {
        let wcspr_avail = self.spendable(Asset::Wcspr)?;
        let wcspr_needed = match asset {
            Asset::Long => self.rates.wcspr_in_for_long_out(token_deficit)?,
            Asset::Short => self.rates.wcspr_in_for_short_out(token_deficit)?,
            _ => unreachable!(),
        };
        let wcspr_spend = min_u256(wcspr_avail, wcspr_needed);
        if wcspr_spend.is_zero() {
            tracing::warn!("Cannot fill {asset:?}: WCSPR source below floor");
            return Ok(());
        }
        tracing::info!("Buying {asset:?} with {wcspr_spend} WCSPR");
        if !self.dry_run {
            match asset {
                Asset::Long => self.ops.buy_long(wcspr_spend)?,
                Asset::Short => self.ops.buy_short(wcspr_spend)?,
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    // ---- Drain ------------------------------------------------------------

    fn drain(&self, asset: Asset, excess: U256) -> Result<(), Error> {
        match asset {
            Asset::Cspr => self.drain_cspr(excess),
            Asset::Wcspr => {
                tracing::info!("Unwrapping {excess} WCSPR → CSPR");
                if !self.dry_run {
                    self.ops.unwrap(excess)?;
                }
                Ok(())
            }
            Asset::StCspr => self.drain_stcspr(excess),
            Asset::Long => {
                tracing::info!("Selling {excess} long");
                if !self.dry_run {
                    self.ops.sell_long(excess)?;
                }
                Ok(())
            }
            Asset::Short => {
                tracing::info!("Selling {excess} short");
                if !self.dry_run {
                    self.ops.sell_short(excess)?;
                }
                Ok(())
            }
        }
    }

    /// Drain excess CSPR. Prefer wrap (reversible, cheapest gas); fall back
    /// to stake only when WCSPR has no headroom. Amount is clamped to the
    /// chosen sink's headroom so we never push it past its own max.
    fn drain_cspr(&self, excess: U256) -> Result<(), Error> {
        let wcspr_bal = self.balances.wcspr()?;
        let wcspr_headroom = self.cfg.wcspr.max.saturating_sub(wcspr_bal);
        if !wcspr_headroom.is_zero() {
            let amount = min_u256(excess, wcspr_headroom);
            tracing::info!("Wrapping {amount} CSPR → WCSPR");
            if !self.dry_run {
                self.ops.wrap(amount)?;
            }
            return Ok(());
        }
        let stcspr_bal = self.balances.stcspr()?;
        let stcspr_headroom = self.cfg.stcspr.max.saturating_sub(stcspr_bal);
        let stcspr_headroom_cspr = self.rates.stcspr_to_cspr(stcspr_headroom)?;
        if !stcspr_headroom_cspr.is_zero() {
            let amount = min_u256(excess, stcspr_headroom_cspr);
            tracing::info!("Staking {amount} CSPR → stCSPR");
            if !self.dry_run {
                self.ops.stake(amount)?;
            }
            return Ok(());
        }
        tracing::warn!("Cannot drain CSPR: both WCSPR and stCSPR at/above max");
        Ok(())
    }

    /// Drain excess stCSPR by unstaking (async, single-slot).
    fn drain_stcspr(&self, excess_stcspr: U256) -> Result<(), Error> {
        if self.state.load()?.is_some() {
            tracing::warn!("Cannot unstake excess stCSPR: prior unstake pending");
            return Ok(());
        }
        tracing::info!("Unstaking {excess_stcspr} stCSPR");
        if !self.dry_run {
            self.ops.unstake(excess_stcspr)?;
        }
        self.state.save(&PendingUnstake {
            created_at_secs: self.clock.now_secs(),
        })?;
        Ok(())
    }

    // ---- Helpers ----------------------------------------------------------

    /// How much of `asset` can be drawn without crossing its own `min*1.25`
    /// floor. Returns zero if already at or below the floor.
    fn spendable(&self, asset: Asset) -> Result<U256, Error> {
        let bal = self.balance(asset)?;
        let floor = self.cfg.levels(asset).spend_floor();
        Ok(bal.saturating_sub(floor))
    }
}

/// Render a 9-decimal motes value as `whole.ddd` (three decimal places).
fn format_amount(motes: U256) -> String {
    let one_token = U256::from(1_000_000_000u64);
    let whole = motes / one_token;
    let frac = (motes % one_token) / U256::from(1_000_000u64);
    format!("{whole}.{:03}", frac.as_u64())
}

fn min_u256(a: U256, b: U256) -> U256 {
    if a < b {
        a
    } else {
        b
    }
}

// ---------------------------------------------------------------------------
// Real implementations
// ---------------------------------------------------------------------------

pub struct RealBalances<'a> {
    env: &'a HostEnv,
    refs: &'a ContractRefs<'a>,
}

impl<'a> RealBalances<'a> {
    pub fn new(env: &'a HostEnv, refs: &'a ContractRefs<'a>) -> Self {
        Self { env, refs }
    }

    fn me(&self) -> Address {
        self.env.caller()
    }
}

impl Balances for RealBalances<'_> {
    fn cspr(&self) -> Result<U256, Error> {
        self.env
            .balance_of(&self.me())
            .to_u256()
            .map_err(|_| Error::OdraError {
                message: "Failed to convert CSPR balance to U256".to_string(),
            })
    }
    fn wcspr(&self) -> Result<U256, Error> {
        Ok(self.refs.wcspr()?.balance_of(&self.me()))
    }
    fn stcspr(&self) -> Result<U256, Error> {
        Ok(self.refs.staked_cspr()?.balance_of(&self.me()))
    }
    fn long(&self) -> Result<U256, Error> {
        Ok(self.refs.long()?.balance_of(&self.me()))
    }
    fn short(&self) -> Result<U256, Error> {
        Ok(self.refs.short()?.balance_of(&self.me()))
    }
}

pub struct RealRates<'a> {
    refs: &'a ContractRefs<'a>,
}

impl<'a> RealRates<'a> {
    pub fn new(refs: &'a ContractRefs<'a>) -> Self {
        Self { refs }
    }
}

impl Rates for RealRates<'_> {
    fn stcspr_to_cspr(&self, stcspr: U256) -> Result<U256, Error> {
        let sc = self.refs.staked_cspr()?;
        let staked: U512 = sc.staked_cspr();
        let supply: U256 = sc.total_supply();
        if supply.is_zero() {
            return Ok(U256::zero());
        }
        // cspr = stcspr * staked / supply, done in U512 to avoid overflow.
        let stcspr_u512 = U512::from(stcspr.as_u128());
        let supply_u512 = U512::from(supply.as_u128());
        let product = stcspr_u512 * staked / supply_u512;
        product.to_u256().map_err(|_| Error::OdraError {
            message: "stcspr_to_cspr overflow".to_string(),
        })
    }

    fn cspr_to_stcspr(&self, cspr: U256) -> Result<U256, Error> {
        let sc = self.refs.staked_cspr()?;
        let staked: U512 = sc.staked_cspr();
        let supply: U256 = sc.total_supply();
        if staked.is_zero() {
            return Ok(cspr);
        }
        // stcspr = cspr * supply / staked (U512 math).
        let cspr_u512 = U512::from(cspr.as_u128());
        let supply_u512 = U512::from(supply.as_u128());
        let product = cspr_u512 * supply_u512 / staked;
        product.to_u256().map_err(|_| Error::OdraError {
            message: "cspr_to_stcspr overflow".to_string(),
        })
    }

    fn wcspr_in_for_long_out(&self, long_out: U256) -> Result<U256, Error> {
        // long-wcspr pair: token0 = long, token1 = wcspr.
        let (reserves_long, reserves_wcspr, _) = self.refs.long_wcspr_pair()?.get_reserves();
        Ok(self
            .refs
            .router()?
            .get_amount_in(long_out, reserves_wcspr, reserves_long))
    }

    fn wcspr_in_for_short_out(&self, short_out: U256) -> Result<U256, Error> {
        // wcspr-short pair: token0 = wcspr, token1 = short.
        let (reserves_wcspr, reserves_short, _) = self.refs.wcspr_short_pair()?.get_reserves();
        Ok(self
            .refs
            .router()?
            .get_amount_in(short_out, reserves_wcspr, reserves_short))
    }
}

pub struct RealOps<'a> {
    env: &'a HostEnv,
    refs: &'a ContractRefs<'a>,
}

impl<'a> RealOps<'a> {
    pub fn new(env: &'a HostEnv, refs: &'a ContractRefs<'a>) -> Self {
        Self { env, refs }
    }

    fn set_gas(&self) {
        self.env.set_gas(cspr!(4));
    }
}

impl Ops for RealOps<'_> {
    fn wrap(&self, cspr_amount: U256) -> Result<(), Error> {
        self.set_gas();
        let amount = U512::from(cspr_amount.as_u128());
        self.env.set_gas(cspr!(3));
        self.refs.wcspr()?.with_tokens(amount).try_deposit()?;
        Ok(())
    }

    fn unwrap(&self, wcspr_amount: U256) -> Result<(), Error> {
        self.set_gas();
        self.refs.wcspr()?.try_withdraw(&wcspr_amount)?;
        Ok(())
    }

    fn stake(&self, cspr_amount: U256) -> Result<(), Error> {
        self.env.set_gas(cspr!(9));
        let amount = U512::from(cspr_amount.as_u128());
        self.refs.staked_cspr()?.with_tokens(amount).try_stake()?;
        Ok(())
    }

    fn unstake(&self, stcspr_amount: U256) -> Result<(), Error> {
        self.env.set_gas(cspr!(8));
        self.refs.staked_cspr()?.try_unstake(stcspr_amount)?;
        Ok(())
    }

    fn claim(&self) -> Result<(), Error> {
        self.env.set_gas(cspr!(8));
        self.refs.staked_cspr()?.try_claim()?;
        Ok(())
    }

    fn buy_long(&self, wcspr_amount: U256) -> Result<(), Error> {
        self.set_gas();
        self.refs.market()?.try_deposit_long(wcspr_amount)?;
        Ok(())
    }

    fn sell_long(&self, long_amount: U256) -> Result<(), Error> {
        self.set_gas();
        self.refs.market()?.try_withdraw_long(long_amount)?;
        Ok(())
    }

    fn buy_short(&self, wcspr_amount: U256) -> Result<(), Error> {
        self.set_gas();
        self.refs.market()?.try_deposit_short(wcspr_amount)?;
        Ok(())
    }

    fn sell_short(&self, short_amount: U256) -> Result<(), Error> {
        self.set_gas();
        self.refs.market()?.try_withdraw_short(short_amount)?;
        Ok(())
    }

    fn has_allowance(&self, approval: Approval) -> Result<bool, Error> {
        let me = self.env.caller();
        let market_addr = self.refs.market()?.address();
        let cspr_trade = self.refs.router()?.address();
        Ok(match approval {
            Approval::WcsprMarket => !self.refs.wcspr()?.allowance(&me, &market_addr).is_zero(),
            Approval::WcsprRouter => !self.refs.wcspr()?.allowance(&me, &cspr_trade).is_zero(),
            Approval::StCspr => !self
                .refs
                .staked_cspr()?
                .allowance(&me, &cspr_trade)
                .is_zero(),
            Approval::Short => !self.refs.short()?.allowance(&me, &cspr_trade).is_zero(),
            Approval::Long => !self.refs.long()?.allowance(&me, &cspr_trade).is_zero(),
        })
    }

    fn approve_max(&self, approval: Approval) -> Result<(), Error> {
        self.env.set_gas(cspr!(4));
        let market_addr = self.refs.market()?.address();
        let cspr_trade = self.refs.router()?.address();
        match approval {
            Approval::WcsprMarket => {
                self.refs.wcspr()?.approve(&market_addr, &U256::MAX);
            }
            Approval::WcsprRouter => {
                self.refs.wcspr()?.approve(&cspr_trade, &U256::MAX);
            }
            Approval::StCspr => {
                self.refs.staked_cspr()?.approve(&cspr_trade, &U256::MAX);
            }
            Approval::Short => {
                self.refs.short()?.approve(&cspr_trade, &U256::MAX);
            }
            Approval::Long => {
                self.refs.long()?.approve(&cspr_trade, &U256::MAX);
            }
        }
        Ok(())
    }
}

pub struct FileStateStore {
    path: PathBuf,
}

impl FileStateStore {
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        Self { path: path.into() }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl StateStore for FileStateStore {
    fn load(&self) -> Result<Option<PendingUnstake>, Error> {
        if !self.path().exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(self.path()).map_err(|e| Error::OdraError {
            message: format!("read {:?}: {e}", self.path()),
        })?;
        let p: PendingUnstake = serde_json::from_str(&raw).map_err(|e| Error::OdraError {
            message: format!("parse {:?}: {e}", self.path()),
        })?;
        Ok(Some(p))
    }

    fn save(&self, pending: &PendingUnstake) -> Result<(), Error> {
        let raw = serde_json::to_string(pending).map_err(|e| Error::OdraError {
            message: format!("serialize pending: {e}"),
        })?;
        fs::write(self.path(), raw).map_err(|e| Error::OdraError {
            message: format!("write {:?}: {e}", self.path()),
        })
    }

    fn clear(&self) -> Result<(), Error> {
        if self.path().exists() {
            fs::remove_file(self.path()).map_err(|e| Error::OdraError {
                message: format!("remove {:?}: {e}", self.path()),
            })?;
        }
        Ok(())
    }
}

pub struct SystemClock;

/// Bundles the real implementations and owns their storage so the caller only
/// needs to hold a single value and call [`RealRebalancer::rebalance`].
pub struct RealRebalancer<'a> {
    cfg: Config,
    dry_run: bool,
    balances: RealBalances<'a>,
    rates: RealRates<'a>,
    ops: RealOps<'a>,
    state: FileStateStore,
    clock: SystemClock,
}

impl<'a> RealRebalancer<'a> {
    pub fn from_env(env: &'a HostEnv, refs: &'a ContractRefs<'a>, dry_run: bool) -> Self {
        let cfg = Config::from_env();
        let state = FileStateStore::new(cfg.state_path.clone());
        Self {
            balances: RealBalances::new(env, refs),
            rates: RealRates::new(refs),
            ops: RealOps::new(env, refs),
            state,
            clock: SystemClock,
            dry_run,
            cfg,
        }
    }

    pub fn rebalance(&self) -> Result<(), Error> {
        Rebalancer::new(
            &self.cfg,
            self.dry_run,
            &self.balances,
            &self.rates,
            &self.ops,
            &self.state,
            &self.clock,
        )
        .rebalance()
    }

    pub fn approve_all(&self) -> Result<(), Error> {
        for approval in Approval::ALL {
            if !self.ops.has_allowance(approval)? {
                tracing::info!("Approving {approval:?}");
                if !self.dry_run {
                    self.ops.approve_max(approval)?;
                }
            }
        }
        Ok(())
    }
}

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn u(x: u128) -> U256 {
        U256::from(x)
    }

    fn base_cfg() -> Config {
        Config {
            cspr: Levels {
                min: u(10_000),
                max: u(30_000),
            },
            wcspr: Levels {
                min: u(10_000),
                max: u(30_000),
            },
            stcspr: Levels {
                min: u(10_000),
                max: u(30_000),
            },
            long: Levels {
                min: u(1_000),
                max: u(4_000),
            },
            short: Levels {
                min: u(80),
                max: u(300),
            },
            state_path: PathBuf::from("/tmp/unused"),
        }
    }

    fn no_pending_store() -> MockStateStore {
        let mut s = MockStateStore::new();
        s.expect_load().returning(|| Ok(None));
        s
    }

    fn fixed_clock(t: u64) -> MockClock {
        let mut c = MockClock::new();
        c.expect_now_secs().returning(move || t);
        c
    }

    fn approvals_ok_ops() -> MockOps {
        let mut ops = MockOps::new();
        ops.expect_has_allowance().returning(|_| Ok(true));
        ops
    }

    #[test]
    fn levels_midpoint_and_spend_floor() {
        let l = Levels {
            min: u(10_000),
            max: u(30_000),
        };
        assert_eq!(l.midpoint(), u(20_000));
        assert_eq!(l.spend_floor(), u(12_500));
    }

    #[test]
    fn noop_when_all_within_band() {
        let cfg = base_cfg();
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(20_000)));
        b.expect_wcspr().returning(|| Ok(u(20_000)));
        b.expect_stcspr().returning(|| Ok(u(20_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(150)));
        let rates = MockRates::new();
        let ops = approvals_ok_ops();
        let state = no_pending_store();
        let clock = fixed_clock(0);

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn fills_cspr_by_unwrapping_wcspr_when_wcspr_has_most() {
        let cfg = base_cfg();
        // CSPR below min: midpoint target = 20_000, deficit = 15_000.
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(5_000)));
        b.expect_wcspr().returning(|| Ok(u(30_000)));
        b.expect_stcspr().returning(|| Ok(u(15_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(150)));
        // wcspr_avail = 17_500 (30_000 - 12_500), stcspr_avail = 2_500.
        // Both are always queried; wcspr is tried first and covers full deficit.
        let mut rates = MockRates::new();
        rates
            .expect_stcspr_to_cspr()
            .withf(|a| *a == u(2_500))
            .returning(|a| Ok(a));
        let mut ops = MockOps::new();
        ops.expect_has_allowance().returning(|_| Ok(true));
        ops.expect_unwrap()
            .withf(|a| *a == u(15_000))
            .times(1)
            .returning(|_| Ok(()));
        let state = no_pending_store();
        let clock = fixed_clock(0);

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn fills_cspr_falls_through_to_unstake_when_wcspr_insufficient() {
        let cfg = base_cfg();
        // CSPR=5_000 → deficit=15_000. WCSPR spendable=2_500, stCSPR spendable=17_500.
        // Sources sorted by spendable desc: stCSPR first → covers full 15_000 via
        // unstake. WCSPR is NOT touched (remaining hits zero first).
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(5_000)));
        b.expect_wcspr().returning(|| Ok(u(15_000)));
        b.expect_stcspr().returning(|| Ok(u(30_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(150)));
        let mut rates = MockRates::new();
        rates
            .expect_stcspr_to_cspr()
            .withf(|a| *a == u(17_500))
            .returning(|a| Ok(a));
        rates
            .expect_cspr_to_stcspr()
            .withf(|a| *a == u(15_000))
            .returning(|a| Ok(a));
        let mut ops = MockOps::new();
        ops.expect_has_allowance().returning(|_| Ok(true));
        ops.expect_unstake()
            .withf(|a| *a == u(15_000))
            .times(1)
            .returning(|_| Ok(()));
        let mut state = MockStateStore::new();
        state.expect_load().returning(|| Ok(None));
        state
            .expect_save()
            .withf(|p| p.created_at_secs == 42)
            .times(1)
            .returning(|_| Ok(()));
        let clock = fixed_clock(42);

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn fill_cspr_defers_unstake_when_pending_exists() {
        let cfg = base_cfg();
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(5_000)));
        b.expect_wcspr().returning(|| Ok(u(12_500))); // no spendable wcspr
        b.expect_stcspr().returning(|| Ok(u(30_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(150)));
        let mut rates = MockRates::new();
        rates.expect_stcspr_to_cspr().returning(|a| Ok(a));
        let mut ops = MockOps::new();
        ops.expect_has_allowance().returning(|_| Ok(true));
        // Neither unwrap nor unstake is called.
        let mut state = MockStateStore::new();
        // called twice: once inside try_claim_matured (returns pending),
        // and inside fill_cspr after wcspr source exhausted.
        // But try_claim_matured would see pending and return immaturity,
        // not proceed to fill. So actually fill_cspr isn't reached.
        // Adjust: clock still within maturity window.
        state
            .expect_load()
            .returning(|| Ok(Some(PendingUnstake { created_at_secs: 0 })));
        let clock = fixed_clock(10); // well below UNSTAKE_MATURITY_SECS

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn claims_matured_unstake_and_clears_state() {
        let cfg = base_cfg();
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(20_000)));
        b.expect_wcspr().returning(|| Ok(u(20_000)));
        b.expect_stcspr().returning(|| Ok(u(20_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(150)));
        let rates = MockRates::new();
        let mut ops = MockOps::new();
        ops.expect_claim().times(1).returning(|| Ok(()));
        let mut state = MockStateStore::new();
        state
            .expect_load()
            .returning(|| Ok(Some(PendingUnstake { created_at_secs: 0 })));
        state.expect_clear().times(1).returning(|| Ok(()));
        let clock = fixed_clock(UNSTAKE_MATURITY_SECS + 1);

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn dry_run_skips_state_changes_but_reads_balances() {
        let cfg = base_cfg();
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(5_000)));
        b.expect_wcspr().returning(|| Ok(u(30_000)));
        b.expect_stcspr().returning(|| Ok(u(15_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(150)));
        let mut rates = MockRates::new();
        rates.expect_stcspr_to_cspr().returning(|a| Ok(a));
        let mut ops = MockOps::new();
        ops.expect_has_allowance().returning(|_| Ok(true));
        // No unwrap / wrap / etc. expected.
        let state = no_pending_store();
        let clock = fixed_clock(0);

        let r = Rebalancer::new(&cfg, true, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn drain_cspr_prefers_wrap_when_wcspr_has_headroom() {
        let cfg = base_cfg();
        // CSPR=40_000 (above 30_000 max); target=midpoint 20_000; excess=20_000.
        // WCSPR=10_000 → headroom=20_000 → wrap full excess. stCSPR is not queried.
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(40_000)));
        b.expect_wcspr().returning(|| Ok(u(10_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(150)));
        // stCSPR balance is read once by log_state.
        b.expect_stcspr().returning(|| Ok(u(20_000)));
        let rates = MockRates::new();
        let mut ops = MockOps::new();
        ops.expect_has_allowance().returning(|_| Ok(true));
        ops.expect_wrap()
            .withf(|a| *a == u(20_000))
            .times(1)
            .returning(|_| Ok(()));
        let state = no_pending_store();
        let clock = fixed_clock(0);

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn drain_cspr_falls_back_to_stake_when_wcspr_at_max_and_clamps_to_headroom() {
        let cfg = base_cfg();
        // CSPR=40_000 (excess=20_000 vs midpoint).
        // WCSPR=30_000 (at max → no headroom). stCSPR=25_000 → headroom=5_000.
        // Stake is clamped to 5_000 (sink headroom), not the full 20_000 excess.
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(40_000)));
        b.expect_wcspr().returning(|| Ok(u(30_000)));
        b.expect_stcspr().returning(|| Ok(u(25_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(150)));
        let mut rates = MockRates::new();
        // 1:1 staking rate for test purposes.
        rates
            .expect_stcspr_to_cspr()
            .withf(|a| *a == u(5_000))
            .returning(|a| Ok(a));
        let mut ops = MockOps::new();
        ops.expect_has_allowance().returning(|_| Ok(true));
        ops.expect_stake()
            .withf(|a| *a == u(5_000))
            .times(1)
            .returning(|_| Ok(()));
        let state = no_pending_store();
        let clock = fixed_clock(0);

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn drain_cspr_noop_when_all_sinks_at_max() {
        let cfg = base_cfg();
        // WCSPR and stCSPR both at max; no drain possible this tick.
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(40_000)));
        b.expect_wcspr().returning(|| Ok(u(30_000)));
        b.expect_stcspr().returning(|| Ok(u(30_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(150)));
        let mut rates = MockRates::new();
        rates
            .expect_stcspr_to_cspr()
            .withf(|a| a.is_zero())
            .returning(|_| Ok(U256::zero()));
        let mut ops = MockOps::new();
        ops.expect_has_allowance().returning(|_| Ok(true));
        // No wrap/stake expected.
        let state = no_pending_store();
        let clock = fixed_clock(0);

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn drain_long_sells_excess() {
        let cfg = base_cfg();
        // long=5_000 (max=4_000); midpoint=2_500; excess=2_500.
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(20_000)));
        b.expect_wcspr().returning(|| Ok(u(20_000)));
        b.expect_stcspr().returning(|| Ok(u(20_000)));
        b.expect_long().returning(|| Ok(u(5_000)));
        b.expect_short().returning(|| Ok(u(150)));
        let rates = MockRates::new();
        let mut ops = MockOps::new();
        ops.expect_has_allowance().returning(|_| Ok(true));
        ops.expect_sell_long()
            .withf(|a| *a == u(2_500))
            .times(1)
            .returning(|_| Ok(()));
        let state = no_pending_store();
        let clock = fixed_clock(0);

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn fill_short_uses_wcspr_per_rates_quote() {
        let cfg = base_cfg();
        // short=50 (min=80); midpoint=190; deficit=140 shorts.
        // rates quote 140 shorts = 70 wcspr; wcspr spendable = 20_000-12_500 = 7_500.
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(20_000)));
        b.expect_wcspr().returning(|| Ok(u(20_000)));
        b.expect_stcspr().returning(|| Ok(u(20_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(50)));
        let mut rates = MockRates::new();
        rates
            .expect_wcspr_in_for_short_out()
            .withf(|a| *a == u(140))
            .returning(|_| Ok(u(70)));
        let mut ops = MockOps::new();
        ops.expect_has_allowance().returning(|_| Ok(true));
        ops.expect_buy_short()
            .withf(|a| *a == u(70))
            .times(1)
            .returning(|_| Ok(()));
        let state = no_pending_store();
        let clock = fixed_clock(0);

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn cspr_imbalance_takes_priority_over_wcspr_imbalance() {
        let cfg = base_cfg();
        // Both CSPR and WCSPR are below min — CSPR wins; WCSPR is not touched.
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(5_000)));
        b.expect_wcspr().returning(|| Ok(u(5_000)));
        b.expect_stcspr().returning(|| Ok(u(30_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(150)));
        let mut rates = MockRates::new();
        rates.expect_stcspr_to_cspr().returning(|a| Ok(a));
        rates.expect_cspr_to_stcspr().returning(|a| Ok(a));
        let mut ops = MockOps::new();
        ops.expect_has_allowance().returning(|_| Ok(true));
        // Only one action in this tick — an unstake for CSPR fill.
        ops.expect_unstake().times(1).returning(|_| Ok(()));
        let mut state = MockStateStore::new();
        state.expect_load().returning(|| Ok(None));
        state.expect_save().returning(|_| Ok(()));
        let clock = fixed_clock(100);

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn approves_missing_allowance() {
        let cfg = base_cfg();
        let mut b = MockBalances::new();
        b.expect_cspr().returning(|| Ok(u(20_000)));
        b.expect_wcspr().returning(|| Ok(u(20_000)));
        b.expect_stcspr().returning(|| Ok(u(20_000)));
        b.expect_long().returning(|| Ok(u(2_000)));
        b.expect_short().returning(|| Ok(u(150)));
        let rates = MockRates::new();
        let mut ops = MockOps::new();
        ops.expect_has_allowance()
            .withf(|a| *a == Approval::WcsprMarket)
            .returning(|_| Ok(false));
        ops.expect_has_allowance()
            .withf(|a| *a != Approval::WcsprMarket)
            .returning(|_| Ok(true));
        ops.expect_approve_max()
            .withf(|a| *a == Approval::WcsprMarket)
            .times(1)
            .returning(|_| Ok(()));
        let state = no_pending_store();
        let clock = fixed_clock(0);

        let r = Rebalancer::new(&cfg, false, &b, &rates, &ops, &state, &clock);
        r.rebalance().unwrap();
    }

    #[test]
    fn file_state_store_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("rebalancer-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&tmp);
        let store = FileStateStore::new(&tmp);
        assert!(store.load().unwrap().is_none());
        let p = PendingUnstake {
            created_at_secs: 12345,
        };
        store.save(&p).unwrap();
        assert_eq!(store.load().unwrap(), Some(p));
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }
}
