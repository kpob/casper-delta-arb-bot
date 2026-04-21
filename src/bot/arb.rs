// // //! Arbitrage detection between a native protocol's fair price and a DEX
// // //! reached through an opaque quoter (single- or multi-hop).
// // //!
// // //! Assumes all assets share a common decimal scale (9) and that `quote` is
// // //! USD-pegged. Math is in f64 human units; if the caller needs on-chain
// // //! calldata, they convert at the boundary.

// // use std::collections::HashMap;

// // use odra::{casper_types::U256, prelude::{Address, Addressable}};
// // use odra_cli::scenario;

// // use crate::contracts::ContractRefs;

// // // ── Native protocol: fair-price abstraction ─────────────────────────────────

// // /// Fair value of `base` denominated in `quote`.
// // pub trait FairPriceOracle {
// //     fn fair_price(&self, base: &str, quote: &str) -> Option<f64>;
// // }

// // #[derive(Default)]
// // pub struct StaticOracle {
// //     prices: HashMap<(String, String), f64>,
// // }

// // impl StaticOracle {
// //     pub fn new() -> Self { Self::default() }
// //     pub fn set(&mut self, base: &str, quote: &str, price: f64) {
// //         self.prices.insert((base.into(), quote.into()), price);
// //     }
// // }

// // impl FairPriceOracle for StaticOracle {
// //     fn fair_price(&self, base: &str, quote: &str) -> Option<f64> {
// //         self.prices.get(&(base.into(), quote.into())).copied()
// //     }
// // }

// // // ── DEX: opaque quoter ──────────────────────────────────────────────────────

// // /// Output amount for swapping `amount_in` of `token_in` into `token_out`.
// // /// The implementor picks and executes the route (single pool, multi-hop,
// // /// split across venues — up to the caller). Returns `None` when no route
// // /// exists or the underlying call fails.
// // pub trait DexQuoter {
// //     fn amount_out(&self, token_in: Address, token_out: Address, amount_in: U256) -> Result<U256, scenario::Error>;
// // }

// // // ── Costs and results ───────────────────────────────────────────────────────

// // /// Flat per-cycle cost (DEX gas + native-side tx, relayer tip, etc.) plus a
// // /// variable bps fee charged on notional by the native protocol.
// // pub struct CostModel {
// //     pub gas_cost_usd: f64,
// //     pub protocol_fee_bps: f64,
// // }

// // impl CostModel {
// //     pub fn total(&self, notional_usd: f64) -> f64 {
// //         self.gas_cost_usd + notional_usd * self.protocol_fee_bps / 10_000.0
// //     }
// // }

// // #[derive(Debug, Clone, Copy, PartialEq, Eq)]
// // pub enum ArbDirection {
// //     /// DEX undervalues `base`: buy `base` on DEX, unwind at protocol fair.
// //     BuyOnDex,
// //     /// DEX overvalues `base`: source `base` at fair, sell on DEX.
// //     SellOnDex,
// // }

// // #[derive(Debug)]
// // pub struct ArbOpportunity {
// //     pub direction: ArbDirection,
// //     pub fair_price: f64,
// //     pub dex_effective_price: f64,
// //     pub trade_size_usd: f64,
// //     pub gross_profit_usd: f64,
// //     pub total_cost_usd: f64,
// //     pub net_profit_usd: f64,
// //     pub edge_bps: f64,
// // }

// // // ── Per-leg P&L (shared by fixed-size and optimizer) ────────────────────────

