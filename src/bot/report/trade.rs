use std::fmt::Debug;

use crate::bot::utils::round_2dp;

pub fn executed_delta<P: Debug>(
    path: &P,
    long_diff_pct: f64,
    short_diff_pct: f64,
    predicted_cspr: f64,
    actual_cspr: f64,
) {
    tracing::info!(
        path = ?path,
        long_diff_pct = round_2dp(long_diff_pct),
        short_diff_pct = round_2dp(short_diff_pct),
        predicted_cspr = round_2dp(predicted_cspr),
        actual_cspr = round_2dp(actual_cspr),
        slippage_cspr = round_2dp(predicted_cspr - actual_cspr),
        is_loss = actual_cspr < 0.0,
        "delta.trade_executed"
    );
}

pub fn executed_ls<P: Debug>(path: &P, st_diff_pct: f64, predicted_cspr: f64, actual_cspr: f64) {
    tracing::info!(
        path = ?path,
        st_diff_pct = round_2dp(st_diff_pct),
        predicted_cspr = round_2dp(predicted_cspr),
        actual_cspr = round_2dp(actual_cspr),
        slippage_cspr = round_2dp(predicted_cspr - actual_cspr),
        is_loss = actual_cspr < 0.0,
        "ls.trade_executed"
    );
}
