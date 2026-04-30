//! Centralized log-call vocabulary.
//!
//! Each function in the submodules wraps a single `tracing::info!` /
//! `warn!` / `error!` and encodes the level once — call sites do not choose.
//! Generic over `Debug` where strategy / op types leak in, so the `report`
//! module stays decoupled from concrete types.

pub mod on_chain;
pub mod rebalance;
pub mod trade;