// // /// Net P&L of a single leg at a given USD notional. Returns `None` when the
// // /// quoter can't route this pair.
// // ///
// // /// Gross profit is computed in quote units and converted to USD by
// // /// `usd_per_quote`. This works whether or not `quote` is USD-pegged: if it
// // /// is, `usd_per_quote == 1` and the formula collapses to the stablecoin case.
// // fn evaluate_leg<Q: DexQuoter>(
// //     quoter: &Q,
// //     direction: ArbDirection,
// //     base: Address,
// //     quote: Address,
// //     fair: f64,
// //     usd_per_quote: f64,
// //     trade_size_usd: f64,
// //     costs: &CostModel,
// // ) -> Option<(f64 /*net*/, f64 /*gross*/, f64 /*dex_px*/)> {
// //     let total_cost = costs.total(trade_size_usd);
// //     match direction {
// //         ArbDirection::BuyOnDex => {
// //             // Spend `trade_size_usd` of quote on DEX → base → unwind at fair.
// //             let quote_in = trade_size_usd / usd_per_quote;
// //             let base_out = quoter.amount_out(quote, base, quote_in)?;
// //             let dex_px = quote_in / base_out;
// //             let gross_usd = (base_out * fair - quote_in) * usd_per_quote;
// //             Some((gross_usd - total_cost, gross_usd, dex_px))
// //         }
// //         ArbDirection::SellOnDex => {
// //             // Source base at fair with `trade_size_usd` → sell on DEX.
// //             // 1 base = `fair` quote = `fair * usd_per_quote` USD.
// //             let base_in = trade_size_usd / (fair * usd_per_quote);
// //             let quote_out = quoter.amount_out(base, quote, base_in)?;
// //             let dex_px = quote_out / base_in;
// //             let gross_usd = (quote_out - base_in * fair) * usd_per_quote;
// //             Some((gross_usd - total_cost, gross_usd, dex_px))
// //         }
// //     }
// // }

// // fn edge_bps(direction: ArbDirection, fair: f64, dex_px: f64) -> f64 {
// //     match direction {
// //         ArbDirection::BuyOnDex => (fair - dex_px) / fair * 10_000.0,
// //         ArbDirection::SellOnDex => (dex_px - fair) / fair * 10_000.0,
// //     }
// // }

// // // ── Numerical sizing ────────────────────────────────────────────────────────

// // /// Golden-section search for the maximum of a unimodal function on [a, b].
// // /// `f` is called O(log((b-a)/tol)) times — ~30–40 calls for cent-level
// // /// precision on sizes up to $10M. Each call is a quoter query (a contract
// // /// read), so budget accordingly.
// // fn golden_section_max<F: FnMut(f64) -> f64>(mut f: F, mut a: f64, mut b: f64, tol: f64) -> f64 {
// //     let r = (5.0_f64.sqrt() - 1.0) / 2.0; // ≈ 0.618
// //     let (mut x1, mut x2) = (b - r * (b - a), a + r * (b - a));
// //     let (mut f1, mut f2) = (f(x1), f(x2));
// //     while b - a > tol {
// //         if f1 >= f2 {
// //             b = x2;  x2 = x1;  f2 = f1;
// //             x1 = b - r * (b - a);
// //             f1 = f(x1);
// //         } else {
// //             a = x1;  x1 = x2;  f1 = f2;
// //             x2 = a + r * (b - a);
// //             f2 = f(x2);
// //         }
// //     }
// //     (a + b) / 2.0
// // }

// // /// Find the profit-maximizing USD notional in each direction numerically,
// // /// then return the better leg — provided it clears `min_profit_usd`.
// // ///
// // /// Net profit is unimodal in size (composition of concave AMM curves minus
// // /// linear cost), so golden-section converges on the peak without needing
// // /// reserves, gradients, or knowledge of the path length.
// // ///
// // /// `max_size_usd` bounds the search — set it from bankroll, liquidity
// // /// assumptions, or position limits. If the optimum returned equals
// // /// `max_size_usd`, the true peak is likely beyond your bound.
// // pub fn find_optimal_arbitrage<O: FairPriceOracle, Q: DexQuoter>(
// //     oracle: &O,
// //     quoter: &Q,
// //     base: &str,
// //     quote: &str,
// //     costs: &CostModel,
// //     min_profit_usd: f64,
// //     max_size_usd: f64,
// // ) -> Option<ArbOpportunity> {
// //     let fair = oracle.fair_price(base, quote)?;
// //     let usd_per_quote = oracle.fair_price(quote, "USD")?;
// //     const TOL: f64 = 0.01; // 1 cent

