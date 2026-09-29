# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**DS2000** is a professional, open-source Tauri-based desktop application for managing Discord integration and serial device communication. The application is designed with enterprise-grade code quality, maintainability, and internationalization (i18n) support in mind.

**Core Features:**
- Discord bot-like interface with system tray support
- Serial port communication for hardware control (RGB LEDs, buttons)
- Discord presence and voice settings management
- Graceful shutdown with proper resource cleanup
- Multi-language UI support (Spanish and English, with extensibility for others)

## Code Quality Standards

As an open-source project, this codebase must maintain high professional standards:

- **Rust Code**: All code must pass `cargo clippy` with no warnings. Use `cargo fmt` before commits.
- **Error Handling**: Comprehensive error types with clear context. Propagate errors properly without silencing failures.
- **Logging**: Use `tracing` macros for structured logging. Never use `println!` or `eprintln!`.
- **Comments**: Document non-obvious logic. Public APIs must have doc comments.
- **Testing**: Tests for critical business logic and message parsing.
- **Dependencies**: Minimize dependencies. Justify each external crate.
- **Performance**: Prefer clarity over micro-optimizations unless critical.
- **Security**: Sensitive data (tokens, secrets) must be encrypted/hashed. Never log sensitive information.

## Tech Stack

- **Backend**: Rust 2021 edition with Tauri 2.7, Tokio async runtime
- **Frontend**: Vanilla JavaScript with Vite build tool (no heavy frameworks)
- **Communication**: Tauri IPC with strong typing
- **Hardware**: Serial communication (tokio-serial) at 115200 baud
- **Discord**: OAuth2 integration with IPC pipes for voice settings
- **Internationalization**: i18n system via JSON translation files (planned)

## Repository Structure

### Core Rust Workspace (src-tauri/src/)

```
src-tauri/src/
├── main.rs                          # Tauri app setup, system tray, window management
├── backend/
│   ├── discord/                     # Discord bot integration
│   │   ├── discord_worker.rs        # Async Discord connection worker
│   │   ├── discord_state.rs         # Discord state management
│   │   ├── ipc.rs                   # IPC pipe handling for Discord
│   │   └── pipe_message.rs          # Message protocol for Discord IPC
│   └── serial/                      # Serial device communication
│       ├── serial_worker.rs         # Serial port management
│       ├── serial_state.rs          # Device state tracking
│       ├── port.rs                  # Port enumeration and connection
│       ├── serial_message.rs        # Message protocol for devices
│       └── messages/                # Message types (button, RGB, voice settings)
├── controller/
│   ├── controller.rs                # Main orchestration (Discord + Serial workers)
│   ├── commands.rs                  # Tauri command handlers (IPC from frontend)
│   └── error.rs                     # Error types
├── config/
│   ├── config.rs                    # Configuration loading/saving
│   └── secrets.rs                   # Secret management (OAuth tokens, etc)
└── common/                          # Shared types (RGB configs, LED updates)
```

### Frontend (src-tauri/frontend/)

```
src-tauri/frontend/
├── index.html                       # Main UI structure
├── backendEvents.js                 # Event listeners for backend messages
├── serverButtonEvents.js            # Server/social media button handlers
├── i18n.js                          # i18n system (TO BE CREATED)
├── translations/                    # Translation files (TO BE CREATED)
│   ├── en.json                      # English
│   └── es.json                      # Spanish
├── styles.css                       # Styling
└── vite.config.js                   # Vite build config
```

### Configuration Files

- **tauri.conf.json**: Tauri app configuration (window, plugins, permissions, bundling)
- **Cargo.toml**: Rust workspace setup, edition "2024" (requires Rust 1.85+)
- **src-tauri/frontend/package.json**: Frontend dependencies (minimal)

## Architecture Insights

### IPC Command Flow

**Frontend → Tauri Commands → Backend**
- `controller_start`: Begin monitoring connections
- `serial_set_rgb(mode, brightness, leds)`: Send RGB updates to device
- `ds_set_voice_settings_command(mute, deaf)`: Control voice state in Discord

