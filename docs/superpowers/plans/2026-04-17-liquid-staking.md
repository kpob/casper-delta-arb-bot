# Liquid Staking Arbitrage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a self-contained liquid staking arbitrage module (`src/bot/liquid_staking/`) that runs alongside the existing Casper Delta engine, detecting price deviations between StCSPR on Casper Trade DEX and the StakedCSPR contract's fair redemption value.

**Architecture:** A new `LsEngine` (parallel to the existing `BotEngine`) is instantiated in `src/bot.rs` and called on every event. It owns its own price calculator, path selection, and a file-backed `PendingClaims` tracker for delayed unstake claims. All existing Casper Delta code is untouched except for two lines in `src/bot.rs`.

**Tech Stack:** Rust (nightly), Odra 2.5, liquid-staking-contracts (git), serde_json for claim persistence, existing rdkafka/Kafka event plumbing.

---

## File Map

| Action | Path | Purpose |
|--------|------|---------|
| Create | `src/bot/liquid_staking/mod.rs` | `LsEngine` — orchestrates all LS logic |
| Create | `src/bot/liquid_staking/path.rs` | `LsPath` enum + threshold logic |
| Create | `src/bot/liquid_staking/data.rs` | `LsPriceData` struct |
| Create | `src/bot/liquid_staking/utils.rs` | `LsPriceCalculator` — reads contracts, computes gain |
| Create | `src/bot/liquid_staking/claims.rs` | `PendingClaims` — file-backed unstake tracker |
| Modify | `Cargo.toml` | Add `liquid-staking-contracts` git dependency |
| Modify | `contracts-main.toml` | Add `StakedCSPR` + `WCSPR-StCSPR LP` entries |
| Modify | `src/contracts.rs` | Add `staked_cspr()` + `wcspr_stcspr_pair()` |
| Modify | `src/bot.rs` | Declare module, wire `LsEngine`, extend `relevant_addresses` |

---

## Task 1: Add dependency, contract entries, and ContractRefs accessors

**Files:**
- Modify: `Cargo.toml`
- Modify: `contracts-main.toml`
- Modify: `src/contracts.rs`

- [ ] **Step 1: Add the git dependency to `Cargo.toml`**

In `Cargo.toml`, add after the existing path dependencies:

```toml
liquid-staking-contracts = { git = "https://github.com/casper-ecosystem/liquid-staking-contracts", branch = "develop" }
```

- [ ] **Step 2: Add contract entries to `contracts-main.toml`**

Append to the end of `contracts-main.toml`:

```toml
[[contracts]]
name = "StakedCSPR"
package_name = "StakedCSPR"
package_hash = "hash-d08ca0c6b4d567a671b5a26f51fb78333ffce403544d19a09a93ebb7c04a6a53"

[[contracts]]
name = "Pair"
package_name = "WCSPR-StCSPR LP"
package_hash = "hash-99227bb4082ce12f9198651c7eec88dbdb290030da1dfe17cef487bd7d2fe68b"
```

- [ ] **Step 3: Add the import and two accessors to `src/contracts.rs`**

At the top of `src/contracts.rs`, add the new import alongside the existing ones:

```rust
use liquid_staking_contracts::token::{StakedCSPR, StakedCSPRHostRef};
```

At the end of the `impl ContractRefs` block (after the `short()` method), add:

```rust
pub fn staked_cspr(&self) -> Result<StakedCSPRHostRef, Error> {
    Ok(self.container.contract_ref::<StakedCSPR>(self.env)?)
}

pub fn wcspr_stcspr_pair(&self) -> Result<PairHostRef, Error> {
    Ok(self
        .container
        .contract_ref_named::<Pair>(self.env, Some("WCSPR-StCSPR LP".to_string()))?)
}
```

- [ ] **Step 4: Verify the project compiles**

Run:
```bash
cargo build 2>&1 | head -40
```

Expected: no errors. If you see an `odra` version conflict between `2.4.0` (liquid-staking-contracts) and `2.5.0` (bot), Cargo will normally unify to `2.5.0` automatically — this is fine as long as the API is compatible. If the build fails with a type mismatch in the liquid-staking-contracts code, pin the dependency to a specific commit hash that has been updated to 2.5.0, or open an issue upstream.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml contracts-main.toml src/contracts.rs
git commit -m "feat: add liquid-staking-contracts dependency and ContractRefs accessors"
```

---

## Task 2: LsPriceData

**Files:**
- Create: `src/bot/liquid_staking/data.rs`

- [ ] **Step 1: Create the directory and write a failing test**

```bash
mkdir -p src/bot/liquid_staking
```

Create `src/bot/liquid_staking/data.rs` with the tests first:

```rust
use odra::casper_types::U256;

const DECIMAL_PLACES: u32 = 9;