// //     let best = [ArbDirection::BuyOnDex, ArbDirection::SellOnDex]
// //         .into_iter()
// //         .map(|dir| {
// //             let size = golden_section_max(
// //                 |x| evaluate_leg(quoter, dir, base, quote, fair, usd_per_quote, x, costs)
// //                     .map(|(net, _, _)| net)
// //                     .unwrap_or(f64::NEG_INFINITY),
// //                 0.0,
// //                 max_size_usd,
// //                 TOL,
// //             );
// //             (dir, size)
// //         })
// //         .filter_map(|(dir, size)| {
// //             let (net, gross, dex_px) = evaluate_leg(quoter, dir, base, quote, fair, usd_per_quote, size, costs)?;
// //             Some(ArbOpportunity {
// //                 direction: dir,
// //                 fair_price: fair,
// //                 dex_effective_price: dex_px,
// //                 trade_size_usd: size,
// //                 gross_profit_usd: gross,
// //                 total_cost_usd: costs.total(size),
// //                 net_profit_usd: net,
// //                 edge_bps: edge_bps(dir, fair, dex_px),
// //             })
// //         })
// //         .max_by(|a, b| a.net_profit_usd.partial_cmp(&b.net_profit_usd).unwrap())?;

// //     (best.net_profit_usd >= min_profit_usd).then_some(best)
// // }

// // pub struct DexPool<'a> {
// //     pub refs: &'a ContractRefs<'a>,
// //     pub token0: Address,
// //     pub token1: Address,
// //     pub reserve0: U256,
// //     pub reserve1: U256,
// // }

// // impl<'a> DexQuoter for DexPool<'a> {
// //      fn amount_out(&self, token_in: Address, token_out: Address, amount_in: U256) -> Result<U256, scenario::Error> {
// //         let router = self.refs.router()?;

// //         let (r_in, r_out) = match (token_in, token_out) {
// //             (a, b) if a == self.token0 && b == self.token1 => (self.reserve0, self.reserve1),
// //             (a, b) if a == self.token1 && b == self.token0 => (self.reserve1, self.reserve0),
// //             _ => return Err(scenario::Error::OdraError { message: format!("Invalid pair {:?}-{:?}", token_in, token_out) }),
// //         };
// //         Ok(router.get_amount_out(amount_in, r_in, r_out))
// //     }
// // }

// // // ── Demo ────────────────────────────────────────────────────────────────────

// // // fn main() {
// // //     let mut oracle = StaticOracle::new();
// // //     oracle.set("WETH", "USDC", 2000.0);

// // //     let quoter = MockUniV2Pool {
// // //         token0: "WETH".into(),
// // //         token1: "USDC".into(),
// // //         reserve0: 500.0,
// // //         reserve1: 950_000.0,
// // //         fee_bps: 30,
// // //     };

// // //     let costs = CostModel { gas_cost_usd: 25.0, protocol_fee_bps: 5.0 };
// // //     let min_profit_usd = 50.0;
// // //     let max_size_usd = 1_000_000.0;

// // //     let print = |label: &str, op: Option<ArbOpportunity>| {
// // //         println!("── {label} ──");
// // //         match op {
// // //             Some(op) => {
// // //                 println!("  direction       : {:?}", op.direction);
// // //                 println!("  fair price      : {:.4} USDC/WETH", op.fair_price);
// // //                 println!("  dex exec price  : {:.4} USDC/WETH", op.dex_effective_price);
// // //                 println!("  trade size      : ${:.2}", op.trade_size_usd);
// // //                 println!("  gross profit    : ${:.2}", op.gross_profit_usd);
// // //                 println!("  costs           : ${:.2}", op.total_cost_usd);
// // //                 println!("  net profit      : ${:.2}", op.net_profit_usd);
// // //                 println!("  edge            : {:.1} bps", op.edge_bps);
// // //             }
// // //             None => println!("  no arb clearing the ${:.2} threshold.", min_profit_usd),
// // //         }
// // //     };