**Backend → Tauri Events → Frontend**
- `DISCORD_CONNECTION_STATUS_EVENT`: Discord connection status
- `DISCORD_VOICE_SETTINGS_EVENT`: Voice settings from Discord
- `SERIAL_CONNECTION_STATUS_EVENT`: Serial device status

### Worker Architecture

The application uses a **multi-worker pattern** for concurrent operations:

- **DiscordWorker**: Manages Discord OAuth2 flow, monitors user presence, handles voice settings
- **SerialWorker**: Scans available serial ports, maintains device connection, processes incoming/outgoing messages
- **Controller**: Central orchestrator that coordinates both workers and handles frontend requests
- All workers run on the Tokio async runtime with proper cancellation support

### Key Dependencies

| Crate | Purpose | Version |
|-------|---------|---------|
| tauri | Desktop framework | 2.7 |
| tokio | Async runtime | 1.0 (full features) |
| tracing | Structured logging | 0.1 |
| serde | JSON serialization | 1.0 |
| serial2-tokio | Serial port handling | 0.1 |
| tokio-util | Framed codec for the serial protocol | 0.7 |
| spawned-concurrency | GenServer actors for the workers | 0.2 |
| reqwest | HTTP client for Discord API | 0.11 |
| thiserror | Error handling macros | 2.0 |

## Development Commands

### Rust Backend

```bash
# Build backend (debug)
cargo build

# Build backend (release)
cargo build --release

# Run tests
cargo test

# Run tests with logging
RUST_LOG=debug cargo test -- --nocapture

# Format code (required before commits)
cargo fmt

# Check for issues (clippy linting)
cargo clippy --all-targets --all-features

# Build specific workspace member
cargo build -p discord
cargo build -p serial
cargo build -p controller
```

### Frontend

```bash
cd src-tauri/frontend

# Install dependencies
npm install

# Development server with hot reload
npm run dev

# Production build
npm run build

# Preview production build locally
npm run preview
```

### Full Application

```bash
# From project root, full development mode
npm run tauri dev

# Build production bundle (creates MSI installer for Windows)
npm run tauri build
```

## Running and Testing

### Initial Setup

1. Install Rust: https://rustup.rs/
2. Install Node.js (v18+): https://nodejs.org/
3. Clone repository
4. From `src-tauri/frontend/`, run `npm install`
5. From project root, run `npm run tauri dev`

### Logging

Logging uses **structured tracing** with environment filters. Control verbosity:

```bash
# Debug level (most detailed)
RUST_LOG=debug npm run tauri dev

# Info level (default, important events)
RUST_LOG=info npm run tauri dev

# Specific modules
RUST_LOG=discord=debug,serial=info npm run tauri dev
```

Available levels: `trace`, `debug`, `info`, `warn`, `error`

### Discord OAuth Configuration

**Location**: `src-tauri/src/config/secrets.rs`

Configuration stored in:
- Discord Client ID, Secret, Redirect URL
- Access/refresh tokens (cached after OAuth flow)
- All sensitive data lives in config directory (not in repository)

### Serial Communication Parameters

- **Default Baudrate**: 115200 (defined in `src-tauri/src/controller/controller.rs:10`)
- **Default Timeout**: 1000ms (defined in `src-tauri/src/controller/controller.rs:11`)
- **Message Types**: Hello, DeviceInfo, Button, RGB, VoiceSettings
- **Framing**: COBS with a CRC-16, delimited by `0x00` (`src-tauri/src/backend/serial/framing.rs`).
  The device reports its protocol and firmware version in `DeviceInfo`; bump `PROTOCOL_VERSION` in
  `serial_message.rs` together with the firmware's on any wire format change
- **Device Discovery**: Port enumeration in `src-tauri/src/backend/serial/port.rs`

## Internationalization (i18n)

The frontend must support multiple languages with language selection. Current implementation needs to be created.

### i18n Implementation Plan

**Frontend Structure:**
1. Create `src-tauri/frontend/i18n.js` with translation management
2. Create `src-tauri/frontend/translations/` directory with JSON files:
   - `en.json` - English strings
   - `es.json` - Spanish strings (current hardcoded labels)
3. Add language selector UI element
4. Store selected language in browser localStorage

