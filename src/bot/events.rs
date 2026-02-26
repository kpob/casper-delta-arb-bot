use std::thread::sleep;
use std::time::Duration;

/// Events that can trigger the bot's price-check-and-trade cycle.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum BotEvent {
    /// Periodic timer fired — check prices and trade if profitable.
    TimerTick,
    /// A trade was executed on the DEX (future: from SSE/WebSocket stream).
    TradeExecuted { pair: String },
    /// Significant price movement detected on-chain (future: from node events).
    PriceChanged { token: String },
    /// Graceful shutdown requested.
    Shutdown,
}

/// A source of events for the bot.
/// Returning `None` signals the bot should stop.
pub trait EventSource {
    fn next_event(&mut self) -> Option<BotEvent>;
}

/// Emits `TimerTick` events at a fixed interval.
/// The first event is emitted immediately.
pub struct TimerEventSource {
    interval: Duration,
    first: bool,
}

impl TimerEventSource {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            first: true,
        }
    }
}

impl EventSource for TimerEventSource {
    fn next_event(&mut self) -> Option<BotEvent> {
        if self.first {
            self.first = false;
            return Some(BotEvent::TimerTick);
        }
        tracing::info!("Sleeping for {} seconds...", self.interval.as_secs());
        sleep(self.interval);
        Some(BotEvent::TimerTick)
    }
}
