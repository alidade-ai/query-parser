set dotenv-load := true

alias fmt := format

build:
    cargo build

clean:
    cargo clean

format:
    cargo fmt --all

lint:
    cargo clippy -- -D warnings

lint-fix:
    cargo clippy --fix --allow-dirty --allow-staged

dev:
    @just lint
    @just format
    @just test
    @echo "Development checks passed!"