**Translation Files Format:**
```json
{
  "ui": {
    "rgb": "RGB",
    "configuration": "Configuration",
    "about": "About",
    "brightness": "Brightness",
    "mode": "Mode"
  },
  "status": {
    "discord_connected": "Discord: Connected",
    "discord_disconnected": "Discord: Not connected",
    "serial_connected": "Serial: Connected",
    "serial_disconnected": "Serial: Not connected"
  }
}
```

**Current Hardcoded Spanish Labels** (to be extracted):
- "RGB", "Configuración", "Acerca" (menu items)
- "Brillo", "Modo" (control labels)
- "Discord: Conectado", "Serial: Conectado" (status messages)
- All found in `index.html`, `backendEvents.js`, `serverButtonEvents.js`

### Backend Localization

Backend should not contain UI strings. Error messages can be logged in English for debugging consistency. Consider backend message strings in future if needed.

## Important Implementation Details

### Error Handling

**Strategy**: Use custom error enums with `thiserror` crate:
- `src-tauri/src/backend/discord/error.rs` - Discord-specific errors
- `src-tauri/src/backend/serial/error.rs` - Serial-specific errors
- `src-tauri/src/controller/error.rs` - High-level controller errors

**Error Propagation**:
- Errors bubble up from workers to Controller
- Controller converts to `ControllerError` for frontend compatibility
- Tauri commands return `Result<T, &'static str>` to JavaScript

**Best Practices**:
- Never silently ignore errors with `.unwrap_or_default()`
- Always log errors at appropriate level (warn/error) with context
- Provide actionable error messages for end users
- Never expose internal paths or system details in user-facing errors

### State Management

- **Controller** holds mutable state for both workers and config
- Wrapped in `Arc<Mutex<Controller>>` for thread-safe sharing across Tauri IPC handlers
- Frontend communicates with backend via explicit commands (not direct state mutation)
- Background loop in `controller_start` command emits status events periodically

### Shutdown Flow

1. User selects "Quit" from system tray menu
2. Tray handler calls `Controller::shutdown()` asynchronously
3. Discord worker closes OAuth connection gracefully
4. Serial worker closes port and stops port scanning
5. Main thread polls `shutdown_complete` flag
6. Once complete, process exits with code 0

**Important**: The shutdown loop (lines 105-111 in `main.rs`) is a workaround. Consider using proper Tauri shutdown hooks in future refactoring.

### Frontend Architecture

- **No framework dependency** - Vanilla JavaScript for minimal bundle size
- **Event-driven** - Listens to backend events and reacts
- **Tab-based UI** - Menu items switch between RGB, Configuration, About
- **Status indicators** - Real-time Discord and Serial connection display
- **Control handlers** - Sliders and dropdowns send updates to backend

## Common Development Tasks

### Adding a New Serial Message Type

1. Create struct in `src-tauri/src/backend/serial/messages/{name}.rs`
2. Add variant to `SerialMessage` enum in `src-tauri/src/backend/serial/serial_message.rs`
3. Implement `Serialize`/`Deserialize` traits
4. Handle message in `SerialWorker::process_message()` - `src-tauri/src/backend/serial/serial_worker.rs`
5. Document message format in comments
6. Add tests for parsing edge cases

### Adding a New Tauri Command

1. Define command function in `src-tauri/src/controller/commands.rs` with `#[tauri::command]` macro
2. Function must return `Result<T, String>` or compatible error type
3. Add function name to `invoke_handler!` macro in `src-tauri/src/main.rs`
4. Document parameters and return value
5. Test from frontend by calling `invoke('command_name', { params })`

### Modifying Frontend UI

- **Structure**: Edit `src-tauri/frontend/index.html` for DOM elements
- **Styling**: Edit `src-tauri/frontend/styles.css` (uses CSS Grid for layout)
- **Events**: Add listeners in `backendEvents.js` (backend integration) or `serverButtonEvents.js` (UI events)
- **Internationalization**: After i18n system is implemented, extract strings to translation JSON
- **Hot Reload**: Vite automatically reloads during `npm run dev`

### Adding Translations

