include discord.env
export

run:
	@set RUST_LOG=info,tauri=info&&cargo tauri dev

run-debug:
	@set RUST_LOG=debug,tauri=debug&& cargo tauri dev

# tauri-cli is only installed when missing: `cargo install` always resolves the newest 2.x, whose
# MSRV can be ahead of the local rustc, and that failure used to abort before `npm install` ran.
deps:
	@cargo tauri --version >NUL 2>&1 || cargo install tauri-cli --version "^2.0.0" --locked
	@cd src-tauri && cd frontend && npm install
	
build-installer-windows:
	@cd src-tauri && cargo tauri build && cargo tauri bundle

clean:
	@cd src-tauri && cargo clean

# --workspace matters: without it cargo only looks at the root DS2000 package. The members are
# compiled as dependencies but never linted, and none of their tests run -- `make test` reported
# success while executing zero tests. These must stay in step with .github/workflows/ci.yml.
lint:
	@cd src-tauri && cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	@cd src-tauri && cargo test --workspace -- --nocapture

# Tests needing the DS-2000 plugged in or Discord running; they are #[ignore]d so CI stays green.
test-hardware:
	@cd src-tauri && cargo test --workspace -- --ignored --nocapture

test-discord:
	@cd src-tauri && cargo test -p discord