#[derive(Debug)]
pub struct LsPriceData {
    pub dex_price: f64,
    pub fair_price: f64,
    pub diff: f64,
    pub stcspr_for_ten_usd: u64,
    pub wcspr_for_ten_usd: u64,
    pub wcspr_price: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_computed_correctly() {
        let data = LsPriceData::new(1.05, 1.0, 0.04);
        // (1.05 / 1.0) * 100 - 100 = 5.0
        assert!((data.diff - 5.0).abs() < 0.001);
    }

    #[test]
    fn test_negative_diff_when_underpriced() {
        let data = LsPriceData::new(0.94, 1.0, 0.04);
        // (0.94 / 1.0) * 100 - 100 = -6.0
        assert!((data.diff - (-6.0)).abs() < 0.001);
    }

    #[test]
    fn test_wcspr_for_ten_usd() {
        // 10 / 0.04 = 250 whole WCSPR
        let data = LsPriceData::new(1.0, 1.0, 0.04);
        assert_eq!(data.wcspr_for_ten_usd, 250);
    }

    #[test]
    fn test_stcspr_for_ten_usd() {
        // 10 / 0.04 / 1.05 ≈ 238 whole sCSPR
        let data = LsPriceData::new(1.05, 1.05, 0.04);
        assert_eq!(data.stcspr_for_ten_usd, 238);
    }

    #[test]
    fn test_amount_per_ten_usd_buy_and_unstake() {
        // wcspr_for_ten_usd = 250, × 10^9 = 250_000_000_000
        let data = LsPriceData::new(1.0, 1.0, 0.04);
        use crate::bot::liquid_staking::path::LsPath;
        assert_eq!(
            data.amount_per_ten_usd(LsPath::BuyAndUnstake),
            U256::from(250_000_000_000u64)
        );
    }

    #[test]
    fn test_amount_per_ten_usd_stake_and_sell() {
        // stcspr_for_ten_usd = 250 (when fair_price=1.0), × 10^9
        let data = LsPriceData::new(1.0, 1.0, 0.04);
        use crate::bot::liquid_staking::path::LsPath;
        assert_eq!(
            data.amount_per_ten_usd(LsPath::StakeAndSell),
            U256::from(250_000_000_000u64)
        );
    }
}
```

- [ ] **Step 2: Run the tests — expect compile failure (LsPath not yet defined)**

```bash
cargo test liquid_staking::data 2>&1 | tail -20
```

Expected: compile error about missing `LsPath`. This is correct — it confirms the test wires the two modules together as intended.

- [ ] **Step 3: Implement `LsPriceData` fully**

Replace the body of `src/bot/liquid_staking/data.rs` with the full implementation:

```rust
use odra::casper_types::U256;

const DECIMAL_PLACES: u32 = 9;

#[derive(Debug)]
pub struct LsPriceData {
    pub dex_price: f64,
    pub fair_price: f64,
    pub diff: f64,
    pub stcspr_for_ten_usd: u64,
    pub wcspr_for_ten_usd: u64,
    pub wcspr_price: f64,
}

impl LsPriceData {
    pub fn new(dex_price: f64, fair_price: f64, wcspr_price: f64) -> Self {
        let diff = (dex_price / fair_price) * 100.0 - 100.0;
        let wcspr_for_ten_usd = (10.0 / wcspr_price) as u64;
        let stcspr_for_ten_usd = (10.0 / wcspr_price / fair_price) as u64;
        Self {
            dex_price,
            fair_price,
            diff,
            stcspr_for_ten_usd,
            wcspr_for_ten_usd,
            wcspr_price,
        }
    }

    pub fn amount_per_ten_usd(&self, path: super::path::LsPath) -> U256 {
        match path {
            super::path::LsPath::StakeAndSell => {
                U256::from(self.stcspr_for_ten_usd * 10u64.pow(DECIMAL_PLACES))
            }
            super::path::LsPath::BuyAndUnstake => {
                U256::from(self.wcspr_for_ten_usd * 10u64.pow(DECIMAL_PLACES))
            }
            super::path::LsPath::Empty => U256::zero(),
        }
    }

    pub fn log(&self) {
        tracing::info!(
            dex_price = self.dex_price,
            fair_price = self.fair_price,
            diff = format!("{:+.2}%", self.diff),
            "LS prices (CSPR)"
        );
    }
}
```

> **Note:** `amount_per_ten_usd` uses `super::path::LsPath` which is defined in Task 3. The tests will fully pass only after Task 3 is complete. The struct itself is ready.

- [ ] **Step 4: Commit the data module**

```bash
git add src/bot/liquid_staking/data.rs
git commit -m "feat: add LsPriceData for liquid staking"
```

---

## Task 3: LsPath

**Files:**
- Create: `src/bot/liquid_staking/path.rs`

- [ ] **Step 1: Create `src/bot/liquid_staking/path.rs` with tests first**

```rust
use odra::prelude::{Address, Addressable};
use odra_cli::scenario::Error;

use crate::contracts::ContractRefs;
use super::data::LsPriceData;

