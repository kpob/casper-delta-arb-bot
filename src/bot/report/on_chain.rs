use std::fmt::Debug;

pub fn calling(op: &str, amount: f64, gas_cspr: u64) {
    tracing::info!(op, amount, gas_cspr, "calling on-chain");
}

pub fn failed<E: Debug>(op: &str, amount: f64, error: &E) {
    tracing::error!(op, amount, error = ?error, "on-chain op failed");
}

pub fn calling_claim(gas_cspr: u64) {
    tracing::info!(op = "claim", gas_cspr, "calling on-chain");
}

pub fn failed_claim<E: Debug>(error: &E) {
    tracing::error!(op = "claim", error = ?error, "claim failed");
}
