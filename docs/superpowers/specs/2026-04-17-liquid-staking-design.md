# Liquid Staking Arbitrage — Design Spec

**Date:** 2026-04-17  
**Status:** Approved

## Overview

Extend the bot to detect and execute arbitrage opportunities between the Staked CSPR (sCSPR) price on Casper Trade DEX and its fair redemption value from the StakedCSPR liquid staking contract. The two strategies (Casper Delta and Liquid Staking) run independently on every bot event.

## Background

The existing bot monitors Casper Delta Long/Short position tokens. This feature is purely additive — the existing `BotEngine` and all Casper Delta modules are untouched except for minor wiring changes in `src/bot.rs` and `src/bot/events.rs`.

## Contracts

| Name | Package Hash |
|------|-------------|
| StakedCspr | `hash-d08ca0c6b4d567a671b5a26f51fb78333ffce403544d19a09a93ebb7c04a6a53` |
| WCSPR-StCSPR LP | `hash-99227bb4082ce12f9198651c7eec88dbdb290030da1dfe17cef487bd7d2fe68b` |

New Cargo dependency:
```toml
liquid-staking-contracts = { git = "https://github.com/casper-ecosystem/liquid-staking-contracts", branch = "develop" }
```

## Arbitrage Mechanics

### Fair Price

```
fair_price = staked_cspr() / total_supply()
```

Both values come from the `StakedCSPR` contract. This is analogous to Casper Delta's `liquidity / total_supply` calculation.

### Two Paths

#### StakeAndSell — sCSPR overpriced on DEX

Trigger: `dex_price > fair_price` by more than **2.5%**, estimated gain > **1.0 CSPR**.

Steps (3 transactions, estimated cost **18 CSPR**):
1. Unwrap WCSPR → native CSPR (call `wcspr.withdraw()`)
2. Call `staked_cspr.stake()` with attached native CSPR → receive sCSPR
3. Router swap: `[sCSPR, WCSPR]`

#### BuyAndUnstake — sCSPR underpriced on DEX

Trigger: `dex_price < fair_price` by more than **5.0%**, estimated gain > **5.0 CSPR** (higher buffer absorbs ~16h CSPR price risk during unbonding).

Steps (2 txs to enter + 1 to claim, total estimated cost **19 CSPR**):
1. Router swap: `[WCSPR, sCSPR]`
2. Call `staked_cspr.unstake(scspr_amount)` → initiate unbonding, receive `unstake_id` + `claimable_from` timestamp
3. Persist pending claim to `pending_claims.json`
4. Later (after ~16h): call `staked_cspr.claim()` → receive native CSPR

## Module Layout

```
src/bot/
  liquid_staking/
    mod.rs          — LsEngine
    path.rs         — LsPath enum + threshold logic
    data.rs         — LsPriceData struct
    utils.rs        — LsPriceCalculator
    claims.rs       — PendingClaims (file-backed)
  engine.rs         — existing DeltaEngine, untouched
  path.rs           — untouched
  data.rs           — untouched
  utils.rs          — untouched
  asset_manager.rs  — untouched
  events.rs         — untouched
src/contracts.rs    — add staked_cspr() + wcspr_stcspr_pair() accessors
src/bot.rs          — wire LsEngine alongside DeltaEngine; extend relevant_addresses with sCSPR
contracts-main.toml — add StakedCspr + WCSPR-StCSPR LP entries
```

## LsPath

```rust
pub enum LsPath {
    StakeAndSell,   // sCSPR overpriced: unwrap CSPR → stake → sell sCSPR on DEX
    BuyAndUnstake,  // sCSPR underpriced: buy sCSPR on DEX → initiate unstake → claim
    Empty,
}
```

## LsPriceData

```rust
pub struct LsPriceData {
    pub dex_price: f64,          // sCSPR/WCSPR from DEX pair reserves
    pub fair_price: f64,         // staked_cspr() / total_supply()
    pub diff: f64,               // (dex_price / fair_price) * 100 - 100  (%)
    pub stcspr_for_one_usd: u64,
    pub wcspr_for_one_usd: u64,
    pub wcspr_price: f64,        // USD price, from same Market oracle as Casper Delta
}
```

`LsPriceCalculator` fetches WCSPR price independently from the same Casper Delta Market contract. The two engines remain decoupled at runtime.

## Claim Tracking

`PendingClaims` persists to `pending_claims.json` in the working directory:

```json
[
  {
    "unstake_id": 0,
    "claimable_from": "2026-04-18T12:00:00Z",
    "cspr_amount": "1000000000000"
  }
]
```

- **On startup**: load file; missing file treated as empty.
- **On unstake**: append entry and rewrite file.
- **On each event**: `LsEngine::try_claim_ready()` runs before the arb check. If any entry's `claimable_from` has passed, call `staked_cspr.claim()` (claims all ready unstakes in one tx) and remove those entries.

**Known limitation**: if the process restarts after a claim is attempted but before the file is updated, `claim()` will be called again — the contract handles this gracefully (already-claimed entries are skipped).

## Engine Coordination

```rust
// src/bot.rs — inside the event loop
while let Some(event) = event_source.next_event() {
    ls_engine.try_claim_ready()?;
    delta_engine.handle_event(&event)?;
    ls_engine.handle_event(&event)?;
}
```

`LsEngine::handle_event` mirrors `BotEngine::handle_event`: on `TimerTick | TradeExecuted | PriceChanged` it fetches `LsPriceData`, selects an `LsPath`, estimates gain, and executes if profitable.

## Kafka Filtering

sCSPR token address is added to `relevant_addresses` alongside Long/Short:

```rust
let relevant_addresses = vec![
    contracts.long()?.address().to_string(),
    contracts.short()?.address().to_string(),
    contracts.staked_cspr()?.address().to_string(), // new
];
```

A WCSPR↔sCSPR trade triggers both engines. The Delta engine returns `Path::Empty` immediately (sCSPR is not in its pairs).

## Profit Calculation

Gain in CSPR:

```
gain = (amount_out_cspr - amount_in_cspr) / 1_000_000_000 - tx_cost
```

- **StakeAndSell**: `amount_in` = CSPR staked, `amount_out` = WCSPR received from DEX swap. `tx_cost = 18.0`.
- **BuyAndUnstake**: `amount_in` = WCSPR spent on DEX, `amount_out` = CSPR expected at claim (`sCSPR_bought * fair_price`). `tx_cost = 19.0`.

Swap size is calibrated to ~$1 USD worth of the input token (same approach as Casper Delta).

## Thresholds Summary

| Path | Dev. Threshold | Min Gain | Tx Cost |
|------|---------------|----------|---------|
| StakeAndSell | 2.5% | 1.0 CSPR | 18.0 CSPR |
| BuyAndUnstake | 5.0% | 5.0 CSPR | 19.0 CSPR |

## Dry-Run Mode

`LsEngine` accepts the same `dry_run: bool` flag. In dry-run mode all contract writes (stake, unstake, claim, swap) are no-ops; price fetching and path selection still execute and log.
