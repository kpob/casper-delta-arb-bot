use std::sync::mpsc;
use std::thread::sleep;
use std::time::Duration;

use rdkafka::client::ClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer, ConsumerContext};
use rdkafka::error::KafkaError;
use rdkafka::message::Message;
use rdkafka::types::RDKafkaErrorCode;
use rdkafka::util::Timeout;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionLifecycle {
    pub accepted_at: String,
    pub processed_at: String,
    pub status: String,
    pub sender: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_id: String,
    pub tx_hash: String,
    pub timestamp: String,
    pub lifecycle: TransactionLifecycle,
    pub app_data: serde_json::Value,
}

/// Custom Kafka context that downgrades "unknown topic" errors to debug level.
/// The consumer will keep retrying; topics created later are picked up automatically.
struct BotConsumerContext;

impl ClientContext for BotConsumerContext {
    fn error(&self, error: KafkaError, reason: &str) {
        if matches!(
            error,
            KafkaError::Global(RDKafkaErrorCode::UnknownTopicOrPartition)
                | KafkaError::Global(RDKafkaErrorCode::UnknownPartition)
        ) {
            tracing::debug!("Kafka: topic not yet available ({}), retrying...", reason);
        } else {
            tracing::error!("Kafka error: {} - {}", error, reason);
        }
    }
}

impl ConsumerContext for BotConsumerContext {}

/// Events that can trigger the bot's price-check-and-trade cycle.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum BotEvent {
    /// Periodic timer fired — check prices and trade if profitable.
    TimerTick,
    /// A trade was executed on the DEX.
    TradeExecuted,
    /// A new price was observed for a token.
    PriceChanged,
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

#[allow(dead_code)]
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

/// Configuration for the Kafka event source.
pub struct KafkaConfig {
    pub bootstrap_servers: String,
    pub topic_price_changed: String,
    pub topic_trade_executed: String,
    pub timer_fallback_secs: u64,
}

impl KafkaConfig {
    pub fn from_env() -> Self {
        Self {
            bootstrap_servers: std::env::var("KAFKA_BOOTSTRAP_SERVERS")
                .unwrap_or_else(|_| "localhost:9092".to_string()),
            topic_price_changed: std::env::var("KAFKA_TOPIC_PRICE_CHANGED")
                .unwrap_or_else(|_| "apps.styks".to_string()),
            topic_trade_executed: std::env::var("KAFKA_TOPIC_TRADE_EXECUTED")
                .unwrap_or_else(|_| "apps.casper-trade".to_string()),
            timer_fallback_secs: std::env::var("KAFKA_TIMER_FALLBACK_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60 * 120),
        }
    }
}

/// Consumes Kafka topics and translates messages into `BotEvent`s.
/// A timer fallback thread also emits `TimerTick` at the configured interval.
pub struct KafkaEventSource {
    rx: mpsc::Receiver<BotEvent>,
}

impl KafkaEventSource {
    pub fn new(config: KafkaConfig, relevant_addresses: Vec<String>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self::spawn_consumer(&config, relevant_addresses, tx.clone());
        Self::spawn_timer(config.timer_fallback_secs, tx);
        Self { rx }
    }

    fn spawn_consumer(
        config: &KafkaConfig,
        relevant_addresses: Vec<String>,
        tx: mpsc::Sender<BotEvent>,
    ) {
        let topics = vec![
            config.topic_price_changed.clone(),
            config.topic_trade_executed.clone(),
        ];
        let topic_price_changed = config.topic_price_changed.clone();
        let topic_trade_executed = config.topic_trade_executed.clone();
        let bootstrap_servers = config.bootstrap_servers.clone();

        std::thread::spawn(move || {
            let consumer: BaseConsumer<BotConsumerContext> = ClientConfig::new()
                .set("bootstrap.servers", &bootstrap_servers)
                .set("group.id", "casper-delta-bot")
                .set("auto.offset.reset", "latest")
                .set("enable.auto.commit", "true")
                .create_with_context(BotConsumerContext)
                .expect("Failed to create Kafka consumer");

            let topic_refs: Vec<&str> = topics.iter().map(String::as_str).collect();
            consumer
                .subscribe(&topic_refs)
                .expect("Failed to subscribe to Kafka topics");

            tracing::info!(
                "Kafka consumer subscribed to topics: {:?} on {}",
                topics,
                bootstrap_servers
            );

            loop {
                match consumer.poll(Timeout::After(Duration::from_secs(1))) {
                    Some(Ok(msg)) => {
                        let topic = msg.topic();
                        let payload = msg.payload().unwrap_or_default();
                        tracing::debug!(
                            "Kafka message received on topic '{}': {} bytes",
                            topic,
                            payload.len()
                        );

                        if let Some(event) = Self::message_to_event(
                            topic,
                            payload,
                            &topic_price_changed,
                            &topic_trade_executed,
                            &relevant_addresses,
                        ) {
                            tracing::info!("Kafka event received: {:?}", event);
                            if tx.send(event).is_err() {
                                break;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        tracing::warn!("Kafka consumer error: {}", e);
                    }
                    None => {}
                }
            }
        });
    }

    fn spawn_timer(fallback_secs: u64, tx: mpsc::Sender<BotEvent>) {
        std::thread::spawn(move || loop {
            sleep(Duration::from_secs(fallback_secs));
            tracing::info!("Timer fallback tick ({}s)", fallback_secs);
            if tx.send(BotEvent::TimerTick).is_err() {
                break;
            }
        });
    }

    fn message_to_event(
        topic: &str,
        payload: &[u8],
        topic_price_changed: &str,
        topic_trade_executed: &str,
        relevant_addresses: &[String],
    ) -> Option<BotEvent> {
        if topic == topic_price_changed {
            Some(BotEvent::PriceChanged)
        } else if topic == topic_trade_executed {
            match serde_json::from_slice::<Event>(payload) {
                Ok(event) => {
                    if Self::is_relevant_trade(&event.app_data, relevant_addresses) {
                        Some(BotEvent::TradeExecuted)
                    } else {
                        tracing::debug!(
                            "Trade event ignored: path does not involve Long/Short tokens"
                        );
                        None
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to parse trade event payload: {}", e);
                    None
                }
            }
        } else {
            tracing::warn!("Received message on unknown topic: {}", topic);
            None
        }
    }

    fn is_relevant_trade(app_data: &serde_json::Value, relevant_addresses: &[String]) -> bool {
        let Some(path) = app_data["args"]["path"].as_array() else {
            return false;
        };
        path.iter().any(|addr| {
            addr.as_str()
                .map(|s| relevant_addresses.iter().any(|r| r == s))
                .unwrap_or(false)
        })
    }
}

impl EventSource for KafkaEventSource {
    fn next_event(&mut self) -> Option<BotEvent> {
        self.rx.recv().ok()
    }
}
