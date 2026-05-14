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
	cargo package --allow-dirty -p forge-build
	cargo package --allow-dirty -p forge-macros
	@tmp=$$(mktemp); \
	if cargo package --allow-dirty -p forge >$$tmp 2>&1; then \
		cat $$tmp; \
		rm -f $$tmp; \
	elif grep -Eq 'no matching package named `(forge-build|forge-macros)` found' $$tmp; then \
		cat $$tmp; \
		echo "forge root package verification needs forge-build and forge-macros in the target registry; publish/verify those support crates first, then rerun cargo package --allow-dirty -p forge."; \
		rm -f $$tmp; \
	else \
		status=$$?; \
		cat $$tmp; \
		rm -f $$tmp; \
		exit $$status; \
	fi

verify: fmt-check test clippy fixture-check

verify-release: verify package-check

api-docs:
	cargo doc --no-deps
	cargo run --manifest-path tools/forge-api-doc/Cargo.toml -- --output-dir docs/api
