# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**DS2000** is the Windows desktop app for the DS2000 device: three keys (mute, deafen, disconnect)
and two RGB status LEDs, connected over USB. The app controls Discord voice through Discord's local
RPC and drives the device over a serial protocol.

**Core features:**
- Discord voice control (mute, deafen, leave the voice channel) from the device keys and the UI
- Device LEDs follow the Discord voice state; RGB modes rainbow, fixed and breathing
- Tray app: closing the window hides it; start with Windows and start minimised are options
- Single instance; English and Spanish UI
- Bring-your-own Discord application: the user registers one and enters its credentials

## Code Quality Standards

- **Rust Code**: Must pass `make lint` (clippy `-D warnings`, `--workspace`) and `cargo fmt --all`.
- **Error Handling**: Error enums with `thiserror`, clear context. Propagate errors; never silence
  them.
- **Logging**: `tracing` macros only. Never `println!` or `eprintln!`.
- **Comments**: Document non-obvious logic. Public APIs have doc comments.
- **Testing**: Tests for business logic and message parsing. Tests that need the device or Discord
  are `#[ignore]`d.
- **Dependencies**: Minimise them; justify each crate.
- **Security**: Secrets live in the OS keyring, never in the config file or the logs.

## Tech Stack

- **Backend**: Rust, edition 2024, toolchain pinned to 1.88 (`rust-toolchain.toml`), Tauri 2, Tokio
- **Frontend**: Vanilla JavaScript with Vite, no framework
- **Workers**: `spawned-concurrency` GenServer actors
- **Serial**: `serial2-tokio` for I/O, `serialport` for enumeration, `tokio-util` codec for framing
- **Discord**: local RPC over the IPC pipe; OAuth token exchange with `reqwest`
- **Secrets**: `keyring` (Windows Credential Manager, service `DS2000`)

## Repository Structure

```
src-tauri/src/
├── main.rs                      # Tauri setup: plugins (single instance, autostart, shell), tray,
│                                # window (close hides it), quit/shutdown
├── backend/
│   ├── discord/                 # crate `discord`
│   │   ├── discord_worker.rs    # Public handle used by the controller
│   │   ├── discord_state.rs     # GenServer: connect, reconnect, voice settings
│   │   ├── ipc.rs, pipe_message.rs  # Discord IPC pipe and its frames
│   │   ├── oauth.rs             # Authorisation and token exchange/refresh
│   │   └── credentials.rs       # Client id / secret / tokens passed in by the controller
│   └── serial/                  # crate `serial`
│       ├── serial_worker.rs     # Public handle
│       ├── serial_state.rs      # GenServer: discovery, connection, DeviceStatus events
│       ├── port.rs              # Port discovery (USB VID filter) and handshake
│       ├── framing.rs           # COBS + CRC-16 codec
│       ├── serial_message.rs    # SerialMessage enum, PROTOCOL_VERSION
│       └── messages/            # hello, device_info, button, voice_settings, rgb,
│                                # reboot_to_bootloader
├── controller/                  # crate `controller`
│   ├── controller.rs            # Owns the workers and the config
│   ├── coordinator.rs           # Event-driven: device buttons -> Discord, Discord -> LEDs, events
│   │                            # to the frontend
│   ├── commands.rs              # Tauri commands
│   ├── tray.rs                  # Tray menu (rebuilt when the language changes)
│   └── error.rs
├── config/                      # crate `config`
│   ├── config.rs                # %APPDATA%\Mechardo\config.json
│   ├── credentials.rs           # Keyring access, redirect URI, setup guide URL
│   └── language.rs
└── common/                      # crate `common`: RGB types, task guard

src-tauri/frontend/
├── index.html, styles.css
├── tabs.js                      # Sidebar tabs: RGB, Discord, Settings, About
├── backendEvents.js             # Backend events, RGB controls, mic/headset toggles
├── discordSetup.js              # Discord tab: credentials, connect, unlink
├── startupSettings.js           # Settings tab: autostart, start minimised, language
├── serverButtonEvents.js        # External links
├── i18n.js                      # Bundled translations; the language is persisted by the backend
└── translations/{en,es}.json    # en is the fallback and must be complete
```

## Architecture

### IPC

**Commands** (`controller/commands.rs`, registered in `main.rs`): `app_version`, `controller_start`,
`ds_set_voice_settings_command`, `serial_set_rgb`, `discord_credentials_status`,
`discord_set_credentials`, `discord_clear_credentials`, `startup_preferences`,
`set_startup_preferences`, `ui_language`, `set_ui_language`.

