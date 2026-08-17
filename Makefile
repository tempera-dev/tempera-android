.PHONY: test lint companion

test:
	cargo test --workspace

lint:
	cargo fmt --check
	cargo clippy --workspace --all-targets -- -D warnings

companion:
	bash scripts/build-companion.sh