1. Edit `translations/en.json` for English text
2. Edit `translations/es.json` for Spanish text (keeping parity)
3. Update HTML: Replace hardcoded text with `i18n.t('key.path')`
4. Test language switcher
5. Verify all text is translated (check console for missing keys)

### Debugging

```bash
# Enable detailed backend logging
RUST_LOG=debug npm run tauri dev

# Enable specific module logging
RUST_LOG=serial=debug,discord=debug npm run tauri dev

# Check frontend console
# - Open Developer Tools in Tauri window (Ctrl+Shift+I on Windows)
# - View console for JS errors and i18n warnings
```

### Running Tests

Tests that need a running Discord client or a physical DS2000 device are marked `#[ignore]` so
that `cargo test` stays green in CI. Run them explicitly with `cargo test -- --ignored`.

```bash
# Run all Rust tests with output
cargo test -- --nocapture

# Run the hardware / Discord integration tests (needs the device and Discord running)
cargo test -- --ignored --nocapture

# Run tests for specific workspace member
cargo test -p serial -- --nocapture

# Run specific test
cargo test discord_worker::tests -- --nocapture
```

## Code Style and Conventions

### Rust

- **Edition**: 2024 (do not downgrade; the codebase relies on 2024 semantics, e.g. `unsafe { env::set_var }`)
- **Formatting**: `cargo fmt` enforces standard formatting
- **Linting**: `cargo clippy` must pass with no warnings
- **Naming**: `snake_case` for functions/variables, `CamelCase` for types
- **Visibility**: Explicit `pub`/`pub(crate)` boundaries. Avoid pub globs.
- **Documentation**: Doc comments (`///`) for public items
- **Error Context**: Use `with_context()` in error handling for better diagnostics

### JavaScript/Frontend

- **Formatting**: 2-space indentation (match existing style)
- **Naming**: `camelCase` for functions/variables, `PascalCase` for classes
- **Comments**: Explain complex logic, not obvious code
- **Async**: Use async/await with proper error handling
- **DOM**: Cache frequently accessed elements to avoid repeated queries

### Configuration Files

- **JSON**: Properly formatted and validated (use `npm run build` to catch issues)
- **Comments**: Limited to non-JSON files (TOML, YAML allow comments)

## Open Source Practices

- **LICENSE**: Ensure proper license file is present (add if missing)
- **README**: Keep up-to-date with build/run instructions
- **CHANGELOG**: Track notable changes in each release
- **CONTRIBUTING**: Document contribution guidelines (to be created)
- **Issues**: Use clear issue templates for bug reports and features
- **Git Workflow**:
  - **NEVER push directly to `main` branch** - all changes must go through Pull Requests
  - Create feature branches from `main`: `git checkout -b feature/description`
  - Submit PR for code review before merging
  - PRs must pass all checks (linting, tests, builds) before approval
  - Require at least one approval before merging to `main`
- **Commit Messages**: Clear, descriptive messages following conventional commits format
  - Good: "feat: add language selector to settings"
  - Good: "fix: resolve serial port timeout issue"
  - Bad: "fixes stuff", "wip", "asdf"
  - **No AI attribution**: commit messages and PR bodies must not contain `Co-Authored-By` trailers
    naming an AI assistant, nor "Generated with ..." footers. The history is the project's own.

## Known Issues and Technical Debt

- **Single-Instance Plugin**: Not implemented. The `tauri-plugin-single-instance` dependency was
  removed while unused; re-add it when implementing. Without it, a second launch competes with the
  first for the serial port and the Discord IPC pipe.
- **Shutdown Polling**: Uses blocking thread with polling flag - should use proper Tauri shutdown hooks
- **Frontend i18n**: Hardcoded Spanish labels - needs extraction to translation files
- **Tests**: Limited test coverage - add tests for critical business logic
- **Error Messages**: Some error messages in Spanish - should be English in backend

## Resources

- **Tauri Docs**: https://docs.tauri.app/
- **Tokio**: https://tokio.rs/
- **Rust Book**: https://doc.rust-lang.org/book/
- **Vite**: https://vitejs.dev/
- **Tracing**: https://docs.rs/tracing/

