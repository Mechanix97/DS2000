<p align="center">
  <img src="docs/img/ds2000-logo.webp" alt="DS2000" width="150">
</p>

# DS2000

Desktop app for the DS2000, a small desk device with three keys (mute, deafen, disconnect) and two
RGB status LEDs. The app links the device to Discord: the keys control your voice state, and the
LEDs show it.

It runs on Windows. Built with Rust and Tauri 2, with a plain HTML, CSS and JavaScript frontend.

## Features

- **Discord voice control** from the device: mute, deafen, and leave the voice channel. The device
  LEDs follow your mute and deafen state, including changes made in Discord itself.
- **RGB lighting**: rainbow, fixed or breathing, with brightness, speed and a colour per LED.
- **Tray app**: closing the window keeps it running in the tray. Optional start with Windows,
  optionally minimised.
- **Automatic device detection** over USB.
- **In-app updates**: **About → Check for updates** downloads and installs a newer release.
- **Firmware updates**: **About → Check for firmware updates** installs newer device firmware,
  with no tools and no button to hold.
- **English and Spanish** interface.

## Install

Download the `.msi` from the [latest release](https://github.com/Mechanix97/DS2000/releases/latest)
and run it.

The installer is unsigned, so SmartScreen warns the first time: **More info → Run anyway**.
To check the download, `Get-FileHash <file>.msi` in PowerShell must match the SHA-256 in the
release notes.

After that, **About → Check for updates** installs newer releases: it downloads the MSI, checks its
signature, and the app restarts on the new version. Windows may ask for administrator permission.

The device firmware has to speak the same serial protocol as the app; each release says which one.
If they do not match, **Settings → Connections** says whether the firmware or the app is out of date. The
firmware lives in [DS2000-Firmware](https://github.com/Mechanix97/DS2000-Firmware).

**About → Check for firmware updates** installs the latest firmware release for your board: it
downloads the image, checks it against the release's `SHA256SUMS` and its minisign signature (the
app only installs firmware signed with the DS2000-Firmware key), restarts the device into its USB
drive mode, copies the image and waits for the device to come back on the new version. It also works
on firmware too old to talk to the app. Do not unplug the device while it says "Installing".

## Connect Discord

Discord only lets an application control your microphone if you own that application, so DS2000
does not ship one: you create your own. It is free and done once. You need the Discord **desktop**
app running; the browser version cannot be controlled.

The [step-by-step guide on mechardo3d.xyz](https://www.mechardo3d.xyz/ds2000/discord-setup) has a
screenshot for every step and a troubleshooting section; the Discord tab in the app links to it
too. In short:

1. Open the [Discord Developer Portal](https://discord.com/developers/applications) and click
   **New Application**. Any name works, for example "DS2000".
2. Go to **OAuth2**:
   - Copy the **Client ID**.
   - Click **Reset Secret** and copy the **Client Secret**. Discord shows it only once.
   - Under **Redirects**, add exactly `http://localhost/` and save. Without it the connection
     fails.
3. In DS2000, open the **Discord** tab, paste both values and click **Connect**.
4. Discord shows an authorisation request: accept it.

The Discord tab then reads *Connected to Discord.*, and **Settings → Connections** reads
*Discord: Connected*. Your credentials survive restarts; **Unlink** in the same tab removes them.

## Using it

| Key | Does |
|---|---|
| Mute | Toggles mute |
| Deafen | Toggles deafen |
| Disconnect | Leaves the current voice channel |
| Shift + Mute | Switches Discord to the next input device (all by default, pick them in Settings) |
| Shift + Deafen | Switches Discord to the next output device (all by default, pick them in Settings) |
| Shift + Disconnect | Rejoins the server voice channel you left, within 5 minutes |

Shift is the one on your computer's keyboard. It cannot be read on Linux under Wayland, so the
Shift shortcuts are not available there.

The microphone and headphone icons in the app toggle mute and deafen too.

Tabs:

- **RGB**: mode (rainbow, fixed, breathing), brightness, speed, and the colour of each LED.
  Changes reach the device immediately and are remembered.
- **Discord**: the credentials above.
- **Settings**: start with Windows, start minimised, desktop notifications, language, and the
  connection status.
- **About**: app and firmware versions, app and firmware updates, and links.

The tray icon opens the window on a left click; its menu has **Open DS2000** and **Quit**. Closing
the window does not quit the app.

## Where things are stored

| What | Where |
|---|---|
| Settings (Discord client id, RGB, language, startup options, last port) | `%APPDATA%\Mechardo\config.json` |
| Discord client secret and tokens | Windows Credential Manager, service `DS2000` |
| Logs (`ds2000.log`, and `ds2000.1.log` once it fills up; 5 MB at most) | `%APPDATA%\Mechardo\logs` |

No secret is written to the config file.

## Development

Requirements: Windows, [Rust](https://rustup.rs/) (the toolchain is pinned to 1.88 by
`rust-toolchain.toml`, rustup picks it up), Node.js 18 or later, and `make`.

```bash
git clone https://github.com/Mechanix97/DS2000.git
cd DS2000
make deps   # tauri-cli if missing, and the frontend's npm packages
make run    # cargo tauri dev: Vite with hot reload plus the Rust backend
```

| Command | Does |
|---|---|
| `make run` / `make run-debug` | Run the app, with `info` or `debug` logging |
| `make lint` | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| `make test` | `cargo test --workspace` |
| `make test-hardware` | The `#[ignore]`d tests that need the device or Discord |
| `make build-installer-windows` | Build the MSI locally |

CI runs format, lint and tests on every pull request. Pushing a `v*` tag builds the MSI and attaches
it to a draft release. The version comes from `src-tauri/Cargo.toml`. The release pull request bumps it
and moves **Unreleased** in `CHANGELOG.md` under the new version; that entry opens the release
notes, and a tag without one fails before the build.

### Releasing and updates

The release build signs the MSI with an update-signing key (not a code-signing certificate) and
publishes `latest.json`, which the app reads from `releases/latest/download/latest.json`. The app
only installs an MSI signed with the key whose public half is in `tauri.conf.json`
(`plugins.updater.pubkey`), and drafts are not `latest`, so nothing is offered until the release is
published.

- The private key and its password are the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Keep a copy elsewhere: if the key is lost, installed copies
  can no longer be updated and users have to reinstall by hand.
- A local `make build-installer-windows` signs only when `TAURI_SIGNING_PRIVATE_KEY` is set.
- Publishing a release (not a pre-release) posts its changes to Discord through the webhook in the
  repository secret `DISCORD_WEBHOOK_URL`. Without the secret nothing is posted. **Actions →
  Announce → Run workflow** sends it again for a given tag.

Logging uses `RUST_LOG`, for example `RUST_LOG=serial=debug,discord=info`; without it, `info`. The
same lines go to the console and to the log file.

### Layout

```
src-tauri/
├── src/
│   ├── main.rs              # Tauri setup, tray, window, single instance, autostart
│   ├── controller/          # Tauri commands; the coordinator links device events and Discord
│   ├── backend/discord/     # Discord local RPC over the IPC pipe, OAuth
│   ├── backend/serial/      # Device discovery, framing and messages
│   ├── config/              # config.json, keyring credentials, language
│   └── common/              # Types shared by the crates
├── frontend/                # index.html, JS modules, translations/{en,es}.json
└── tauri.conf.json
docs/PROTOCOL.md             # Serial protocol shared with the firmware
```

The serial protocol between the app and the firmware is specified in
[docs/PROTOCOL.md](docs/PROTOCOL.md). Any change to it follows the procedure described there, with
paired pull requests in this repository and the firmware's.

## Contributing

Issues and pull requests are welcome. Branch from `main` and open a pull request; `main` is never
pushed to directly. Before opening it, run `cargo fmt`, `make lint` and `make test`. Commit
messages follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`,
`docs:`, ...). A change users will notice also adds a line under **Unreleased** in
[CHANGELOG.md](CHANGELOG.md), written for them rather than for developers. For a bug, include the steps, what you expected, and the log files: **About → Open logs folder**
in the app, or a log from `make run-debug`.

## Related projects

- [DS2000-Firmware](https://github.com/Mechanix97/DS2000-Firmware): device firmware (RP2350, Arduino)
- [DS2000-PCB](https://github.com/Mechanix97/DS2000-PCB): KiCad board
- [DS2000-Enclosure](https://github.com/Mechanix97/DS2000-Enclosure): 3D-printed enclosure

## License

[GNU Affero General Public License v3.0](LICENSE).

The DS2000 name and the DS2000 and Mechardo Labs logos are not covered by the license: as an
additional term under AGPL-3.0 section 7(e), their use is governed by the
[trademark policy](TRADEMARKS.md). Forks are welcome under a name of their own.

Made by [Mechardo Labs](https://www.mechardo3d.xyz/). Community on
[Discord](https://discord.gg/VtbFAGJe86).

DS2000 is not affiliated with or endorsed by Discord Inc.