const STAKE_AND_SELL_THRESHOLD: f64 = 2.5;
const BUY_AND_UNSTAKE_THRESHOLD: f64 = 5.0;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum LsPath {
    StakeAndSell,
    BuyAndUnstake,
    Empty,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price_data(dex: f64, fair: f64) -> LsPriceData {
        LsPriceData::new(dex, fair, 0.04)
    }

    #[test]
    fn test_stake_and_sell_when_overpriced_above_threshold() {
        // diff = +3.0% > 2.5% → StakeAndSell
        let data = price_data(1.03, 1.0);
        assert_eq!(LsPath::from(&data), LsPath::StakeAndSell);
    }

    #[test]
    fn test_empty_when_overpriced_below_threshold() {
        // diff = +1.0% < 2.5% → Empty
        let data = price_data(1.01, 1.0);
        assert_eq!(LsPath::from(&data), LsPath::Empty);
    }

    #[test]
    fn test_buy_and_unstake_when_underpriced_above_threshold() {
        // diff = -6.0% and abs > 5.0% → BuyAndUnstake
        let data = price_data(0.94, 1.0);
        assert_eq!(LsPath::from(&data), LsPath::BuyAndUnstake);
    }

    #[test]
    fn test_empty_when_underpriced_below_5_percent_threshold() {
        // diff = -3.0%, abs < 5.0% → Empty (not BuyAndUnstake)
        let data = price_data(0.97, 1.0);
        assert_eq!(LsPath::from(&data), LsPath::Empty);
    }

    #[test]
    fn test_empty_at_exact_stake_and_sell_boundary() {
        // diff = exactly 2.5% — must be strictly greater → Empty
        let dex = 1.025;
        let fair = 1.0;
        let data = price_data(dex, fair);
        // (1.025/1.0)*100-100 = 2.5 → not strictly > 2.5 → Empty
        assert_eq!(LsPath::from(&data), LsPath::Empty);
    }

    #[test]
    fn test_stake_and_sell_just_above_boundary() {
        let dex = 1.0251;
        let fair = 1.0;
        let data = price_data(dex, fair);
        assert_eq!(LsPath::from(&data), LsPath::StakeAndSell);
    }
}
```

- [ ] **Step 2: Run tests — expect compile failure (impl missing)**

```bash
cargo test liquid_staking::path 2>&1 | tail -20
```

Expected: compile error about missing `From` impl and `LsPath` methods.

- [ ] **Step 3: Implement `LsPath`**

Append to `src/bot/liquid_staking/path.rs` (after the enum definition):

```rust
impl From<&LsPriceData> for LsPath {
    fn from(data: &LsPriceData) -> Self {
        Self::calc(data)
    }
}

impl LsPath {
    fn calc(data: &LsPriceData) -> Self {
        if data.diff > STAKE_AND_SELL_THRESHOLD {
            LsPath::StakeAndSell
        } else if data.diff < -BUY_AND_UNSTAKE_THRESHOLD {
            LsPath::BuyAndUnstake
        } else {
            LsPath::Empty
        }
    }

    /// Returns the DEX swap path as a list of token addresses.
    /// For StakeAndSell the swap direction is sCSPR → WCSPR.
    /// For BuyAndUnstake it is WCSPR → sCSPR.
    pub fn build(&self, refs: &ContractRefs) -> Result<Vec<Address>, Error> {
        let wcspr = refs.wcspr()?.address();
        let stcspr = refs.staked_cspr()?.address();
        match self {
            LsPath::StakeAndSell => Ok(vec![stcspr, wcspr]),
            LsPath::BuyAndUnstake => Ok(vec![wcspr, stcspr]),
            LsPath::Empty => Ok(vec![]),
        }
    }
}
```

- [ ] **Step 4: Create a minimal `src/bot/liquid_staking/mod.rs` so the module tree compiles**

```rust
pub mod claims;
pub mod data;
pub mod path;
pub mod utils;
```

Also create empty placeholder files so Rust doesn't complain about missing modules:

```bash
touch src/bot/liquid_staking/claims.rs
touch src/bot/liquid_staking/utils.rs
```

- [ ] **Step 5: Declare the module in `src/bot.rs`**

In `src/bot.rs`, add inside the existing `mod` declarations block:

```rust
mod liquid_staking;
```

- [ ] **Step 6: Run all LS tests so far**

```bash
cargo test liquid_staking 2>&1 | tail -30
```

Expected: all `path` and `data` tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/bot/liquid_staking/
git commit -m "feat: add LsPath and LsPriceData"
```

---

## Task 4: PendingClaims

**Files:**
- Modify: `src/bot/liquid_staking/claims.rs`

- [ ] **Step 1: Write the failing tests in `src/bot/liquid_staking/claims.rs`**

