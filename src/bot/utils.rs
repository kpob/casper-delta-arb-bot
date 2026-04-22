use odra::{
    casper_types::{U256, U512},
    uints::ToU512,
};

pub(super) fn calculate_price(amount0: U256, amount1: U256) -> f64 {
    (amount0 * U256::from(1_000_000u64) / amount1).as_u64() as f64 / 1_000_000.0
}

/// Same as `calculate_price` but accepts U512 as the numerator (for
/// `staked_cspr()` which returns U512).
pub(super) fn calculate_price_u512(amount0: U512, amount1: U256) -> f64 {
    let scaled = amount0 * U512::from(1_000_000u64) / amount1.to_u512();
    scaled.as_u64() as f64 / 1_000_000.0
}
