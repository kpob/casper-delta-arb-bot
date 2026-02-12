dry-run:
	cargo run --bin  bot -- -c contracts-main.toml scenario Bot --dry-run true

run:
	cargo run --bin  bot -- -c contracts-main.toml scenario Bot
	