```rust
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use odra_cli::scenario::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingClaim {
    /// Casper block time (ms since epoch) after which claim() can be called.
    pub claimable_from_ms: u64,
}

pub struct PendingClaims {
    pub(crate) claims: Vec<PendingClaim>,
    file_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path() -> String {
        format!(
            "{}/test_pending_claims_{}.json",
            std::env::temp_dir().display(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        )
    }

    #[test]
    fn test_load_from_missing_file_returns_empty() {
        let claims = PendingClaims::load("/nonexistent/path/claims.json");
        assert_eq!(claims.claims.len(), 0);
    }

    #[test]
    fn test_add_persists_to_file_and_reloads() {
        let path = tmp_path();
        let mut claims = PendingClaims::load(&path);
        claims.add(PendingClaim { claimable_from_ms: 9_999_999_999_999 }).unwrap();

        let reloaded = PendingClaims::load(&path);
        assert_eq!(reloaded.claims.len(), 1);
        assert_eq!(reloaded.claims[0].claimable_from_ms, 9_999_999_999_999);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_has_ready_claims_when_claimable_from_is_in_past() {
        let claims = PendingClaims {
            claims: vec![PendingClaim { claimable_from_ms: 1_000 }],
            file_path: String::new(),
        };
        assert!(claims.has_ready_claims());
    }

    #[test]
    fn test_no_ready_claims_when_claimable_from_is_in_future() {
        let claims = PendingClaims {
            claims: vec![PendingClaim { claimable_from_ms: u64::MAX }],
            file_path: String::new(),
        };
        assert!(!claims.has_ready_claims());
    }

    #[test]
    fn test_remove_ready_removes_past_keeps_future() {
        let path = tmp_path();
        let mut claims = PendingClaims {
            claims: vec![
                PendingClaim { claimable_from_ms: 1_000 },       // past
                PendingClaim { claimable_from_ms: u64::MAX },    // future
            ],
            file_path: path.clone(),
        };
        claims.remove_ready().unwrap();
        assert_eq!(claims.claims.len(), 1);
        assert_eq!(claims.claims[0].claimable_from_ms, u64::MAX);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_remove_ready_persists_remaining_claims() {
        let path = tmp_path();
        let mut claims = PendingClaims {
            claims: vec![
                PendingClaim { claimable_from_ms: 1_000 },
                PendingClaim { claimable_from_ms: u64::MAX },
            ],
            file_path: path.clone(),
        };
        claims.remove_ready().unwrap();

        let reloaded = PendingClaims::load(&path);
        assert_eq!(reloaded.claims.len(), 1);
        assert_eq!(reloaded.claims[0].claimable_from_ms, u64::MAX);
        let _ = std::fs::remove_file(&path);
    }
}
```

- [ ] **Step 2: Run tests — expect compile failure (impl missing)**

```bash
cargo test liquid_staking::claims 2>&1 | tail -20
```

Expected: compile error about missing methods.

- [ ] **Step 3: Implement `PendingClaims`**

Add the implementation after the struct definitions in `src/bot/liquid_staking/claims.rs`:

```rust
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl PendingClaims {
    /// Loads claims from a JSON file. Returns an empty list if the file is
    /// missing or unreadable — this is not treated as an error.
    pub fn load(file_path: &str) -> Self {
        let claims = std::fs::read_to_string(file_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self {
            claims,
            file_path: file_path.to_string(),
        }
    }

    /// Appends a claim and rewrites the file atomically.
    pub fn add(&mut self, claim: PendingClaim) -> Result<(), Error> {
        self.claims.push(claim);
        self.persist()
    }

    /// Returns true if any claim's `claimable_from_ms` has passed.
    pub fn has_ready_claims(&self) -> bool {
        let now = now_ms();
        self.claims.iter().any(|c| c.claimable_from_ms <= now)
    }

    /// Removes all claims whose `claimable_from_ms` has passed and rewrites the
    /// file. Call this after a successful `staked_cspr.claim()` transaction.
    pub fn remove_ready(&mut self) -> Result<(), Error> {
        let now = now_ms();
        self.claims.retain(|c| c.claimable_from_ms > now);
        if self.file_path.is_empty() {
            return Ok(());
        }
        self.persist()
    }

    fn persist(&self) -> Result<(), Error> {
        if self.file_path.is_empty() {
            return Ok(());
        }
        let json = serde_json::to_string_pretty(&self.claims).map_err(|e| Error::OdraError {
            message: format!("Failed to serialize claims: {e}"),
        })?;
        std::fs::write(&self.file_path, json).map_err(|e| Error::OdraError {
            message: format!("Failed to write claims file: {e}"),
        })
    }
}
```

- [ ] **Step 4: Run tests — expect all pass**

```bash
cargo test liquid_staking::claims 2>&1 | tail -20
```

Expected output:
```
test bot::liquid_staking::claims::tests::test_add_persists_to_file_and_reloads ... ok
test bot::liquid_staking::claims::tests::test_has_ready_claims_when_claimable_from_is_in_past ... ok
test bot::liquid_staking::claims::tests::test_load_from_missing_file_returns_empty ... ok
test bot::liquid_staking::claims::tests::test_no_ready_claims_when_claimable_from_is_in_future ... ok
test bot::liquid_staking::claims::tests::test_remove_ready_persists_remaining_claims ... ok
test bot::liquid_staking::claims::tests::test_remove_ready_removes_past_keeps_future ... ok
```

