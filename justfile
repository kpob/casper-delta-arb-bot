dry-run:
	KAFKA_BOOTSTRAP_SERVERS=127.0.0.1:9092 KAFKA_TOPIC_PRICE_CHANGED=apps.styks KAFKA_TOPIC_TRADE_EXECUTED=apps.casper-trade KAFKA_TIMER_FALLBACK_SECS=6 cargo run --bin  bot -- -c contracts-main.toml scenario Bot --dry-run true

run:
	KAFKA_BOOTSTRAP_SERVERS=kafka:29092 KAFKA_TOPIC_PRICE_CHANGED=apps.styks KAFKA_TOPIC_TRADE_EXECUTED=apps.casper-trade KAFKA_TIMER_FALLBACK_SECS=180 cargo run --bin  bot -- -c contracts-main.toml scenario Bot
	
build:
	cargo build --bin bot