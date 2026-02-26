use casper_delta_bot::{
    Bot, UnwrapWcspr, CD_LONG_ID, CD_SHORT_ID, LP_LONG_WCSPR_ID, LP_WCSPR_SHORT_ID,
};
use casper_delta_contracts::{
    market::Market, position_token::PositionToken, wrapped_native::WrappedNativeToken,
};
use casper_trade_contracts::{
    factory::Factory,
    pair::{Pair, PairFactory},
    router::Router,
};
use odra_cli::OdraCli;
use styks_contracts::styks_price_feed::StyksPriceFeed;

/// Main function to run the CLI tool.
pub fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_ansi(false)
        .init();
    OdraCli::new()
        .about("Casper Delta CLI Tool")
        .contract::<StyksPriceFeed>()
        .contract::<Market>()
        .named_contract::<PositionToken>(CD_LONG_ID.to_string())
        .named_contract::<PositionToken>(CD_SHORT_ID.to_string())
        .contract::<WrappedNativeToken>()
        .contract::<Router>()
        .contract::<Factory>()
        .contract::<PairFactory>()
        .named_contract::<Pair>(LP_LONG_WCSPR_ID.to_string())
        .named_contract::<Pair>(LP_WCSPR_SHORT_ID.to_string())
        .scenario(Bot)
        .scenario(UnwrapWcspr)
        .build()
        .run();
}