- [ ] **Step 5: Commit**

```bash
git add src/bot/liquid_staking/claims.rs
git commit -m "feat: add file-backed PendingClaims for liquid staking unstakes"
```

---

## Task 5: LsPriceCalculator

**Files:**
- Modify: `src/bot/liquid_staking/utils.rs`

> This module makes live contract calls — the same pattern as the existing `PriceCalculator` in `src/bot/utils.rs`, which also has no unit tests. Integration testing is done via a full bot run.

- [ ] **Step 1: Implement `LsPriceCalculator` in `src/bot/liquid_staking/utils.rs`**

```rust
use odra::casper_types::{U256, U512};
use odra_cli::scenario::Error;

use super::data::LsPriceData;
use super::path::LsPath;
use crate::contracts::ContractRefs;

pub(super) struct LsPriceCalculator<'a> {
    contracts: &'a ContractRefs<'a>,
}

impl<'a> LsPriceCalculator<'a> {
    pub(super) fn new(contracts: &'a ContractRefs<'a>) -> Self {
        Self { contracts }
    }

    /// Fetches DEX price, fair price, and WCSPR/USD price.
    pub(super) fn prices(&self) -> Result<LsPriceData, Error> {
        // DEX price: WCSPR per sCSPR from the pair reserves.
        // Pair name is "WCSPR-StCSPR LP": token0 = WCSPR, token1 = sCSPR.
        let (reserves_wcspr, reserves_stcspr, _) =
            self.contracts.wcspr_stcspr_pair()?.get_reserves();
        let dex_price = Self::calculate_price(reserves_wcspr, reserves_stcspr);

        // Fair price: staked_cspr (U512 motes) / total_supply (U256 motes).
        let sc = self.contracts.staked_cspr()?;
        let staked: U512 = sc.staked_cspr();
        let supply: U256 = sc.total_supply();
        let fair_price = Self::calculate_price_u512(staked, supply);

        // WCSPR USD price: reuse the same Market oracle as Casper Delta.
        let market = self.contracts.market()?;
        let state = market
            .try_get_address_market_state(market.address())?
            .market_state;
        let wcspr_price = state.price().as_u64() as f64 / 100_000.0;

        Ok(LsPriceData::new(dex_price, fair_price, wcspr_price))
    }

    /// Calculates gain in CSPR for a completed (or simulated) LS swap.
    ///
    /// For `StakeAndSell`: `amount_in` is sCSPR motes fed to the DEX,
    ///   `amount_out` is WCSPR motes received.
    /// For `BuyAndUnstake`: `amount_in` is WCSPR motes spent on the DEX,
    ///   `amount_out` is sCSPR motes received.
    pub(super) fn calc_gains_in_cspr(
        amount_in: U256,
        amount_out: U256,
        price_data: &LsPriceData,
        path: LsPath,
    ) -> f64 {
        let (amount_in_cspr, amount_out_cspr, tx_cost) = match path {
            LsPath::StakeAndSell => (
                amount_in.as_u64() as f64 * price_data.fair_price,
                amount_out.as_u64() as f64,
                20.0f64,
            ),
            LsPath::BuyAndUnstake => (
                amount_in.as_u64() as f64,
                amount_out.as_u64() as f64 * price_data.fair_price,
                17.0f64,
            ),
            LsPath::Empty => return 0.0,
        };
        (amount_out_cspr - amount_in_cspr) / 1_000_000_000.0 - tx_cost
    }

    fn calculate_price(amount0: U256, amount1: U256) -> f64 {
        (amount0 * U256::from(1_000_000u64) / amount1).as_u64() as f64 / 1_000_000.0
    }

    /// Same as `calculate_price` but accepts U512 as the numerator (for
    /// `staked_cspr()` which returns U512).
    fn calculate_price_u512(amount0: U512, amount1: U256) -> f64 {
        let scaled = amount0 * U512::from(1_000_000u64) / U512::from(amount1);
        scaled.as_u64() as f64 / 1_000_000.0
    }
}
```

- [ ] **Step 2: Verify the project still compiles**

```bash
cargo build 2>&1 | tail -20
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/bot/liquid_staking/utils.rs
git commit -m "feat: add LsPriceCalculator for liquid staking"
```

---

## Task 6: LsEngine

**Files:**
- Modify: `src/bot/liquid_staking/mod.rs`

- [ ] **Step 1: Replace the placeholder `mod.rs` with the full `LsEngine` implementation**

