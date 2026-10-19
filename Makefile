.PHONY: check lint typecheck test-rust build test-ts validate

# Everything Jensen verifies a plan with. The order matters: test-ts reads the backend.wasm that build writes.
check: lint typecheck test-rust build test-ts validate

# A fresh worktree installs its own dependencies. The touch makes the folder newer than its inputs.
node_modules: package.json bun.lock
	bun install --frozen-lockfile
	touch node_modules

lint: node_modules
	bunx biome check --error-on-warnings .
	cargo fmt --manifest-path backend/Cargo.toml -- --check
	cargo clippy --manifest-path backend/Cargo.toml --all-targets -- -D warnings

typecheck: node_modules
	bunx tsc --noEmit

test-rust:
	cargo test --manifest-path backend/Cargo.toml

# The release build needs the wasm target; adding it is a no-op once it is there.
build: node_modules
	rustup target add wasm32-unknown-unknown
	bunx jensen-plugin build

test-ts: node_modules build
	bun test ./test

validate: node_modules
	bunx jensen-plugin validate
