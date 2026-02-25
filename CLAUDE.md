# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

An automated arbitrage bot for the Casper blockchain that monitors price differences between Long/Short position tokens (from Casper Delta) and their fair prices, then executes profitable swap paths via Casper Trade DEX.

Requires **nightly Rust toolchain** (see `rust-toolchain`: `nightly-2025-01-01`).

## Commands

```bash
# Setup approvals before first run
cargo run --bin bot -- -c contracts-main.toml scenario BotSetup

# Run bot (live, executes real swaps)
just run
# or: cargo run --bin bot -- -c contracts-main.toml scenario Bot

# Dry-run (no swaps executed)
just dry-run
# or: cargo run --bin bot -- -c contracts-main.toml scenario Bot --dry-run true

# Run tests
cargo test
cargo test -- --test-threads=1   # if tests interfere

# Run a single test
cargo test <test_name>
```

## Architecture

The bot uses the [Odra framework](https://github.com/odradev/odra) for Casper contract interaction. Scenarios (`BotSetup`, `Bot`) are Odra CLI entry points registered in `src/bin/cli.rs`.

### Bot Loop (`src/bot/bot.rs`)
Runs every 180 seconds:
1. Fetch current DEX prices and fair prices from on-chain state
2. Calculate `Path` based on price deviations
3. If estimated profit > 1.0 CSPR, execute the swap path via Router

### Key Modules

- **`src/bot/path.rs`** — `Path` enum with 7 variants (multi-hop and single-hop arbitrage routes). `Path::calculate()` applies a **2.5% threshold** to determine if a price deviation is actionable. Multi-hop paths (both tokens mispriced) have priority over single-hop.

- **`src/bot/asset_manager.rs`** — Trait-based token management (`Balances`, `TokenManager`). `AssetManager` ensures sufficient balances before swaps, auto-topping up 2,000 CSPR when needed. Uses `mockall` for unit testing with mock implementations.

- **`src/bot/utils.rs`** — `PriceCalculator`: fetches reserves from DEX pairs (`casper_trade_prices()`), fetches fair prices from Casper Delta market state (`fair_prices()`), and calculates profit in CSPR (`calc_gains_in_cspr()`). Transaction cost assumptions: 12.5 CSPR (multi-hop), 7.0 CSPR (single-hop).

- **`src/bot/data.rs`** — `PriceData` struct capturing all market prices, fair prices, deviations, and conversion ratios. Implements `Display` for formatted output.

- **`src/bot/contracts.rs`** — `ContractRefs` wrapper that retrieves deployed contracts (Router, Pairs, Market, WCSPR, Position tokens) from `ContractProvider`.

### External Dependencies

The bot depends on **local sibling repositories** referenced by path in `Cargo.toml`:
- `../casper-delta/casper-delta-contracts` — Market and position token contracts
- `../casper-trade/casper_trade_contracts` — DEX Router, Factory, Pair contracts

These sibling repos must exist on disk for the project to compile.

### Contract Addresses

Deployed contract package hashes are stored in `contracts-main.toml` (Casper mainnet). Environment config (node address, secret key path, chain name) is in `.env` (gitignored).

### Key Thresholds

| Parameter | Value |
|-----------|-------|
| Price diff threshold | 2.5% |
| Minimum profit to execute | 1.0 CSPR |
| Balance top-up amount | 2,000 CSPR |
| Loop interval | 180 seconds |
