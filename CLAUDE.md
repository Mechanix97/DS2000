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
  are `#[ignore]`d. Frontend logic that does not need the DOM lives in its own module (e.g.
  `color.js`) and is tested with Vitest in `frontend/tests/`.
- **Dependencies**: Minimise them; justify each crate.
- **Security**: Secrets live in the OS keyring, never in the config file or the logs.

## Tech Stack

- **Backend**: Rust, edition 2024, toolchain pinned to 1.88 (`rust-toolchain.toml`), Tauri 2, Tokio
- **Frontend**: Vanilla JavaScript with Vite, no framework
- **Workers**: `spawned-concurrency` 0.5 actors (`#[protocol]` + `#[actor]`)
- **Serial**: `serial2-tokio` for I/O, `serialport` for enumeration, `tokio-util` codec for framing
- **Discord**: local RPC over the IPC pipe; OAuth token exchange with `reqwest`
- **Secrets**: `keyring` (Windows Credential Manager, service `DS2000`)

## Repository Structure

```
src-tauri/src/
├── main.rs                      # Tauri setup: plugins (single instance, autostart, shell,
│                                # updater), tray, window (close hides it), quit/shutdown
├── backend/
│   ├── discord/                 # crate `discord`
│   │   ├── discord_worker.rs    # Public handle used by the controller
│   │   ├── discord_state.rs     # Actor: connect, reconnect, voice settings
│   │   ├── ipc.rs, pipe_message.rs  # Discord IPC pipe and its frames
│   │   ├── oauth.rs             # Authorisation and token exchange/refresh
│   │   └── credentials.rs       # Client id / secret / tokens passed in by the controller
│   └── serial/                  # crate `serial`
│       ├── serial_worker.rs     # Public handle
│       ├── serial_state.rs      # Actor: discovery, connection, DeviceStatus events
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
│   ├── notifications.rs         # Desktop notifications: what is worth one (NoticeTracker), and
│   │                            # the DesktopNotifier trait with the plugin-backed SystemNotifier
│   ├── updater.rs               # In-app updates: check latest.json, download, install
│   ├── firmware.rs              # Device firmware updates: latest DS2000-Firmware release, UF2 to
│   │                            # the USB boot drive
│   └── error.rs
├── config/                      # crate `config`
│   ├── config.rs                # %APPDATA%\Mechardo\config.json
│   ├── credentials.rs           # Keyring access, redirect URI
│   └── language.rs
└── common/                      # crate `common`: RGB types, task guard, rotating log file

src-tauri/frontend/
├── index.html, styles.css       # Every colour is a :root token; Noto Sans bundled in static/fonts
├── tabs.js                      # Sidebar sections: RGB, Discord, Settings, About
├── statusPanel.js               # Bottom of the sidebar: what to notify about, account, badges
├── notices.js                   # Notification stack: problems stay, fixes show briefly
├── backendEvents.js             # Mic/headset toggles, device keys lighting up, versions
├── rgb.js, color.js             # RGB tab: mode, brightness, speed, palette + hex, live preview
├── discordSetup.js              # Discord tab: three steps, credentials, connect, unlink
├── startupSettings.js           # Settings tab: autostart, start minimised, language
├── updater.js                   # About tab: app updates, also checked daily on their own
├── firmwareUpdater.js           # About tab: firmware updates, also checked when the device connects
├── logsFolder.js                # About tab: open the logs folder
├── whatsNew.js, changelog.js    # "What's new" dialog after an update, and the full history from
│                                # About → Changelog, both from the bundled CHANGELOG.md
├── links.js                     # Project site URLs, in the app's language (/{lang}/...)
├── serverButtonEvents.js        # Site and social links in About
├── i18n.js                      # Bundled translations; the language is persisted by the backend
├── translations/{en,es}.json    # en is the fallback and must be complete
└── tests/                       # Vitest: colours, links, changelog, translation parity (`npm test`)
```

## Architecture

### IPC

**Commands** (`controller/commands.rs`, registered in `main.rs`): `app_version`, `open_logs_folder`,
`controller_start`,
`ds_set_voice_settings_command`, `rgb_config`, `serial_set_rgb`, `discord_credentials_status`,
`discord_set_credentials`, `discord_clear_credentials`, `startup_preferences`,
`set_startup_preferences`, `desktop_notifications`, `set_desktop_notifications`, `ui_language`,
`set_ui_language`, `release_notes_unseen`,
`release_notes_seen`; `update_check` and `update_install`
in `controller/updater.rs`; `firmware_check` and `firmware_install` in `controller/firmware.rs`.

**Events** (emitted by `coordinator.rs`): `DISCORD_CONNECTION_STATUS_EVENT`,
`DISCORD_VOICE_SETTINGS_EVENT`, `DISCORD_AWAITING_AUTHORIZATION_EVENT`, `DISCORD_USER_EVENT`
(display name and avatar URL, kept in memory only), `DISCORD_RUNNING_EVENT` (whether Discord
itself is open), `SERIAL_CONNECTION_STATUS_EVENT`,
`DEVICE_BUTTON_EVENT` (a key pressed on the device).