```rust
pub mod claims;
pub mod data;
pub mod path;
pub mod utils;

use odra::casper_types::{U256, U512};
use odra::host::HostEnv;
use odra::prelude::{Address, Addressable};
use odra_cli::{cspr, scenario::Error};
use tracing::instrument;

use crate::bot::events::BotEvent;
use crate::contracts::ContractRefs;
use claims::{PendingClaim, PendingClaims};
use data::LsPriceData;
use path::LsPath;
use utils::LsPriceCalculator;

const CLAIMS_FILE: &str = "pending_claims.json";
const MIN_GAIN_CSPR: f64 = 50.0;

pub struct LsEngine<'a> {
    contracts: &'a ContractRefs<'a>,
    env: &'a HostEnv,
    caller: Address,
    claims: PendingClaims,
    dry_run: bool,
}

impl<'a> LsEngine<'a> {
    pub fn new(
        contracts: &'a ContractRefs<'a>,
        env: &'a HostEnv,
        caller: Address,
        dry_run: bool,
    ) -> Self {
        Self {
            contracts,
            env,
            caller,
            claims: PendingClaims::load(CLAIMS_FILE),
            dry_run,
        }
    }

    /// Approves the Router to spend sCSPR on behalf of the caller.
    /// Must be called once at startup before any StakeAndSell swap.
    pub fn approve_stcspr(&self) -> Result<(), Error> {
        let me = self.caller;
        let router_addr = self.contracts.router()?.address();
        if self.contracts.staked_cspr()?.allowance(&me, &router_addr).is_zero() {
            self.env.set_gas(cspr!(4));
            self.contracts.staked_cspr()?.approve(&router_addr, &U256::MAX);
        }
        Ok(())
    }

    /// Attempts to claim any matured unstakes. Call this before each arb check.
    #[instrument(skip(self))]
    pub fn try_claim_ready(&mut self) -> Result<(), Error> {
        if !self.claims.has_ready_claims() {
            return Ok(());
        }
        if self.dry_run {
            tracing::info!("Dry run — skipping claim (ready claims exist)");
            return Ok(());
        }
        tracing::info!("Claiming matured unstakes...");
        self.env.set_gas(cspr!(5));
        self.contracts.staked_cspr()?.try_claim()?;
        self.claims.remove_ready()?;
        tracing::info!("Claim complete");
        Ok(())
    }

    /// Handle a single event. Returns `Ok(true)` to continue, `Ok(false)` to stop.
    #[instrument(skip(self))]
    pub fn handle_event(&mut self, event: &BotEvent) -> Result<bool, Error> {
        match event {
            BotEvent::TimerTick | BotEvent::TradeExecuted | BotEvent::PriceChanged => {
                self.check_and_trade()?;
                Ok(true)
            }
            BotEvent::Shutdown => Ok(false),
        }
    }

    fn check_and_trade(&mut self) -> Result<(), Error> {
        let price_data = LsPriceCalculator::new(self.contracts).prices()?;
        price_data.log();

        let path = LsPath::from(&price_data);
        tracing::info!("LS swap path: {:?}", path);
        if path == LsPath::Empty {
            tracing::info!("No LS arbitrage opportunity");
            return Ok(());
        }

        let amounts = self.get_swap_amounts(&price_data, path)?;
        let (amount_in, amount_out) = match amounts.as_slice() {
            [a_in, .., a_out] => (*a_in, *a_out),
            _ => {
                tracing::info!("No valid LS swap amounts");
                return Ok(());
            }
        };

        let gain = LsPriceCalculator::calc_gains_in_cspr(amount_in, amount_out, &price_data, path);
        tracing::info!("LS gain estimate: {:.4} CSPR", gain);

        if gain < MIN_GAIN_CSPR {
            tracing::info!("LS gain below threshold ({MIN_GAIN_CSPR} CSPR), skipping");
            return Ok(());
        }

        match path {
            LsPath::StakeAndSell => {
                // amount_in = sCSPR to sell (estimated from staking)
                // amount_out = WCSPR to receive
                let cspr_to_stake_motes =
                    (amount_in.as_u64() as f64 * price_data.fair_price) as u64;
                self.execute_stake_and_sell(
                    U256::from(cspr_to_stake_motes),
                    amount_in,
                    amount_out,
                )?;
            }
            LsPath::BuyAndUnstake => {
                // amount_in = WCSPR to spend, amount_out = sCSPR to receive
                self.execute_buy_and_unstake(amount_in, amount_out)?;
            }
            LsPath::Empty => unreachable!(),
        }

        Ok(())
    }

    fn get_swap_amounts(&self, price_data: &LsPriceData, path: LsPath) -> Result<Vec<U256>, Error> {
        let amount_in = price_data.amount_per_ten_usd(path);
        let dex_path = path.build(self.contracts)?;
        let amounts = self
            .contracts
            .router()?
            .try_get_amounts_out(amount_in, dex_path)
            .map_err(|e| Error::OdraError {
                message: format!("get_amounts_out failed: {e:?}"),
            })?;
        Ok(amounts)
    }

    /// Unwraps WCSPR → native CSPR, stakes it, then sells the resulting sCSPR on the DEX.
    ///
    /// * `cspr_to_stake` — native CSPR motes to stake (derived from `stcspr_in * fair_price`)
    /// * `stcspr_in`     — sCSPR motes to pass as `amount_in_max` to the swap
    /// * `wcspr_out`     — exact WCSPR motes the swap should return
    #[instrument(skip(self))]
    fn execute_stake_and_sell(
        &mut self,
        cspr_to_stake: U256,
        stcspr_in: U256,
        wcspr_out: U256,
    ) -> Result<(), Error> {
        if self.dry_run {
            tracing::info!("Dry run — stake_and_sell skipped");
            return Ok(());
        }

        let me = self.caller;

        // 1. Unwrap WCSPR → native CSPR
        tracing::info!("Unwrapping {:.4} WCSPR", cspr_to_stake.as_u64() as f64 / 1e9);
        self.env.set_gas(cspr!(4));
        self.contracts.wcspr()?.try_withdraw(&cspr_to_stake)?;

        // 2. Stake native CSPR → receive sCSPR
        tracing::info!("Staking {:.4} CSPR", cspr_to_stake.as_u64() as f64 / 1e9);
        let cspr_u512 = U512::from(cspr_to_stake.as_u64());
        self.env.set_gas(cspr!(9));
        self.contracts
            .staked_cspr()?
            .with_tokens(cspr_u512)
            .try_stake()?;

        // 3. Sell sCSPR → WCSPR via Router (use actual balance as amount_in_max)
        let scspr_balance = self.contracts.staked_cspr()?.balance_of(&me);
        let dex_path = LsPath::StakeAndSell.build(self.contracts)?;
        tracing::info!(
            "Swapping {:.4} sCSPR for {:.4} WCSPR",
            scspr_balance.as_u64() as f64 / 1e9,
            wcspr_out.as_u64() as f64 / 1e9
        );
        self.env.set_gas(cspr!(8));
        self.contracts.router()?.swap_tokens_for_exact_tokens(
            wcspr_out,
            scspr_balance,
            dex_path,
            me,
            u64::MAX,
        );

        tracing::info!("StakeAndSell complete");
        Ok(())
    }

    /// Buys sCSPR on the DEX then initiates an unstake. Records a pending claim.
    ///
    /// * `wcspr_in`   — WCSPR motes to spend (amount_in_max)
    /// * `stcspr_out` — exact sCSPR motes to buy
    #[instrument(skip(self))]
    fn execute_buy_and_unstake(
        &mut self,
        wcspr_in: U256,
        stcspr_out: U256,
    ) -> Result<(), Error> {
        if self.dry_run {
            tracing::info!("Dry run — buy_and_unstake skipped");
            return Ok(());
        }

        let me = self.caller;

        // 1. Buy sCSPR on DEX
        let dex_path = LsPath::BuyAndUnstake.build(self.contracts)?;
        tracing::info!(
            "Buying {:.4} sCSPR with {:.4} WCSPR",
            stcspr_out.as_u64() as f64 / 1e9,
            wcspr_in.as_u64() as f64 / 1e9,
        );
        self.env.set_gas(cspr!(8));
        self.contracts.router()?.swap_tokens_for_exact_tokens(
            stcspr_out,
            wcspr_in,
            dex_path,
            me,
            u64::MAX,
        );

        // 2. Initiate unstake
        tracing::info!("Initiating unstake of {:.4} sCSPR", stcspr_out.as_u64() as f64 / 1e9);
        self.env.set_gas(cspr!(6));
        self.contracts.staked_cspr()?.try_unstake(stcspr_out)?;

        // 3. Record pending claim (use claim_time from contract + wall clock)
        let claim_delay_ms = self.contracts.staked_cspr()?.get_claim_time();
        let claimable_from_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
            + claim_delay_ms;

        tracing::info!("Recording pending claim (claimable in ~{} ms)", claim_delay_ms);
        self.claims.add(PendingClaim { claimable_from_ms })?;

        tracing::info!("BuyAndUnstake complete — claim recorded");
        Ok(())
    }
}
```