**Events** (emitted by `coordinator.rs`): `DISCORD_CONNECTION_STATUS_EVENT`,
`DISCORD_VOICE_SETTINGS_EVENT`, `DISCORD_AWAITING_AUTHORIZATION_EVENT`,
`SERIAL_CONNECTION_STATUS_EVENT`.

### Workers and coordinator

- The **Discord** and **serial** workers are GenServer actors. Each reports changes on an `mpsc`
  event channel.
- The **coordinator** awaits both channels (no polling) and owns the mapping between them: a device
  button becomes a Discord command, a Discord voice change becomes an LED update, and both become
  frontend events.
- The **controller** owns the workers and the config, wrapped in `Arc<Mutex<Controller>>` for the
  Tauri commands.

### Shutdown

Tray **Quit** → `quit()` in `main.rs` spawns `Controller::shutdown()` (abort the coordinator, save
the config, shut down both workers), then sends on `ShutdownSignal`. A task in `setup` waits for it
and calls `app_handle.exit(0)`. Closing the window only hides it.

### Configuration and secrets

- `%APPDATA%\Mechardo\config.json`: Discord client id, last used port, RGB config, language, start
  with Windows, start minimised. Unknown and missing fields fall back to defaults.
- Keyring, service `DS2000`: `discord_client_secret`, `discord_access_token`,
  `discord_refresh_token`.
- Redirect URI the user registers in Discord: `http://localhost/` (`DISCORD_REDIRECT_URI`).

### Serial

- 115200 baud, 1000 ms timeout (`controller/controller.rs`). Only ports with the Raspberry Pi USB
  VID `0x2E8A` are opened; the handshake identifies the device.
- **Protocol**: specified in `docs/PROTOCOL.md`, the contract with the firmware. Any wire format
  change follows its "Changing the protocol" procedure: update the doc, bump `PROTOCOL_VERSION` in
  both repos, paired PRs.

## Development Commands

```bash
make deps                     # tauri-cli if missing + frontend npm install
make run                      # cargo tauri dev, RUST_LOG=info
make run-debug                # same with debug logging
make lint                     # clippy --workspace --all-targets --all-features -D warnings
make test                     # cargo test --workspace
make test-hardware            # #[ignore]d tests: needs the device and/or Discord
make build-installer-windows  # MSI
cd src-tauri && cargo fmt --all
cd src-tauri/frontend && npm run build
```

`--workspace` matters: without it only the root package is linted and tested. Keep the Makefile in
step with `.github/workflows/ci.yml`.

The version comes from `src-tauri/Cargo.toml`. Pushing a `v*` tag runs `release.yml`, which builds
the MSI and attaches it to a draft release.

## Common Development Tasks

### Adding a serial message

This is a protocol change: follow `docs/PROTOCOL.md` first.

1. Add the message in `backend/serial/messages/{name}.rs`
2. Add the variant to `SerialMessage` in `serial_message.rs`, with a reference-frame test
3. Handle it in `serial_state.rs` and, if it crosses to Discord or the UI, in `coordinator.rs`

### Adding a Tauri command

1. Write it in `controller/commands.rs` with `#[tauri::command]`, returning `Result<T, String>`
2. Register it in `generate_handler!` in `main.rs`
3. Call it from the frontend with `invoke('name', { ... })`

### UI text

Every string goes through `i18n.js`. Add the key to `translations/en.json` (required) and
`translations/es.json`.

## Code Style and Conventions

- **Rust**: edition 2024 (do not downgrade; the code relies on it, e.g. `unsafe { env::set_var }`
  and let-chains). `snake_case` / `CamelCase`. Explicit `pub` / `pub(crate)`, no pub globs.
- **JavaScript**: 2-space indentation, `camelCase`, async/await with error handling.

## Git Workflow

- **NEVER push directly to `main`**: branch (`feat/...`, `fix/...`, `docs/...`) and open a PR. CI
  (fmt, clippy, tests, frontend build) must pass.
- **Commit messages**: Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`).
- **No AI attribution**: commit messages and PR bodies must not contain `Co-Authored-By` trailers
  naming an AI assistant, nor "Generated with ..." footers. The history is the project's own.

## Known Issues and Technical Debt

- The Discord setup guide linked from the Discord tab (`URL_DISCORD_SETUP_GUIDE`,
  `mechardo3d.xyz/ds2000/discord-setup`) lives in the site repo (Mechanix97/mechardo3d#112); its
  screenshots are pending (mechardo3d#110, #111). The README has the steps too.
- The About tab's "Check for updates" is a placeholder (#30).
- The MSI is unsigned (#29).
- Windows only. Nothing has been tested on macOS or Linux.