// // //     print("fixed $10k size",
// // //           find_arbitrage(&oracle, &quoter, "WETH", "USDC", 10_000.0, &costs, min_profit_usd));
// // //     print("optimal size (golden section)",
// // //           find_optimal_arbitrage(&oracle, &quoter, "WETH", "USDC", &costs, min_profit_usd, max_size_usd));
// // // }

// use odra::{casper_types::U256, prelude::Addressable};
// use odra_cli::scenario::Error;

// use super::path::Path;
// use crate::{bot::casper_delta::data::PriceData, contracts::ContractRefs};

// pub struct PriceCalculator<'a> {
//     contracts: &'a ContractRefs<'a>,
//     reserves0: U256,
//     reserves1: U256
// }

// impl<'a> PriceCalculator<'a> {
//     pub fn new(reserves) -> Self {
//         Self { contracts }
//     }

//     pub fn casper_trade_prices(&self) -> Result<(f64, f64), Error> {
//         let (reserves_long, reserves_wcspr_long, _) =
//             self.contracts.long_wcspr_pair()?.get_reserves();
//         let (reserves_wcspr_short, reserves_short, _) =
//             self.contracts.wcspr_short_pair()?.get_reserves();

//         let long_token_price = Self::calculate_price(reserves_wcspr_long, reserves_long);
//         let short_token_price = Self::calculate_price(reserves_wcspr_short, reserves_short);

//         Ok((long_token_price, short_token_price))
//     }

//     pub fn fair_prices(&self) -> Result<(f64, f64, f64), Error> {
//         let market = self.contracts.market()?;
//         let state = market
//             .try_get_address_market_state(market.address())?
//             .market_state;
//         let long_token_price = Self::calculate_price(state.long_liquidity, state.long_total_supply);
//         let short_token_price =
//             Self::calculate_price(state.short_liquidity, state.short_total_supply);
//         let wcspr_price = state.price().as_u64() as f64 / 100_000.0f64;

//         Ok((long_token_price, short_token_price, wcspr_price))
//     }

//     fn calculate_price(amount0: U256, amount1: U256) -> f64 {
//         (amount0 * U256::from(1_000_000) / amount1).as_u64() as f64 / 1000_000.0f64
//     }

//     pub fn calc_gains_in_cspr(
//         amount_in: U256,
//         amount_out: U256,
//         price_data: &PriceData,
//         path: Path,
//     ) -> f64 {
//         let average_transaction_cost = if path.is_multi_hop() { 12.5f64 } else { 7.0f64 };
//         let (amount_in_cspr, amount_out_cspr) = match path {
//             Path::LongWcsprShort => (
//                 amount_in.as_u64() as f64 * price_data.long_fair_price,
//                 amount_out.as_u64() as f64 * price_data.short_fair_price,
//             ),
//             Path::ShortWcsprLong => (
//                 amount_in.as_u64() as f64 * price_data.short_fair_price,
//                 amount_out.as_u64() as f64 * price_data.long_fair_price,
//             ),
//             Path::LongWcspr => (
//                 amount_in.as_u64() as f64 * price_data.long_fair_price,
//                 amount_out.as_u64() as f64,
//             ),
//             Path::ShortWcspr => (
//                 amount_in.as_u64() as f64 * price_data.short_fair_price,
//                 amount_out.as_u64() as f64,
//             ),
//             Path::WcsprLong => (
//                 amount_in.as_u64() as f64,
//                 amount_out.as_u64() as f64 * price_data.long_fair_price,
//             ),
//             Path::WcsprShort => (
//                 amount_in.as_u64() as f64,
//                 amount_out.as_u64() as f64 * price_data.short_fair_price,
//             ),
//             Path::Empty => return 0.0f64,
//         };
//         (amount_out_cspr - amount_in_cspr) / 1_000_000_000.0f64 - average_transaction_cost
//     }
// }