- [ ] **Step 2: Verify everything compiles**

```bash
cargo build 2>&1 | tail -20
```

Expected: no errors.

- [ ] **Step 3: Run all liquid_staking tests**

```bash
cargo test liquid_staking 2>&1 | tail -30
```

Expected: all `path`, `data`, and `claims` tests pass. No `LsEngine` unit tests exist (contract calls can't be mocked at this layer — same pattern as `BotEngine`).

- [ ] **Step 4: Commit**

```bash
git add src/bot/liquid_staking/mod.rs
git commit -m "feat: add LsEngine for liquid staking arbitrage"
```

---

## Task 7: Wire LsEngine into the bot event loop

**Files:**
- Modify: `src/bot.rs`

- [ ] **Step 1: Add the module declaration and import to `src/bot.rs`**

In `src/bot.rs`, in the existing `mod` declarations block (alongside `mod engine;`, `mod events;`, etc.), add:

```rust
mod liquid_staking;
```

At the top of `src/bot.rs`, alongside the existing `use` statements, add:

```rust
use self::liquid_staking::LsEngine;
```

- [ ] **Step 2: Extend `relevant_addresses` and wire `LsEngine` in `Bot::run`**

The existing `Bot::run` method in `src/bot.rs` currently has this structure (lines ~47–80):

```rust
fn run(...) {
    let contracts = ContractRefs::new(env, container);
    let calc = PriceCalculator::new(&contracts);
    let caller = env.caller();

    let dry_run = args.get_single("dry-run").unwrap_or(false);
    let token_manager = self.build_token_manager(dry_run, env, &contracts);
    let balances = RealBalances::new(env, &contracts);
    let asset_manager = AssetManager::new(&balances, &*token_manager);
    token_manager.approve_markets()?;
    asset_manager.print_balances()?;

    let engine = BotEngine::new(calc, asset_manager, &contracts, caller);
    let config = KafkaConfig::from_env();
    let relevant_addresses = vec![
        contracts.long()?.address().to_string(),
        contracts.short()?.address().to_string(),
    ];
    let mut event_source = KafkaEventSource::new(config, relevant_addresses);

    while let Some(event) = event_source.next_event() {
        tracing::info!("Event: {:?}", event);
        match engine.handle_event(&event) {
            Ok(true) => continue,
            Ok(false) => break,
            Err(e) => {
                tracing::error!("Error handling event: {:?}", e);
            }
        }
    }
    Ok(())
}
```

Replace the body of `Bot::run` with the following:

```rust
fn run(
    &self,
    env: &HostEnv,
    container: &DeployedContractsContainer,
    args: Args,
) -> Result<(), Error> {
    let contracts = ContractRefs::new(env, container);
    let calc = PriceCalculator::new(&contracts);
    let caller = env.caller();

    let dry_run = args.get_single("dry-run").unwrap_or(false);
    let token_manager = self.build_token_manager(dry_run, env, &contracts);
    let balances = RealBalances::new(env, &contracts);
    let asset_manager = AssetManager::new(&balances, &*token_manager);
    token_manager.approve_markets()?;
    asset_manager.print_balances()?;

    let engine = BotEngine::new(calc, asset_manager, &contracts, caller);
    let mut ls_engine = LsEngine::new(&contracts, env, caller, dry_run);
    ls_engine.approve_stcspr()?;

    let config = KafkaConfig::from_env();
    tracing::info!("Connecting to Kafka at {}", config.bootstrap_servers);
    let relevant_addresses = vec![
        contracts.long()?.address().to_string(),
        contracts.short()?.address().to_string(),
        contracts.staked_cspr()?.address().to_string(),
    ];
    let mut event_source = KafkaEventSource::new(config, relevant_addresses);

    while let Some(event) = event_source.next_event() {
        tracing::info!("Event: {:?}", event);
        if let Err(e) = ls_engine.try_claim_ready() {
            tracing::error!("LS claim error: {:?}", e);
        }
        match engine.handle_event(&event) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => tracing::error!("Delta engine error: {:?}", e),
        }
        match ls_engine.handle_event(&event) {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) => tracing::error!("LS engine error: {:?}", e),
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Build the full project**

```bash
cargo build 2>&1 | tail -20
```

Expected: clean build, no warnings about unused imports.

- [ ] **Step 4: Run the full test suite**

```bash
cargo test 2>&1 | tail -30
```

Expected: all existing tests still pass; new `liquid_staking` tests pass.

- [ ] **Step 5: Dry-run smoke test**

If a `.env` file is present and the Kafka broker is reachable:

```bash
just dry-run 2>&1 | head -40
```

Expected: bot starts, logs `LS prices` and `No LS arbitrage opportunity` (or a path if live conditions warrant), then waits for events. If Kafka is unavailable, the timer fallback fires after the configured interval.

- [ ] **Step 6: Commit**

```bash
git add src/bot.rs
git commit -m "feat: wire LsEngine into bot event loop with sCSPR Kafka filtering"
```

---

## Self-Review Notes

- **Spec coverage:** All sections covered — two paths, thresholds (2.5%/5%), gain check (50 CSPR), file-backed claims, `try_claim_ready` before arb check, dry-run mode, Kafka filtering, ContractRefs accessors, contracts-main.toml entries.
- **Placeholders:** None — all code is complete.
- **Type consistency:**
  - `LsPath` used in `data.rs`, `path.rs`, `utils.rs`, `mod.rs` — consistent.
  - `LsPriceData::amount_per_ten_usd` references `super::path::LsPath` — resolved once both files exist.
  - `PendingClaim.claimable_from_ms: u64` — consistent across `claims.rs` and `mod.rs`.
  - `calc_gains_in_cspr` signature matches usage in `check_and_trade`.
- **Known build risk:** `liquid-staking-contracts` uses `odra 2.4.0`; the bot uses `2.5.0`. Cargo should unify to `2.5.0`. If not, pin to a git commit that has been updated, or apply a `[patch]` in `Cargo.toml`.
