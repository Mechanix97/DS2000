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
	
# The release build signs the MSI for the updater (#30) and needs the private key. Without
# TAURI_SIGNING_PRIVATE_KEY set, a local build skips the signature instead of failing on it.
build-installer-windows:
ifdef TAURI_SIGNING_PRIVATE_KEY
	@cd src-tauri && cargo tauri build
else
	@cd src-tauri && cargo tauri build --config "{\"bundle\":{\"createUpdaterArtifacts\":false}}"
endif

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