### Workers and coordinator

- The **Discord** and **serial** workers are actors. Each reports changes on an `mpsc`
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

### Desktop notifications

With start minimised there is no webview, so the status notices in the sidebar cannot reach anyone.
The coordinator feeds device and Discord changes to `NoticeTracker` (`controller/notifications.rs`),
which only reports a change from working to broken: the device unplugged or Discord's connection
lost (while Discord still runs) for 10 s, a protocol mismatch, Discord's authorisation modal. When
one is due and the window is not on screen (hidden or minimised), it goes out through
`DesktopNotifier`; `SystemNotifier` is `tauri-plugin-notification` (Windows toasts, macOS
Notification Center, freedesktop D-Bus on Linux). Text comes from the `notifications` section of
the translation files, read by `config/language.rs` like the tray labels. `notify-rust` is held at
4.17 in `Cargo.lock`: 4.18 needs Rust 1.89.

### Configuration and secrets

- `%APPDATA%\Mechardo\config.json`: Discord client id, last used port, RGB config, language, start
  with Windows, start minimised, the version whose release notes were last seen, desktop
  notifications (on by default). Unknown and missing fields fall back to defaults.
- Keyring, service `DS2000`: `discord_client_secret`, `discord_access_token`,
  `discord_refresh_token`.
- Redirect URI the user registers in Discord: `http://localhost/` (`DISCORD_REDIRECT_URI`).
- Logs: `init_logging` in `main.rs` writes to the console and to `%APPDATA%\Mechardo\logs\ds2000.log`
  (`common::log_file`): at 2.5 MB it becomes `ds2000.1.log`, replacing the previous one, so logs
  never pass 5 MB. Users attach them to bug reports, so nothing secret may be logged; the Discord
  tokens and credentials have redacting `Debug` impls for that reason.

### Serial

- 115200 baud, 1000 ms timeout (`controller/controller.rs`). Only ports with the Raspberry Pi USB
  VID `0x2E8A` are opened; the handshake identifies the device.
- **Firmware updates** (`controller/firmware.rs`): the latest DS2000-Firmware release, the UF2
  for the board the device reports (`DS2000-Firmware_<version>_<environment>.uf2`, verified
  against `SHA256SUMS` and its `.minisig` against `FIRMWARE_PUBLIC_KEY`, the firmware repo's
  `minisign.pub`; never change that key without a plan, released apps refuse images signed with any
  other). The device goes to USB boot with RebootToBootloader, or with the 1200-baud
  touch when it is on another protocol; the serial worker stops reconnecting until the image is
  written; the drive is found by its `INFO_UF2.TXT`. Releases on another protocol are not offered.
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
the MSI, signs it for the updater (secrets `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`), writes `latest.json` and attaches all of it to a draft
release. The app offers an update only once the release is published. Publishing it also runs
`announce.yml`, which posts the release notes' "What's new" section to Discord (secret
`DISCORD_WEBHOOK_URL`); the same workflow lives in DS2000-Firmware.

`CHANGELOG.md` (Keep a Changelog) is written for users, not developers. A PR with a change users
notice adds a line under **Unreleased**; the release PR bumps the version and moves **Unreleased**
under `## [x.y.z] - date`, with its compare link at the bottom. `release.yml` puts that entry at
the top of the notes as "What's new" and fails on a tag that has none. The frontend bundles the
file too: on the first start after an update, `whatsNew.js` shows the entries since the version the
user had (`release_notes_unseen`), and closing the dialog records them as seen. So the entry is
also what users read inside the app; keep it in the Markdown `changelog.js` understands (`###`
sections, `-` lists, **bold**, `code`, https links).

### Updates

`controller/updater.rs` wraps `tauri-plugin-updater` for the check and the verified download.
Installing shuts the controller down first (config saved, port and pipe released). On Windows the
MSI is not handed to the plugin's `install`, which exits on the spot: a detached PowerShell waits
for the app to exit, runs `msiexec`, and starts the old version again if the install fails or UAC
is declined. Never change `plugins.updater.pubkey` without a plan: installed copies only accept
MSIs signed with the matching private key.

`TRADEMARKS.md` lists what a renamed fork changes (identifier, updater endpoint and key, firmware
key, icons); keep it in step when any of them moves.

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

- The Discord setup guide linked from the Discord tab (`frontend/links.js`,
  `mechardo3d.xyz/{lang}/ds2000/discord-setup`) lives in the site repo (Mechanix97/mechardo3d#112); its
  screenshots are pending (mechardo3d#110, #111). The README has the steps too.
- The MSI is unsigned (#29).
- Windows only. Nothing has been tested on macOS or Linux.
