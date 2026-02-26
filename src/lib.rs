pub const CD_LONG_ID: &str = "CD_LONG";
pub const CD_SHORT_ID: &str = "CD_SHORT";
pub const LP_LONG_WCSPR_ID: &str = "CD_LONG-WCSPR LP";
pub const LP_WCSPR_SHORT_ID: &str = "WCSPR-CD_SHORT LP";
mod bot;
mod contracts;
mod unwrap_wcspr;

pub use bot::Bot;
pub use unwrap_wcspr::UnwrapWcspr;
