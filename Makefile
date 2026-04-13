.PHONY: fmt fmt-check test test-postgres fixture-check clippy package-check verify verify-release api-docs

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

test:
	cargo test --all-targets

test-postgres:
	cargo test --test database_acceptance

fixture-check:
	cargo test --test blueprint_fixture_acceptance
	cargo test --test plugin_fixture_acceptance

clippy:
	cargo clippy --all-targets -- -D warnings

package-check:
	cargo package --allow-dirty

verify: fmt-check test clippy fixture-check

verify-release: verify package-check

api-docs:
	cargo doc --no-deps
	cargo run --manifest-path tools/forge-api-doc/Cargo.toml -- --output-dir docs/api
