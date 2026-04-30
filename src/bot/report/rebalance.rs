use crate::bot::rebalancer::{Asset, RebalanceOp};

/// Snapshot of all five tracked balances in whole-token units. Embedded in
/// every `rebalance_executed` event so a single Telegram message tells the
/// operator the post-action state without correlating across log lines.
#[derive(Debug, Clone, Copy)]
pub struct BalanceSnapshot {
    pub cspr: f64,
    pub wcspr: f64,
    pub stcspr: f64,
    pub long: f64,
    pub short: f64,
}

pub fn executed_asset(
    direction: &str,
    asset: Asset,
    pre_balance: f64,
    target_midpoint: f64,
    ops: &[RebalanceOp],
    snapshot: BalanceSnapshot,
) {
    let action = format!("{direction}_{}", asset.slug());
    tracing::info!(
        action = %action,
        asset = ?asset,
        pre_balance,
        target_midpoint,
        ops = ?ops,
        cspr_balance = snapshot.cspr,
        wcspr_balance = snapshot.wcspr,
        stcspr_balance = snapshot.stcspr,
        long_balance = snapshot.long,
        short_balance = snapshot.short,
        "rebalance_executed"
    );
}

pub fn executed_claim(elapsed_secs: u64, snapshot: BalanceSnapshot) {
    let ops = [RebalanceOp::Claim];
    tracing::info!(
        action = "claim_matured",
        elapsed_secs,
        ops = ?ops,
        cspr_balance = snapshot.cspr,
        wcspr_balance = snapshot.wcspr,
        stcspr_balance = snapshot.stcspr,
        long_balance = snapshot.long,
        short_balance = snapshot.short,
        "rebalance_executed"
    );
}

pub fn pending_unstake_cleared(elapsed_secs: u64) {
    tracing::info!(elapsed_secs, "pending unstake cleared");
}

pub fn cant_fill_wcspr() {
    tracing::warn!("cannot fill WCSPR: CSPR source below floor");
}

pub fn cant_fill_stcspr() {
    tracing::warn!("cannot fill stCSPR: CSPR source below floor");
}

pub fn cant_fill_position() {
    tracing::warn!("cannot fill: WCSPR source below floor");
}

pub fn cant_drain_cspr() {
    tracing::warn!("cannot drain CSPR: both WCSPR and stCSPR at/above max");
}

pub fn cant_unstake_pending() {
    tracing::warn!("cannot unstake: prior unstake pending");
}
