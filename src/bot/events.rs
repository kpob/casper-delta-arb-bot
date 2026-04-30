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

/// Identifies the strategy domain a trade event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeScope {
    /// Trade involving Casper Delta Long/Short position tokens.
    Delta,
    /// Trade involving the staked-CSPR / CSPR pool.
    LiquidStaking,
}

/// Events that can trigger the bot's price-check-and-trade cycle.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum BotEvent {
    /// Periodic timer fired — check prices and trade if profitable.
    TimerTick,
    /// A trade was executed on the DEX within the given scope.
    TradeExecuted {
        scope: TradeScope,
        tx_hash: Option<String>,
    },
    /// A new price was observed within the given scope.
    PriceChanged {
        scope: TradeScope,
        tx_hash: Option<String>,
    },
    /// Graceful shutdown requested.
    Shutdown,
}

impl BotEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            BotEvent::TimerTick => "timer",
            BotEvent::TradeExecuted { .. } => "trade",
            BotEvent::PriceChanged { .. } => "price",
            BotEvent::Shutdown => "shutdown",
        }
    }

    pub fn tx_hash(&self) -> Option<&str> {
        match self {
            BotEvent::TradeExecuted { tx_hash, .. } | BotEvent::PriceChanged { tx_hash, .. } => {
                tx_hash.as_deref()
            }
            _ => None,
        }
    }
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
        tracing::debug!("Sleeping for {} seconds...", self.interval.as_secs());
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
    pub fn new(config: KafkaConfig, scoped_addresses: Vec<(String, TradeScope)>) -> Self {
        let (tx, rx) = mpsc::channel();
        Self::spawn_consumer(&config, scoped_addresses, tx.clone());
        Self::spawn_timer(config.timer_fallback_secs, tx);
        Self { rx }
    }

    fn spawn_consumer(
        config: &KafkaConfig,
        scoped_addresses: Vec<(String, TradeScope)>,
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
                .inspect_err(|e| {
                    tracing::error!(
                        bootstrap_servers = %bootstrap_servers,
                        error = ?e,
                        "Kafka consumer creation failed"
                    );
                })
                .expect("Failed to create Kafka consumer");

            let topic_refs: Vec<&str> = topics.iter().map(String::as_str).collect();
            consumer
                .subscribe(&topic_refs)
                .inspect_err(|e| {
                    tracing::error!(
                        topics = ?topics,
                        error = ?e,
                        "Kafka subscribe failed"
                    );
                })
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

                        let events = Self::messages_to_events(
                            topic,
                            payload,
                            &topic_price_changed,
                            &topic_trade_executed,
                            &scoped_addresses,
                        );
                        for event in events {
                            tracing::debug!("Kafka event received: {:?}", event);
                            if tx.send(event).is_err() {
                                tracing::info!("Kafka consumer thread exiting: channel closed");
                                return;
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
            tracing::debug!("Timer fallback tick ({}s)", fallback_secs);
            if tx.send(BotEvent::TimerTick).is_err() {
                tracing::info!("Timer thread exiting: channel closed");
                break;
            }
        });
    }

    fn messages_to_events(
        topic: &str,
        payload: &[u8],
        topic_price_changed: &str,
        topic_trade_executed: &str,
        scoped_addresses: &[(String, TradeScope)],
    ) -> Vec<BotEvent> {
        if topic == topic_price_changed {
            let tx_hash = parse_event_meta(payload);
            vec![BotEvent::PriceChanged {
                scope: TradeScope::Delta,
                tx_hash,
            }]
        } else if topic == topic_trade_executed {
            match serde_json::from_slice::<Event>(payload) {
                Ok(event) => {
                    let scopes = Self::matched_scopes(&event.app_data, scoped_addresses);
                    tracing::debug!("{:?}", event.app_data);
                    if scopes.is_empty() {
                        let path: Vec<&str> = event.app_data["args"]["path"]
                            .as_array()
                            .map(|arr| arr.iter().filter_map(|v| v.as_str()).take(8).collect())
                            .unwrap_or_default();
                        tracing::info!(
                            tx_hash = %event.tx_hash,
                            path = ?path,
                            "trade event ignored: path does not involve tracked tokens"
                        );
                    }
                    scopes
                        .into_iter()
                        .map(|scope| BotEvent::TradeExecuted {
                            scope,
                            tx_hash: Some(event.tx_hash.clone()),
                        })
                        .collect()
                }
                Err(e) => {
                    tracing::warn!("Failed to parse trade event payload: {}", e);
                    Vec::new()
                }
            }
        } else {
            tracing::warn!("Received message on unknown topic: {}", topic);
            Vec::new()
        }
    }

    fn matched_scopes(
        app_data: &serde_json::Value,
        scoped_addresses: &[(String, TradeScope)],
    ) -> Vec<TradeScope> {
        let Some(path) = app_data["args"]["path"].as_array() else {
            return Vec::new();
        };
        let mut scopes: Vec<TradeScope> = Vec::new();
        for addr in path.iter().filter_map(|a| a.as_str()) {
            for (tracked, scope) in scoped_addresses {
                if tracked == addr && !scopes.contains(scope) {
                    scopes.push(*scope);
                }
            }
        }
        scopes
    }
}

impl EventSource for KafkaEventSource {
    fn next_event(&mut self) -> Option<BotEvent> {
        self.rx.recv().ok()
    }
}

fn parse_event_meta(payload: &[u8]) -> Option<String> {
    serde_json::from_slice::<Event>(payload)
        .ok()
        .map(|e| e.tx_hash)
}
